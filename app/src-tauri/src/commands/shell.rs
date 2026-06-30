//! PTY-backed shell commands for the embedded terminal.
//!
//! Each shell session is a real PTY child process. The frontend sends
//! keystrokes via `shell_write`, and output is pushed back through
//! Tauri events (`"shell-output"`). Sessions are identified by a
//! caller-provided string ID (typically `crypto.randomUUID()` from
//! the frontend).

use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;

use base64::Engine;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Mutex;

/// A running shell session backed by a PTY.
pub struct ShellSession {
    /// Writer handle to send input to the PTY.
    writer: Box<dyn std::io::Write + Send>,
    /// Master PTY handle, kept alive for resize operations.
    master: Box<dyn MasterPty + Send>,
    /// Child process handle, kept alive so the shell keeps running.
    _child: Box<dyn portable_pty::Child + Send + Sync>,
}

/// Thread-safe map of active shell sessions.
///
/// Managed as Tauri state so all commands can access it.
pub type ShellSessions = Arc<Mutex<HashMap<String, ShellSession>>>;

/// Payload emitted to the frontend on `"shell-output"` events.
#[derive(Debug, Clone, Serialize)]
pub struct ShellOutputPayload {
    /// Session ID this output belongs to.
    pub id: String,
    /// Base64-encoded output bytes from the PTY.
    pub data: String,
}

/// Spawn a new shell in the given working directory.
///
/// Opens a PTY, starts the user's shell (from `$SHELL`, defaulting
/// to `/bin/zsh`), and begins forwarding output as Tauri events.
#[tauri::command]
pub async fn spawn_shell(
    id: String,
    cwd: String,
    sessions: State<'_, ShellSessions>,
    app: AppHandle,
) -> Result<(), String> {
    let pty_system = native_pty_system();

    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("failed to open PTY: {e}"))?;

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let mut cmd = CommandBuilder::new(&shell);
    cmd.cwd(&cwd);

    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("failed to spawn shell: {e}"))?;

    // The slave is consumed by spawn_command; we only need the master.
    drop(pair.slave);

    let writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("failed to get PTY writer: {e}"))?;

    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("failed to get PTY reader: {e}"))?;

    let session = ShellSession {
        writer,
        master: pair.master,
        _child: child,
    };

    {
        let mut map = sessions.lock().await;
        map.insert(id.clone(), session);
    }

    // Spawn a background task to read PTY output and emit events.
    // The task runs on a blocking thread because PTY reads are
    // synchronous I/O and must not block the tokio runtime.
    let event_id = id.clone();
    tokio::task::spawn_blocking(move || {
        read_pty_output(reader, &event_id, &app);
    });

    Ok(())
}

/// Continuously read from the PTY reader and emit output events.
///
/// Runs on a blocking thread. Stops when the PTY reader returns EOF
/// (child process exited) or an I/O error occurs.
fn read_pty_output(mut reader: Box<dyn Read + Send>, id: &str, app: &AppHandle) {
    let engine = base64::engine::general_purpose::STANDARD;
    let mut buf = [0u8; 4096];

    loop {
        match reader.read(&mut buf) {
            Ok(0) => break, // EOF — child exited
            Ok(n) => {
                let encoded = engine.encode(&buf[..n]);
                let payload = ShellOutputPayload {
                    id: id.to_string(),
                    data: encoded,
                };
                // Best-effort: if no frontend is listening, the event
                // is silently dropped.
                let _ = app.emit("shell-output", &payload);
            }
            Err(e) => {
                // EPIPE or similar — the child is gone.
                let _ = app.emit(
                    "shell-output",
                    &ShellOutputPayload {
                        id: id.to_string(),
                        data: engine.encode(format!("\r\n[shell exited: {e}]\r\n").as_bytes()),
                    },
                );
                break;
            }
        }
    }
}

/// Write data to a running shell session (keystrokes from the frontend).
///
/// The `data` parameter is a raw UTF-8 string of the characters typed.
#[tauri::command]
pub async fn shell_write(
    id: String,
    data: String,
    sessions: State<'_, ShellSessions>,
) -> Result<(), String> {
    let mut map = sessions.lock().await;
    let session = map
        .get_mut(&id)
        .ok_or_else(|| format!("shell session not found: {id}"))?;

    session
        .writer
        .write_all(data.as_bytes())
        .map_err(|e| format!("failed to write to shell: {e}"))?;

    session
        .writer
        .flush()
        .map_err(|e| format!("failed to flush shell writer: {e}"))?;

    Ok(())
}

/// Resize a running shell session's PTY.
///
/// Called when the frontend terminal container changes size.
#[tauri::command]
pub async fn shell_resize(
    id: String,
    cols: u16,
    rows: u16,
    sessions: State<'_, ShellSessions>,
) -> Result<(), String> {
    let map = sessions.lock().await;
    let session = map
        .get(&id)
        .ok_or_else(|| format!("shell session not found: {id}"))?;

    session
        .master
        .resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("failed to resize shell: {e}"))?;

    Ok(())
}

/// Kill a running shell session and clean up resources.
///
/// Removes the session from the map, which drops the writer, master,
/// and child handles — causing the PTY to close and the child to
/// receive SIGHUP.
#[tauri::command]
pub async fn shell_kill(id: String, sessions: State<'_, ShellSessions>) -> Result<(), String> {
    let mut map = sessions.lock().await;
    map.remove(&id)
        .ok_or_else(|| format!("shell session not found: {id}"))?;
    Ok(())
}
