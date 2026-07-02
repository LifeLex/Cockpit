//! On-disk persistence of cockpit session state (D5).
//!
//! The whole session — active [`Review`]s and first-class [`Project`]s — is
//! serialized to a single JSON document at `<cockpit_home>/state.json` (see
//! [`crate::config::cockpit_home`]). Writes are atomic: state is written to
//! `state.json.tmp` first and then renamed over `state.json`, so a crash
//! mid-write can never leave a torn file.
//!
//! Loading never panics (Invariant §0.1). A missing file yields `None`, and a
//! corrupt or version-mismatched file is moved aside to `state.json.corrupt`
//! (best-effort) before `None` is returned — the app starts clean while the bad
//! document is preserved on disk for manual inspection. There is deliberately
//! no logging call here: cockpit-core pulls in no logging crate, and the
//! renamed `state.json.corrupt` is itself the durable record of the failure.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::model::{Project, Review};

/// Current schema version of the persisted state document.
///
/// Bumped whenever [`PersistedState`]'s layout changes incompatibly. A loaded
/// file whose `version` differs is treated as unreadable (see [`load`]).
pub const STATE_VERSION: u32 = 1;

/// File name of the persisted state document under the cockpit home directory.
const STATE_FILE: &str = "state.json";
/// Temp file name used for the atomic write-then-rename.
const STATE_TMP_FILE: &str = "state.json.tmp";
/// File name the bad document is moved to when it cannot be loaded.
const STATE_CORRUPT_FILE: &str = "state.json.corrupt";

/// The full serializable snapshot of a cockpit session.
///
/// Persisted as pretty JSON so it stays human-inspectable on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedState {
    /// Schema version; see [`STATE_VERSION`].
    pub version: u32,
    /// All active reviews at save time.
    pub reviews: Vec<Review>,
    /// All first-class projects at save time.
    pub projects: Vec<Project>,
}

/// Errors from saving cockpit state to disk.
///
/// Loading deliberately has no error type: [`load`] never fails loudly
/// (Invariant §0.1) and returns `None` for every non-success outcome.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Writing the state files to disk failed.
    #[error("state I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Serializing the state to JSON failed.
    #[error("state serialize error: {0}")]
    Serialize(#[from] serde_json::Error),
}

/// Atomically save `state` to `<home>/state.json`.
///
/// Creates `home` if it does not exist, serializes `state` to pretty JSON,
/// writes it to `<home>/state.json.tmp`, then renames that over
/// `<home>/state.json`. The rename is atomic within a single directory on
/// every supported platform, so a reader never observes a partially-written
/// document.
pub fn save_atomic(home: &Path, state: &PersistedState) -> Result<(), Error> {
    std::fs::create_dir_all(home)?;

    let json = serde_json::to_string_pretty(state)?;

    let tmp_path = home.join(STATE_TMP_FILE);
    let final_path = home.join(STATE_FILE);

    std::fs::write(&tmp_path, json)?;
    std::fs::rename(&tmp_path, &final_path)?;

    Ok(())
}

/// Load the persisted state from `<home>/state.json`, if any.
///
/// Returns:
/// - `Some(state)` when the file exists, parses, and its `version` matches
///   [`STATE_VERSION`];
/// - `None` when the file is missing (first launch — nothing to load);
/// - `None` after moving the file aside to `<home>/state.json.corrupt` when it
///   cannot be parsed or its `version` does not match [`STATE_VERSION`].
///
/// This function never panics: an unreadable file, a parse failure, or a failed
/// rename all resolve to `None` so the app can always start (Invariant §0.1).
pub fn load(home: &Path) -> Option<PersistedState> {
    let path = home.join(STATE_FILE);

    // A missing (or otherwise unreadable) file is not an error: start clean.
    // We do not move anything aside here — there may be nothing to move.
    let content = std::fs::read_to_string(&path).ok()?;

    match serde_json::from_str::<PersistedState>(&content) {
        Ok(state) if state.version == STATE_VERSION => Some(state),
        _ => {
            // Corrupt JSON or an incompatible schema version. Preserve the bad
            // document for inspection and start clean. Best-effort: a failed
            // rename must not stop the app from launching.
            let corrupt_path = home.join(STATE_CORRUPT_FILE);
            let _ = std::fs::rename(&path, &corrupt_path);
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::model::{
        DiffData, GateState, IssueRef, PrRef, Project, ProjectId, ProjectSource, Review, ReviewId,
        ReviewSource,
    };

    /// Build a minimal `Review` with the given PR number.
    fn make_review(pr_num: u64) -> Review {
        Review {
            id: ReviewId::new(format!("r-{pr_num}")),
            issue: IssueRef::new(format!("ISSUE-{pr_num}")),
            pr: PrRef::new(format!("owner/repo#{pr_num}")),
            title: String::new(),
            body: String::new(),
            branch: format!("alejandro/test-{pr_num}"),
            base: "main".into(),
            base_sha: "000".into(),
            source: ReviewSource::Frontier,
            worktree: PathBuf::from(format!("/tmp/wt-{pr_num}")),
            gate_state: GateState::Pending,
            diff: DiffData { raw: String::new() },
            head_sha: "abc123".into(),
            comments: vec![],
            parents: vec![],
            children: vec![],
            stale: false,
            agent: None,
            repo_slug: None,
            project: None,
            dispatch_snapshot: None,
        }
    }

    /// Build a minimal ad-hoc `Project` with the given id.
    fn make_project(id: &str) -> Project {
        Project {
            id: ProjectId::new(id),
            name: format!("Project {id}"),
            source: ProjectSource::AdHoc,
            plan: None,
        }
    }

    fn sample_state() -> PersistedState {
        PersistedState {
            version: STATE_VERSION,
            reviews: vec![make_review(1), make_review(2)],
            projects: vec![make_project("p-1")],
        }
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().expect("temp dir");
        let state = sample_state();

        save_atomic(dir.path(), &state).expect("save should succeed");
        let loaded = load(dir.path()).expect("load should return the saved state");

        assert_eq!(loaded, state);
    }

    #[test]
    fn save_creates_home_directory() {
        let dir = tempfile::tempdir().expect("temp dir");
        // A nested home directory that does not yet exist.
        let home = dir.path().join("nested").join("cockpit");
        assert!(!home.exists());

        save_atomic(&home, &sample_state()).expect("save should create the home dir");
        assert!(home.join("state.json").exists());
    }

    #[test]
    fn load_missing_file_returns_none() {
        let dir = tempfile::tempdir().expect("temp dir");
        // No state.json written.
        assert!(load(dir.path()).is_none());
        // A missing file must not spawn a corrupt sidecar.
        assert!(!dir.path().join("state.json.corrupt").exists());
    }

    #[test]
    fn load_corrupt_json_returns_none_and_moves_file_aside() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("state.json");
        std::fs::write(&path, "{ this is not valid json").expect("write corrupt file");

        assert!(load(dir.path()).is_none());

        // The bad file is preserved for inspection and removed from its slot.
        assert!(!path.exists(), "corrupt state.json should be moved aside");
        assert!(
            dir.path().join("state.json.corrupt").exists(),
            "corrupt copy should exist"
        );
    }

    #[test]
    fn load_version_mismatch_returns_none_and_moves_file_aside() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("state.json");

        // Valid JSON, but a schema version this build does not understand.
        let future = PersistedState {
            version: STATE_VERSION + 1,
            reviews: vec![],
            projects: vec![],
        };
        let json = serde_json::to_string_pretty(&future).expect("serialize");
        std::fs::write(&path, json).expect("write mismatched-version file");

        assert!(load(dir.path()).is_none());
        assert!(
            !path.exists(),
            "mismatched state.json should be moved aside"
        );
        assert!(dir.path().join("state.json.corrupt").exists());
    }
}
