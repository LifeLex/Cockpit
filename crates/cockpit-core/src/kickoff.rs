//! Batch kickoff orchestration — take a Linear project from issues to
//! a batch of diff-gate reviews, optionally passing through the plan gate.
//!
//! See `SPEC.md` §5 and `IMPLEMENTATION_PLAN.md` T2.3. The entry point is
//! [`kickoff`], which:
//! 1. Fetches issues from Linear, builds the DAG, computes the frontier.
//! 2. Optionally creates a `ProjectPlan` and runs it through the plan gate.
//! 3. On plan approval (or skip): creates a [`Review`] per frontier issue
//!    with stacked worktree bases (parent branch = parent issue's branch).
//!
//! Side effects (plan approval, merge) are **never** automatic — the caller
//! must explicitly approve the plan before the batch is spawned (Invariant 5).

use std::collections::HashMap;

use crate::adapters::{agent, git, linear};
use crate::config;
use crate::dag;
use crate::model::{
    DiffData, GateState, IssueRef, PrRef, Project, ProjectId, ProjectPlan, ProjectRef,
    ProjectSource, Review, ReviewId, ReviewSource,
};
use crate::plan_parser;
use crate::prompt::{self, ReworkInput};

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/// Errors from kickoff orchestration.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Linear API call failed.
    #[error("Linear API error: {0}")]
    Linear(#[from] linear::Error),

    /// Git worktree operation failed.
    #[error("git error: {0}")]
    Git(#[from] git::Error),

    /// Agent spawn or tracking failed.
    #[error("agent error: {0}")]
    Agent(#[from] agent::Error),

    /// Gate state transition failed.
    #[error("gate transition error: {0}")]
    Gate(#[from] crate::gate::Error),

    /// Plan parsing failed.
    #[error("plan parse error: {0}")]
    PlanParse(#[from] plan_parser::Error),

    /// The project produced an empty frontier — no issues are ready for work.
    #[error("project has no frontier issues (all issues have unmet dependencies)")]
    EmptyFrontier,

    /// An I/O error occurred (e.g. reading a plan file).
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Failed to resolve a cockpit path (e.g. the worktrees directory).
    #[error("config error: {0}")]
    Config(#[from] config::Error),
}

// ---------------------------------------------------------------------------
// PlanDecision
// ---------------------------------------------------------------------------

/// Whether to run the plan gate or skip straight to implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanDecision {
    /// Skip the plan gate entirely — go straight to batch creation.
    Skip,
    /// Run the plan gate: create a `ProjectPlan` and wait for approval.
    RunGate,
}

// ---------------------------------------------------------------------------
// KickoffResult
// ---------------------------------------------------------------------------

/// Result of a kickoff operation.
///
/// Serializable so it can cross the Tauri IPC boundary.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub struct KickoffResult {
    /// The reviews created for each frontier issue.
    pub reviews: Vec<Review>,
    /// The project plan, if the plan gate was used.
    pub plan: Option<ProjectPlan>,
    /// All issues fetched from Linear.
    pub issue_count: usize,
    /// The computed frontier (issues ready for work).
    pub frontier: Vec<IssueRef>,
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for a kickoff run.
pub struct KickoffConfig<'a> {
    /// Agent session map for tracking spawned agents.
    pub session_map: &'a agent::SessionMap,
    /// Hook URL for agent completion callbacks.
    pub hook_url: &'a str,
    /// Agent spawn configuration.
    pub spawn_config: &'a agent::SpawnConfig,
    /// Maximum number of implementer agents to run concurrently.
    ///
    /// `spawn_batch` never has more than this many agent processes in flight
    /// at once. A value of `0` is treated as `1` so the fan-out always makes
    /// progress.
    pub max_parallel_agents: u16,
}

/// A review's worktree prepared for spawning, pairing its index in the batch
/// slice with the assembled implementation prompt.
///
/// Produced by [`prepare_batch_worktrees`] (which needs the non-`Send`
/// `git2::Repository`) and consumed by [`spawn_batch`] (which is `Send` and
/// safe to `.await` across, because it holds no repository handle).
pub struct PreparedReview {
    /// Index into the `reviews` slice this preparation belongs to.
    pub index: usize,
    /// The assembled implementation prompt for the review.
    pub prompt: prompt::AssembledPrompt,
}

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

/// Fetch project data from Linear, build the DAG, and compute the frontier.
///
/// Returns the project data and the frontier issues in sorted order.
pub async fn fetch_and_compute_frontier(
    http_client: &reqwest::Client,
    api_key: &str,
    project: &ProjectRef,
) -> Result<(linear::ProjectData, Vec<IssueRef>), Error> {
    let data = linear::fetch_project_issues(http_client, api_key, project).await?;
    let issue_dag = linear::build_issue_dag(&data);
    let frontier = dag::compute_frontier(&issue_dag);
    Ok((data, frontier))
}

/// Build a review for a single frontier issue.
///
/// The review starts in `Pending` state with no diff, comments, or agent.
/// The `base` field is set to either "main" (for root issues) or to the
/// parent issue's branch name (for stacked issues).
///
/// `branch_map` maps `IssueRef` to branch names so stacked reviews can
/// resolve their parent's branch. `issue_dag` maps each issue to its
/// dependencies. `project` is the first-class project these reviews belong
/// to, if any.
///
/// The `worktree` field is left empty: callers resolve the on-disk path with
/// [`review_worktree_path`] (the single source of the keying scheme) once the
/// review's project and branch are settled.
fn build_review(
    issue: &IssueRef,
    issue_data: &linear::IssueNode,
    issue_dag: &HashMap<IssueRef, Vec<IssueRef>>,
    branch_map: &HashMap<IssueRef, String>,
    default_base: &str,
    project: Option<&ProjectId>,
) -> Review {
    // Determine the base branch: if this issue depends on another issue that
    // has a known branch, use that branch. Otherwise use the default base.
    let deps = issue_dag.get(issue).cloned().unwrap_or_default();
    let (base, parents) = resolve_base_and_parents(&deps, branch_map, default_base);

    let branch = issue_data.branch_name.clone();

    Review {
        id: ReviewId::new(format!("r-{}", issue.as_str())),
        issue: issue.clone(),
        pr: PrRef::new(format!("pending-{}", issue.as_str())),
        title: String::new(),
        body: String::new(),
        branch,
        base,
        base_sha: String::new(),
        source: ReviewSource::Frontier,
        worktree: std::path::PathBuf::new(),
        gate_state: GateState::Pending,
        diff: DiffData { raw: String::new() },
        head_sha: String::new(),
        comments: vec![],
        parents,
        children: vec![],
        stale: false,
        agent: None,
        repo_slug: None,
        project: project.cloned(),
        dispatch_snapshot: None,
    }
}

/// Build reviews for all frontier issues, correctly wiring parent/child edges.
///
/// Returns a `Vec<Review>` with `parents` and `children` fields set to
/// reflect the dependency DAG. Issues not in the frontier are excluded.
///
/// Each review's `worktree` is left empty; the caller resolves it through
/// [`review_worktree_path`] (the single source of the keying scheme) once the
/// project and branch are settled. This keeps worktree resolution — which
/// reads the cockpit home and is therefore fallible — out of this pure DAG
/// wiring step.
pub fn build_reviews_for_frontier(
    frontier: &[IssueRef],
    data: &linear::ProjectData,
    issue_dag: &HashMap<IssueRef, Vec<IssueRef>>,
    default_base: &str,
    project: Option<&ProjectId>,
) -> Vec<Review> {
    // Build a branch map: issue identifier -> branch name.
    let branch_map: HashMap<IssueRef, String> = data
        .issues
        .iter()
        .map(|n| (IssueRef::new(&n.identifier), n.branch_name.clone()))
        .collect();

    // Build a lookup from identifier -> IssueNode for frontier issues.
    let issue_lookup: HashMap<&str, &linear::IssueNode> = data
        .issues
        .iter()
        .map(|n| (n.identifier.as_str(), n))
        .collect();

    let mut reviews: Vec<Review> = frontier
        .iter()
        .filter_map(|issue_ref| {
            let node = issue_lookup.get(issue_ref.as_str())?;
            Some(build_review(
                issue_ref,
                node,
                issue_dag,
                &branch_map,
                default_base,
                project,
            ))
        })
        .collect();

    // Wire up children edges: for each review, find other reviews whose
    // parents include this review's issue, and add the child reference.
    wire_children(&mut reviews);

    reviews
}

/// Resolve the base branch and parent ReviewIds for an issue from its dependencies.
///
/// If the issue depends on another issue that has a known branch in
/// `branch_map`, uses that as the base (stacked). Otherwise falls back
/// to `default_base`. Returns `(base_branch, parent_review_ids)`.
fn resolve_base_and_parents(
    deps: &[IssueRef],
    branch_map: &HashMap<IssueRef, String>,
    default_base: &str,
) -> (String, Vec<ReviewId>) {
    let mut parents = Vec::new();
    let mut base = default_base.to_string();

    for dep in deps {
        let parent_id = ReviewId::new(format!("r-{}", dep.as_str()));
        parents.push(parent_id);

        // Use the first dependency's branch as the base. In a linear chain
        // there's only one parent; in a diamond the first is an approximation.
        if let Some(branch) = branch_map.get(dep) {
            if base == default_base {
                base = branch.clone();
            }
        }
    }

    (base, parents)
}

/// Wire up `children` edges across all reviews based on `parents` relationships.
fn wire_children(reviews: &mut [Review]) {
    // Collect (parent_review_id -> child_review_id) pairs first to avoid
    // borrow conflicts on the mutable slice.
    let edges: Vec<(ReviewId, ReviewId)> = reviews
        .iter()
        .flat_map(|r| {
            r.parents
                .iter()
                .map(|parent_id| (parent_id.clone(), r.id.clone()))
                .collect::<Vec<_>>()
        })
        .collect();

    for (parent_id, child_id) in edges {
        if let Some(parent) = reviews.iter_mut().find(|r| r.id == parent_id) {
            if !parent.children.contains(&child_id) {
                parent.children.push(child_id);
            }
        }
    }
}

/// Select a project's frontier reviews — those ready for an implementer.
///
/// A frontier review is one with no parents (a root of the stack). Stacked
/// children are built after their parents' branches exist, so the initial
/// fan-out targets only roots. Returns the matching reviews' [`ReviewId`]s in
/// input order.
pub fn select_frontier_reviews(reviews: &[Review]) -> Vec<ReviewId> {
    reviews
        .iter()
        .filter(|r| r.parents.is_empty())
        .map(|r| r.id.clone())
        .collect()
}

/// Create the git worktree for every review and assemble its implementation
/// prompt (the synchronous, non-`Send` half of the fan-out).
///
/// Worktree creation is serial because `git2::Repository` is not `Sync`. This
/// function holds the repository handle and therefore must complete (and the
/// repo be dropped) before any `.await` — its output [`PreparedReview`]s carry
/// no repository reference, so [`spawn_batch`] can be `.await`ed across freely.
///
/// Updates each review's `base_sha` in place; returns one [`PreparedReview`]
/// per review, in slice order.
///
/// `custom_preamble` is the [`crate::model::AgentMode::Implement`] override from
/// config, injected verbatim into every implementer prompt (or `None` to fall
/// back to the builtin).
pub fn prepare_batch_worktrees(
    reviews: &mut [Review],
    repo: &git2::Repository,
    project: &ProjectRef,
    custom_preamble: Option<&str>,
) -> Result<Vec<PreparedReview>, Error> {
    let mut prepared = Vec::with_capacity(reviews.len());
    for (index, review) in reviews.iter_mut().enumerate() {
        let base_oid = git::ensure_worktree(repo, &review.worktree, &review.branch, &review.base)?;
        review.base_sha = base_oid.to_string();
        let prompt = assemble_implement_prompt(review, project, custom_preamble);
        prepared.push(PreparedReview { index, prompt });
    }
    Ok(prepared)
}

/// Spawn implementer agents for prepared reviews, bounded by
/// [`KickoffConfig::max_parallel_agents`] concurrent agent processes.
///
/// Fan-out is the guarded side effect of plan approval (`SPEC.md` §12 /
/// Invariant 5): callers must have obtained explicit user approval first —
/// this function performs no gate transition of its own.
///
/// Call [`prepare_batch_worktrees`] first to create the worktrees (that step
/// needs the non-`Send` repo handle); pass its output here. Agent processes
/// run in waves of at most `max_parallel_agents`; each wave is awaited before
/// the next starts, which is the concurrency bound. Each review's `agent` is
/// set in place. Reviews are left in their current gate state (`Pending`) —
/// the implementer fills the diff; a human still opens the review.
pub async fn spawn_batch(
    reviews: &mut [Review],
    prepared: &[PreparedReview],
    config: &KickoffConfig<'_>,
) -> Result<(), Error> {
    // A max of 0 would stall the fan-out; clamp to at least one in flight.
    let max_parallel = config.max_parallel_agents.max(1) as usize;

    for wave in prepared.chunks(max_parallel) {
        let mut children = Vec::with_capacity(wave.len());
        for item in wave {
            let review = &mut reviews[item.index];
            let spawn_result = agent::spawn_agent(
                &review.worktree,
                &item.prompt,
                crate::model::AgentMode::Implement,
                review.id.as_str(),
                config.session_map,
                config.hook_url,
                config.spawn_config,
            )
            .await?;
            review.agent = Some(spawn_result.run);
            children.push(spawn_result.child);
        }

        // Wait for this wave's processes to exit before starting the next,
        // enforcing the `max_parallel` bound.
        for mut child in children {
            let _ = child.wait().await;
        }
    }

    Ok(())
}

/// Assemble a minimal implementation prompt for an issue.
///
/// The implementer uses the issue reference and project context to build
/// the initial PR from scratch.
fn assemble_implement_prompt(
    review: &Review,
    project: &ProjectRef,
    custom_preamble: Option<&str>,
) -> prompt::AssembledPrompt {
    let intent = format!(
        "Implement issue {} for project {}. \
         Create the initial implementation, commit, and push.",
        review.issue, project
    );

    // Skills relevant to the review's current diff (empty at implement time =>
    // universal/untagged skills only). Discovery failures are non-fatal.
    let skills = crate::skills::relevant_for_diff(&review.diff.raw);

    let input = ReworkInput {
        intent: &intent,
        custom_preamble,
        approved_plan: None,
        artifact: &crate::model::Artifact::Diff(review.diff.clone()),
        comments: &[],
        ci_failures: None,
        skills: &skills,
    };

    prompt::assemble_rework(&input)
}

// ---------------------------------------------------------------------------
// Project construction
// ---------------------------------------------------------------------------

/// Create a first-class [`Project`] backed by a Linear project.
///
/// The project's [`ProjectId`] mirrors the Linear project ref so kickoff can
/// tag its reviews consistently; the [`ProjectSource`] retains the Linear id.
pub fn project_from_linear(project: &ProjectRef, name: impl Into<String>) -> Project {
    Project {
        id: ProjectId::new(project.as_str()),
        name: name.into(),
        source: ProjectSource::Linear(project.as_str().to_string()),
        plan: None,
    }
}

/// Create a first-class ad-hoc [`Project`] with no external backing.
///
/// The [`ProjectId`] is derived from a slug of the name; ad-hoc projects have
/// no Linear source and start without a plan. This is an explicit user action.
pub fn create_ad_hoc_project(name: impl Into<String>) -> Project {
    let name = name.into();
    Project {
        id: ProjectId::new(slugify(&name)),
        name,
        source: ProjectSource::AdHoc,
        plan: None,
    }
}

/// Turn a human name into a filesystem/URL-safe slug (lowercase, `-`-joined).
fn slugify(name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    // Collapse runs of '-' and trim edges so we never emit `--` or leading `-`.
    let collapsed: Vec<&str> = slug.split('-').filter(|s| !s.is_empty()).collect();
    let joined = collapsed.join("-");
    if joined.is_empty() {
        "project".to_string()
    } else {
        joined
    }
}

// ---------------------------------------------------------------------------
// Worktree keying
// ---------------------------------------------------------------------------

/// Resolve the on-disk worktree path for a review, using a single consistent
/// keying scheme so every call site agrees.
///
/// The scheme is `<worktrees_dir>/<project-or-"ungrouped">/<sanitized-key>`,
/// where the project segment is the review's [`ProjectId`] (or the literal
/// `ungrouped` when the review has no project), and the key is the review's
/// branch when set, otherwise its issue ref. Any `/` in the key is flattened
/// to `-` so it never introduces extra path segments.
pub fn review_worktree_path(review: &Review) -> Result<std::path::PathBuf, Error> {
    let base = config::worktrees_dir()?;
    let project_segment = review
        .project
        .as_ref()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| "ungrouped".to_string());
    let raw_key = if !review.branch.is_empty() {
        review.branch.as_str()
    } else {
        review.issue.as_str()
    };
    let key = raw_key.replace('/', "-");
    Ok(base.join(project_segment).join(key))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use super::*;
    use crate::model::{GateState, IssueRef, ReviewId};

    /// Build a minimal `IssueNode` for testing.
    fn make_issue_node(identifier: &str, branch: &str) -> linear::IssueNode {
        linear::IssueNode {
            id: format!("id-{identifier}"),
            identifier: identifier.to_string(),
            title: format!("Title for {identifier}"),
            branch_name: branch.to_string(),
        }
    }

    /// Build a simple DAG: A has no deps, B depends on A, C depends on B.
    fn linear_dag() -> HashMap<IssueRef, Vec<IssueRef>> {
        let mut dag = HashMap::new();
        dag.insert(IssueRef::new("NEX-1"), vec![]);
        dag.insert(IssueRef::new("NEX-2"), vec![IssueRef::new("NEX-1")]);
        dag.insert(IssueRef::new("NEX-3"), vec![IssueRef::new("NEX-2")]);
        dag
    }

    /// Build project data with three issues in a chain.
    fn three_issue_data() -> linear::ProjectData {
        linear::ProjectData {
            issues: vec![
                make_issue_node("NEX-1", "alejandro/nex-1-first"),
                make_issue_node("NEX-2", "alejandro/nex-2-second"),
                make_issue_node("NEX-3", "alejandro/nex-3-third"),
            ],
            relations: vec![],
        }
    }

    #[test]
    fn skip_plan_goes_directly_to_batch() {
        // With PlanDecision::Skip, no ProjectPlan is created. The frontier
        // issues produce reviews directly.
        let decision = PlanDecision::Skip;
        assert_eq!(decision, PlanDecision::Skip);

        // Simulate: compute frontier, build reviews.
        let dag = linear_dag();
        let frontier = dag::compute_frontier(&dag);

        // Only NEX-1 is in the frontier (no deps).
        assert_eq!(frontier, vec![IssueRef::new("NEX-1")]);

        let data = three_issue_data();
        let reviews = build_reviews_for_frontier(&frontier, &data, &dag, "main", None);

        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews[0].issue, IssueRef::new("NEX-1"));
        assert_eq!(reviews[0].gate_state, GateState::Pending);
        assert!(reviews[0].agent.is_none());
    }

    #[test]
    fn reviews_created_with_correct_parent_child_relationships() {
        // All three issues in the frontier (e.g. no deps for this test).
        let mut dag = HashMap::new();
        dag.insert(IssueRef::new("NEX-1"), vec![]);
        dag.insert(IssueRef::new("NEX-2"), vec![IssueRef::new("NEX-1")]);
        dag.insert(IssueRef::new("NEX-3"), vec![IssueRef::new("NEX-2")]);

        // Put all issues in the "frontier" for this test (normally only
        // root issues are, but we want to test parent/child wiring).
        let all_issues = vec![
            IssueRef::new("NEX-1"),
            IssueRef::new("NEX-2"),
            IssueRef::new("NEX-3"),
        ];

        let data = three_issue_data();
        let reviews = build_reviews_for_frontier(&all_issues, &data, &dag, "main", None);

        assert_eq!(reviews.len(), 3);

        // Find reviews by issue.
        let r1 = reviews.iter().find(|r| r.issue == IssueRef::new("NEX-1"));
        let r2 = reviews.iter().find(|r| r.issue == IssueRef::new("NEX-2"));
        let r3 = reviews.iter().find(|r| r.issue == IssueRef::new("NEX-3"));

        let r1 = r1.expect("NEX-1 review should exist");
        let r2 = r2.expect("NEX-2 review should exist");
        let r3 = r3.expect("NEX-3 review should exist");

        // NEX-1: no parents, child is NEX-2.
        assert!(r1.parents.is_empty(), "NEX-1 should have no parents");
        assert_eq!(r1.base, "main", "NEX-1 base should be main");
        assert_eq!(
            r1.children,
            vec![ReviewId::new("r-NEX-2")],
            "NEX-1 should have NEX-2 as child"
        );

        // NEX-2: parent is NEX-1, child is NEX-3, base is NEX-1's branch.
        assert_eq!(
            r2.parents,
            vec![ReviewId::new("r-NEX-1")],
            "NEX-2 should have NEX-1 as parent"
        );
        assert_eq!(
            r2.base, "alejandro/nex-1-first",
            "NEX-2 base should be NEX-1's branch (stacked)"
        );
        assert_eq!(
            r2.children,
            vec![ReviewId::new("r-NEX-3")],
            "NEX-2 should have NEX-3 as child"
        );

        // NEX-3: parent is NEX-2, no children, base is NEX-2's branch.
        assert_eq!(
            r3.parents,
            vec![ReviewId::new("r-NEX-2")],
            "NEX-3 should have NEX-2 as parent"
        );
        assert_eq!(
            r3.base, "alejandro/nex-2-second",
            "NEX-3 base should be NEX-2's branch (stacked)"
        );
        assert!(r3.children.is_empty(), "NEX-3 should have no children");
    }

    #[test]
    fn independent_issues_all_base_on_main() {
        // Three independent issues (no deps).
        let mut dag = HashMap::new();
        dag.insert(IssueRef::new("NEX-10"), vec![]);
        dag.insert(IssueRef::new("NEX-11"), vec![]);
        dag.insert(IssueRef::new("NEX-12"), vec![]);

        let data = linear::ProjectData {
            issues: vec![
                make_issue_node("NEX-10", "alejandro/nex-10-alpha"),
                make_issue_node("NEX-11", "alejandro/nex-11-beta"),
                make_issue_node("NEX-12", "alejandro/nex-12-gamma"),
            ],
            relations: vec![],
        };

        let all_issues = vec![
            IssueRef::new("NEX-10"),
            IssueRef::new("NEX-11"),
            IssueRef::new("NEX-12"),
        ];

        let reviews = build_reviews_for_frontier(&all_issues, &data, &dag, "main", None);

        assert_eq!(reviews.len(), 3);
        for review in &reviews {
            assert_eq!(
                review.base, "main",
                "independent issues should all base on main"
            );
            assert!(
                review.parents.is_empty(),
                "independent issues should have no parents"
            );
            assert!(
                review.children.is_empty(),
                "independent issues should have no children"
            );
        }
    }

    #[test]
    fn review_worktree_paths_are_correct() {
        // `build_reviews_for_frontier` leaves `worktree` empty; the worktree is
        // resolved by the single keying scheme in `review_worktree_path`.
        let dag = linear_dag();
        let frontier = dag::compute_frontier(&dag);
        let data = three_issue_data();

        let reviews = build_reviews_for_frontier(&frontier, &data, &dag, "main", None);

        assert_eq!(reviews.len(), 1);
        assert_eq!(
            reviews[0].worktree,
            PathBuf::new(),
            "worktree is resolved by the caller via review_worktree_path, not here"
        );

        let home = tempfile::tempdir().expect("temp dir");
        temp_env::with_var("COCKPIT_HOME", Some(home.path()), || {
            let path = review_worktree_path(&reviews[0]).expect("path resolves");
            let expected = home
                .path()
                .join("worktrees")
                .join("ungrouped")
                .join("alejandro-nex-1-first");
            assert_eq!(path, expected);
        });
    }

    #[test]
    fn review_ids_use_issue_ref() {
        let dag = linear_dag();
        let frontier = dag::compute_frontier(&dag);
        let data = three_issue_data();

        let reviews = build_reviews_for_frontier(&frontier, &data, &dag, "main", None);

        assert_eq!(reviews[0].id, ReviewId::new("r-NEX-1"));
    }

    #[test]
    fn review_branches_match_issue_branches() {
        let dag = linear_dag();
        let frontier = dag::compute_frontier(&dag);
        let data = three_issue_data();

        let reviews = build_reviews_for_frontier(&frontier, &data, &dag, "main", None);

        assert_eq!(reviews[0].branch, "alejandro/nex-1-first");
    }

    #[test]
    fn plan_decision_enum_variants() {
        // Verify the enum is usable.
        let skip = PlanDecision::Skip;
        let gate = PlanDecision::RunGate;
        assert_ne!(skip, gate);
        assert_eq!(skip.clone(), PlanDecision::Skip);
    }

    #[test]
    fn wire_children_no_op_for_independent() {
        let mut reviews = vec![Review {
            id: ReviewId::new("r-1"),
            issue: IssueRef::new("ISS-1"),
            pr: PrRef::new("p-1"),
            title: String::new(),
            body: String::new(),
            branch: "b-1".into(),
            base: "main".into(),
            base_sha: String::new(),
            source: ReviewSource::Frontier,
            worktree: PathBuf::new(),
            gate_state: GateState::Pending,
            diff: DiffData { raw: String::new() },
            head_sha: String::new(),
            comments: vec![],
            parents: vec![],
            children: vec![],
            stale: false,
            agent: None,
            repo_slug: None,
            project: None,
            dispatch_snapshot: None,
        }];

        wire_children(&mut reviews);
        assert!(reviews[0].children.is_empty());
    }

    #[test]
    fn implement_prompt_contains_issue() {
        let review = Review {
            id: ReviewId::new("r-NEX-1"),
            issue: IssueRef::new("NEX-1"),
            pr: PrRef::new("pending-NEX-1"),
            title: String::new(),
            body: String::new(),
            branch: "alejandro/nex-1-thing".into(),
            base: "main".into(),
            base_sha: String::new(),
            source: ReviewSource::Frontier,
            worktree: PathBuf::new(),
            gate_state: GateState::Pending,
            diff: DiffData { raw: String::new() },
            head_sha: String::new(),
            comments: vec![],
            parents: vec![],
            children: vec![],
            stale: false,
            agent: None,
            repo_slug: None,
            project: None,
            dispatch_snapshot: None,
        };

        let prompt = assemble_implement_prompt(&review, &ProjectRef::new("proj-1"), None);
        assert!(
            prompt.text.contains("NEX-1"),
            "prompt should mention the issue"
        );
        assert!(
            prompt.text.contains("proj-1"),
            "prompt should mention the project"
        );
        assert!(!prompt.hash.is_empty(), "prompt hash should be computed");
    }

    #[test]
    fn ad_hoc_project_slugifies_name() {
        let project = create_ad_hoc_project("My Cool Project!");
        assert_eq!(project.id.as_str(), "my-cool-project");
        assert_eq!(project.name, "My Cool Project!");
        assert_eq!(project.source, ProjectSource::AdHoc);
        assert!(project.plan.is_none());
    }

    #[test]
    fn ad_hoc_project_empty_name_falls_back() {
        let project = create_ad_hoc_project("!!!");
        assert_eq!(project.id.as_str(), "project");
    }

    #[test]
    fn linear_project_retains_source_id() {
        let project = project_from_linear(&ProjectRef::new("lin-42"), "Batch");
        assert_eq!(project.id.as_str(), "lin-42");
        assert_eq!(project.source, ProjectSource::Linear("lin-42".to_string()));
    }

    #[test]
    fn review_worktree_path_uses_project_and_branch() {
        let home = tempfile::tempdir().expect("temp dir");
        temp_env::with_var("COCKPIT_HOME", Some(home.path()), || {
            let mut review = build_review(
                &IssueRef::new("NEX-1"),
                &make_issue_node("NEX-1", "alejandro/nex-1/feature"),
                &linear_dag(),
                &HashMap::new(),
                "main",
                Some(&ProjectId::new("p-1")),
            );
            review.branch = "alejandro/nex-1/feature".to_string();

            let path = review_worktree_path(&review).expect("path resolves");
            let expected = home
                .path()
                .join("worktrees")
                .join("p-1")
                .join("alejandro-nex-1-feature");
            assert_eq!(path, expected, "slashes flattened, keyed by project");
        });
    }

    #[test]
    fn review_worktree_path_ungrouped_uses_placeholder_segment() {
        let home = tempfile::tempdir().expect("temp dir");
        temp_env::with_var("COCKPIT_HOME", Some(home.path()), || {
            let review = build_review(
                &IssueRef::new("NEX-9"),
                &make_issue_node("NEX-9", "alejandro/nex-9-thing"),
                &linear_dag(),
                &HashMap::new(),
                "main",
                None,
            );

            let path = review_worktree_path(&review).expect("path resolves");
            let expected = home
                .path()
                .join("worktrees")
                .join("ungrouped")
                .join("alejandro-nex-9-thing");
            assert_eq!(path, expected);
        });
    }
}
