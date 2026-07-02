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

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router, routing};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{broadcast, oneshot};
use uuid::Uuid;

use crate::adapters::agent::SessionMap;
use crate::model::AgentMode;

/// Seconds cockpit waits for a human permission decision before giving up.
///
/// On timeout the broker resolves the request as a denial so the agent's tool
/// call fails cleanly rather than hanging forever.
const PERMISSION_DECISION_TIMEOUT_SECS: u64 = 300;

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
#[derive(Debug, Clone, Serialize)]
pub struct CompletionEvent {
    /// The session ID that completed.
    pub session_id: String,
    /// Identifier of the reviewed object (e.g. a `ReviewId` or `ProjectRef`).
    pub object_id: String,
    /// Which agent mode was running.
    pub mode: AgentMode,
}

// ---------------------------------------------------------------------------
// Permission broker
// ---------------------------------------------------------------------------

/// The outcome of a permission request: allow the tool call, or deny it with a
/// human-readable reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Allow the tool call. The original tool input is echoed back unchanged.
    Allow,
    /// Deny the tool call, carrying a reason surfaced to the agent.
    Deny(String),
}

/// A pending tool-permission request awaiting a human decision.
///
/// Emitted on the broker's broadcast channel so the UI can render it, and
/// resolved later by [`PermissionBroker::resolve`]. Not exported to TS: like
/// [`CompletionEvent`], the app layer hand-types the event payload it forwards
/// to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct PermissionRequest {
    /// Unique id for this request; used to resolve it.
    pub id: String,
    /// Identifier of the reviewed object the requesting agent is working on.
    pub object_id: String,
    /// The tool the agent is asking to run (e.g. `"Write"`, `"Bash"`).
    pub tool_name: String,
    /// The original tool input, passed through opaquely.
    pub input: Value,
    /// Wall-clock time the request arrived, in epoch milliseconds.
    pub requested_at_epoch_ms: u64,
}

/// A registered permission request paired with the waiter its decision goes to.
type PendingEntry = (PermissionRequest, oneshot::Sender<Decision>);

/// Routes headless-agent permission requests to a human and back.
///
/// The MCP `approve` endpoint calls [`request`](Self::request), which registers
/// a one-shot waiter, broadcasts the [`PermissionRequest`] to subscribers (the
/// UI), and blocks until [`resolve`](Self::resolve) delivers a decision or the
/// timeout elapses (a denial). Cloneable: all clones share one registry and one
/// broadcast channel.
#[derive(Clone)]
pub struct PermissionBroker {
    /// Registry of in-flight requests keyed by id, each with its waiter.
    inner: Arc<Mutex<HashMap<String, PendingEntry>>>,
    /// Broadcasts each new request to UI subscribers.
    tx: broadcast::Sender<PermissionRequest>,
    /// How long [`request`](Self::request) waits before denying on timeout.
    timeout: Duration,
}

impl PermissionBroker {
    /// Create a broker with the default 300s decision timeout.
    pub fn new() -> Self {
        Self::with_timeout(Duration::from_secs(PERMISSION_DECISION_TIMEOUT_SECS))
    }

    /// Create a broker with a custom decision timeout.
    ///
    /// Primarily for configuration and tests that need a short timeout; the
    /// default path should use [`new`](Self::new).
    pub fn with_timeout(timeout: Duration) -> Self {
        let (tx, _rx) = broadcast::channel(32);
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            tx,
            timeout,
        }
    }

    /// Subscribe to newly-arriving permission requests.
    pub fn subscribe(&self) -> broadcast::Receiver<PermissionRequest> {
        self.tx.subscribe()
    }

    /// Register `req`, broadcast it, and await a decision (or timeout → deny).
    ///
    /// Cancellation- and lock-safe: the mutex guard is dropped before the
    /// `.await`, so no lock is ever held across a suspension point.
    pub async fn request(&self, req: PermissionRequest) -> Decision {
        let (decision_tx, decision_rx) = oneshot::channel();

        {
            // INVARIANT: guard held only for a HashMap insert; dropped before
            // the await below so no lock crosses a suspension point.
            let mut map = self.inner.lock().expect("permission broker lock poisoned");
            map.insert(req.id.clone(), (req.clone(), decision_tx));
        }

        // Best-effort broadcast: if the UI has not subscribed yet the request
        // still resolves via `resolve`/timeout — it just is not rendered live.
        let _ = self.tx.send(req.clone());

        match tokio::time::timeout(self.timeout, decision_rx).await {
            Ok(Ok(decision)) => decision,
            Ok(Err(_canceled)) => {
                // The waiter's sender was dropped without resolving.
                self.forget(&req.id);
                Decision::Deny("permission request was dropped".to_owned())
            }
            Err(_elapsed) => {
                self.forget(&req.id);
                Decision::Deny("timed out waiting for approval in cockpit".to_owned())
            }
        }
    }

    /// Resolve an outstanding request by id.
    ///
    /// Returns `false` if the id is unknown or was already resolved (so the
    /// caller can surface a stale-decision to the user).
    pub fn resolve(&self, id: &str, allow: bool) -> bool {
        // INVARIANT: guard held only for a HashMap remove — no await.
        let entry = {
            let mut map = self.inner.lock().expect("permission broker lock poisoned");
            map.remove(id)
        };
        match entry {
            Some((_req, decision_tx)) => {
                let decision = if allow {
                    Decision::Allow
                } else {
                    Decision::Deny("denied by reviewer in cockpit".to_owned())
                };
                // Err only if the waiter already timed out and dropped its
                // receiver; treat that as "nothing left to resolve".
                decision_tx.send(decision).is_ok()
            }
            None => false,
        }
    }

    /// Snapshot the currently-pending requests (for rendering the queue).
    pub fn pending(&self) -> Vec<PermissionRequest> {
        // INVARIANT: guard held only for a HashMap scan — no await.
        let map = self.inner.lock().expect("permission broker lock poisoned");
        map.values().map(|(req, _)| req.clone()).collect()
    }

    /// Drop a request from the registry without sending a decision.
    fn forget(&self, id: &str) {
        // INVARIANT: guard held only for a HashMap remove — no await.
        let mut map = self.inner.lock().expect("permission broker lock poisoned");
        map.remove(id);
    }
}

impl Default for PermissionBroker {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// HookState
// ---------------------------------------------------------------------------

/// Shared state for the Stop-hook route, passed to handlers via axum's `State`.
///
/// Deliberately unchanged (only `session_map` and `completion_tx`): the app
/// constructs this with a struct literal, so adding a field here would break the
/// app build, and this unit may not touch app files. The permission broker is
/// therefore threaded through its own [`McpState`] via [`router_with_broker`] /
/// [`serve_with_broker`], which the app adopts in the next unit. Until then the
/// plain [`serve`] path exposes only the Stop-hook route.
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

/// Build the full router: the Stop-hook endpoint plus the MCP permission
/// endpoint backed by `broker`.
///
/// The app adopts this (and [`serve_with_broker`]) once it constructs and
/// subscribes to a [`PermissionBroker`]. Composed by merging two sub-routers
/// so each carries its own state ([`HookState`] and [`McpState`]).
pub fn router_with_broker(state: HookState, broker: PermissionBroker) -> Router {
    router(state).merge(mcp_router(broker))
}

/// Build the MCP permission sub-router (`/mcp/{object_id}`).
fn mcp_router(broker: PermissionBroker) -> Router {
    Router::new()
        .route(
            "/mcp/{object_id}",
            routing::post(handle_mcp).get(handle_mcp_get),
        )
        .with_state(McpState { broker })
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
// MCP permission endpoint
// ---------------------------------------------------------------------------

/// State for the MCP permission sub-router: just the broker.
#[derive(Clone)]
struct McpState {
    /// Broker that routes each permission request to a human decision.
    broker: PermissionBroker,
}

/// A minimal JSON-RPC 2.0 request, permissive about extra fields.
///
/// Only the fields cockpit needs are captured; unknown fields (e.g. `jsonrpc`)
/// are ignored. A missing `id` marks the message as a notification.
#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    /// The JSON-RPC method name.
    method: String,
    /// Method parameters; defaults to null when absent.
    #[serde(default)]
    params: Value,
    /// Request id; absent for notifications.
    #[serde(default)]
    id: Option<Value>,
}

/// Reject `GET` on the MCP endpoint — cockpit does not offer SSE streaming.
async fn handle_mcp_get() -> impl IntoResponse {
    StatusCode::METHOD_NOT_ALLOWED
}

/// Handle a streamable-HTTP MCP JSON-RPC call on `/mcp/{object_id}`.
///
/// Implements the subset the Claude Code permission-prompt tool drives:
/// `initialize`, `notifications/initialized`, `tools/list`, and `tools/call`
/// for the single `approve` tool. Unknown methods yield JSON-RPC error -32601.
async fn handle_mcp(
    State(state): State<McpState>,
    Path(object_id): Path<String>,
    Json(req): Json<JsonRpcRequest>,
) -> Response {
    // A JSON-RPC notification (no id) expects no response body.
    let Some(id) = req.id.clone() else {
        return StatusCode::ACCEPTED.into_response();
    };

    match req.method.as_str() {
        "initialize" => {
            // Echo the client's protocol version when it sends one.
            let protocol_version = req
                .params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("2024-11-05");
            let result = serde_json::json!({
                "protocolVersion": protocol_version,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "cockpit", "version": "0" },
            });
            json_rpc_result(id, result).into_response()
        }
        "tools/list" => {
            let result = serde_json::json!({
                "tools": [{
                    "name": "approve",
                    "description":
                        "Route a Claude Code permission request to cockpit for a human decision.",
                    "inputSchema": { "type": "object", "additionalProperties": true },
                }]
            });
            json_rpc_result(id, result).into_response()
        }
        "tools/call" => handle_tools_call(&state, &object_id, id, &req.params).await,
        _ => json_rpc_error(id, -32601, "method not found").into_response(),
    }
}

/// Handle a `tools/call` for the `approve` tool: block until a human decides.
async fn handle_tools_call(
    state: &McpState,
    object_id: &str,
    id: Value,
    params: &Value,
) -> Response {
    // The CLI nests the original tool name + input inside `arguments`. Be
    // permissive: prefer explicit `tool_name`/`input` keys, else treat the
    // whole arguments object as the input with an "unknown" tool name.
    let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
    let tool_name = arguments
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    let input = arguments
        .get("input")
        .cloned()
        .unwrap_or_else(|| arguments.clone());

    let request = PermissionRequest {
        id: Uuid::new_v4().to_string(),
        object_id: object_id.to_owned(),
        tool_name,
        input: input.clone(),
        requested_at_epoch_ms: epoch_millis(),
    };

    let decision = state.broker.request(request).await;

    // CONTRACT: the SDK permission-prompt docs require the tool result's text
    // content to be a JSON *string* of one of these shapes. A secondary source
    // claimed `{"allowed":bool}` — that is NOT the contract; do not use it. If
    // the runtime disagrees, adjust the two `behavior` objects below.
    let behavior = match decision {
        Decision::Allow => serde_json::json!({ "behavior": "allow", "updatedInput": input }),
        Decision::Deny(message) => serde_json::json!({ "behavior": "deny", "message": message }),
    };

    let result = serde_json::json!({
        "content": [{ "type": "text", "text": behavior.to_string() }],
        "isError": false,
    });
    json_rpc_result(id, result).into_response()
}

/// Build a JSON-RPC 2.0 success envelope.
fn json_rpc_result(id: Value, result: Value) -> Json<Value> {
    Json(serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

/// Build a JSON-RPC 2.0 error envelope.
fn json_rpc_error(id: Value, code: i64, message: &str) -> Json<Value> {
    Json(serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    }))
}

/// Current time in epoch milliseconds, saturating rather than panicking.
fn epoch_millis() -> u64 {
    // `try_from` (not `as`) so the u128→u64 narrowing is explicit; saturate on
    // the impossible overflow rather than risk a panic.
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
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

/// Run the hook server (Stop-hook route only) on `127.0.0.1:{port}`.
///
/// Binds to localhost only (never `0.0.0.0`) and serves until the future is
/// cancelled or an error occurs. Supports graceful shutdown via tokio's
/// cooperative cancellation.
pub async fn serve(state: HookState, port: u16) -> Result<(), Error> {
    bind_and_serve(router(state), port).await
}

/// Run the hook server including the MCP permission endpoint on
/// `127.0.0.1:{port}`.
///
/// Identical to [`serve`] but also mounts `/mcp/{object_id}` backed by `broker`.
/// The app switches to this once it subscribes to a [`PermissionBroker`].
pub async fn serve_with_broker(
    state: HookState,
    broker: PermissionBroker,
    port: u16,
) -> Result<(), Error> {
    bind_and_serve(router_with_broker(state, broker), port).await
}

/// Bind `127.0.0.1:{port}` and serve `router` until cancelled or errored.
async fn bind_and_serve(router: Router, port: u16) -> Result<(), Error> {
    let addr = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|source| Error::Bind {
            addr: addr.clone(),
            source,
        })?;

    axum::serve(listener, router)
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

    /// Build the full router (Stop-hook + MCP) backed by `broker`.
    fn mcp_app(broker: PermissionBroker) -> Router {
        let (state, _rx) = make_state();
        router_with_broker(state, broker)
    }

    /// Build a JSON-RPC POST to `/mcp/{object_id}` with the given body.
    fn mcp_request(object_id: &str, body: Value) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(format!("/mcp/{object_id}"))
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap()
    }

    /// Read the response body as arbitrary JSON.
    async fn read_json(response: http::Response<Body>) -> Value {
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    /// Extract and parse the `behavior` JSON string from a `tools/call` result.
    fn behavior_of(result: &Value) -> Value {
        let text = result["result"]["content"][0]["text"]
            .as_str()
            .expect("text content present");
        serde_json::from_str(text).expect("behavior is a JSON string")
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

    // ---------------------------------------------------------------
    // MCP permission endpoint tests
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn mcp_initialize_handshake() {
        // Echoes a supplied protocol version.
        let app = mcp_app(PermissionBroker::new());
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "capabilities": {} },
        });
        let response = app.oneshot(mcp_request("obj", body)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let json = read_json(response).await;
        assert_eq!(json["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(json["result"]["serverInfo"]["name"], "cockpit");
        assert_eq!(json["result"]["serverInfo"]["version"], "0");
        assert!(json["result"]["capabilities"]["tools"].is_object());

        // Falls back to a default when the client omits the version.
        let app = mcp_app(PermissionBroker::new());
        let body = serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize" });
        let response = app.oneshot(mcp_request("obj", body)).await.unwrap();
        let json = read_json(response).await;
        assert_eq!(json["result"]["protocolVersion"], "2024-11-05");
    }

    #[tokio::test]
    async fn mcp_tools_list_exposes_approve() {
        let app = mcp_app(PermissionBroker::new());
        let body = serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" });
        let response = app.oneshot(mcp_request("obj", body)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let json = read_json(response).await;
        let tools = json["result"]["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "approve");
        assert_eq!(tools[0]["inputSchema"]["type"], "object");
        assert_eq!(
            tools[0]["inputSchema"]["additionalProperties"],
            serde_json::json!(true)
        );
    }

    #[tokio::test]
    async fn mcp_tools_call_allow_echoes_input() {
        let broker = PermissionBroker::new();
        let app = mcp_app(broker.clone());

        let resolver_broker = broker.clone();
        let resolver = tokio::spawn(async move {
            loop {
                if let Some(req) = resolver_broker.pending().first() {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    assert!(resolver_broker.resolve(&req.id, true));
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        });

        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/call",
            "params": {
                "name": "approve",
                "arguments": {
                    "tool_name": "Write",
                    "input": { "file_path": "/x/y.rs", "content": "hi" },
                },
            },
        });
        let response = app.oneshot(mcp_request("review-1", body)).await.unwrap();
        resolver.await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let json = read_json(response).await;
        assert_eq!(json["id"], serde_json::json!(7));
        assert_eq!(json["result"]["isError"], serde_json::json!(false));

        let behavior = behavior_of(&json);
        assert_eq!(behavior["behavior"], "allow");
        // updatedInput echoes the original tool input verbatim.
        assert_eq!(behavior["updatedInput"]["file_path"], "/x/y.rs");
        assert_eq!(behavior["updatedInput"]["content"], "hi");
    }

    #[tokio::test]
    async fn mcp_tools_call_deny_carries_message() {
        let broker = PermissionBroker::new();
        let app = mcp_app(broker.clone());

        let resolver_broker = broker.clone();
        let resolver = tokio::spawn(async move {
            loop {
                if let Some(req) = resolver_broker.pending().first() {
                    assert!(resolver_broker.resolve(&req.id, false));
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        });

        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "approve",
                "arguments": { "tool_name": "Bash", "input": { "command": "rm -rf /" } },
            },
        });
        let response = app.oneshot(mcp_request("obj", body)).await.unwrap();
        resolver.await.unwrap();

        let json = read_json(response).await;
        let behavior = behavior_of(&json);
        assert_eq!(behavior["behavior"], "deny");
        assert_eq!(behavior["message"], "denied by reviewer in cockpit");
        assert!(behavior.get("updatedInput").is_none());
    }

    #[tokio::test]
    async fn mcp_tools_call_times_out_to_deny() {
        // A short-timeout broker with no resolver → deny on timeout.
        let broker = PermissionBroker::with_timeout(Duration::from_millis(50));
        let app = mcp_app(broker);
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "approve",
                "arguments": { "tool_name": "Bash", "input": { "command": "ls" } },
            },
        });
        let response = app.oneshot(mcp_request("obj", body)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let json = read_json(response).await;
        let behavior = behavior_of(&json);
        assert_eq!(behavior["behavior"], "deny");
        assert!(
            behavior["message"]
                .as_str()
                .expect("message")
                .contains("timed out"),
            "expected a timeout message, got {behavior}"
        );
    }

    #[tokio::test]
    async fn mcp_tools_call_falls_back_to_whole_arguments() {
        let broker = PermissionBroker::new();
        let app = mcp_app(broker.clone());

        let resolver_broker = broker.clone();
        let captured = Arc::new(Mutex::new(None));
        let captured_writer = captured.clone();
        let resolver = tokio::spawn(async move {
            loop {
                if let Some(req) = resolver_broker.pending().first() {
                    *captured_writer.lock().unwrap() = Some((
                        req.object_id.clone(),
                        req.tool_name.clone(),
                        req.input.clone(),
                    ));
                    resolver_broker.resolve(&req.id, true);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        });

        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": { "name": "approve", "arguments": { "some": "raw", "fields": 1 } },
        });
        let response = app.oneshot(mcp_request("review-42", body)).await.unwrap();
        resolver.await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let (object_id, tool_name, input) = captured.lock().unwrap().clone().expect("captured");
        // object_id comes from the path; unknown tool falls back; whole
        // arguments object becomes the input.
        assert_eq!(object_id, "review-42");
        assert_eq!(tool_name, "unknown");
        assert_eq!(input, serde_json::json!({ "some": "raw", "fields": 1 }));
    }

    #[tokio::test]
    async fn mcp_notification_returns_202() {
        let app = mcp_app(PermissionBroker::new());
        let body = serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        let response = app.oneshot(mcp_request("obj", body)).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(bytes.is_empty(), "a notification response has no body");
    }

    #[tokio::test]
    async fn mcp_unknown_method_is_method_not_found() {
        let app = mcp_app(PermissionBroker::new());
        let body = serde_json::json!({ "jsonrpc": "2.0", "id": 9, "method": "resources/list" });
        let response = app.oneshot(mcp_request("obj", body)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let json = read_json(response).await;
        assert_eq!(json["id"], serde_json::json!(9));
        assert_eq!(json["error"]["code"], serde_json::json!(-32601));
    }

    #[tokio::test]
    async fn mcp_get_is_method_not_allowed() {
        let app = mcp_app(PermissionBroker::new());
        let request = Request::builder()
            .method("GET")
            .uri("/mcp/obj")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn broker_resolve_unknown_returns_false() {
        let broker = PermissionBroker::new();
        assert!(!broker.resolve("nonexistent", true));
    }

    #[tokio::test]
    async fn broker_pending_lists_and_resolves_once() {
        let broker = PermissionBroker::new();
        let requester = broker.clone();
        let handle = tokio::spawn(async move {
            requester
                .request(PermissionRequest {
                    id: "req-1".into(),
                    object_id: "obj-1".into(),
                    tool_name: "Write".into(),
                    input: serde_json::json!({ "file_path": "/a" }),
                    requested_at_epoch_ms: 0,
                })
                .await
        });

        // Wait for the request to register.
        loop {
            if !broker.pending().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let pending = broker.pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, "req-1");
        assert_eq!(pending[0].object_id, "obj-1");
        assert_eq!(pending[0].tool_name, "Write");

        assert!(broker.resolve("req-1", true));
        let decision = handle.await.unwrap();
        assert_eq!(decision, Decision::Allow);

        // The registry is empty again and a second resolve is a no-op.
        assert!(broker.pending().is_empty());
        assert!(!broker.resolve("req-1", true));
    }
}
