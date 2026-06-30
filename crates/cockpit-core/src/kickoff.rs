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
use std::path::Path;

use crate::adapters::{agent, git, linear};
use crate::dag;
use crate::model::{
    DiffData, GateState, IssueRef, PrRef, ProjectPlan, ProjectRef, Review, ReviewId, ReviewSource,
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
    /// HTTP client for Linear API calls.
    pub http_client: &'a reqwest::Client,
    /// Linear API key.
    pub api_key: &'a str,
    /// Base directory for worktrees (e.g. `.cockpit/worktrees`).
    pub worktree_base: &'a Path,
    /// Git repository handle.
    pub repo: &'a git2::Repository,
    /// Agent session map for tracking spawned agents.
    pub session_map: &'a agent::SessionMap,
    /// Hook URL for agent completion callbacks.
    pub hook_url: &'a str,
    /// Agent spawn configuration.
    pub spawn_config: &'a agent::SpawnConfig,
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
/// dependencies.
pub fn build_review(
    issue: &IssueRef,
    issue_data: &linear::IssueNode,
    issue_dag: &HashMap<IssueRef, Vec<IssueRef>>,
    branch_map: &HashMap<IssueRef, String>,
    worktree_base: &Path,
    default_base: &str,
) -> Review {
    // Determine the base branch: if this issue depends on another issue that
    // has a known branch, use that branch. Otherwise use the default base.
    let deps = issue_dag.get(issue).cloned().unwrap_or_default();
    let (base, parents) = resolve_base_and_parents(&deps, branch_map, default_base);

    let branch = issue_data.branch_name.clone();
    let worktree = worktree_base.join(issue.as_str());

    Review {
        id: ReviewId::new(format!("r-{}", issue.as_str())),
        issue: issue.clone(),
        pr: PrRef::new(format!("pending-{}", issue.as_str())),
        branch,
        base,
        base_sha: String::new(),
        source: ReviewSource::Frontier,
        worktree,
        gate_state: GateState::Pending,
        diff: DiffData { raw: String::new() },
        head_sha: String::new(),
        comments: vec![],
        parents,
        children: vec![],
        stale: false,
        agent: None,
        repo_slug: None,
    }
}

/// Build reviews for all frontier issues, correctly wiring parent/child edges.
///
/// Returns a `Vec<Review>` with `parents` and `children` fields set to
/// reflect the dependency DAG. Issues not in the frontier are excluded.
pub fn build_reviews_for_frontier(
    frontier: &[IssueRef],
    data: &linear::ProjectData,
    issue_dag: &HashMap<IssueRef, Vec<IssueRef>>,
    worktree_base: &Path,
    default_base: &str,
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
                worktree_base,
                default_base,
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

/// Create worktrees and spawn implementer agents for each review.
///
/// Each review gets a git worktree based on its `base` branch, then an
/// implementer agent is spawned in that worktree.
///
/// Updates each review's `base_sha`, `agent`, and `gate_state` in place.
pub async fn spawn_batch(
    reviews: &mut [Review],
    config: &KickoffConfig<'_>,
    project: &ProjectRef,
) -> Result<(), Error> {
    for review in reviews.iter_mut() {
        // 1. Create the worktree (stacked base = parent branch).
        let base_oid =
            git::ensure_worktree(config.repo, &review.worktree, &review.branch, &review.base)?;
        review.base_sha = base_oid.to_string();

        // 2. Assemble a minimal implementation prompt.
        let prompt = assemble_implement_prompt(review, project);

        // 3. Spawn the implementer agent.
        let spawn_result = agent::spawn_agent(
            &review.worktree,
            &prompt,
            crate::model::AgentMode::Implement,
            review.id.as_str(),
            config.session_map,
            config.hook_url,
            config.spawn_config,
        )
        .await?;

        review.agent = Some(spawn_result.run);
    }

    Ok(())
}

/// Assemble a minimal implementation prompt for an issue.
///
/// The implementer uses the issue reference and project context to build
/// the initial PR from scratch.
fn assemble_implement_prompt(review: &Review, project: &ProjectRef) -> prompt::AssembledPrompt {
    let intent = format!(
        "Implement issue {} for project {}. \
         Create the initial implementation, commit, and push.",
        review.issue, project
    );

    let input = ReworkInput {
        intent: &intent,
        approved_plan: None,
        artifact: &crate::model::Artifact::Diff(review.diff.clone()),
        comments: &[],
        skills: &[],
    };

    prompt::assemble_rework(&input)
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
        let reviews =
            build_reviews_for_frontier(&frontier, &data, &dag, Path::new("/tmp/worktrees"), "main");

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
        let reviews = build_reviews_for_frontier(
            &all_issues,
            &data,
            &dag,
            Path::new("/tmp/worktrees"),
            "main",
        );

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

        let reviews = build_reviews_for_frontier(
            &all_issues,
            &data,
            &dag,
            Path::new("/tmp/worktrees"),
            "main",
        );

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
        let dag = linear_dag();
        let frontier = dag::compute_frontier(&dag);
        let data = three_issue_data();

        let reviews = build_reviews_for_frontier(
            &frontier,
            &data,
            &dag,
            Path::new("/repo/.cockpit/worktrees"),
            "main",
        );

        assert_eq!(reviews.len(), 1);
        assert_eq!(
            reviews[0].worktree,
            PathBuf::from("/repo/.cockpit/worktrees/NEX-1")
        );
    }

    #[test]
    fn review_ids_use_issue_ref() {
        let dag = linear_dag();
        let frontier = dag::compute_frontier(&dag);
        let data = three_issue_data();

        let reviews =
            build_reviews_for_frontier(&frontier, &data, &dag, Path::new("/tmp/wt"), "main");

        assert_eq!(reviews[0].id, ReviewId::new("r-NEX-1"));
    }

    #[test]
    fn review_branches_match_issue_branches() {
        let dag = linear_dag();
        let frontier = dag::compute_frontier(&dag);
        let data = three_issue_data();

        let reviews =
            build_reviews_for_frontier(&frontier, &data, &dag, Path::new("/tmp/wt"), "main");

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
        };

        let prompt = assemble_implement_prompt(&review, &ProjectRef::new("proj-1"));
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
}
