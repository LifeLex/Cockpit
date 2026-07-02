//! In-memory stores for active [`Review`]s and first-class [`Project`]s.
//!
//! Each project owns its optional [`ProjectPlan`] (see [`Project::plan`]); the
//! plan gate operates per project via [`ProjectStore::update_plan`] and
//! [`ProjectStore::plan`].
//!
//! These back [`AppState`](../../app/src-tauri) and are driven by the Tauri
//! commands. Thread-safe in-memory access via `Arc<Mutex<…>>`; the app owns
//! the lifetime. Each store carries a monotonic revision counter so the
//! persistence layer ([`crate::persist`]) can cheaply detect changes and
//! re-save without diffing the whole map.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::model::{GateState, PrRef, Project, ProjectId, ProjectPlan, Review, ReviewId};

// INVARIANT: every `.lock().expect("... lock poisoned")` below deliberately
// propagates a poisoned-lock panic (CLAUDE.md §2). A `Mutex` becomes poisoned
// only when another thread panicked while holding it, leaving the map in an
// unknown, unrecoverable state; continuing on that state would be worse than
// crashing, so re-panicking via `expect` is the correct response.

// ---------------------------------------------------------------------------
// ReviewStore (in-memory)
// ---------------------------------------------------------------------------

/// Thread-safe in-memory store for active reviews.
///
/// Keyed by [`PrRef`]. Uses `std::sync::Mutex` because the lock is held only
/// for trivial `HashMap` operations (no `.await` while locked). A shared
/// [`AtomicU64`] revision counter bumps on every mutation so the persistence
/// layer can detect changes without diffing the map.
#[derive(Debug, Clone, Default)]
pub struct ReviewStore {
    inner: Arc<Mutex<HashMap<PrRef, Review>>>,
    revision: Arc<AtomicU64>,
}

impl ReviewStore {
    /// Create an empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Current revision of the store.
    ///
    /// A monotonic generation counter that increments on every mutating call.
    /// Callers snapshot it and compare later to decide whether a re-save is
    /// needed; the absolute value carries no meaning beyond "changed since".
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Relaxed)
    }

    /// Increment the revision counter.
    ///
    /// `Relaxed` is sufficient: the counter is a coarse change signal, not a
    /// synchronization primitive for the map contents (that is the `Mutex`'s
    /// job), so it needs only atomicity, not ordering.
    fn bump(&self) {
        self.revision.fetch_add(1, Ordering::Relaxed);
    }

    /// Insert a review, keyed by its `pr` field.
    pub fn insert(&self, review: Review) {
        // INVARIANT: lock held only for a HashMap insert — no .await, no blocking.
        let mut map = self.inner.lock().expect("review store lock poisoned");
        map.insert(review.pr.clone(), review);
        drop(map);
        self.bump();
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
        let found = if let Some(review) = map.get_mut(pr) {
            f(review);
            true
        } else {
            false
        };
        drop(map);
        // Only bump when the closure actually ran on an existing entry: a
        // missing-key update mutates nothing and must not trigger a re-save.
        if found {
            self.bump();
        }
        found
    }

    /// Remove the review for the given PR reference, returning it if present.
    pub fn remove(&self, pr: &PrRef) -> Option<Review> {
        // INVARIANT: lock held only for a HashMap op, no .await, no blocking.
        let mut map = self.inner.lock().expect("review store lock poisoned");
        let removed = map.remove(pr);
        drop(map);
        self.bump();
        removed
    }

    /// Clone all reviews as a `Vec`.
    pub fn list(&self) -> Vec<Review> {
        // INVARIANT: lock held only for a HashMap op, no .await, no blocking.
        let map = self.inner.lock().expect("review store lock poisoned");
        map.values().cloned().collect()
    }

    /// Bulk-replace the store's contents with `reviews` and bump the revision.
    ///
    /// Used at startup to hydrate the store from persisted state: every existing
    /// entry is dropped and each review is re-keyed by its `pr` field.
    pub fn hydrate(&self, reviews: Vec<Review>) {
        // INVARIANT: lock held only for HashMap ops, no .await, no blocking.
        let mut map = self.inner.lock().expect("review store lock poisoned");
        map.clear();
        for review in reviews {
            map.insert(review.pr.clone(), review);
        }
        drop(map);
        self.bump();
    }

    /// Mark every descendant of `parent` as [`stale`](Review::stale).
    ///
    /// Performs a breadth-first walk over `children` edges starting from
    /// `parent`'s direct children and sets `stale = true` on each review it
    /// reaches. The `parent` itself is never marked (it is not its own
    /// descendant), and an unknown `parent` is a no-op that leaves the revision
    /// untouched.
    ///
    /// `children` edges hold [`ReviewId`]s while the map is keyed by [`PrRef`],
    /// so each hop resolves a review by scanning on id (the graph is small). A
    /// `visited` set makes the walk terminate even if the edges form a cycle.
    pub fn mark_descendants_stale(&self, parent: &ReviewId) {
        // INVARIANT: lock held only for in-memory graph work, no .await, no blocking.
        let mut map = self.inner.lock().expect("review store lock poisoned");

        let mut visited: HashSet<ReviewId> = HashSet::new();
        // Pre-mark the parent visited: it is not a descendant, and this also
        // guards against a cycle whose back-edge points at the parent.
        visited.insert(parent.clone());

        let mut queue: VecDeque<ReviewId> = VecDeque::new();
        // Seed with the parent's direct children. Unknown parent -> no seeds.
        if let Some(parent_review) = map.values().find(|r| &r.id == parent) {
            queue.extend(parent_review.children.iter().cloned());
        }

        let mut changed = false;
        while let Some(id) = queue.pop_front() {
            if !visited.insert(id.clone()) {
                continue;
            }
            if let Some(review) = map.values_mut().find(|r| r.id == id) {
                review.stale = true;
                changed = true;
                queue.extend(review.children.iter().cloned());
            }
        }

        drop(map);
        if changed {
            self.bump();
        }
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
// ProjectStore (in-memory)
// ---------------------------------------------------------------------------

/// Thread-safe in-memory store for first-class projects.
///
/// Keyed by [`ProjectId`]. Mirrors [`ReviewStore`]: the lock is held only for
/// trivial `HashMap` operations (no `.await` while locked), and a shared
/// [`AtomicU64`] revision counter bumps on every mutation so the persistence
/// layer can detect changes without diffing the map.
#[derive(Debug, Clone, Default)]
pub struct ProjectStore {
    inner: Arc<Mutex<HashMap<ProjectId, Project>>>,
    revision: Arc<AtomicU64>,
}

impl ProjectStore {
    /// Create an empty project store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Current revision of the store.
    ///
    /// A monotonic generation counter that increments on every mutating call;
    /// see [`ReviewStore::revision`] for the same contract.
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Relaxed)
    }

    /// Increment the revision counter.
    ///
    /// `Relaxed` is sufficient: the counter is a coarse change signal, not a
    /// synchronization primitive for the map contents.
    fn bump(&self) {
        self.revision.fetch_add(1, Ordering::Relaxed);
    }

    /// Insert a project, keyed by its `id` field.
    pub fn insert(&self, project: Project) {
        // INVARIANT: lock held only for a HashMap insert — no .await, no blocking.
        let mut map = self.inner.lock().expect("project store lock poisoned");
        map.insert(project.id.clone(), project);
        drop(map);
        self.bump();
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
        let found = if let Some(project) = map.get_mut(id) {
            f(project);
            true
        } else {
            false
        };
        drop(map);
        // Only bump when the closure actually ran on an existing entry: a
        // missing-key update mutates nothing and must not trigger a re-save.
        if found {
            self.bump();
        }
        found
    }

    /// Remove the project for the given id, returning it if present.
    pub fn remove(&self, id: &ProjectId) -> Option<Project> {
        // INVARIANT: lock held only for a HashMap op, no .await, no blocking.
        let mut map = self.inner.lock().expect("project store lock poisoned");
        let removed = map.remove(id);
        drop(map);
        self.bump();
        removed
    }

    /// Clone all projects as a `Vec`.
    pub fn list(&self) -> Vec<Project> {
        // INVARIANT: lock held only for a HashMap op, no .await, no blocking.
        let map = self.inner.lock().expect("project store lock poisoned");
        map.values().cloned().collect()
    }

    /// Bulk-replace the store's contents with `projects` and bump the revision.
    ///
    /// Used at startup to hydrate the store from persisted state: every existing
    /// entry is dropped and each project is re-keyed by its `id` field.
    pub fn hydrate(&self, projects: Vec<Project>) {
        // INVARIANT: lock held only for HashMap ops, no .await, no blocking.
        let mut map = self.inner.lock().expect("project store lock poisoned");
        map.clear();
        for project in projects {
            map.insert(project.id.clone(), project);
        }
        drop(map);
        self.bump();
    }

    /// Get a clone of the plan owned by the given project, if any.
    ///
    /// Returns `None` when the project is unknown or has no plan yet.
    pub fn plan(&self, id: &ProjectId) -> Option<ProjectPlan> {
        // INVARIANT: lock held only for a HashMap op, no .await, no blocking.
        let map = self.inner.lock().expect("project store lock poisoned");
        map.get(id).and_then(|p| p.plan.clone())
    }

    /// Mutate the plan slot of the given project.
    ///
    /// The closure receives the project's `Option<ProjectPlan>` so it can read,
    /// set, replace, or clear the plan. Returns `true` if the project exists (and
    /// the closure ran), `false` if the project is unknown.
    pub fn update_plan(&self, id: &ProjectId, f: impl FnOnce(&mut Option<ProjectPlan>)) -> bool {
        // INVARIANT: lock held only for a HashMap op, no .await, no blocking.
        let mut map = self.inner.lock().expect("project store lock poisoned");
        let found = if let Some(project) = map.get_mut(id) {
            f(&mut project.plan);
            true
        } else {
            false
        };
        drop(map);
        // Only bump when the closure actually ran on an existing entry: a
        // missing-key update mutates nothing and must not trigger a re-save.
        if found {
            self.bump();
        }
        found
    }
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
            ci_summary: None,
            review_findings: vec![],
            conversation: vec![],
            last_reviewed_sha: None,
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

    // ---------------------------------------------------------------
    // Plan helpers (per-project) — shared fixture
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
    fn project_store_plan_set_get_and_update() {
        let store = ProjectStore::new();
        let id = ProjectId::new("p-1");
        store.insert(make_project("p-1", ProjectSource::AdHoc));

        // No plan initially.
        assert!(store.plan(&id).is_none(), "new project has no plan");

        // Set a plan via update_plan.
        let existed = store.update_plan(&id, |slot| *slot = Some(make_plan()));
        assert!(existed, "update_plan returns true for a known project");
        let got = store.plan(&id).expect("plan present after set");
        assert_eq!(got.gate_state, GateState::Pending);

        // Mutate the existing plan in place.
        store.update_plan(&id, |slot| {
            if let Some(plan) = slot.as_mut() {
                plan.gate_state = GateState::InReview;
            }
        });
        assert_eq!(
            store.plan(&id).expect("plan present").gate_state,
            GateState::InReview
        );
    }

    #[test]
    fn project_store_update_plan_unknown_project_returns_false() {
        let store = ProjectStore::new();
        let existed = store.update_plan(&ProjectId::new("nope"), |slot| *slot = Some(make_plan()));
        assert!(!existed, "update_plan returns false for an unknown project");
        assert!(store.plan(&ProjectId::new("nope")).is_none());
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

    // ---------------------------------------------------------------
    // Revision counter + hydrate
    // ---------------------------------------------------------------

    #[test]
    fn review_store_revision_bumps_on_mutators() {
        let store = ReviewStore::new();
        assert_eq!(store.revision(), 0, "fresh store starts at revision 0");

        store.insert(make_review(1));
        let after_insert = store.revision();
        assert!(after_insert > 0, "insert bumps the revision");

        store.update(&PrRef::new("owner/repo#1"), |r| r.stale = true);
        let after_update = store.revision();
        assert!(after_update > after_insert, "update bumps the revision");

        store.remove(&PrRef::new("owner/repo#1"));
        let after_remove = store.revision();
        assert!(after_remove > after_update, "remove bumps the revision");

        // A missing-key update mutates nothing and must not bump the revision.
        let missing = store.update(&PrRef::new("owner/repo#404"), |r| r.stale = true);
        assert!(!missing, "update on a missing key returns false");
        assert_eq!(
            store.revision(),
            after_remove,
            "a missing-key update must not bump the revision"
        );
    }

    #[test]
    fn review_store_hydrate_replaces_and_bumps() {
        let store = ReviewStore::new();
        store.insert(make_review(1));
        let before = store.revision();

        store.hydrate(vec![make_review(2), make_review(3)]);
        assert!(store.revision() > before, "hydrate bumps the revision");

        // Old contents are gone; only the hydrated reviews remain.
        assert!(store.get(&PrRef::new("owner/repo#1")).is_none());
        assert_eq!(store.list().len(), 2);
    }

    #[test]
    fn project_store_revision_bumps_on_mutators() {
        let store = ProjectStore::new();
        assert_eq!(store.revision(), 0, "fresh store starts at revision 0");

        store.insert(make_project("p-1", ProjectSource::AdHoc));
        let after_insert = store.revision();
        assert!(after_insert > 0, "insert bumps the revision");

        store.update(&ProjectId::new("p-1"), |p| p.name = "renamed".into());
        let after_update = store.revision();
        assert!(after_update > after_insert, "update bumps the revision");

        store.update_plan(&ProjectId::new("p-1"), |slot| *slot = Some(make_plan()));
        let after_plan = store.revision();
        assert!(after_plan > after_update, "update_plan bumps the revision");

        store.remove(&ProjectId::new("p-1"));
        let after_remove = store.revision();
        assert!(after_remove > after_plan, "remove bumps the revision");

        // Missing-key mutators (`update` / `update_plan`) change nothing and
        // must not bump the revision.
        let missing = store.update(&ProjectId::new("absent"), |p| p.name = "x".into());
        assert!(!missing, "update on a missing key returns false");
        assert_eq!(
            store.revision(),
            after_remove,
            "a missing-key update must not bump the revision"
        );

        let missing_plan =
            store.update_plan(&ProjectId::new("absent"), |slot| *slot = Some(make_plan()));
        assert!(!missing_plan, "update_plan on a missing key returns false");
        assert_eq!(
            store.revision(),
            after_remove,
            "a missing-key update_plan must not bump the revision"
        );
    }

    #[test]
    fn project_store_hydrate_replaces_and_bumps() {
        let store = ProjectStore::new();
        store.insert(make_project("p-1", ProjectSource::AdHoc));
        let before = store.revision();

        store.hydrate(vec![
            make_project("p-2", ProjectSource::AdHoc),
            make_project("p-3", ProjectSource::AdHoc),
        ]);
        assert!(store.revision() > before, "hydrate bumps the revision");

        assert!(store.get(&ProjectId::new("p-1")).is_none());
        assert_eq!(store.list().len(), 2);
    }

    // ---------------------------------------------------------------
    // mark_descendants_stale
    // ---------------------------------------------------------------

    /// Build a review numbered `num` whose `children` edges point at `children`.
    fn make_linked(num: u64, children: &[u64]) -> Review {
        let mut review = make_review(num);
        review.children = children
            .iter()
            .map(|c| ReviewId::new(format!("r-{c}")))
            .collect();
        review
    }

    /// Read the `stale` flag of the review numbered `num`.
    fn is_stale(store: &ReviewStore, num: u64) -> bool {
        store
            .get(&PrRef::new(format!("owner/repo#{num}")))
            .expect("review should be present")
            .stale
    }

    #[test]
    fn mark_descendants_stale_linear_chain() {
        // A → B → C
        let store = ReviewStore::new();
        store.insert(make_linked(1, &[2]));
        store.insert(make_linked(2, &[3]));
        store.insert(make_linked(3, &[]));
        let before = store.revision();

        store.mark_descendants_stale(&ReviewId::new("r-1"));

        assert!(!is_stale(&store, 1), "parent is not its own descendant");
        assert!(is_stale(&store, 2));
        assert!(is_stale(&store, 3));
        assert!(store.revision() > before, "marking bumps the revision");
    }

    #[test]
    fn mark_descendants_stale_from_middle() {
        // A → B → C, staling from B.
        let store = ReviewStore::new();
        store.insert(make_linked(1, &[2]));
        store.insert(make_linked(2, &[3]));
        store.insert(make_linked(3, &[]));

        store.mark_descendants_stale(&ReviewId::new("r-2"));

        assert!(!is_stale(&store, 1), "ancestor is untouched");
        assert!(!is_stale(&store, 2), "starting node is not a descendant");
        assert!(is_stale(&store, 3), "only descendants are staled");
    }

    #[test]
    fn mark_descendants_stale_diamond() {
        // A → B, A → C, B → D, C → D
        let store = ReviewStore::new();
        store.insert(make_linked(1, &[2, 3]));
        store.insert(make_linked(2, &[4]));
        store.insert(make_linked(3, &[4]));
        store.insert(make_linked(4, &[]));

        store.mark_descendants_stale(&ReviewId::new("r-1"));

        assert!(!is_stale(&store, 1));
        assert!(is_stale(&store, 2));
        assert!(is_stale(&store, 3));
        assert!(is_stale(&store, 4), "converging descendant marked once");
    }

    #[test]
    fn mark_descendants_stale_unknown_parent_is_noop() {
        let store = ReviewStore::new();
        store.insert(make_linked(1, &[2]));
        store.insert(make_linked(2, &[]));
        let before = store.revision();

        store.mark_descendants_stale(&ReviewId::new("r-999"));

        assert!(!is_stale(&store, 1));
        assert!(!is_stale(&store, 2));
        assert_eq!(store.revision(), before, "a no-op must not bump");
    }

    #[test]
    fn mark_descendants_stale_handles_cycles() {
        // Pathological cycle A → B → C → A: the walk must terminate.
        let store = ReviewStore::new();
        store.insert(make_linked(1, &[2]));
        store.insert(make_linked(2, &[3]));
        store.insert(make_linked(3, &[1]));

        store.mark_descendants_stale(&ReviewId::new("r-1"));

        assert!(
            !is_stale(&store, 1),
            "back-edge to parent leaves it unmarked"
        );
        assert!(is_stale(&store, 2));
        assert!(is_stale(&store, 3));
    }
}
