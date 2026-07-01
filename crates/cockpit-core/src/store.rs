//! In-memory and file-backed stores for active [`Review`]s and the
//! optional [`ProjectPlan`].
//!
//! V1 uses JSON file persistence: `start` writes the initial state,
//! `comment add` and `request-changes` read/modify/write it back.
//! Thread-safe in-memory access via `Arc<Mutex<…>>`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::config;
use crate::model::{GateState, PrRef, Project, ProjectId, ProjectPlan, Review};

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

    /// Failed to resolve a cockpit path (e.g. the cockpit home directory).
    #[error("config error: {0}")]
    Config(#[from] config::Error),
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
        // INVARIANT: lock held only for a HashMap op, no .await, no blocking.
        let map = self.inner.lock().expect("review store lock poisoned");
        map.get(pr).cloned()
    }

    /// Apply a mutation to the review for the given PR reference.
    ///
    /// Returns `true` if the review was found and updated, `false` otherwise.
    pub fn update(&self, pr: &PrRef, f: impl FnOnce(&mut Review)) -> bool {
        // INVARIANT: lock held only for a HashMap op, no .await, no blocking.
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
        // INVARIANT: lock held only for a HashMap op, no .await, no blocking.
        let mut map = self.inner.lock().expect("review store lock poisoned");
        map.remove(pr)
    }

    /// Clone all reviews as a `Vec`.
    pub fn list(&self) -> Vec<Review> {
        // INVARIANT: lock held only for a HashMap op, no .await, no blocking.
        let map = self.inner.lock().expect("review store lock poisoned");
        map.values().cloned().collect()
    }
}

/// Return all reviews belonging to the given project.
///
/// Passing `None` returns the ungrouped reviews (those with no project).
pub fn reviews_by_project(store: &ReviewStore, project: Option<&ProjectId>) -> Vec<Review> {
    store
        .list()
        .into_iter()
        .filter(|r| r.project.as_ref() == project)
        .collect()
}

// ---------------------------------------------------------------------------
// Batch status aggregation
// ---------------------------------------------------------------------------

/// Aggregate progress of a project's implementer batch after fan-out.
///
/// Serializable so it can cross the Tauri IPC boundary and drive a progress
/// view. Counts partition the project's reviews by where they are in the loop:
/// a review is `building` while its implementer runs (an agent is attached and
/// it is still `Pending`), `ready` once it is reviewable but not yet
/// approved/merged, and `approved` when the diff gate has passed.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub struct BatchStatus {
    /// Total reviews belonging to the project.
    pub total: usize,
    /// Reviews whose implementer agent is still running (`Pending` + agent).
    pub building: usize,
    /// Reviews ready for human review but not yet approved.
    pub ready: usize,
    /// Reviews whose diff gate has been approved.
    pub approved: usize,
}

/// Compute the [`BatchStatus`] for a project's reviews.
///
/// Passing `None` aggregates the ungrouped reviews. A review counts as
/// `building` when it still carries a running agent and has not advanced past
/// `Pending`; `approved` when its gate state is [`GateState::Approved`]; and
/// `ready` otherwise (any state a human can act on: `Pending` with no agent,
/// `InReview`, `Dispatched`, or `Reworked`).
pub fn batch_status(store: &ReviewStore, project: Option<&ProjectId>) -> BatchStatus {
    let reviews = reviews_by_project(store, project);
    let total = reviews.len();
    let mut building = 0;
    let mut ready = 0;
    let mut approved = 0;

    for review in &reviews {
        match review.gate_state {
            GateState::Approved => approved += 1,
            GateState::Pending if review.agent.is_some() => building += 1,
            _ => ready += 1,
        }
    }

    BatchStatus {
        total,
        building,
        ready,
        approved,
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
        // INVARIANT: lock held only for an Option read — no .await, no blocking.
        let guard = self.inner.lock().expect("plan store lock poisoned");
        guard.clone()
    }

    /// Apply a mutation to the stored plan.
    ///
    /// Returns `true` if a plan was present and updated, `false` otherwise.
    pub fn update(&self, f: impl FnOnce(&mut ProjectPlan)) -> bool {
        // INVARIANT: lock held only for an Option op — no .await, no blocking.
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
        // INVARIANT: lock held only for an Option op — no .await, no blocking.
        let mut guard = self.inner.lock().expect("plan store lock poisoned");
        guard.take()
    }
}

// ---------------------------------------------------------------------------
// ProjectStore (in-memory)
// ---------------------------------------------------------------------------

/// Thread-safe in-memory store for first-class projects.
///
/// Keyed by [`ProjectId`]. Mirrors [`ReviewStore`]: the lock is held only for
/// trivial `HashMap` operations (no `.await` while locked).
#[derive(Debug, Clone, Default)]
pub struct ProjectStore {
    inner: Arc<Mutex<HashMap<ProjectId, Project>>>,
}

impl ProjectStore {
    /// Create an empty project store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a project, keyed by its `id` field.
    pub fn insert(&self, project: Project) {
        // INVARIANT: lock held only for a HashMap insert — no .await, no blocking.
        let mut map = self.inner.lock().expect("project store lock poisoned");
        map.insert(project.id.clone(), project);
    }

    /// Get a clone of the project for the given id.
    pub fn get(&self, id: &ProjectId) -> Option<Project> {
        // INVARIANT: lock held only for a HashMap op, no .await, no blocking.
        let map = self.inner.lock().expect("project store lock poisoned");
        map.get(id).cloned()
    }

    /// Apply a mutation to the project for the given id.
    ///
    /// Returns `true` if the project was found and updated, `false` otherwise.
    pub fn update(&self, id: &ProjectId, f: impl FnOnce(&mut Project)) -> bool {
        // INVARIANT: lock held only for a HashMap op, no .await, no blocking.
        let mut map = self.inner.lock().expect("project store lock poisoned");
        if let Some(project) = map.get_mut(id) {
            f(project);
            true
        } else {
            false
        }
    }

    /// Remove the project for the given id, returning it if present.
    pub fn remove(&self, id: &ProjectId) -> Option<Project> {
        // INVARIANT: lock held only for a HashMap op, no .await, no blocking.
        let mut map = self.inner.lock().expect("project store lock poisoned");
        map.remove(id)
    }

    /// Clone all projects as a `Vec`.
    pub fn list(&self) -> Vec<Project> {
        // INVARIANT: lock held only for a HashMap op, no .await, no blocking.
        let map = self.inner.lock().expect("project store lock poisoned");
        map.values().cloned().collect()
    }
}

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

/// Resolve the reviews state file path (`<cockpit_home>/state.json`).
///
/// This lives under the user's cockpit home rather than the current working
/// directory, so a bundled app never writes scratch state into a checkout.
pub fn state_file_path() -> Result<PathBuf, Error> {
    Ok(config::cockpit_home()?.join("state.json"))
}

/// Resolve the plan state file path (`<cockpit_home>/plan.json`).
pub fn plan_state_file_path() -> Result<PathBuf, Error> {
    Ok(config::cockpit_home()?.join("plan.json"))
}

/// Resolve the projects state file path (`<cockpit_home>/projects.json`).
pub fn project_state_file_path() -> Result<PathBuf, Error> {
    Ok(config::cockpit_home()?.join("projects.json"))
}

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

/// Load projects from a JSON state file into a [`ProjectStore`].
///
/// Returns an empty store if the file does not exist.
pub fn load_projects_from_file(path: &Path) -> Result<ProjectStore, Error> {
    if !path.exists() {
        return Ok(ProjectStore::new());
    }

    let content = std::fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;

    let projects: Vec<Project> = serde_json::from_str(&content).map_err(|source| Error::Parse {
        path: path.to_path_buf(),
        source,
    })?;

    let store = ProjectStore::new();
    for project in projects {
        store.insert(project);
    }
    Ok(store)
}

/// Write all projects from a [`ProjectStore`] to a JSON state file.
///
/// Creates parent directories if they don't exist.
pub fn save_projects_to_file(store: &ProjectStore, path: &Path) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| Error::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let projects = store.list();
    let content = serde_json::to_string_pretty(&projects).map_err(|source| Error::Parse {
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
            project: None,
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
            plan_path: None,
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

    // ---------------------------------------------------------------
    // ProjectStore + reviews_by_project tests
    // ---------------------------------------------------------------

    use crate::model::{Project, ProjectId, ProjectSource};

    fn make_project(id: &str, source: ProjectSource) -> Project {
        Project {
            id: ProjectId::new(id),
            name: format!("Project {id}"),
            source,
            plan: None,
        }
    }

    #[test]
    fn project_store_insert_get_list() {
        let store = ProjectStore::new();
        store.insert(make_project("p-1", ProjectSource::AdHoc));
        store.insert(make_project(
            "p-2",
            ProjectSource::Linear("lin-42".to_string()),
        ));

        let got = store.get(&ProjectId::new("p-2")).expect("p-2 present");
        assert_eq!(got.source, ProjectSource::Linear("lin-42".to_string()));
        assert_eq!(store.list().len(), 2);
    }

    #[test]
    fn project_store_update_and_remove() {
        let store = ProjectStore::new();
        store.insert(make_project("p-1", ProjectSource::AdHoc));

        let updated = store.update(&ProjectId::new("p-1"), |p| {
            p.name = "renamed".into();
        });
        assert!(updated);
        assert_eq!(
            store.get(&ProjectId::new("p-1")).expect("present").name,
            "renamed"
        );

        let removed = store.remove(&ProjectId::new("p-1")).expect("removed");
        assert_eq!(removed.name, "renamed");
        assert!(store.get(&ProjectId::new("p-1")).is_none());
    }

    #[test]
    fn project_file_round_trip() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(".cockpit/projects.json");

        let store = ProjectStore::new();
        store.insert(make_project("p-1", ProjectSource::AdHoc));
        store.insert(make_project(
            "p-2",
            ProjectSource::Linear("lin-42".to_string()),
        ));

        save_projects_to_file(&store, &path).expect("save");
        let loaded = load_projects_from_file(&path).expect("load");
        assert_eq!(loaded.list().len(), 2);
    }

    #[test]
    fn reviews_by_project_splits_grouped_and_ungrouped() {
        let store = ReviewStore::new();

        let mut r_grouped = make_review(1);
        r_grouped.project = Some(ProjectId::new("p-1"));
        store.insert(r_grouped);

        let r_ungrouped = make_review(2); // project: None
        store.insert(r_ungrouped);

        let grouped = reviews_by_project(&store, Some(&ProjectId::new("p-1")));
        assert_eq!(grouped.len(), 1);
        assert_eq!(grouped[0].pr, PrRef::new("owner/repo#1"));

        let ungrouped = reviews_by_project(&store, None);
        assert_eq!(ungrouped.len(), 1);
        assert_eq!(ungrouped[0].pr, PrRef::new("owner/repo#2"));
    }

    // ---------------------------------------------------------------
    // batch_status tests
    // ---------------------------------------------------------------

    /// Build a review in a project with the given state and optional agent.
    fn make_batch_review(pr_num: u64, state: GateState, has_agent: bool) -> Review {
        let mut review = make_review(pr_num);
        review.project = Some(ProjectId::new("p-batch"));
        review.gate_state = state;
        if has_agent {
            review.agent = Some(crate::model::AgentRun {
                pid: 1,
                mode: crate::model::AgentMode::Implement,
                started_at: std::time::SystemTime::UNIX_EPOCH,
                prompt_hash: "h".into(),
                log_path: PathBuf::from("/tmp/log"),
            });
        }
        review
    }

    #[test]
    fn batch_status_partitions_reviews() {
        let store = ReviewStore::new();
        // Two building (Pending + agent), one ready (InReview), one approved.
        store.insert(make_batch_review(1, GateState::Pending, true));
        store.insert(make_batch_review(2, GateState::Pending, true));
        store.insert(make_batch_review(3, GateState::InReview, false));
        store.insert(make_batch_review(4, GateState::Approved, false));

        let status = batch_status(&store, Some(&ProjectId::new("p-batch")));
        assert_eq!(status.total, 4);
        assert_eq!(status.building, 2);
        assert_eq!(status.ready, 1);
        assert_eq!(status.approved, 1);
    }

    #[test]
    fn batch_status_pending_without_agent_is_ready() {
        let store = ReviewStore::new();
        // Pending but no agent yet — human can still act, so it's "ready".
        store.insert(make_batch_review(1, GateState::Pending, false));

        let status = batch_status(&store, Some(&ProjectId::new("p-batch")));
        assert_eq!(status.building, 0);
        assert_eq!(status.ready, 1);
    }

    #[test]
    fn batch_status_empty_project() {
        let store = ReviewStore::new();
        let status = batch_status(&store, Some(&ProjectId::new("nope")));
        assert_eq!(status.total, 0);
        assert_eq!(status.building, 0);
        assert_eq!(status.ready, 0);
        assert_eq!(status.approved, 0);
    }
}
