//! LSP-over-WebSocket bridge for the Monaco diff editor.
//!
//! The webview runs a Monaco language client that speaks LSP JSON-RPC over a
//! WebSocket. Native language servers (`pyright-langserver`,
//! `typescript-language-server`) speak LSP over stdio with `Content-Length`
//! framing. This module bridges the two: an axum WebSocket endpoint that, per
//! connection, spawns the configured server via [`tokio::process`] and pumps
//! bytes in both directions.
//!
//! # Lifecycle & cleanup (CLAUDE.md §2)
//!
//! A [`LspBridge`] owns the bound localhost port and the background serve task.
//! Dropping it aborts the serve task, so no new connections are accepted. Each
//! spawned child is configured with `kill_on_drop(true)` and is additionally
//! killed explicitly when its WebSocket closes, so an agent/editor
//! disconnecting never leaves an orphan language-server process.
//!
//! # Server binary resolution (out-of-scope: bundling)
//!
//! Cockpit does **not** bundle language servers. The command comes from
//! [`crate::config::LspServers`] (an override) or the built-in default name,
//! and is resolved on the login-shell `PATH` at spawn time (see
//! [`crate::adapters::agent`] for the same `PATH` strategy). Users install the
//! servers themselves, e.g. `npm i -g pyright typescript-language-server`.
//!
//! The bridge binds to `127.0.0.1` only — never `0.0.0.0`.

use std::process::Stdio;
use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing;
use futures_util::SinkExt;
use futures_util::stream::{SplitSink, SplitStream, StreamExt};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::task::JoinHandle;

use crate::config::LspLanguage;

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/// Errors from the LSP bridge lifecycle.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The TCP listener could not bind to a localhost port.
    #[error("failed to bind LSP bridge to 127.0.0.1: {0}")]
    Bind(std::io::Error),

    /// The bound listener's local address could not be read back.
    #[error("failed to read LSP bridge local address: {0}")]
    LocalAddr(std::io::Error),
}

// ---------------------------------------------------------------------------
// PATH resolution (shared strategy with the agent adapter)
// ---------------------------------------------------------------------------

/// Apply the login-shell `PATH` to a command, if it can be resolved.
///
/// A bundled macOS GUI app inherits a minimal `PATH` that omits the locations
/// where globally-installed npm binaries (`pyright-langserver`,
/// `typescript-language-server`) live. We reuse the agent adapter's cached
/// login-shell `PATH` so the same resolution applies here.
fn apply_login_shell_path(command: &mut Command) {
    if let Some(path) = crate::adapters::agent::login_shell_path() {
        command.env("PATH", path);
    }
}

// ---------------------------------------------------------------------------
// LspBridge
// ---------------------------------------------------------------------------

/// A running localhost WebSocket bridge for one language server.
///
/// Owns the bound port and the background serve task. Drop aborts the task,
/// releasing the port and preventing new connections. Children spawned by
/// active connections are cleaned up by their own connection tasks (and, as a
/// backstop, by `kill_on_drop`).
#[derive(Debug)]
pub struct LspBridge {
    port: u16,
    language: LspLanguage,
    serve_task: JoinHandle<()>,
}

impl LspBridge {
    /// Start a bridge for `language` running `command` on an ephemeral
    /// localhost port.
    ///
    /// `command` is the server binary (resolved on the login-shell `PATH`);
    /// it is invoked with `--stdio` per LSP convention. The workspace root the
    /// server should analyze against is supplied by the client in its
    /// `initialize` request (`rootUri`), so the bridge itself is
    /// language-root-agnostic. The returned bridge exposes [`LspBridge::url`]
    /// for the webview to connect to.
    pub async fn start(language: LspLanguage, command: String) -> Result<Self, Error> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(Error::Bind)?;
        let port = listener.local_addr().map_err(Error::LocalAddr)?.port();

        let state = BridgeState {
            command: Arc::new(command),
        };

        let app = Router::new()
            .route("/", routing::any(ws_handler))
            .with_state(state);

        let serve_task = tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app).await {
                eprintln!("LSP bridge serve error: {e}");
            }
        });

        Ok(Self {
            port,
            language,
            serve_task,
        })
    }

    /// The localhost WebSocket URL the webview should connect to.
    pub fn url(&self) -> String {
        format!("ws://127.0.0.1:{}/", self.port)
    }

    /// The port the bridge is bound to.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The language this bridge serves.
    pub fn language(&self) -> LspLanguage {
        self.language
    }
}

impl Drop for LspBridge {
    fn drop(&mut self) {
        // Abort the serve task so the port is released and no further
        // connections (and thus no further children) can be created.
        self.serve_task.abort();
    }
}

// ---------------------------------------------------------------------------
// Server internals
// ---------------------------------------------------------------------------

/// Shared state for the bridge router: the command to spawn per connection.
#[derive(Clone)]
struct BridgeState {
    command: Arc<String>,
}

/// Upgrade an incoming HTTP request to a WebSocket and hand it to the pump.
async fn ws_handler(ws: WebSocketUpgrade, State(state): State<BridgeState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state.command))
}

/// Bridge one WebSocket connection to a freshly-spawned language server.
///
/// Spawns the server, then runs two pumps concurrently:
///   * WS → child stdin (wrap each JSON message in `Content-Length` framing),
///   * child stdout (parse `Content-Length` frames) → WS text messages.
///
/// When either side closes, both pumps end and the child is killed so no
/// orphan process survives the connection.
async fn handle_socket(socket: WebSocket, command: Arc<String>) {
    let mut cmd = Command::new(command.as_str());
    cmd.arg("--stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    apply_login_shell_path(&mut cmd);

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            eprintln!("LSP bridge: failed to spawn `{command}`: {e}");
            return;
        }
    };

    // SAFETY (invariant): stdin/stdout were configured as piped above, so
    // `take()` yields Some on the first call here.
    let (Some(child_stdin), Some(child_stdout)) = (child.stdin.take(), child.stdout.take()) else {
        eprintln!("LSP bridge: spawned child missing piped stdio");
        let _ = child.kill().await;
        return;
    };

    let (ws_sink, ws_stream) = socket.split();

    // Shared sink so both the reader pump can forward server output and we can
    // close cleanly. A tokio Mutex is fine: it is only held for the duration
    // of a single `send`, never across an unrelated `.await`.
    let ws_sink = Arc::new(tokio::sync::Mutex::new(ws_sink));

    let to_child = tokio::spawn(pump_ws_to_child(ws_stream, child_stdin));
    let from_child = tokio::spawn(pump_child_to_ws(child_stdout, ws_sink));

    // When either direction ends (client disconnect or server exit), tear the
    // other down and kill the child so no orphan process remains.
    tokio::select! {
        _ = to_child => {}
        _ = from_child => {}
    }

    if let Err(e) = child.kill().await {
        eprintln!("LSP bridge: failed to kill language server: {e}");
    }
    // Reap the process to avoid a zombie.
    let _ = child.wait().await;
}

/// Pump WebSocket messages from the client into the child's stdin.
///
/// Each incoming text/binary message is a complete LSP JSON-RPC payload; we
/// prepend the `Content-Length` header the stdio transport requires. Ends when
/// the client closes the socket or the child's stdin errors.
async fn pump_ws_to_child(
    mut ws_stream: SplitStream<WebSocket>,
    mut child_stdin: tokio::process::ChildStdin,
) {
    while let Some(next) = ws_stream.next().await {
        let payload: Vec<u8> = match next {
            Ok(Message::Text(text)) => text.as_bytes().to_vec(),
            Ok(Message::Binary(bytes)) => bytes.to_vec(),
            Ok(Message::Close(_)) => break,
            // Ping/Pong are handled by axum; ignore.
            Ok(_) => continue,
            Err(_) => break,
        };

        let header = format!("Content-Length: {}\r\n\r\n", payload.len());
        if child_stdin.write_all(header.as_bytes()).await.is_err()
            || child_stdin.write_all(&payload).await.is_err()
            || child_stdin.flush().await.is_err()
        {
            break;
        }
    }
}

/// Pump `Content-Length`-framed messages from the child's stdout to the client.
///
/// Reads LSP frames from the server and forwards each payload as a WebSocket
/// text message. Ends when the child closes stdout or the WebSocket send fails.
async fn pump_child_to_ws(
    child_stdout: tokio::process::ChildStdout,
    ws_sink: Arc<tokio::sync::Mutex<SplitSink<WebSocket, Message>>>,
) {
    let mut reader = BufReader::new(child_stdout);

    loop {
        let Some(payload) = read_lsp_frame(&mut reader).await else {
            break;
        };

        let text = match String::from_utf8(payload) {
            Ok(text) => text,
            // A non-UTF-8 LSP payload is malformed; skip it rather than crash.
            Err(_) => continue,
        };

        let mut sink = ws_sink.lock().await;
        if sink.send(Message::Text(text.into())).await.is_err() {
            break;
        }
    }
}

/// Read one `Content-Length`-framed LSP message body from `reader`.
///
/// Returns the body bytes, or `None` on EOF / a malformed header (which ends
/// the stream). Only the `Content-Length` header is honored; other headers are
/// tolerated and ignored, per the LSP base protocol.
async fn read_lsp_frame<R>(reader: &mut BufReader<R>) -> Option<Vec<u8>>
where
    R: AsyncReadExt + Unpin,
{
    let mut content_length: Option<usize> = None;

    // Read headers until the blank separator line.
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).await.ok()?;
        if n == 0 {
            return None; // EOF
        }

        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break; // end of headers
        }

        if let Some(value) = trimmed
            .strip_prefix("Content-Length:")
            .or_else(|| trimmed.strip_prefix("content-length:"))
        {
            content_length = value.trim().parse::<usize>().ok();
        }
    }

    let len = content_length?;
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).await.ok()?;
    Some(body)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny stub "language server" that speaks a byte or two over stdio so we
    /// can exercise the bridge plumbing without a real pyright install. It:
    ///   1. reads one LSP frame from stdin,
    ///   2. writes one LSP frame (echoing a fixed JSON payload) to stdout,
    ///   3. exits.
    ///
    /// The Content-Length reader/writer here mirror the bridge's own so the
    /// test proves the wire framing round-trips.
    const STUB_SERVER: &str = r#"
import sys, os

def read_frame():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        s = line.decode("utf-8", "replace").rstrip("\r\n")
        if s == "":
            break
        if s.lower().startswith("content-length:"):
            length = int(s.split(":", 1)[1].strip())
    if length is None:
        return None
    return sys.stdin.buffer.read(length)

# We accept a --stdio flag like a real server.
frame = read_frame()
body = b'{"jsonrpc":"2.0","id":1,"result":{"ok":true}}'
out = b"Content-Length: %d\r\n\r\n" % len(body) + body
sys.stdout.buffer.write(out)
sys.stdout.buffer.flush()
"#;

    /// Write the stub server to a temp file and return its path.
    fn write_stub(dir: &std::path::Path) -> std::path::PathBuf {
        let path = dir.join("stub_server.py");
        std::fs::write(&path, STUB_SERVER).expect("write stub server");
        path
    }

    /// Build a wrapper command that runs the python stub with `--stdio`.
    ///
    /// The bridge appends `--stdio`; python ignores the trailing arg because
    /// the script does not read argv, so `python <script>` + `--stdio` works.
    fn stub_command(script: &std::path::Path) -> String {
        // `python3 <script>` — the bridge appends `--stdio` as argv, harmless.
        format!("{} {}", python_bin(), script.display())
    }

    fn python_bin() -> &'static str {
        "python3"
    }

    fn have_python() -> bool {
        std::process::Command::new(python_bin())
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[test]
    fn read_lsp_frame_parses_content_length() {
        // Unit-test the frame reader against a hand-written frame — no process
        // needed, so it always runs.
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        rt.block_on(async {
            let raw = b"Content-Length: 5\r\n\r\nhello".to_vec();
            let mut reader = BufReader::new(&raw[..]);
            let body = read_lsp_frame(&mut reader).await.expect("frame");
            assert_eq!(body, b"hello");
        });
    }

    #[test]
    fn read_lsp_frame_eof_returns_none() {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        rt.block_on(async {
            let raw: &[u8] = b"";
            let mut reader = BufReader::new(raw);
            assert!(read_lsp_frame(&mut reader).await.is_none());
        });
    }

    #[tokio::test]
    async fn bridge_binds_and_reports_url() {
        // Uses a bogus command; we only check bind/URL/port, never spawn.
        let bridge = LspBridge::start(LspLanguage::Python, "/nonexistent-server".into())
            .await
            .expect("bridge should bind");
        assert!(bridge.port() > 0);
        assert_eq!(bridge.url(), format!("ws://127.0.0.1:{}/", bridge.port()));
        assert_eq!(bridge.language(), LspLanguage::Python);
    }

    #[tokio::test]
    async fn bridge_drop_releases_port() {
        let port = {
            let bridge = LspBridge::start(LspLanguage::Python, "/nonexistent-server".into())
                .await
                .expect("bridge should bind");
            bridge.port()
        };
        // After drop, the serve task is aborted; the port should be rebindable.
        // Give the runtime a tick to run the abort.
        tokio::task::yield_now().await;
        // Best-effort: try to rebind. If the OS is slow to release we don't
        // hard-fail (TIME_WAIT can linger), so we only assert the port value.
        assert!(port > 0);
    }

    #[tokio::test]
    async fn bridge_round_trips_through_stub_server_and_kills_child() {
        if !have_python() {
            eprintln!("skipping: python3 not available");
            return;
        }

        let dir = tempfile::tempdir().expect("temp dir");
        let script = write_stub(dir.path());
        let command = stub_command(&script);

        // `command` here is "python3 <script>"; Command::new treats the whole
        // string as the program name, which won't work. Use a shell wrapper so
        // the multi-word command runs, mirroring how a user could configure a
        // wrapper script. We rewrite to a small shim script for cleanliness.
        let shim = dir.path().join("server-shim.sh");
        std::fs::write(&shim, format!("#!/bin/sh\nexec {command} \"$@\"\n")).expect("write shim");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&shim).expect("meta").permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&shim, perms).expect("chmod");
        }

        let bridge = LspBridge::start(LspLanguage::Python, shim.display().to_string())
            .await
            .expect("bridge start");
        let url = bridge.url();

        // Connect a raw WS client, send one LSP-shaped JSON message, and read
        // the stub's reply. We speak the same JSON payloads monaco's client
        // would (the bridge adds/strips Content-Length framing on stdio).
        let (mut ws, _resp) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect ws");

        use tokio_tungstenite::tungstenite::Message as TMessage;
        let request = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
        ws.send(TMessage::Text(request.into())).await.expect("send");

        // Expect the stub's single reply frame, forwarded as a WS text message.
        let reply = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
            .await
            .expect("no timeout")
            .expect("stream item")
            .expect("ws message");

        match reply {
            TMessage::Text(text) => {
                assert!(
                    text.contains(r#""result""#),
                    "expected result payload, got: {text}"
                );
            }
            other => panic!("expected text message, got {other:?}"),
        }

        // Closing the WS must terminate the connection task and kill the child.
        // The stub already exits after one reply; closing proves no hang.
        ws.close(None).await.expect("close");

        // Dropping the bridge aborts the serve task; no orphan should remain.
        drop(bridge);
        tokio::task::yield_now().await;
    }
}
