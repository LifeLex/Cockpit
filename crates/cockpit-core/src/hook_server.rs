//! Axum-based Stop-hook listener for Claude Code agent completion callbacks.
//!
//! `cockpit-core` runs this server on a fixed localhost port. The repo's
//! Claude Code config registers a Stop hook that POSTs `{ session_id }` to
//! `/hook/stop`. The handler looks up the session in the [`SessionMap`],
//! removes it, and emits a [`CompletionEvent`] on a broadcast channel.
//!
//! The actual reconciliation (re-reading git state, calling `mark_reworked`)
//! is **not** done in the HTTP handler. The caller (CLI or Tauri app) listens
//! on the broadcast channel and performs reconciliation. This keeps the hook
//! server thin and avoids needing mutable access to `Review`/`ProjectPlan`
//! objects inside the handler.
//!
//! See `SPEC.md` 11 and `CLAUDE.md` 2 (async section).

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Json, Router, routing};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::adapters::agent::SessionMap;
use crate::model::AgentMode;

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/// Errors from the hook server lifecycle.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The TCP listener could not bind to the requested address.
    #[error("failed to bind hook server to {addr}: {source}")]
    Bind {
        /// The `host:port` string that was attempted.
        addr: String,
        /// The underlying I/O error.
        source: std::io::Error,
    },

    /// The server encountered an error while serving requests.
    #[error("hook server error: {0}")]
    Serve(String),
}

// ---------------------------------------------------------------------------
// Payload / Response types
// ---------------------------------------------------------------------------

/// Payload from Claude Code's Stop hook.
#[derive(Debug, Deserialize)]
pub struct StopHookPayload {
    /// The session ID assigned to the agent process at spawn time.
    pub session_id: String,
}

/// Response returned by the `/hook/stop` endpoint.
#[derive(Debug, Serialize, Deserialize)]
pub struct StopHookResponse {
    /// `"ok"` on success, `"unknown_session"` when the session is not found.
    pub status: String,
    /// The reviewed object's identifier, present only on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_id: Option<String>,
}

// ---------------------------------------------------------------------------
// CompletionEvent
// ---------------------------------------------------------------------------

/// Event emitted on the broadcast channel when an agent session completes.
///
/// Consumers (CLI or Tauri) receive this and perform reconciliation on the
/// corresponding reviewed object.
#[derive(Debug, Clone)]
pub struct CompletionEvent {
    /// The session ID that completed.
    pub session_id: String,
    /// Identifier of the reviewed object (e.g. a `ReviewId` or `ProjectRef`).
    pub object_id: String,
    /// Which agent mode was running.
    pub mode: AgentMode,
}

// ---------------------------------------------------------------------------
// HookState
// ---------------------------------------------------------------------------

/// Shared state for the hook server, passed to handlers via axum's `State`.
#[derive(Clone)]
pub struct HookState {
    /// Maps session IDs to their reviewed objects.
    pub session_map: SessionMap,
    /// Sender side of the completion broadcast channel.
    pub completion_tx: broadcast::Sender<CompletionEvent>,
}

// ---------------------------------------------------------------------------
// Router + handler
// ---------------------------------------------------------------------------

/// Build the axum [`Router`] with the Stop-hook endpoint.
///
/// The router exposes a single route: `POST /hook/stop`.
pub fn router(state: HookState) -> Router {
    Router::new()
        .route("/hook/stop", routing::post(handle_stop))
        .with_state(state)
}

/// Handle a Stop-hook callback from Claude Code.
///
/// Looks up the session in the session map. If found, removes the entry,
/// emits a [`CompletionEvent`], and returns 200. If not found, returns 404.
async fn handle_stop(
    State(state): State<HookState>,
    Json(payload): Json<StopHookPayload>,
) -> impl IntoResponse {
    let entry = state.session_map.remove(&payload.session_id);

    match entry {
        Some(entry) => {
            let event = CompletionEvent {
                session_id: payload.session_id,
                object_id: entry.object_id.clone(),
                mode: entry.mode,
            };

            // Best-effort send — if no receivers are listening, the event is
            // dropped. This is fine: the caller may not have subscribed yet,
            // or may have already shut down.
            let _ = state.completion_tx.send(event);

            (
                StatusCode::OK,
                Json(StopHookResponse {
                    status: "ok".into(),
                    object_id: Some(entry.object_id),
                }),
            )
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(StopHookResponse {
                status: "unknown_session".into(),
                object_id: None,
            }),
        ),
    }
}

// ---------------------------------------------------------------------------
// Server lifecycle
// ---------------------------------------------------------------------------

/// Create a broadcast channel for [`CompletionEvent`]s.
///
/// Returns both the sender and a receiver. Additional receivers can be
/// obtained by calling `sender.subscribe()`.
pub fn completion_channel() -> (
    broadcast::Sender<CompletionEvent>,
    broadcast::Receiver<CompletionEvent>,
) {
    broadcast::channel(32)
}

/// Run the hook server on `127.0.0.1:{port}`.
///
/// Binds to localhost only (never `0.0.0.0`) and serves until the future is
/// cancelled or an error occurs. Supports graceful shutdown via tokio's
/// cooperative cancellation.
pub async fn serve(state: HookState, port: u16) -> Result<(), Error> {
    let addr = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|source| Error::Bind {
            addr: addr.clone(),
            source,
        })?;

    axum::serve(listener, router(state))
        .await
        .map_err(|e| Error::Serve(e.to_string()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use http::Request;
    use tower::ServiceExt; // for oneshot()

    use super::*;
    use crate::adapters::agent::{SessionEntry, SessionMap};
    use crate::model::AgentMode;

    /// Build a `HookState` with a fresh session map and broadcast channel.
    fn make_state() -> (HookState, broadcast::Receiver<CompletionEvent>) {
        let session_map = SessionMap::new();
        let (tx, rx) = completion_channel();
        let state = HookState {
            session_map,
            completion_tx: tx,
        };
        (state, rx)
    }

    /// Register a session in the given state's session map.
    fn register_session(state: &HookState, session_id: &str, object_id: &str, mode: AgentMode) {
        state
            .session_map
            .register(
                session_id.into(),
                SessionEntry {
                    object_id: object_id.into(),
                    mode,
                    pid: 42,
                },
            )
            .unwrap();
    }

    /// Build a POST request to `/hook/stop` with the given session_id.
    fn stop_request(session_id: &str) -> Request<Body> {
        let payload = serde_json::json!({ "session_id": session_id });
        Request::builder()
            .method("POST")
            .uri("/hook/stop")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&payload).unwrap()))
            .unwrap()
    }

    /// Read the response body as a `StopHookResponse`.
    async fn read_response(response: http::Response<Body>) -> StopHookResponse {
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    #[tokio::test]
    async fn stop_hook_known_session() {
        let (state, _rx) = make_state();
        register_session(&state, "sess-1", "review-42", AgentMode::Fix);

        let app = router(state.clone());
        let response = app.oneshot(stop_request("sess-1")).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = read_response(response).await;
        assert_eq!(body.status, "ok");
        assert_eq!(body.object_id.as_deref(), Some("review-42"));

        // Session should be removed from the map.
        assert!(
            state.session_map.get("sess-1").is_none(),
            "session should be removed after stop hook"
        );
    }

    #[tokio::test]
    async fn stop_hook_unknown_session() {
        let (state, _rx) = make_state();

        let app = router(state);
        let response = app.oneshot(stop_request("nonexistent")).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let body = read_response(response).await;
        assert_eq!(body.status, "unknown_session");
        assert!(body.object_id.is_none());
    }

    #[tokio::test]
    async fn stop_hook_emits_completion_event() {
        let (state, mut rx) = make_state();
        register_session(&state, "sess-evt", "review-99", AgentMode::Plan);

        let app = router(state);
        let response = app.oneshot(stop_request("sess-evt")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let event = rx
            .try_recv()
            .expect("should have received a CompletionEvent");
        assert_eq!(event.session_id, "sess-evt");
        assert_eq!(event.object_id, "review-99");
        assert_eq!(event.mode, AgentMode::Plan);
    }

    #[tokio::test]
    async fn multiple_sessions() {
        let (state, mut rx) = make_state();
        register_session(&state, "sess-a", "review-a", AgentMode::Fix);
        register_session(&state, "sess-b", "review-b", AgentMode::Implement);

        // Stop session A.
        let app = router(state.clone());
        let response = app.oneshot(stop_request("sess-a")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = read_response(response).await;
        assert_eq!(body.object_id.as_deref(), Some("review-a"));

        // Session A removed, B still present.
        assert!(state.session_map.get("sess-a").is_none());
        assert!(state.session_map.get("sess-b").is_some());

        // Received the event for A.
        let event_a = rx
            .try_recv()
            .expect("should have received event for sess-a");
        assert_eq!(event_a.session_id, "sess-a");
        assert_eq!(event_a.object_id, "review-a");
        assert_eq!(event_a.mode, AgentMode::Fix);

        // Stop session B.
        let app = router(state.clone());
        let response = app.oneshot(stop_request("sess-b")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = read_response(response).await;
        assert_eq!(body.object_id.as_deref(), Some("review-b"));

        assert!(state.session_map.get("sess-b").is_none());

        let event_b = rx
            .try_recv()
            .expect("should have received event for sess-b");
        assert_eq!(event_b.session_id, "sess-b");
        assert_eq!(event_b.object_id, "review-b");
        assert_eq!(event_b.mode, AgentMode::Implement);
    }
}
