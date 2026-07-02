//! Agent stdout streaming — tees JSONL lines to a log file and emits
//! parsed [`Event`]s as Tauri events for the frontend.
//!
//! When an agent is spawned with `--output-format stream-json`, its stdout
//! is piped. This module reads that pipe line by line, writes each raw line
//! to the log file, parses it into a [`cockpit_core::adapters::agent_stream::Event`],
//! and emits it wrapped in an [`AgentEventEnvelope`] on the `"agent-event"`
//! Tauri channel — the envelope carries the object's UI key so the frontend
//! can tell which review/plan a given event belongs to.
//!
//! When the stream ends (agent process exits), a [`CompletionEvent`] is
//! emitted on the broadcast channel so the completion handler in `lib.rs`
//! can reconcile the review state. The completion *outcome* is decided by git
//! HEAD in `lib.rs`, not by the stream — an agent can claim success on stdout
//! while committing nothing — so the child's exit status and any observed
//! [`Event::Error`](agent_stream::Event::Error) are captured here only as
//! advisory diagnostics, never threaded into the (authoritative) completion.

use std::path::PathBuf;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use cockpit_core::adapters::agent::SpawnResult;
use cockpit_core::adapters::agent_stream;
use cockpit_core::hook_server::CompletionEvent;
use cockpit_core::model::AgentMode;

/// Envelope wrapping a parsed agent [`Event`](agent_stream::Event) with the UI
/// key of the object it belongs to, emitted on the `"agent-event"` channel.
///
/// Hand-typed on the frontend (no `ts-rs` binding) to mirror how other Tauri
/// payloads (e.g. the shell output payload) are threaded across the boundary.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AgentEventEnvelope {
    /// UI key of the object this event belongs to: a review's PR ref for review
    /// agents, or the project id for plan agents.
    pub object_id: String,
    /// The parsed agent stream event.
    pub event: agent_stream::Event,
}

/// Emit a parsed agent [`Event`](agent_stream::Event) wrapped in an
/// [`AgentEventEnvelope`] on the `"agent-event"` channel, keyed by `object_id`.
///
/// Best-effort: if no frontend window is listening, the event is dropped.
pub fn emit_agent_event(
    app_handle: &tauri::AppHandle,
    object_id: &str,
    event: agent_stream::Event,
) {
    use tauri::Emitter;
    let envelope = AgentEventEnvelope {
        object_id: object_id.to_string(),
        event,
    };
    let _ = app_handle.emit("agent-event", &envelope);
}

/// Context needed by the streaming task to emit a completion event
/// when the agent stream ends.
pub struct StreamContext {
    /// PR ref or object identifier for the reviewed object.
    pub object_id: String,
    /// Agent mode (Fix, Plan, etc.).
    pub mode: AgentMode,
    /// Completion channel sender.
    pub completion_tx: tokio::sync::broadcast::Sender<CompletionEvent>,
}

/// Spawn a background task that reads agent stdout, tees to the log file,
/// emits parsed events to the frontend, and fires a completion event when
/// the stream ends.
///
/// Takes ownership of the child process from the [`SpawnResult`]. Returns
/// the [`cockpit_core::model::AgentRun`] for the caller to store on the
/// reviewed object.
pub fn start_stream_forwarding(
    mut spawn_result: SpawnResult,
    app_handle: tauri::AppHandle,
    ctx: StreamContext,
) -> cockpit_core::model::AgentRun {
    let agent_run = spawn_result.run.clone();
    let log_path = spawn_result.log_path.clone();

    tauri::async_runtime::spawn(async move {
        let saw_error = stream_agent_output(
            &mut spawn_result.child,
            &log_path,
            &app_handle,
            &ctx.object_id,
        )
        .await;

        // Wait for the child process to fully exit.
        let exit_status = spawn_result.child.wait().await;

        // Advisory only: the completion outcome is decided by git HEAD in
        // lib.rs (an agent can claim success while committing nothing), so we
        // log an abnormal exit / observed stream error for diagnostics but do
        // NOT thread it into CompletionEvent — the HEAD check is authoritative.
        let clean_exit = matches!(&exit_status, Ok(status) if status.success());
        if saw_error || !clean_exit {
            eprintln!(
                "agent stream for {} ended abnormally (exit: {exit_status:?}, stream_error: {saw_error})",
                ctx.object_id
            );
        }

        // Emit a completion event so the handler in lib.rs reconciles the
        // reviewed object's state against git HEAD.
        let event = CompletionEvent {
            session_id: String::new(),
            object_id: ctx.object_id,
            mode: ctx.mode,
        };
        let _ = ctx.completion_tx.send(event);
    });

    agent_run
}

/// Read the child's stdout line by line, write each line to the log file,
/// and emit parsed events (wrapped in an [`AgentEventEnvelope`] keyed by
/// `object_id`) via the Tauri event system.
///
/// Returns `true` if an [`Event::Error`](agent_stream::Event::Error) was seen
/// in the stream — advisory diagnostic only; the completion outcome is decided
/// by git HEAD in `lib.rs`.
async fn stream_agent_output(
    child: &mut tokio::process::Child,
    log_path: &PathBuf,
    app_handle: &tauri::AppHandle,
    object_id: &str,
) -> bool {
    // Take stdout from the child. If piped stdout is unavailable, nothing
    // to stream.
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => return false,
    };

    // Open the log file for appending raw lines.
    let log_file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .await;

    let mut log_writer = match log_file {
        Ok(f) => Some(tokio::io::BufWriter::new(f)),
        Err(_) => None,
    };

    let reader = BufReader::new(stdout);
    let mut lines = reader.lines();

    let mut saw_error = false;
    loop {
        let line = match lines.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) => break, // EOF — process exited.
            Err(_) => break,   // Read error — process likely exited.
        };

        // Tee raw line to the log file.
        if let Some(ref mut writer) = log_writer {
            // Best-effort: don't fail the stream if the log write fails.
            let _ = writer.write_all(line.as_bytes()).await;
            let _ = writer.write_all(b"\n").await;
            let _ = writer.flush().await;
        }

        // Parse and emit, wrapped in the object-keyed envelope.
        if let Some(event) = agent_stream::parse_stream_line(&line) {
            if matches!(event, agent_stream::Event::Error { .. }) {
                saw_error = true;
            }
            emit_agent_event(app_handle, object_id, event);
        }
    }

    // Flush log writer on exit.
    if let Some(ref mut writer) = log_writer {
        let _ = writer.flush().await;
    }

    saw_error
}
