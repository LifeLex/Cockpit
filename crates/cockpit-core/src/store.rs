//! In-memory and file-backed stores for active [`Review`]s and the
//! optional [`ProjectPlan`].
//!
//! V1 uses JSON file persistence: `start` writes the initial state,
//! `comment add` and `request-changes` read/modify/write it back.
//! Thread-safe in-memory access via `Arc<Mutex<…>>`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::model::{PrRef, ProjectPlan, Review};

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/// Errors from the review store.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The state file could not be read or written.
    #[error("state file I/O error at {path}: {source}")]
    Io {
        /// Path that was being read/written.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// The state file contained invalid JSON.
    #[error("failed to parse state file {path}: {source}")]
    Parse {
        /// Path that was being parsed.
        path: PathBuf,
        /// Underlying serde error.
        source: serde_json::Error,
    },

    /// No review found for the given PR reference.
    #[error("no review found for PR {0}")]
    NotFound(PrRef),

    /// No project plan is loaded.
    #[error("no project plan loaded")]
    NoPlan,
}

// ---------------------------------------------------------------------------
// ReviewStore (in-memory)
// ---------------------------------------------------------------------------

/// Thread-safe in-memory store for active reviews.
///
/// Keyed by [`PrRef`]. Uses `std::sync::Mutex` because the lock is held only
/// for trivial `HashMap` operations (no `.await` while locked).
#[derive(Debug, Clone, Default)]
pub struct ReviewStore {
    inner: Arc<Mutex<HashMap<PrRef, Review>>>,
}

impl ReviewStore {
    /// Create an empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a review, keyed by its `pr` field.
    pub fn insert(&self, review: Review) {
        // INVARIANT: lock held only for a HashMap insert — no .await, no blocking.
        let mut map = self.inner.lock().expect("review store lock poisoned");
        map.insert(review.pr.clone(), review);
    }

    /// Get a clone of the review for the given PR reference.
    pub fn get(&self, pr: &PrRef) -> Option<Review> {
        let map = self.inner.lock().expect("review store lock poisoned");
        map.get(pr).cloned()
    }

    /// Apply a mutation to the review for the given PR reference.
    ///
    /// Returns `true` if the review was found and updated, `false` otherwise.
    pub fn update(&self, pr: &PrRef, f: impl FnOnce(&mut Review)) -> bool {
        let mut map = self.inner.lock().expect("review store lock poisoned");
        if let Some(review) = map.get_mut(pr) {
            f(review);
            true
        } else {
            false
        }
    }

    /// Remove the review for the given PR reference, returning it if present.
    pub fn remove(&self, pr: &PrRef) -> Option<Review> {
        let mut map = self.inner.lock().expect("review store lock poisoned");
        map.remove(pr)
    }

    /// Clone all reviews as a `Vec`.
    pub fn list(&self) -> Vec<Review> {
        let map = self.inner.lock().expect("review store lock poisoned");
        map.values().cloned().collect()
    }
}

// ---------------------------------------------------------------------------
// PlanStore (in-memory)
// ---------------------------------------------------------------------------

/// Thread-safe in-memory store for the optional project plan.
///
/// Holds at most one [`ProjectPlan`]. Uses `std::sync::Mutex` because the lock
/// is held only for trivial get/set operations (no `.await` while locked).
#[derive(Debug, Clone, Default)]
pub struct PlanStore {
    inner: Arc<Mutex<Option<ProjectPlan>>>,
}

impl PlanStore {
    /// Create an empty plan store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the project plan, replacing any existing one.
    pub fn set(&self, plan: ProjectPlan) {
        // INVARIANT: lock held only for an Option assignment — no .await, no blocking.
        let mut guard = self.inner.lock().expect("plan store lock poisoned");
        *guard = Some(plan);
    }

    /// Get a clone of the current project plan, if any.
    pub fn get(&self) -> Option<ProjectPlan> {
        let guard = self.inner.lock().expect("plan store lock poisoned");
        guard.clone()
    }

    /// Apply a mutation to the stored plan.
    ///
    /// Returns `true` if a plan was present and updated, `false` otherwise.
    pub fn update(&self, f: impl FnOnce(&mut ProjectPlan)) -> bool {
        let mut guard = self.inner.lock().expect("plan store lock poisoned");
        if let Some(plan) = guard.as_mut() {
            f(plan);
            true
        } else {
            false
        }
    }

    /// Remove the stored plan, returning it if present.
    pub fn clear(&self) -> Option<ProjectPlan> {
        let mut guard = self.inner.lock().expect("plan store lock poisoned");
        guard.take()
    }
}

/// The default plan state file path relative to the repo root.
pub const PLAN_STATE_FILE: &str = ".cockpit/plan.json";

/// Save the plan from a [`PlanStore`] to a JSON file.
///
/// Creates parent directories if they don't exist.
pub fn save_plan_to_file(store: &PlanStore, path: &Path) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| Error::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let plan = store.get();
    let content = serde_json::to_string_pretty(&plan).map_err(|source| Error::Parse {
        path: path.to_path_buf(),
        source,
    })?;

    std::fs::write(path, content).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;

    Ok(())
}

/// Load a [`PlanStore`] from a JSON file.
///
/// Returns an empty store if the file does not exist.
pub fn load_plan_from_file(path: &Path) -> Result<PlanStore, Error> {
    if !path.exists() {
        return Ok(PlanStore::new());
    }

    let content = std::fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;

    let plan: Option<ProjectPlan> =
        serde_json::from_str(&content).map_err(|source| Error::Parse {
            path: path.to_path_buf(),
            source,
        })?;

    let store = PlanStore::new();
    if let Some(p) = plan {
        store.set(p);
    }
    Ok(store)
}

// ---------------------------------------------------------------------------
// File-backed persistence
// ---------------------------------------------------------------------------

/// The default state file path relative to the repo root.
pub const STATE_FILE: &str = ".cockpit/state.json";

/// Load reviews from a JSON state file into a `ReviewStore`.
///
/// Returns an empty store if the file does not exist.
pub fn load_from_file(path: &Path) -> Result<ReviewStore, Error> {
    if !path.exists() {
        return Ok(ReviewStore::new());
    }

    let content = std::fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;

    let reviews: Vec<Review> = serde_json::from_str(&content).map_err(|source| Error::Parse {
        path: path.to_path_buf(),
        source,
    })?;

    let store = ReviewStore::new();
    for review in reviews {
        store.insert(review);
    }
    Ok(store)
}

/// Write all reviews from a `ReviewStore` to a JSON state file.
///
/// Creates parent directories if they don't exist.
pub fn save_to_file(store: &ReviewStore, path: &Path) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| Error::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let reviews = store.list();
    let content = serde_json::to_string_pretty(&reviews).map_err(|source| Error::Parse {
        path: path.to_path_buf(),
        source,
    })?;

    std::fs::write(path, content).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::model::{DiffData, GateState, IssueRef, ReviewId, ReviewSource};

    /// Build a minimal `Review` with the given PR number.
    fn make_review(pr_num: u64) -> Review {
        Review {
            id: ReviewId::new(format!("r-{pr_num}")),
            issue: IssueRef::new(format!("ISSUE-{pr_num}")),
            pr: PrRef::new(format!("owner/repo#{pr_num}")),
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
        }
    }

    #[test]
    fn insert_and_get() {
        let store = ReviewStore::new();
        let review = make_review(1);
        let pr = review.pr.clone();

        store.insert(review.clone());

        let got = store.get(&pr).expect("review should be present");
        assert_eq!(got.id, review.id);
        assert_eq!(got.pr, pr);
    }

    #[test]
    fn update_modifies_in_place() {
        let store = ReviewStore::new();
        let review = make_review(2);
        let pr = review.pr.clone();

        store.insert(review);

        let updated = store.update(&pr, |r| {
            r.gate_state = GateState::InReview;
        });
        assert!(updated, "update should return true for existing review");

        let got = store.get(&pr).expect("review should be present");
        assert_eq!(got.gate_state, GateState::InReview);
    }

    #[test]
    fn update_returns_false_for_missing() {
        let store = ReviewStore::new();
        let pr = PrRef::new("owner/repo#999");

        let updated = store.update(&pr, |_r| {});
        assert!(!updated, "update should return false for missing review");
    }

    #[test]
    fn remove_returns_review() {
        let store = ReviewStore::new();
        let review = make_review(3);
        let pr = review.pr.clone();

        store.insert(review.clone());

        let removed = store.remove(&pr).expect("remove should return the review");
        assert_eq!(removed.id, review.id);

        assert!(
            store.get(&pr).is_none(),
            "review should be gone after remove"
        );
    }

    #[test]
    fn list_returns_all() {
        let store = ReviewStore::new();
        store.insert(make_review(10));
        store.insert(make_review(20));
        store.insert(make_review(30));

        let all = store.list();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn file_round_trip() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let path = dir.path().join(".cockpit/state.json");

        let store = ReviewStore::new();
        store.insert(make_review(100));
        store.insert(make_review(200));

        save_to_file(&store, &path).expect("save should succeed");

        let loaded = load_from_file(&path).expect("load should succeed");
        let reviews = loaded.list();
        assert_eq!(reviews.len(), 2);

        // Verify the reviews round-tripped correctly.
        let pr100 = PrRef::new("owner/repo#100");
        let got = loaded.get(&pr100).expect("review 100 should be present");
        assert_eq!(got.id.as_str(), "r-100");
    }

    #[test]
    fn load_missing_file_returns_empty() {
        let path = PathBuf::from("/nonexistent/path/state.json");
        let store = load_from_file(&path).expect("load of missing file should return empty store");
        assert!(store.list().is_empty());
    }

    // ---------------------------------------------------------------
    // PlanStore tests
    // ---------------------------------------------------------------

    use crate::model::{PlanDoc, PlanStep, ProjectRef};

    fn make_plan() -> ProjectPlan {
        ProjectPlan {
            project: ProjectRef::new("proj-1"),
            doc: PlanDoc {
                summary: "Build a thing".into(),
                steps: vec![PlanStep {
                    index: 0,
                    title: "Step one".into(),
                    description: "Do something".into(),
                }],
                files: vec![PathBuf::from("src/lib.rs")],
                risks: vec!["migration needed".into()],
                raw: "# Plan: Build a thing\n\n## Steps\n\n1. Step one\n   Do something\n\n## Files\n\n- src/lib.rs\n\n## Risks\n\n- migration needed\n".into(),
            },
            gate_state: GateState::Pending,
            comments: vec![],
            agent: None,
        }
    }

    #[test]
    fn plan_store_set_and_get() {
        let store = PlanStore::new();
        assert!(store.get().is_none(), "empty store should return None");

        let plan = make_plan();
        store.set(plan.clone());

        let got = store.get().expect("plan should be present after set");
        assert_eq!(got.project, plan.project);
        assert_eq!(got.doc.summary, plan.doc.summary);
    }

    #[test]
    fn plan_store_update() {
        let store = PlanStore::new();
        let plan = make_plan();
        store.set(plan);

        let updated = store.update(|p| {
            p.gate_state = GateState::InReview;
        });
        assert!(updated, "update should return true when plan exists");

        let got = store.get().unwrap();
        assert_eq!(got.gate_state, GateState::InReview);
    }

    #[test]
    fn plan_store_update_empty_returns_false() {
        let store = PlanStore::new();
        let updated = store.update(|_| {});
        assert!(!updated, "update should return false on empty store");
    }

    #[test]
    fn plan_store_clear() {
        let store = PlanStore::new();
        store.set(make_plan());

        let removed = store.clear();
        assert!(removed.is_some(), "clear should return the plan");
        assert!(store.get().is_none(), "store should be empty after clear");
    }

    #[test]
    fn plan_store_clear_empty() {
        let store = PlanStore::new();
        let removed = store.clear();
        assert!(removed.is_none(), "clear on empty store returns None");
    }

    #[test]
    fn plan_file_round_trip() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let path = dir.path().join(".cockpit/plan.json");

        let store = PlanStore::new();
        store.set(make_plan());

        save_plan_to_file(&store, &path).expect("save should succeed");

        let loaded = load_plan_from_file(&path).expect("load should succeed");
        let got = loaded.get().expect("plan should be present after load");
        assert_eq!(got.doc.summary, "Build a thing");
        assert_eq!(got.doc.steps.len(), 1);
    }

    #[test]
    fn plan_load_missing_file_returns_empty() {
        let path = PathBuf::from("/nonexistent/path/plan.json");
        let store =
            load_plan_from_file(&path).expect("load of missing file should return empty store");
        assert!(store.get().is_none());
    }
}
