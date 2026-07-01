//! Domain types for cockpit: newtypes, the shared data model, and supporting enums.
//!
//! Every ID is a distinct newtype — `ReviewId`, `IssueRef`, `PrRef`, `CommentId`,
//! `ProjectRef` — so the DAG is impossible to wire up wrong at compile time.
//! See `SPEC.md` §6 for the canonical definitions and `CLAUDE.md` §2 for the
//! naming/derive conventions.

use std::fmt;
use std::path::PathBuf;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

// ---------------------------------------------------------------------------
// Newtype IDs
// ---------------------------------------------------------------------------

macro_rules! newtype_id {
    (
        $(#[doc = $doc:expr])*
        $name:ident
    ) => {
        $(#[doc = $doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
        #[ts(export, export_to = "../../../app/src/bindings/")]
        pub struct $name(String);

        impl $name {
            /// Create a new instance from anything that converts to `String`.
            pub fn new(s: impl Into<String>) -> Self {
                Self(s.into())
            }

            /// Borrow the inner value (free, no allocation).
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

newtype_id! {
    /// Locally-unique identifier for a [`Review`] in the cockpit session.
    ReviewId
}

newtype_id! {
    /// Reference to a Linear issue (e.g. `NEX-123`).
    IssueRef
}

newtype_id! {
    /// Reference to a GitHub pull request (e.g. `owner/repo#42`).
    PrRef
}

newtype_id! {
    /// Locally-unique identifier for a [`Comment`].
    CommentId
}

newtype_id! {
    /// Reference to a Linear project.
    ProjectRef
}

newtype_id! {
    /// Locally-unique identifier for a first-class [`Project`] that groups
    /// reviews. Distinct from [`ProjectRef`], which references a Linear project.
    ProjectId
}

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// The shared gate state that drives the review loop for both [`ProjectPlan`]
/// and [`Review`]. See `SPEC.md` §7 for the transition table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub enum GateState {
    /// Awaiting first review.
    Pending,
    /// Under active human review.
    InReview,
    /// Comments dispatched to an agent; awaiting the Stop hook.
    Dispatched,
    /// Agent finished rework; ready for re-review.
    Reworked,
    /// Human approved — terminal for the loop.
    Approved,
}

/// Which mode the spawned agent runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub enum AgentMode {
    /// Produce or revise the project plan.
    Plan,
    /// Build the initial implementation from an approved plan.
    Implement,
    /// Fix issues flagged during diff-gate review.
    Fix,
    /// Rebase / resolve conflicts after a parent branch changed.
    Restack,
}

/// Where a comment originated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub enum CommentOrigin {
    /// Created locally inside cockpit.
    Local,
    /// Mirrored from a GitHub PR review thread.
    GitHubMirror,
}

/// Where a [`Review`] originated, determining how comments are handled.
///
/// `Authored` and `Frontier` reviews dispatch comments to a local agent.
/// `ReviewRequested` reviews mirror comments to GitHub instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub enum ReviewSource {
    /// PR authored by the current user (from `gh pr list --author=@me`).
    Authored,
    /// PR where the current user is requested as reviewer.
    ReviewRequested,
    /// Review created from a Linear project kickoff.
    Frontier,
}

/// Where a first-class [`Project`] came from.
///
/// Linear is one *optional* source, not the entry point: a project may be
/// created ad-hoc with no Linear backing at all.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub enum ProjectSource {
    /// Backed by a Linear project, carrying its Linear project id.
    Linear(String),
    /// Created directly in cockpit with no external backing.
    AdHoc,
}

/// A location inside the current artifact that a [`Comment`] points to.
///
/// Anchors are ephemeral — they reference the *current* artifact version only
/// and are cleared together with comments on `Reworked`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub enum Anchor {
    /// A step in the project plan, by zero-based index.
    PlanStep(usize),
    /// A file listed in the plan's intended touch set.
    PlanFile(PathBuf),
    /// A line range in the current diff.
    DiffLine {
        /// Path relative to the repo root.
        path: PathBuf,
        /// Inclusive start and end line in the current head.
        range: (u32, u32),
    },
}

/// The reviewable artifact — either a plan or a diff.
///
/// Using an enum makes illegal states unrepresentable: a reviewed object holds
/// exactly one artifact kind, never both or neither.
//
// No `#[derive(TS)]`: the frontend never consumes an `Artifact` directly (it
// switches on `GateState` and reads the concrete `PlanDoc`/`DiffData`), so
// exporting a binding would only emit an orphan `.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Artifact {
    /// A project plan document.
    Plan(PlanDoc),
    /// A PR diff.
    Diff(DiffData),
}

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

/// A single step inside a project plan, used as a comment anchor target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub struct PlanStep {
    /// Zero-based index in the plan's step list.
    pub index: usize,
    /// Human-readable title.
    pub title: String,
    /// Longer description / details.
    pub description: String,
}

/// Parsed project-plan document. See `SPEC.md` §6.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub struct PlanDoc {
    /// One-line summary of the plan.
    pub summary: String,
    /// Ordered implementation steps.
    pub steps: Vec<PlanStep>,
    /// Files the plan intends to touch.
    pub files: Vec<PathBuf>,
    /// Risks: migrations, new deps, breaking changes.
    pub risks: Vec<String>,
    /// The original raw text of the plan.
    pub raw: String,
}

/// Placeholder for parsed diff content.
///
/// Will be fleshed out when the GitHub adapter (T0.5) and the diff-gate UI
/// (T4.3) need real structure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub struct DiffData {
    /// Raw unified-diff text.
    pub raw: String,
}

/// An ephemeral review comment. Lives for one review -> rework cycle and is
/// cleared on `Reworked`. No `resolved` flag, no durable SHA anchoring
/// (Invariant 4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub struct Comment {
    /// Locally-unique comment identifier.
    pub id: CommentId,
    /// Where in the artifact this comment points.
    pub anchor: Anchor,
    /// The comment body text.
    pub body: String,
    /// Where the comment came from.
    pub origin: CommentOrigin,
}

/// A running or completed agent process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub struct AgentRun {
    /// OS process ID of the spawned agent.
    pub pid: u32,
    /// Which mode the agent is running in.
    pub mode: AgentMode,
    /// Wall-clock time the agent was spawned.
    ///
    /// `SystemTime` rather than `Instant` because it must be serializable and
    /// meaningful across process boundaries.
    #[ts(type = "{ secs_since_epoch: number; nanos_since_epoch: number }")]
    pub started_at: SystemTime,
    /// Hash of the assembled prompt, for dedup / audit.
    pub prompt_hash: String,
    /// Path to the agent's log file.
    pub log_path: PathBuf,
}

/// The optional project-level plan, reviewed at the plan gate.
///
/// One per project. When approved, triggers implementation of the full batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub struct ProjectPlan {
    /// Which Linear project this plan belongs to.
    pub project: ProjectRef,
    /// The parsed plan document.
    pub doc: PlanDoc,
    /// Current gate state in the review loop.
    pub gate_state: GateState,
    /// Ephemeral comments for the current review cycle.
    pub comments: Vec<Comment>,
    /// The agent run responsible for producing / revising the plan.
    pub agent: Option<AgentRun>,
    /// On-disk markdown file the planner agent writes its finished plan to.
    ///
    /// Set before spawning a planner (`AgentMode::Plan`); on completion the
    /// file is read back and parsed into [`Self::doc`]. `None` for plans that
    /// were loaded directly from a document rather than produced by an agent.
    #[serde(default)]
    #[ts(optional)]
    pub plan_path: Option<PathBuf>,
}

/// A first-class project that groups reviews.
///
/// Reviews may belong to a project (via [`Review::project`]) or be ungrouped
/// (`None`). Linear is one optional [`ProjectSource`], not the entry point.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub struct Project {
    /// Locally-unique project identifier.
    pub id: ProjectId,
    /// Human-readable project name.
    pub name: String,
    /// Where this project came from (Linear or ad-hoc).
    pub source: ProjectSource,
    /// The optional project-level plan reviewed at the plan gate.
    pub plan: Option<ProjectPlan>,
}

/// A single PR under review at the diff gate.
///
/// Reviews form a DAG via `parents` / `children`, mirroring the Linear issue
/// dependency graph and the git branch stack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub struct Review {
    /// Locally-unique review identifier.
    pub id: ReviewId,
    /// The Linear issue this review implements.
    pub issue: IssueRef,
    /// The GitHub PR opened for this review.
    pub pr: PrRef,
    /// Git branch name (e.g. `alejandro/nex-123-do-thing`).
    pub branch: String,
    /// Base branch — either `main` or a parent review's branch (stacked).
    pub base: String,
    /// OID of the base branch tip when this review was created or last restacked.
    ///
    /// Used as the fork point for `restack`: only commits after this OID belong
    /// to this review's branch. Updated after each successful restack.
    pub base_sha: String,
    /// Where this review originated (authored, review request, or frontier).
    pub source: ReviewSource,
    /// Path to the git worktree on disk.
    pub worktree: PathBuf,
    /// Current gate state in the review loop.
    pub gate_state: GateState,
    /// The current diff content.
    pub diff: DiffData,
    /// HEAD commit SHA at last reconcile.
    pub head_sha: String,
    /// Ephemeral comments for the current review cycle.
    pub comments: Vec<Comment>,
    /// Ancestor reviews in the stack (from Linear deps).
    pub parents: Vec<ReviewId>,
    /// Descendant reviews in the stack.
    pub children: Vec<ReviewId>,
    /// An ancestor is in rework; gates deep review but not the loop itself.
    pub stale: bool,
    /// The agent run responsible for fixing / restacking.
    pub agent: Option<AgentRun>,
    /// GitHub repository slug (e.g. `"owner/repo"`), used for cross-repo
    /// operations like fetching diffs via `gh --repo`. `None` for reviews
    /// created from the local repo context (kickoff).
    pub repo_slug: Option<String>,
    /// The first-class [`Project`] this review belongs to, if any. `None` for
    /// ungrouped reviews (e.g. GitHub-imported PRs with no project attached).
    pub project: Option<ProjectId>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build a minimal `Review` with the given id, parents, children.
    fn make_review(id: &str, parents: &[&str], children: &[&str]) -> Review {
        Review {
            id: ReviewId::new(id),
            issue: IssueRef::new(format!("ISSUE-{id}")),
            pr: PrRef::new(format!("owner/repo#{id}")),
            branch: format!("alejandro/{id}"),
            base: "main".into(),
            base_sha: "000".into(),
            source: ReviewSource::Frontier,
            worktree: PathBuf::from(format!("/tmp/wt-{id}")),
            gate_state: GateState::Pending,
            diff: DiffData { raw: String::new() },
            head_sha: "aaa".into(),
            comments: vec![],
            parents: parents.iter().map(|s| ReviewId::new(*s)).collect(),
            children: children.iter().map(|s| ReviewId::new(*s)).collect(),
            stale: false,
            agent: None,
            repo_slug: None,
            project: None,
        }
    }

    #[test]
    fn dag_parent_child_edges() {
        //  A → B → C  (A is the root, C is the leaf)
        let a = make_review("a", &[], &["b"]);
        let b = make_review("b", &["a"], &["c"]);
        let c = make_review("c", &["b"], &[]);

        // A: no parents, child is B
        assert!(a.parents.is_empty());
        assert_eq!(a.children, vec![ReviewId::new("b")]);

        // B: parent is A, child is C
        assert_eq!(b.parents, vec![ReviewId::new("a")]);
        assert_eq!(b.children, vec![ReviewId::new("c")]);

        // C: parent is B, no children
        assert_eq!(c.parents, vec![ReviewId::new("b")]);
        assert!(c.children.is_empty());

        // All start Pending
        assert_eq!(a.gate_state, GateState::Pending);
        assert_eq!(b.gate_state, GateState::Pending);
        assert_eq!(c.gate_state, GateState::Pending);
    }

    #[test]
    fn newtype_ids_are_distinct() {
        let review_id = ReviewId::new("x");
        let issue_ref = IssueRef::new("x");

        // Same inner value, but they are distinct types — the compiler
        // prevents mixing them. We can still test Display / as_str.
        assert_eq!(review_id.as_str(), "x");
        assert_eq!(issue_ref.as_str(), "x");
        assert_eq!(review_id.to_string(), "x");
    }

    #[test]
    fn gate_state_is_copy() {
        let state = GateState::InReview;
        let copied = state;
        assert_eq!(state, copied);
    }

    #[test]
    fn comment_is_ephemeral() {
        let comment = Comment {
            id: CommentId::new("c-1"),
            anchor: Anchor::DiffLine {
                path: PathBuf::from("src/main.rs"),
                range: (10, 15),
            },
            body: "fix this".into(),
            origin: CommentOrigin::Local,
        };

        // No `resolved` field exists — comments are ephemeral (Invariant 4).
        assert_eq!(comment.body, "fix this");
        assert_eq!(comment.origin, CommentOrigin::Local);
    }

    #[test]
    fn project_plan_construction() {
        let plan = ProjectPlan {
            project: ProjectRef::new("proj-1"),
            doc: PlanDoc {
                summary: "Build the thing".into(),
                steps: vec![PlanStep {
                    index: 0,
                    title: "Step one".into(),
                    description: "Do the first thing".into(),
                }],
                files: vec![PathBuf::from("src/lib.rs")],
                risks: vec!["migration needed".into()],
                raw: "# Plan\n...".into(),
            },
            gate_state: GateState::Pending,
            comments: vec![],
            agent: None,
            plan_path: None,
        };

        assert_eq!(plan.gate_state, GateState::Pending);
        assert_eq!(plan.doc.steps.len(), 1);
        assert_eq!(plan.doc.risks.len(), 1);
    }

    #[test]
    fn artifact_enum_prevents_illegal_states() {
        let plan_artifact = Artifact::Plan(PlanDoc {
            summary: "s".into(),
            steps: vec![],
            files: vec![],
            risks: vec![],
            raw: String::new(),
        });

        let diff_artifact = Artifact::Diff(DiffData {
            raw: "diff --git a/f b/f".into(),
        });

        // Pattern-match exhaustively — the compiler enforces this.
        match &plan_artifact {
            Artifact::Plan(doc) => assert_eq!(doc.summary, "s"),
            Artifact::Diff(_) => panic!("expected Plan"),
        }

        match &diff_artifact {
            Artifact::Diff(data) => assert!(data.raw.starts_with("diff")),
            Artifact::Plan(_) => panic!("expected Diff"),
        }
    }
}
