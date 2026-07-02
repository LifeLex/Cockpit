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
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use cockpit_core::adapters::agent::SpawnResult;
use cockpit_core::adapters::agent_stream;
use cockpit_core::diff_signals::CommandRun;
use cockpit_core::hook_server::CompletionEvent;
use cockpit_core::model::AgentMode;
use cockpit_core::trajectory::{self, TrajectorySummary};

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

/// A [`CommandRun`] whose outcome may still be pending a matching tool result.
///
/// `ok` is `None` until the [`agent_stream::Event::ToolResult`] carrying the
/// same `tool_use_id` arrives; it is resolved to a concrete value at
/// [`TrajectoryAccumulator::finish`].
struct PendingCommand {
    /// The tool-use id that a later `ToolResult` matches against.
    tool_use_id: String,
    /// The command string (already summarised by the stream parser).
    command: String,
    /// Whether the command succeeded, once its result is seen.
    ok: Option<bool>,
}

/// Folds an agent's JSONL event stream into a [`TrajectorySummary`].
///
/// The accumulator observes each parsed [`agent_stream::Event`] as it streams
/// (see [`stream_agent_output`]) and, at [`Self::finish`], produces the compact
/// summary persisted by [`trajectory::save`].
struct TrajectoryAccumulator {
    /// Wall-clock start of the stream, used as a duration fallback when the
    /// terminal `Complete` event carries no usable duration.
    started_at: Instant,
    /// Count of [`agent_stream::Event::ToolUse`] events seen. Subagent spawns
    /// (promoted `Agent`/`Skill`/`Task` tools) are a distinct event and are not
    /// counted here, matching "tools used".
    tools_used: u32,
    /// Bash command runs recorded from `ToolUse`, resolved by later results.
    commands: Vec<PendingCommand>,
    /// Duration from the terminal `Complete` event, if it reported one.
    complete_duration_ms: Option<u64>,
    /// Final text from the `Complete` event (preferred final message).
    complete_text: Option<String>,
    /// Most recent non-empty `Text` event (fallback final message).
    last_text: Option<String>,
}

/// Upper bound on a recorded command string's length.
///
/// The stream parser already summarises Bash inputs; this is a defensive cap so
/// a pathological summary cannot bloat the persisted trajectory. The full
/// command lives verbatim in the raw log.
const MAX_COMMAND_CHARS: usize = 200;

impl TrajectoryAccumulator {
    /// Start a fresh accumulator, anchoring the wall-clock duration fallback.
    fn new() -> Self {
        Self {
            started_at: Instant::now(),
            tools_used: 0,
            commands: Vec::new(),
            complete_duration_ms: None,
            complete_text: None,
            last_text: None,
        }
    }

    /// Fold a single parsed event into the running summary.
    fn observe(&mut self, event: &agent_stream::Event) {
        match event {
            agent_stream::Event::ToolUse {
                id,
                name,
                input_summary,
            } => {
                self.tools_used += 1;
                // Bash is the only tool whose input is a command line; the
                // stream parser exposes it as the tool's `input_summary`.
                if name == "Bash" {
                    self.commands.push(PendingCommand {
                        tool_use_id: id.clone(),
                        command: cap_command(input_summary),
                        ok: None,
                    });
                }
            }
            agent_stream::Event::ToolResult {
                tool_use_id,
                success,
                ..
            } => {
                // Match the most recent unresolved command with this id.
                if let Some(pending) = self
                    .commands
                    .iter_mut()
                    .rev()
                    .find(|c| c.tool_use_id == *tool_use_id && c.ok.is_none())
                {
                    pending.ok = Some(*success);
                }
            }
            agent_stream::Event::Text { content } if !content.is_empty() => {
                self.last_text = Some(content.clone());
            }
            agent_stream::Event::Complete {
                duration_ms,
                result_text,
                ..
            } => {
                self.complete_duration_ms = Some(*duration_ms);
                if !result_text.is_empty() {
                    self.complete_text = Some(result_text.clone());
                }
            }
            _ => {}
        }
    }

    /// Consume the accumulator and produce the summary to persist.
    ///
    /// `mode` comes from the [`StreamContext`], not the stream. Duration
    /// prefers the `Complete` event's value, falling back to measured
    /// wall-clock when that is absent or reported as zero (a zero duration means
    /// the result event omitted the field). A command with no matching result is
    /// recorded as `ok = true`: the process ended without that tool failing
    /// loudly, so we do not flag it.
    fn finish(self, mode: AgentMode) -> TrajectorySummary {
        let duration_ms = self
            .complete_duration_ms
            .filter(|&d| d > 0)
            .unwrap_or_else(|| {
                u64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u64::MAX)
            });

        let final_text = self.complete_text.or(self.last_text).unwrap_or_default();

        let commands = self
            .commands
            .into_iter()
            .map(|c| CommandRun {
                command: c.command,
                ok: c.ok.unwrap_or(true),
            })
            .collect();

        TrajectorySummary {
            mode,
            tools_used: self.tools_used,
            commands,
            duration_ms,
            final_text,
            ended_at_epoch_ms: now_epoch_ms(),
        }
    }
}

/// Cap a recorded command at [`MAX_COMMAND_CHARS`] characters (char-based, so a
/// multi-byte code point is never split).
fn cap_command(command: &str) -> String {
    if command.chars().count() <= MAX_COMMAND_CHARS {
        command.to_string()
    } else {
        command.chars().take(MAX_COMMAND_CHARS).collect()
    }
}

/// The current time in epoch milliseconds, or `0` if the clock is before the
/// Unix epoch (not reachable in practice).
fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
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
        let (saw_error, accumulator) = stream_agent_output(
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

        // Persist a compact trajectory summary before signalling completion.
        // Best-effort: a save failure is logged but must never block the
        // completion handler in lib.rs.
        let summary = accumulator.finish(ctx.mode);
        if let Err(err) = trajectory::save(&ctx.object_id, &summary) {
            eprintln!("failed to persist trajectory for {}: {err}", ctx.object_id);
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
/// Returns a pair of:
/// - `true` if an [`Event::Error`](agent_stream::Event::Error) was seen in the
///   stream — advisory diagnostic only; the completion outcome is decided by git
///   HEAD in `lib.rs`;
/// - the [`TrajectoryAccumulator`] folded from the stream, ready for
///   [`TrajectoryAccumulator::finish`].
async fn stream_agent_output(
    child: &mut tokio::process::Child,
    log_path: &PathBuf,
    app_handle: &tauri::AppHandle,
    object_id: &str,
) -> (bool, TrajectoryAccumulator) {
    let mut accumulator = TrajectoryAccumulator::new();

    // Take stdout from the child. If piped stdout is unavailable, nothing
    // to stream.
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => return (false, accumulator),
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
            // Fold into the trajectory before the event is moved into emit.
            accumulator.observe(&event);
            emit_agent_event(app_handle, object_id, event);
        }
    }

    // Flush log writer on exit.
    if let Some(ref mut writer) = log_writer {
        let _ = writer.flush().await;
    }

    (saw_error, accumulator)
}
