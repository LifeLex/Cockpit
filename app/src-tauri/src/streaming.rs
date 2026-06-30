//! Agent stdout streaming — tees JSONL lines to a log file and emits
//! parsed [`Event`]s as Tauri events for the frontend.
//!
//! When an agent is spawned with `--output-format stream-json`, its stdout
//! is piped. This module reads that pipe line by line, writes each raw line
//! to the log file, parses it into a [`cockpit_core::adapters::agent_stream::Event`],
//! and emits it as a `"agent-event"` Tauri event for the frontend to render.

use std::path::PathBuf;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use cockpit_core::adapters::agent::SpawnResult;
use cockpit_core::adapters::agent_stream;

/// Spawn a background task that reads agent stdout, tees to the log file,
/// and emits parsed events to the frontend.
///
/// Takes ownership of the child process from the [`SpawnResult`]. Returns
/// the [`cockpit_core::model::AgentRun`] for the caller to store on the
/// reviewed object.
pub fn start_stream_forwarding(
    mut spawn_result: SpawnResult,
    app_handle: tauri::AppHandle,
) -> cockpit_core::model::AgentRun {
    let agent_run = spawn_result.run.clone();
    let log_path = spawn_result.log_path.clone();

    tauri::async_runtime::spawn(async move {
        stream_agent_output(&mut spawn_result.child, &log_path, &app_handle).await;
    });

    agent_run
}

/// Read the child's stdout line by line, write each line to the log file,
/// and emit parsed events via the Tauri event system.
async fn stream_agent_output(
    child: &mut tokio::process::Child,
    log_path: &PathBuf,
    app_handle: &tauri::AppHandle,
) {
    use tauri::Emitter;

    // Take stdout from the child. If piped stdout is unavailable, nothing
    // to stream.
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => return,
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

        // Parse and emit.
        if let Some(event) = agent_stream::parse_stream_line(&line) {
            // Best-effort: if no frontend window is listening, the event is
            // simply dropped.
            let _ = app_handle.emit("agent-event", &event);
        }
    }

    // Flush log writer on exit.
    if let Some(ref mut writer) = log_writer {
        let _ = writer.flush().await;
    }
}
