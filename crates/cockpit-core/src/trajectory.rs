//! Compact per-run agent trajectory summaries persisted alongside logs (D2).
//!
//! Each agent run produces one [`TrajectorySummary`] — a glanceable rollup of
//! what the agent did (how many tools it invoked, which commands it ran and
//! whether they passed, how long it took, and its final message). The summary
//! is written atomically to `<logs_dir>/<slug>.trajectory.json`, next to the
//! raw JSONL log for the same object. It is deliberately small: the full,
//! verbatim agent output already lives in the raw log, so this file only needs
//! to carry enough for the UI to render a summary card without reparsing the
//! whole stream.
//!
//! Loading never panics (Invariant §0.1): a missing or corrupt file yields
//! `None` so the app degrades to "no summary available" rather than failing.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::diff_signals::CommandRun;
use crate::model::AgentMode;

/// Maximum number of characters retained in [`TrajectorySummary::final_text`].
///
/// The summary is meant to be glanceable; a runaway final message would bloat
/// the file and the UI card. The full, untruncated text always lives verbatim
/// in the raw JSONL log, so capping here loses nothing recoverable.
const MAX_FINAL_TEXT_CHARS: usize = 4000;

/// Fallback filename stem when an object id sanitizes to nothing usable.
const SLUG_FALLBACK: &str = "trajectory";

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/// Errors from persisting a [`TrajectorySummary`].
///
/// Loading deliberately has no error type: [`load`] never fails loudly
/// (Invariant §0.1) and returns `None` for every non-success outcome.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The trajectory path could not be resolved (no home directory).
    #[error("could not resolve trajectory path: {0}")]
    Path(#[from] crate::config::Error),

    /// Writing the trajectory file to disk failed.
    #[error("trajectory I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Serializing the summary to JSON failed.
    #[error("trajectory serialize error: {0}")]
    Serialize(#[from] serde_json::Error),
}

// ---------------------------------------------------------------------------
// TrajectorySummary
// ---------------------------------------------------------------------------

/// A compact rollup of a single agent run, persisted for the UI to render.
///
/// Timestamps use epoch milliseconds (a plain `u64`) rather than a
/// `SystemTime`, so the value crosses the IPC boundary as a JS `number` without
/// a bespoke serde shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub struct TrajectorySummary {
    /// The mode the agent ran in.
    pub mode: AgentMode,
    /// Number of tool invocations the agent made.
    pub tools_used: u32,
    /// Commands the agent ran, each paired with its observed outcome.
    ///
    /// Only commands whose matching tool result was actually observed appear
    /// here, so a `✓`/`✗` in the UI always reflects a confirmed outcome.
    pub commands: Vec<CommandRun>,
    /// Commands the agent started that never received a matching tool result
    /// before the stream ended.
    ///
    /// Kept as a count (not folded into [`Self::commands`]) so an unconfirmed
    /// run is neither inflated to a false `✓` nor flagged as a false `✗`.
    /// `#[serde(default)]` so summaries written before this field existed still
    /// load (Invariant §0.1).
    #[serde(default)]
    pub unresolved_commands: u32,
    /// Wall-clock duration of the run in milliseconds.
    #[ts(type = "number")]
    pub duration_ms: u64,
    /// The agent's final message, capped at [`MAX_FINAL_TEXT_CHARS`] on save.
    pub final_text: String,
    /// When the run ended, in epoch milliseconds.
    #[ts(type = "number")]
    pub ended_at_epoch_ms: u64,
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// Resolve the trajectory file path for `object_id`.
///
/// The convention mirrors [`crate::config::findings_file_path`]:
/// `<logs_dir>/<slug>.trajectory.json`, where `<slug>` is `object_id` with
/// filesystem-hostile characters replaced by `-` so an arbitrary PR reference
/// (e.g. `owner/repo#42`) or project id yields one safe filename. Like the
/// config path helpers, this only composes the path — it does not create the
/// directory.
pub fn trajectory_file_path(object_id: &str) -> Result<PathBuf, crate::config::Error> {
    let slug = path_slug(object_id, SLUG_FALLBACK);
    Ok(crate::config::logs_dir()?.join(format!("{slug}.trajectory.json")))
}

/// Turn an arbitrary id into a single filesystem-safe filename stem.
///
/// Every non-alphanumeric character becomes `-`; an id that reduces to only
/// separators (or is empty) falls back to `fallback`. This mirrors the
/// sanitization in [`crate::config`] (whose `path_slug` is private) so both
/// modules produce identical, predictable filenames.
fn path_slug(id: &str, fallback: &str) -> String {
    let slug: String = id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    if slug.trim_matches('-').is_empty() {
        fallback.to_owned()
    } else {
        slug
    }
}

// ---------------------------------------------------------------------------
// Save / load
// ---------------------------------------------------------------------------

/// Atomically persist `summary` for `object_id`.
///
/// Creates the logs directory if needed, caps
/// [`TrajectorySummary::final_text`] at [`MAX_FINAL_TEXT_CHARS`], serializes to
/// pretty JSON, writes to a sibling `.tmp` file, then renames it over the final
/// path. The rename is atomic within a single directory on every supported
/// platform, so a reader never observes a partially-written document.
pub fn save(object_id: &str, summary: &TrajectorySummary) -> Result<(), Error> {
    let final_path = trajectory_file_path(object_id)?;
    if let Some(parent) = final_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let capped = cap_final_text(summary);
    let json = serde_json::to_string_pretty(&capped)?;

    let tmp_path = final_path.with_extension("json.tmp");
    std::fs::write(&tmp_path, json)?;
    std::fs::rename(&tmp_path, &final_path)?;

    Ok(())
}

/// Load the persisted [`TrajectorySummary`] for `object_id`, if any.
///
/// Returns `None` when the path cannot be resolved, the file is missing, or its
/// contents cannot be parsed. This never panics: an unreadable or corrupt file
/// resolves to `None` so a caller can always continue (Invariant §0.1).
pub fn load(object_id: &str) -> Option<TrajectorySummary> {
    let path = trajectory_file_path(object_id).ok()?;
    let content = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Return a copy of `summary` with [`TrajectorySummary::final_text`] truncated
/// to [`MAX_FINAL_TEXT_CHARS`] characters.
///
/// Truncation is char-based (not byte-based) so a multi-byte code point is never
/// split. The full text remains in the raw log, so nothing is lost.
fn cap_final_text(summary: &TrajectorySummary) -> TrajectorySummary {
    let mut out = summary.clone();
    if out.final_text.chars().count() > MAX_FINAL_TEXT_CHARS {
        out.final_text = out.final_text.chars().take(MAX_FINAL_TEXT_CHARS).collect();
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a small, fully-populated summary for round-trip tests.
    fn sample_summary() -> TrajectorySummary {
        TrajectorySummary {
            mode: AgentMode::Fix,
            tools_used: 7,
            commands: vec![
                CommandRun {
                    command: "cargo test".into(),
                    ok: true,
                },
                CommandRun {
                    command: "cargo clippy".into(),
                    ok: false,
                },
            ],
            unresolved_commands: 1,
            duration_ms: 15_800,
            final_text: "Fixed the failing assertion.".into(),
            ended_at_epoch_ms: 1_720_000_000_000,
        }
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempfile::tempdir().expect("temp dir");
        temp_env::with_var("COCKPIT_HOME", Some(dir.path().as_os_str()), || {
            let summary = sample_summary();
            save("owner/repo#1", &summary).expect("save should succeed");
            let loaded = load("owner/repo#1").expect("load should return the saved summary");
            assert_eq!(loaded, summary);
        });
    }

    #[test]
    fn load_missing_file_returns_none() {
        let dir = tempfile::tempdir().expect("temp dir");
        temp_env::with_var("COCKPIT_HOME", Some(dir.path().as_os_str()), || {
            assert!(load("never-saved").is_none());
        });
    }

    #[test]
    fn load_corrupt_file_returns_none() {
        let dir = tempfile::tempdir().expect("temp dir");
        temp_env::with_var("COCKPIT_HOME", Some(dir.path().as_os_str()), || {
            let path = trajectory_file_path("obj-corrupt").expect("resolve path");
            std::fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
            std::fs::write(&path, "{ this is not valid json").expect("write corrupt file");
            assert!(load("obj-corrupt").is_none());
        });
    }

    #[test]
    fn save_caps_final_text() {
        let dir = tempfile::tempdir().expect("temp dir");
        temp_env::with_var("COCKPIT_HOME", Some(dir.path().as_os_str()), || {
            let mut summary = sample_summary();
            summary.final_text = "x".repeat(MAX_FINAL_TEXT_CHARS + 500);
            save("obj-cap", &summary).expect("save should succeed");
            let loaded = load("obj-cap").expect("load should return the summary");
            assert_eq!(loaded.final_text.chars().count(), MAX_FINAL_TEXT_CHARS);
        });
    }

    #[test]
    fn trajectory_file_path_uses_logs_dir_and_slug() {
        temp_env::with_var("COCKPIT_HOME", Some("/tmp/cockpit-home-test"), || {
            let path = trajectory_file_path("obj-1").expect("should resolve");
            assert_eq!(
                path,
                PathBuf::from("/tmp/cockpit-home-test/logs/obj-1.trajectory.json")
            );
        });
    }

    #[test]
    fn trajectory_file_path_sanitizes_hostile_ids() {
        temp_env::with_var("COCKPIT_HOME", Some("/tmp/cockpit-home-test"), || {
            // A `owner/repo#42` PR ref must collapse to one safe filename.
            let path = trajectory_file_path("owner/repo#42").expect("should resolve");
            assert_eq!(
                path,
                PathBuf::from("/tmp/cockpit-home-test/logs/owner-repo-42.trajectory.json")
            );
        });
    }

    #[test]
    fn trajectory_file_path_falls_back_for_empty_slug() {
        temp_env::with_var("COCKPIT_HOME", Some("/tmp/cockpit-home-test"), || {
            let path = trajectory_file_path("///").expect("should resolve");
            assert_eq!(
                path,
                PathBuf::from("/tmp/cockpit-home-test/logs/trajectory.trajectory.json")
            );
        });
    }

    #[test]
    fn summary_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<TrajectorySummary>();
    }
}
