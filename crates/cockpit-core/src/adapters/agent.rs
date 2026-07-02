//! Agent adapter — spawn and track Claude Code agent processes.
//!
//! Spawns `claude` CLI processes in worktrees, tracks their PIDs and sessions,
//! and provides the session-to-object mapping that the Stop-hook listener
//! (T1.3) uses to route completion callbacks. See `SPEC.md` §11 and §14.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use tokio::process::Command;
use uuid::Uuid;

use crate::config::{self, AgentPermissionMode};
use crate::model::{AgentMode, AgentRun};
use crate::prompt::AssembledPrompt;

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/// Errors from agent spawning and tracking.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The `claude` (or substitute) process could not be started.
    #[error("failed to spawn agent process: {0}")]
    SpawnFailed(String),

    /// The worktree directory passed to [`spawn_agent`] does not exist.
    #[error("worktree path does not exist: {0}")]
    WorktreeNotFound(PathBuf),

    /// A session with this ID is already registered in the [`SessionMap`].
    #[error("session {0} already registered")]
    SessionConflict(String),

    /// The agent process exited with an error.
    #[error("agent process failed (pid {pid}): {message}")]
    ProcessFailed {
        /// OS process ID.
        pid: u32,
        /// Human-readable failure description.
        message: String,
    },

    /// Underlying I/O error.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// Failed to resolve a cockpit path (e.g. the logs directory).
    #[error(transparent)]
    Config(#[from] config::Error),
}

// ---------------------------------------------------------------------------
// SessionMap
// ---------------------------------------------------------------------------

/// An entry in the [`SessionMap`] linking a session to its reviewed object.
#[derive(Debug, Clone)]
pub struct SessionEntry {
    /// Identifier for the reviewed object (e.g. a `ReviewId` or `ProjectRef`).
    pub object_id: String,
    /// Which agent mode is running.
    pub mode: AgentMode,
    /// OS process ID of the spawned agent.
    pub pid: u32,
}

/// Tracks which agent session maps to which reviewed object and mode.
///
/// Thread-safe: accessed from the spawn path and the Stop-hook callback.
/// Uses `std::sync::Mutex` because the lock is held only for trivial
/// `HashMap` operations (no `.await` while locked).
#[derive(Debug, Clone)]
pub struct SessionMap {
    inner: Arc<Mutex<HashMap<String, SessionEntry>>>,
}

impl SessionMap {
    /// Create an empty session map.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Register a session. Errors if the session ID is already present.
    pub fn register(&self, session_id: String, entry: SessionEntry) -> Result<(), Error> {
        // INVARIANT: lock is held only for a HashMap insert — no .await, no
        // blocking I/O — so poisoning should not occur in practice.
        let mut map = self.inner.lock().expect("session map lock poisoned");
        if map.contains_key(&session_id) {
            return Err(Error::SessionConflict(session_id));
        }
        map.insert(session_id, entry);
        Ok(())
    }

    /// Remove a session entry, returning it if it existed.
    pub fn remove(&self, session_id: &str) -> Option<SessionEntry> {
        // INVARIANT: lock is held only for a HashMap op — no .await, no
        // blocking I/O — so poisoning should not occur in practice.
        let mut map = self.inner.lock().expect("session map lock poisoned");
        map.remove(session_id)
    }

    /// Look up a session entry without removing it.
    pub fn get(&self, session_id: &str) -> Option<SessionEntry> {
        // INVARIANT: lock is held only for a HashMap op — no .await, no
        // blocking I/O — so poisoning should not occur in practice.
        let map = self.inner.lock().expect("session map lock poisoned");
        map.get(session_id).cloned()
    }

    /// Find the session ID for a given reviewed object.
    ///
    /// Returns `None` if no session is registered for that object.
    /// If multiple sessions exist for the same object (shouldn't happen in
    /// normal operation), returns an arbitrary one.
    pub fn find_by_object(&self, object_id: &str) -> Option<String> {
        // INVARIANT: lock is held only for a HashMap op — no .await, no
        // blocking I/O — so poisoning should not occur in practice.
        let map = self.inner.lock().expect("session map lock poisoned");
        map.iter()
            .find(|(_, entry)| entry.object_id == object_id)
            .map(|(session_id, _)| session_id.clone())
    }
}

impl Default for SessionMap {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// SpawnConfig
// ---------------------------------------------------------------------------

/// Configuration for agent spawning, allowing tests to substitute the command.
#[derive(Debug, Clone)]
pub struct SpawnConfig {
    /// The command to run. Defaults to `"claude"`.
    pub command: String,
    /// Arguments placed before the prompt text.
    pub base_args: Vec<String>,
    /// Arguments placed after the prompt text.
    pub tail_args: Vec<String>,
}

impl Default for SpawnConfig {
    fn default() -> Self {
        Self {
            command: "claude".into(),
            base_args: vec!["--print".into(), "-p".into()],
            tail_args: vec![
                "--output-format".into(),
                "stream-json".into(),
                "--verbose".into(),
            ],
        }
    }
}

impl SpawnConfig {
    /// Build a [`SpawnConfig`] honoring the user's configured agent command.
    ///
    /// Takes the command from [`crate::config::Config::agent_command`] (which
    /// itself defaults to `"claude"`) so a user override in
    /// `~/.cockpit/config.toml` is actually applied at spawn time. The Claude
    /// CLI argument shape is unchanged from [`SpawnConfig::default`].
    pub fn from_config(config: &config::Config) -> Self {
        Self {
            command: config.agent_command.clone(),
            ..Self::default()
        }
    }

    /// Grant the spawned agent write access to a directory outside its
    /// worktree by appending `--add-dir <path>`.
    ///
    /// Headless (`--print`) sessions cannot interactively grant permissions,
    /// so any output contract that lives outside the worktree — the findings
    /// file, the plan document — must be pre-authorized at spawn time or the
    /// agent's writes are silently blocked and its output is lost.
    #[must_use]
    pub fn with_extra_dir(mut self, dir: &Path) -> Self {
        self.tail_args.push("--add-dir".into());
        self.tail_args.push(dir.to_string_lossy().into_owned());
        self
    }

    /// Auto-approve agent writes anywhere under `dir` by adding an
    /// `--allowedTools` rule of the form `Write(//<abs-dir>/**)`.
    ///
    /// In headless (`--print`) mode there is no interactive permission prompt,
    /// so path-scoped `--allowedTools` rules are how a write is pre-authorized.
    /// The `//` prefix anchors the rule to the filesystem root and `**` crosses
    /// subdirectories, so any file beneath `dir` is writable. `dir` should be an
    /// absolute path; its leading slash is folded into the `//` anchor and any
    /// trailing slash is stripped so the emitted rule is well-formed.
    ///
    /// The Claude CLI treats a repeated `--allowedTools` flag as a *replacement*
    /// rather than an accumulation, so when a rule already exists this merges the
    /// new one into it comma-separated (a single flag carrying all rules) instead
    /// of passing the flag twice.
    #[must_use]
    pub fn allow_write_under(mut self, dir: &Path) -> Self {
        let dir_str = dir.to_string_lossy();
        // CONTRACT: Claude Code permission rules use gitignore-style paths where
        // `//path` anchors to the filesystem root (docs example:
        // `Read(//Users/alice/secrets/**)`). We therefore fold the absolute
        // dir's leading slash into the `//` anchor and append `**` to cross
        // subdirectories. Adjust here if the CLI's path syntax changes.
        let anchored = dir_str.trim_end_matches('/').trim_start_matches('/');
        let rule = format!("Write(//{anchored}/**)");

        if let Some(pos) = self
            .tail_args
            .iter()
            .position(|arg| arg == "--allowedTools")
            && let Some(existing) = self.tail_args.get_mut(pos + 1)
        {
            existing.push(',');
            existing.push_str(&rule);
            return self;
        }

        self.tail_args.push("--allowedTools".into());
        self.tail_args.push(rule);
        self
    }

    /// Apply the configured [`AgentPermissionMode`] to this spawn configuration.
    ///
    /// - [`AgentPermissionMode::Bypass`] pushes `--dangerously-skip-permissions`
    ///   (equivalent to `--permission-mode bypassPermissions`): no checks run.
    /// - [`AgentPermissionMode::AcceptEdits`] pushes `--permission-mode
    ///   acceptEdits`: file edits auto-approve, everything else uses the default
    ///   policy.
    /// - [`AgentPermissionMode::Approve`] pushes `--permission-mode default`.
    ///   When `approve_url` is `Some`, it additionally routes permission prompts
    ///   to cockpit's MCP `approve` tool (`--permission-prompt-tool
    ///   mcp__cockpit__approve` plus a `--mcp-config` pointing at the
    ///   streamable-HTTP endpoint). When `approve_url` is `None` there is no
    ///   server to reach, so it falls back to plain default mode with no prompt
    ///   tool — callers should always pass the URL so a request can reach a
    ///   human rather than being silently denied.
    #[must_use]
    pub fn apply_permission_mode(
        mut self,
        mode: AgentPermissionMode,
        approve_url: Option<&str>,
    ) -> Self {
        match mode {
            AgentPermissionMode::Bypass => {
                self.tail_args.push("--dangerously-skip-permissions".into());
            }
            AgentPermissionMode::AcceptEdits => {
                self.tail_args.push("--permission-mode".into());
                self.tail_args.push("acceptEdits".into());
            }
            AgentPermissionMode::Approve => {
                self.tail_args.push("--permission-mode".into());
                self.tail_args.push("default".into());
                if let Some(url) = approve_url {
                    self.tail_args.push("--permission-prompt-tool".into());
                    self.tail_args.push("mcp__cockpit__approve".into());
                    self.tail_args.push("--mcp-config".into());
                    // Build the inline MCP config with serde_json so the URL is
                    // JSON-escaped rather than string-formatted into the arg.
                    let mcp_config = serde_json::json!({
                        "mcpServers": {
                            "cockpit": {
                                "type": "http",
                                "url": url,
                            }
                        }
                    });
                    self.tail_args.push(mcp_config.to_string());
                }
            }
        }
        self
    }
}

// ---------------------------------------------------------------------------
// Spawn
// ---------------------------------------------------------------------------

/// The result of spawning an agent: the run descriptor plus the child
/// process whose stdout can be read for streaming JSONL events.
#[derive(Debug)]
pub struct SpawnResult {
    /// Metadata about the agent run (pid, mode, log path, etc.).
    pub run: AgentRun,
    /// The child process. Stdout is piped so callers can read JSONL lines;
    /// stderr goes to the log file.
    pub child: tokio::process::Child,
    /// Path to the log file that should receive a copy of each stdout line.
    pub log_path: PathBuf,
}

/// Spawn a Claude Code agent in the given worktree.
///
/// The agent runs the assembled prompt in the specified mode. Its session
/// is registered in the session map so the Stop-hook can route completion.
///
/// Returns a [`SpawnResult`] containing the run descriptor and the child
/// process. Stdout is piped so the caller can read JSONL lines for
/// streaming UI updates (and tee each line to the log file).
pub async fn spawn_agent(
    worktree_path: &Path,
    prompt: &AssembledPrompt,
    mode: AgentMode,
    object_id: &str,
    session_map: &SessionMap,
    hook_url: &str,
    spawn_config: &SpawnConfig,
) -> Result<SpawnResult, Error> {
    // 1. Verify worktree exists.
    tokio::fs::metadata(worktree_path)
        .await
        .map_err(|_| Error::WorktreeNotFound(worktree_path.to_path_buf()))?;

    // 2. Generate a unique session ID.
    let session_id = Uuid::new_v4().to_string();

    // 3. Prepare the log directory and file. Logs live under the cockpit
    //    home (not inside the worktree) so they survive worktree cleanup.
    let logs_dir = config::logs_dir()?;
    tokio::fs::create_dir_all(&logs_dir).await?;
    let log_path = logs_dir.join(format!("agent-{session_id}.log"));
    let stderr_file = std::fs::File::create(&log_path)?;

    // 4. Build and spawn the command.
    //    Stdout is piped so we can parse JSONL lines and tee to the log.
    //    Stderr goes directly to the log file.
    let mut command = Command::new(&spawn_config.command);
    command
        .args(&spawn_config.base_args)
        .arg(&prompt.text)
        .args(&spawn_config.tail_args)
        .current_dir(worktree_path)
        .env("CLAUDE_STOP_HOOK_URL", hook_url)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::from(stderr_file));

    // Auth is the user's own Claude Code login (`~/.claude`): no API key and
    // no Agent SDK. A bundled macOS GUI app does not inherit the terminal
    // PATH, so `~/.local/bin/claude` would be invisible. Resolve the PATH
    // from a login shell and set it explicitly; fall back to the inherited
    // PATH if resolution fails.
    if let Some(path) = login_shell_path() {
        command.env("PATH", path);
    }

    let child = command
        .spawn()
        .map_err(|e| Error::SpawnFailed(e.to_string()))?;

    let pid = child
        .id()
        .ok_or_else(|| Error::SpawnFailed("process exited before PID could be read".into()))?;

    // 5. Register the session.
    session_map.register(
        session_id,
        SessionEntry {
            object_id: object_id.to_string(),
            mode,
            pid,
        },
    )?;

    // 6. Return the run descriptor and child process.
    let run = AgentRun {
        pid,
        mode,
        started_at: SystemTime::now(),
        prompt_hash: prompt.hash.clone(),
        log_path: log_path.clone(),
    };

    Ok(SpawnResult {
        run,
        child,
        log_path,
    })
}

// ---------------------------------------------------------------------------
// Login-shell PATH resolution
// ---------------------------------------------------------------------------

/// Cache for the login-shell PATH, resolved at most once per process.
static LOGIN_SHELL_PATH: OnceLock<Option<String>> = OnceLock::new();

/// Resolve the PATH as seen by the user's login shell, cached for the process.
///
/// Cockpit spawns the `claude` CLI using the user's own Claude Code login
/// (`~/.claude`) — there is no API key and no Agent SDK. A bundled macOS GUI
/// app is launched by `launchd` and inherits a minimal PATH that omits
/// `~/.local/bin`, where `claude` is commonly installed. To find it, we run
/// the user's login shell once (`$SHELL -lic 'printf %s "$PATH"'`) and capture
/// the resulting PATH.
///
/// Returns `None` if `$SHELL` is unset, the shell fails, or the output is
/// empty; callers should then fall back to the inherited PATH.
///
/// Exposed to the crate so other spawn sites (e.g. the LSP bridge) share the
/// same cached resolution.
pub(crate) fn login_shell_path() -> Option<String> {
    LOGIN_SHELL_PATH
        .get_or_init(resolve_login_shell_path)
        .clone()
}

/// Perform the actual login-shell PATH capture (uncached).
fn resolve_login_shell_path() -> Option<String> {
    let shell = std::env::var("SHELL").ok()?;

    // Blocking `std::process` is intentional here: this runs at most once per
    // process, guarded by a `OnceLock`, and the result is cached. It is not on
    // an async hot path.
    let output = std::process::Command::new(shell)
        .args(["-lic", "printf %s \"$PATH\""])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if path.is_empty() { None } else { Some(path) }
}

// ---------------------------------------------------------------------------
// Kill
// ---------------------------------------------------------------------------

/// Send SIGTERM to an agent process.
///
/// Uses `kill -TERM` on Unix. Returns an error if the signal cannot be sent.
pub async fn kill_agent(pid: u32) -> Result<(), Error> {
    let status = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .await?;

    if !status.success() {
        return Err(Error::ProcessFailed {
            pid,
            message: format!("kill -TERM exited with {status}"),
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------
    // SessionMap tests
    // ---------------------------------------------------------------

    #[test]
    fn session_map_register_and_get() {
        let map = SessionMap::new();
        let entry = SessionEntry {
            object_id: "review-1".into(),
            mode: AgentMode::Fix,
            pid: 42,
        };
        map.register("s-1".into(), entry).unwrap();

        let got = map.get("s-1").unwrap();
        assert_eq!(got.object_id, "review-1");
        assert_eq!(got.mode, AgentMode::Fix);
        assert_eq!(got.pid, 42);
    }

    #[test]
    fn session_map_conflict() {
        let map = SessionMap::new();
        let entry = SessionEntry {
            object_id: "review-1".into(),
            mode: AgentMode::Fix,
            pid: 42,
        };
        map.register("s-1".into(), entry.clone()).unwrap();

        let err = map.register("s-1".into(), entry).unwrap_err();
        assert!(
            matches!(err, Error::SessionConflict(ref id) if id == "s-1"),
            "expected SessionConflict, got {err:?}"
        );
    }

    #[test]
    fn session_map_remove() {
        let map = SessionMap::new();
        let entry = SessionEntry {
            object_id: "review-1".into(),
            mode: AgentMode::Plan,
            pid: 99,
        };
        map.register("s-2".into(), entry).unwrap();

        let removed = map.remove("s-2").unwrap();
        assert_eq!(removed.object_id, "review-1");

        assert!(
            map.get("s-2").is_none(),
            "entry should be gone after remove"
        );
    }

    #[test]
    fn session_map_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SessionMap>();
    }

    // ---------------------------------------------------------------
    // SpawnConfig tests
    // ---------------------------------------------------------------

    #[test]
    fn spawn_config_from_config_honors_agent_command() {
        let config = config::Config {
            agent_command: "my-claude".into(),
            ..config::Config::default()
        };
        let spawn = SpawnConfig::from_config(&config);
        assert_eq!(spawn.command, "my-claude");
        // Argument shape is unchanged from the default.
        assert_eq!(spawn.base_args, SpawnConfig::default().base_args);
        assert_eq!(spawn.tail_args, SpawnConfig::default().tail_args);
    }

    #[test]
    fn spawn_config_with_extra_dir_appends_add_dir_args() {
        let spawn = SpawnConfig::default()
            .with_extra_dir(Path::new("/home/u/.cockpit/findings"))
            .with_extra_dir(Path::new("/home/u/.cockpit/plans"));
        let tail: Vec<&str> = spawn.tail_args.iter().map(String::as_str).collect();
        // Base tail args are preserved, followed by one --add-dir pair per dir.
        let default_len = SpawnConfig::default().tail_args.len();
        assert_eq!(
            &tail[..default_len],
            ["--output-format", "stream-json", "--verbose"]
        );
        assert_eq!(
            &tail[default_len..],
            [
                "--add-dir",
                "/home/u/.cockpit/findings",
                "--add-dir",
                "/home/u/.cockpit/plans"
            ]
        );
    }

    #[test]
    fn spawn_config_from_default_config_is_claude() {
        let config = config::Config::default();
        let spawn = SpawnConfig::from_config(&config);
        assert_eq!(spawn.command, "claude");
    }

    #[test]
    fn allow_write_under_appends_anchored_rule() {
        let spawn = SpawnConfig::default().allow_write_under(Path::new("/home/u/.cockpit/plans"));
        let default_len = SpawnConfig::default().tail_args.len();
        let tail: Vec<&str> = spawn.tail_args.iter().map(String::as_str).collect();
        assert_eq!(
            &tail[default_len..],
            ["--allowedTools", "Write(//home/u/.cockpit/plans/**)"]
        );
    }

    #[test]
    fn allow_write_under_strips_trailing_slash() {
        let spawn = SpawnConfig::default().allow_write_under(Path::new("/home/u/plans/"));
        assert!(
            spawn
                .tail_args
                .iter()
                .any(|a| a == "Write(//home/u/plans/**)"),
            "trailing slash must not double up, got: {:?}",
            spawn.tail_args
        );
    }

    #[test]
    fn allow_write_under_merges_repeated_rules() {
        let spawn = SpawnConfig::default()
            .allow_write_under(Path::new("/a/plans"))
            .allow_write_under(Path::new("/b/findings"));
        // Exactly one --allowedTools flag, carrying both rules comma-separated.
        let flags = spawn
            .tail_args
            .iter()
            .filter(|a| *a == "--allowedTools")
            .count();
        assert_eq!(
            flags, 1,
            "rules must merge into one flag: {:?}",
            spawn.tail_args
        );
        let pos = spawn
            .tail_args
            .iter()
            .position(|a| a == "--allowedTools")
            .expect("flag present");
        assert_eq!(
            spawn.tail_args[pos + 1],
            "Write(//a/plans/**),Write(//b/findings/**)"
        );
    }

    #[test]
    fn allow_write_under_combines_with_extra_dir() {
        let spawn = SpawnConfig::default()
            .with_extra_dir(Path::new("/out/findings"))
            .allow_write_under(Path::new("/out/findings"));
        let default_len = SpawnConfig::default().tail_args.len();
        let tail: Vec<&str> = spawn.tail_args.iter().map(String::as_str).collect();
        assert_eq!(
            &tail[default_len..],
            [
                "--add-dir",
                "/out/findings",
                "--allowedTools",
                "Write(//out/findings/**)"
            ]
        );
    }

    #[test]
    fn apply_permission_mode_bypass_skips_permissions() {
        let spawn = SpawnConfig::default().apply_permission_mode(AgentPermissionMode::Bypass, None);
        let default_len = SpawnConfig::default().tail_args.len();
        assert_eq!(
            &spawn.tail_args[default_len..],
            ["--dangerously-skip-permissions"]
        );
    }

    #[test]
    fn apply_permission_mode_accept_edits() {
        let spawn = SpawnConfig::default()
            .apply_permission_mode(AgentPermissionMode::AcceptEdits, Some("http://x/mcp/o"));
        let default_len = SpawnConfig::default().tail_args.len();
        // acceptEdits ignores the approve_url — it never routes to cockpit.
        assert_eq!(
            &spawn.tail_args[default_len..],
            ["--permission-mode", "acceptEdits"]
        );
    }

    #[test]
    fn apply_permission_mode_approve_with_url_wires_mcp() {
        let spawn = SpawnConfig::default().apply_permission_mode(
            AgentPermissionMode::Approve,
            Some("http://127.0.0.1:19876/mcp/review-1"),
        );
        let default_len = SpawnConfig::default().tail_args.len();
        let tail = &spawn.tail_args[default_len..];
        assert_eq!(tail[0], "--permission-mode");
        assert_eq!(tail[1], "default");
        assert_eq!(tail[2], "--permission-prompt-tool");
        assert_eq!(tail[3], "mcp__cockpit__approve");
        assert_eq!(tail[4], "--mcp-config");

        // The inline config is valid JSON pointing at the approve URL.
        let parsed: serde_json::Value = serde_json::from_str(&tail[5]).expect("valid mcp json");
        assert_eq!(
            parsed["mcpServers"]["cockpit"]["type"].as_str(),
            Some("http")
        );
        assert_eq!(
            parsed["mcpServers"]["cockpit"]["url"].as_str(),
            Some("http://127.0.0.1:19876/mcp/review-1")
        );
    }

    #[test]
    fn apply_permission_mode_approve_without_url_falls_back_to_default() {
        let spawn =
            SpawnConfig::default().apply_permission_mode(AgentPermissionMode::Approve, None);
        let default_len = SpawnConfig::default().tail_args.len();
        // Without a server URL, no prompt tool is wired — plain default mode.
        assert_eq!(
            &spawn.tail_args[default_len..],
            ["--permission-mode", "default"]
        );
    }

    // ---------------------------------------------------------------
    // Spawn tests
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn spawn_with_stub_command() {
        let dir = tempfile::tempdir().unwrap();
        // Isolate cockpit home so logs land in a temp dir, not the real
        // ~/.cockpit. COCKPIT_HOME is read by config::logs_dir(). temp_env
        // serializes env access and restores it afterward, avoiding the
        // `unsafe` set_var forbidden by the crate-wide `forbid(unsafe_code)`.
        let home = tempfile::tempdir().unwrap();
        temp_env::async_with_vars([("COCKPIT_HOME", Some(home.path()))], async {
            let session_map = SessionMap::new();

            let prompt = AssembledPrompt {
                text: "hello".into(),
                hash: "abc123hash".into(),
            };

            let config = SpawnConfig {
                command: "echo".into(),
                base_args: vec![],
                tail_args: vec![],
            };

            let result = spawn_agent(
                dir.path(),
                &prompt,
                AgentMode::Fix,
                "review-42",
                &session_map,
                "http://localhost:9999/hook/stop",
                &config,
            )
            .await
            .unwrap();

            let run = &result.run;
            assert_eq!(run.mode, AgentMode::Fix);
            assert_eq!(run.prompt_hash, "abc123hash");
            assert!(run.pid > 0);
            assert!(run.log_path.starts_with(home.path().join("logs")));

            // The session map should have exactly one entry with the right
            // object_id. We don't know the session_id (it's a UUID), so we
            // verify via the invariant that the map has one entry whose
            // object_id matches.
            let map = session_map.inner.lock().unwrap();
            assert_eq!(map.len(), 1);
            let entry = map.values().next().unwrap();
            assert_eq!(entry.object_id, "review-42");
            assert_eq!(entry.mode, AgentMode::Fix);
            assert_eq!(entry.pid, run.pid);
        })
        .await;
    }

    #[tokio::test]
    async fn spawn_bad_worktree() {
        let session_map = SessionMap::new();
        let prompt = AssembledPrompt {
            text: "hello".into(),
            hash: "abc123hash".into(),
        };
        let config = SpawnConfig {
            command: "echo".into(),
            base_args: vec![],
            tail_args: vec![],
        };

        let err = spawn_agent(
            Path::new("/nonexistent/worktree/path"),
            &prompt,
            AgentMode::Fix,
            "review-99",
            &session_map,
            "http://localhost:9999/hook/stop",
            &config,
        )
        .await
        .unwrap_err();

        assert!(
            matches!(err, Error::WorktreeNotFound(ref p) if p == Path::new("/nonexistent/worktree/path")),
            "expected WorktreeNotFound, got {err:?}"
        );
    }
}
