//! Agent adapter — spawn and track Claude Code agent processes.
//!
//! Spawns `claude` CLI processes in worktrees, tracks their PIDs and sessions,
//! and provides the session-to-object mapping that the Stop-hook listener
//! (T1.3) uses to route completion callbacks. See `SPEC.md` §11 and §14.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use tokio::process::Command;
use uuid::Uuid;

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
        let mut map = self.inner.lock().expect("session map lock poisoned");
        map.remove(session_id)
    }

    /// Look up a session entry without removing it.
    pub fn get(&self, session_id: &str) -> Option<SessionEntry> {
        let map = self.inner.lock().expect("session map lock poisoned");
        map.get(session_id).cloned()
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
            tail_args: vec!["--output-format".into(), "json".into()],
        }
    }
}

// ---------------------------------------------------------------------------
// Spawn
// ---------------------------------------------------------------------------

/// Spawn a Claude Code agent in the given worktree.
///
/// The agent runs the assembled prompt in the specified mode. Its session
/// is registered in the session map so the Stop-hook can route completion.
///
/// The process runs detached: this function returns immediately after
/// spawning. The Stop-hook listener (T1.3) handles completion.
pub async fn spawn_agent(
    worktree_path: &Path,
    prompt: &AssembledPrompt,
    mode: AgentMode,
    object_id: &str,
    session_map: &SessionMap,
    hook_url: &str,
    config: &SpawnConfig,
) -> Result<AgentRun, Error> {
    // 1. Verify worktree exists.
    tokio::fs::metadata(worktree_path)
        .await
        .map_err(|_| Error::WorktreeNotFound(worktree_path.to_path_buf()))?;

    // 2. Generate a unique session ID.
    let session_id = Uuid::new_v4().to_string();

    // 3. Prepare the log directory and file.
    let cockpit_dir = worktree_path.join(".cockpit");
    tokio::fs::create_dir_all(&cockpit_dir).await?;
    let log_path = cockpit_dir.join(format!("agent-{session_id}.log"));
    let log_file = std::fs::File::create(&log_path)?;
    let stderr_file = log_file.try_clone()?;

    // 4. Build and spawn the command.
    let child = Command::new(&config.command)
        .args(&config.base_args)
        .arg(&prompt.text)
        .args(&config.tail_args)
        .current_dir(worktree_path)
        .env("CLAUDE_STOP_HOOK_URL", hook_url)
        .stdout(std::process::Stdio::from(log_file))
        .stderr(std::process::Stdio::from(stderr_file))
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

    // 6. Return the run descriptor.
    Ok(AgentRun {
        pid,
        mode,
        started_at: SystemTime::now(),
        prompt_hash: prompt.hash.clone(),
        log_path,
    })
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
    // Spawn tests
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn spawn_with_stub_command() {
        let dir = tempfile::tempdir().unwrap();
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

        let run = spawn_agent(
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

        assert_eq!(run.mode, AgentMode::Fix);
        assert_eq!(run.prompt_hash, "abc123hash");
        assert!(run.pid > 0);
        assert!(run.log_path.starts_with(dir.path().join(".cockpit")));

        // The session map should have exactly one entry with the right object_id.
        // We don't know the session_id (it's a UUID), so we verify via the
        // invariant that the map has one entry whose object_id matches.
        let map = session_map.inner.lock().unwrap();
        assert_eq!(map.len(), 1);
        let entry = map.values().next().unwrap();
        assert_eq!(entry.object_id, "review-42");
        assert_eq!(entry.mode, AgentMode::Fix);
        assert_eq!(entry.pid, run.pid);
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
