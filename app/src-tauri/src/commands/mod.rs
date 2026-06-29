//! Thin `#[tauri::command]` handlers that delegate to `cockpit-core`.
//!
//! Commands parse params, call core, and map results through
//! [`CommandError`](crate::error::CommandError). All logic lives in core.

use std::path::PathBuf;
use std::sync::Arc;

use tauri::State;

use cockpit_core::adapters::agent::SpawnConfig;
use cockpit_core::adapters::github::{self, MirrorResult};
use cockpit_core::adapters::linear;
use cockpit_core::batch;
use cockpit_core::config::Config;
use cockpit_core::gate::Gated;
use cockpit_core::kickoff::{self, KickoffResult};
use cockpit_core::model::{
    Anchor, Comment, CommentId, CommentOrigin, DiffData, GateState, PlanDoc, PrRef, ProjectPlan,
    ProjectRef, Review,
};
use cockpit_core::plan_parser;
use cockpit_core::restack;

use crate::error::CommandError;
use crate::state::AppState;

/// Return the cockpit-core crate version — trivial command to prove IPC round-trip.
#[tauri::command]
pub fn get_version() -> String {
    cockpit_core::VERSION.to_string()
}

/// List all reviews currently in the store.
#[tauri::command]
pub fn list_reviews(state: State<'_, Arc<AppState>>) -> Result<Vec<Review>, CommandError> {
    Ok(state.reviews.list())
}

/// Get the frontier: reviews safe for deep-review (not stale, not yet approved).
#[tauri::command]
pub fn get_frontier(state: State<'_, Arc<AppState>>) -> Result<Vec<Review>, CommandError> {
    let frontier = state
        .reviews
        .list()
        .into_iter()
        .filter(|r| !r.stale && r.gate_state != GateState::Approved)
        .collect();
    Ok(frontier)
}

/// Get a single review by PR reference string.
#[tauri::command]
pub fn get_review(state: State<'_, Arc<AppState>>, pr: String) -> Result<Review, CommandError> {
    let pr_ref = PrRef::new(&pr);
    state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {pr}"),
    })
}

/// Open a review for human review (`Pending | Reworked` -> `InReview`).
#[tauri::command]
pub fn open_review(state: State<'_, Arc<AppState>>, pr: String) -> Result<Review, CommandError> {
    let pr_ref = PrRef::new(&pr);
    state.reviews.update(&pr_ref, |review| {
        // Best-effort: if the transition fails, we silently ignore it.
        // The returned review will still reflect the unchanged state,
        // so the caller sees the truth.
        let _ = review.open();
    });
    state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {pr}"),
    })
}

/// Return the diff data for a review identified by PR reference.
#[tauri::command]
pub fn get_review_diff(
    state: State<'_, Arc<AppState>>,
    pr: String,
) -> Result<DiffData, CommandError> {
    let pr_ref = PrRef::new(&pr);
    let review = state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {pr}"),
    })?;
    Ok(review.diff.clone())
}

/// Add an anchored comment to a review at the diff gate.
///
/// Creates an ephemeral [`Comment`] with a `DiffLine` anchor and
/// appends it to the review's comment list.
#[tauri::command]
pub fn add_comment(
    state: State<'_, Arc<AppState>>,
    pr: String,
    file: String,
    line_start: u32,
    line_end: u32,
    body: String,
) -> Result<Review, CommandError> {
    let pr_ref = PrRef::new(&pr);

    let comment_id = CommentId::new(format!("c-{}", uuid::Uuid::new_v4()));
    let comment = Comment {
        id: comment_id,
        anchor: Anchor::DiffLine {
            path: PathBuf::from(&file),
            range: (line_start, line_end),
        },
        body,
        origin: CommentOrigin::Local,
    };

    let found = state.reviews.update(&pr_ref, |review| {
        review.comments.push(comment);
    });

    if !found {
        return Err(CommandError {
            message: format!("Review not found: {pr}"),
        });
    }

    state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {pr}"),
    })
}

/// Trigger the request-changes flow for a review (`InReview` -> `Dispatched`).
///
/// Requires at least one comment to be present. The actual agent dispatch
/// is handled by the `Gated::request_changes` transition; the agent spawn
/// is not yet wired (Phase 1 prerequisite).
#[tauri::command]
pub fn request_changes(
    state: State<'_, Arc<AppState>>,
    pr: String,
) -> Result<Review, CommandError> {
    let pr_ref = PrRef::new(&pr);

    let mut transition_err: Option<cockpit_core::gate::Error> = None;
    let found = state.reviews.update(&pr_ref, |review| {
        if let Err(e) = review.request_changes() {
            transition_err = Some(e);
        }
    });

    if !found {
        return Err(CommandError {
            message: format!("Review not found: {pr}"),
        });
    }

    if let Some(e) = transition_err {
        return Err(CommandError::from(e));
    }

    state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {pr}"),
    })
}

// ---------------------------------------------------------------------------
// Comment mirroring
// ---------------------------------------------------------------------------

/// Mirror local comments for a review to GitHub.
///
/// Only mirrors comments with [`CommentOrigin::Local`] origin to avoid
/// duplicating comments that came from GitHub. This is an explicit user
/// action (Invariant 5: mirroring comments to a public GitHub thread
/// never happens automatically).
#[tauri::command]
pub async fn mirror_comments(
    state: State<'_, Arc<AppState>>,
    pr: String,
) -> Result<MirrorResult, CommandError> {
    let pr_ref = PrRef::new(&pr);
    let review = state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {pr}"),
    })?;

    let result = github::mirror_comments(&pr_ref, &review.comments)
        .await
        .map_err(|e| CommandError {
            message: e.to_string(),
        })?;

    Ok(result)
}

// ---------------------------------------------------------------------------
// Plan gate commands
// ---------------------------------------------------------------------------

/// Return the current project plan, if one is loaded.
#[tauri::command]
pub fn get_plan(state: State<'_, Arc<AppState>>) -> Result<Option<ProjectPlan>, CommandError> {
    Ok(state.plan.get())
}

/// Load a plan from a file on disk and store it.
///
/// Parses the plan document using `cockpit-core`'s plan parser and
/// creates a new `ProjectPlan` in `Pending` state.
#[tauri::command]
pub fn load_plan(
    state: State<'_, Arc<AppState>>,
    file: String,
    project: String,
) -> Result<ProjectPlan, CommandError> {
    let raw = std::fs::read_to_string(&file)?;
    let doc: PlanDoc = plan_parser::parse(&raw)?;
    let plan = ProjectPlan {
        project: ProjectRef::new(project),
        doc,
        gate_state: GateState::Pending,
        comments: vec![],
        agent: None,
    };
    state.plan.set(plan.clone());
    Ok(plan)
}

/// Add a comment anchored to a plan step or file.
///
/// The `anchor` string is parsed by `cockpit-core`'s plan anchor parser
/// (format: `"step:N"` or `"file:path"`).
#[tauri::command]
pub fn add_plan_comment(
    state: State<'_, Arc<AppState>>,
    anchor: String,
    body: String,
) -> Result<ProjectPlan, CommandError> {
    let parsed_anchor: Anchor = plan_parser::parse_plan_anchor(&anchor)?;
    let comment = Comment {
        id: CommentId::new(uuid::Uuid::new_v4().to_string()),
        anchor: parsed_anchor,
        body,
        origin: CommentOrigin::Local,
    };

    let updated = state.plan.update(|plan| {
        plan.comments.push(comment);
    });

    if !updated {
        return Err(CommandError {
            message: "No project plan loaded".into(),
        });
    }

    state.plan.get().ok_or_else(|| CommandError {
        message: "Plan disappeared after update".into(),
    })
}

/// Transition the plan to `Dispatched` (request changes from the planner agent).
///
/// Requires that the plan is in `InReview` state with at least one comment.
/// This is an explicit user action (Invariant 5).
#[tauri::command]
pub fn plan_request_changes(state: State<'_, Arc<AppState>>) -> Result<ProjectPlan, CommandError> {
    let mut plan = state.plan.get().ok_or_else(|| CommandError {
        message: "No project plan loaded".into(),
    })?;

    plan.request_changes()?;
    state.plan.set(plan);

    state.plan.get().ok_or_else(|| CommandError {
        message: "Plan disappeared after update".into(),
    })
}

/// Approve the plan, transitioning it to `Approved`.
///
/// Requires that the plan is in `InReview` state. This is an explicit user
/// action that triggers the batch build (Invariant 5: side effects require
/// explicit confirmation).
#[tauri::command]
pub fn plan_approve(state: State<'_, Arc<AppState>>) -> Result<ProjectPlan, CommandError> {
    let mut plan = state.plan.get().ok_or_else(|| CommandError {
        message: "No project plan loaded".into(),
    })?;

    plan.approve()?;
    state.plan.set(plan);

    state.plan.get().ok_or_else(|| CommandError {
        message: "Plan disappeared after update".into(),
    })
}

/// Open the plan for review (`Pending | Reworked` -> `InReview`).
#[tauri::command]
pub fn plan_open(state: State<'_, Arc<AppState>>) -> Result<ProjectPlan, CommandError> {
    let mut plan = state.plan.get().ok_or_else(|| CommandError {
        message: "No project plan loaded".into(),
    })?;

    plan.open()?;
    state.plan.set(plan);

    state.plan.get().ok_or_else(|| CommandError {
        message: "Plan disappeared after update".into(),
    })
}

// ---------------------------------------------------------------------------
// Batch-approve commands
// ---------------------------------------------------------------------------

/// Preview batch-approve verdicts for all frontier reviews (dry-run only).
///
/// Returns a list of `(Review, Verdict)` pairs. The frontend uses this to
/// show a modal/panel with verdicts; the actual approval happens through
/// individual `approve_review` calls from the UI (Invariant 5: side effects
/// require explicit confirmation).
#[tauri::command]
pub fn batch_approve_preview(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<(Review, batch::Verdict)>, CommandError> {
    let config = batch::Config::default();
    let results = batch::evaluate_frontier(&state.reviews, &config);
    Ok(results)
}

/// Approve a single review by PR reference string (`InReview` -> `Approved`).
///
/// If the review is in `Reworked` state, it is first opened to `InReview`
/// before approving. This is used by the batch-approve UI to approve
/// individual eligible reviews (explicit user action per Invariant 5).
#[tauri::command]
pub fn approve_review(state: State<'_, Arc<AppState>>, pr: String) -> Result<Review, CommandError> {
    let pr_ref = PrRef::new(&pr);

    let mut transition_err: Option<cockpit_core::gate::Error> = None;
    let found = state.reviews.update(&pr_ref, |review| {
        // Transition Reworked -> InReview first if needed.
        if review.gate_state == GateState::Reworked
            && let Err(e) = review.open()
        {
            transition_err = Some(e);
            return;
        }
        if let Err(e) = review.approve() {
            transition_err = Some(e);
        }
    });

    if !found {
        return Err(CommandError {
            message: format!("Review not found: {pr}"),
        });
    }

    if let Some(e) = transition_err {
        return Err(CommandError::from(e));
    }

    state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {pr}"),
    })
}

// ---------------------------------------------------------------------------
// Settings / Config commands
// ---------------------------------------------------------------------------

/// Load the application configuration from `~/.cockpit/config.toml`.
///
/// Returns the default configuration if the file does not exist.
#[tauri::command]
pub fn get_config() -> Result<Config, CommandError> {
    let config = Config::load()?;
    Ok(config)
}

/// Save the application configuration to `~/.cockpit/config.toml`.
///
/// Creates the `~/.cockpit/` directory if it does not already exist.
#[tauri::command]
pub fn save_config(config: Config) -> Result<(), CommandError> {
    config.save()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Kickoff command
// ---------------------------------------------------------------------------

/// Kick off a Linear project: fetch issues, optionally plan, then create
/// reviews for each frontier issue.
///
/// This is an explicit user action (Invariant 5). If `skip_plan` is false,
/// a project plan is created in `Pending` state for the user to review
/// before the batch is spawned.
///
/// Returns a [`KickoffResult`] with the created reviews and frontier.
#[tauri::command]
pub async fn kickoff(
    state: State<'_, Arc<AppState>>,
    project_id: String,
    skip_plan: bool,
) -> Result<KickoffResult, CommandError> {
    let config = Config::load()?;

    let api_key = config.linear_api_key.ok_or_else(|| CommandError {
        message: "Linear API key not configured. Set it in Settings.".into(),
    })?;

    let project = ProjectRef::new(&project_id);
    let client = reqwest::Client::new();

    // 1. Fetch issues and compute the frontier.
    let (data, frontier) = kickoff::fetch_and_compute_frontier(&client, &api_key, &project).await?;

    if frontier.is_empty() {
        return Err(CommandError::from(kickoff::Error::EmptyFrontier));
    }

    // 2. Build the issue DAG for parent/child wiring.
    let issue_dag = linear::build_issue_dag(&data);

    // 3. Handle plan gate decision.
    if !skip_plan {
        let plan = ProjectPlan {
            project: project.clone(),
            doc: cockpit_core::model::PlanDoc {
                summary: format!("Plan for project {project}"),
                steps: vec![],
                files: vec![],
                risks: vec![],
                raw: String::new(),
            },
            gate_state: GateState::Pending,
            comments: vec![],
            agent: None,
        };
        state.plan.set(plan);
    }

    // 4. Build reviews for frontier issues.
    let worktree_base = config
        .repo_path
        .as_ref()
        .map(|p| p.join(".cockpit/worktrees"))
        .unwrap_or_else(|| PathBuf::from(".cockpit/worktrees"));

    let reviews =
        kickoff::build_reviews_for_frontier(&frontier, &data, &issue_dag, &worktree_base, "main");

    // 5. Store reviews in the in-memory store.
    for review in &reviews {
        state.reviews.insert(review.clone());
    }

    let result = KickoffResult {
        reviews,
        plan: state.plan.get(),
        issue_count: data.issues.len(),
        frontier,
    };

    Ok(result)
}

// ---------------------------------------------------------------------------
// Restack command
// ---------------------------------------------------------------------------

/// Restack a stale PR onto its parent's new head.
///
/// If the rebase is clean, clears the stale flag and returns the updated
/// review. If there are conflicts, spawns the conflict-resolver agent and
/// returns the review with the agent run attached.
///
/// This is an explicit user action (Invariant 5).
///
/// The git operations run synchronously (via `restack_review`) before any
/// async agent spawn so that `git2::Repository` (not `Send`) never lives
/// across an `.await` boundary.
#[tauri::command]
pub async fn restack_pr(
    state: State<'_, Arc<AppState>>,
    pr: String,
) -> Result<Review, CommandError> {
    let pr_ref = PrRef::new(&pr);

    let mut review = state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {pr}"),
    })?;

    if !review.stale {
        return Err(CommandError {
            message: format!("PR {pr} is not stale; nothing to restack"),
        });
    }

    let config = Config::load()?;
    let repo_path = config.repo_path.unwrap_or_else(|| PathBuf::from("."));
    let parent_branch = review.base.clone();

    // Phase 1: synchronous git restack. git2::Repository is not Send, so
    // we must not hold it across an .await point.
    let repo = git2::Repository::discover(&repo_path).map_err(|e| CommandError {
        message: format!(
            "not inside a git repository at {}: {e}",
            repo_path.display()
        ),
    })?;

    let clean =
        restack::restack_review(&repo, &mut review, &parent_branch).map_err(|e| CommandError {
            message: format!("restack failed: {e}"),
        })?;

    // Drop the repo before any .await to satisfy Send requirements.
    drop(repo);

    // Phase 2: if conflicts, spawn the conflict-resolver agent (async).
    if !clean {
        let spawn_config = SpawnConfig::default();
        let hook_url = format!("http://127.0.0.1:{}/hook/stop", config.hook_port);
        let worktree_path = review.worktree.clone();

        let prompt = restack::assemble_conflict_prompt(&review, &parent_branch);
        let agent_run = cockpit_core::adapters::agent::spawn_agent(
            &worktree_path,
            &prompt,
            cockpit_core::model::AgentMode::Restack,
            review.id.as_str(),
            &state.sessions,
            &hook_url,
            &spawn_config,
        )
        .await
        .map_err(|e| CommandError {
            message: format!("failed to spawn conflict-resolver agent: {e}"),
        })?;

        review.agent = Some(agent_run);
    }

    // Persist the updated review back to the in-memory store.
    let review_clone = review.clone();
    state.reviews.update(&pr_ref, |r| {
        r.base_sha = review_clone.base_sha.clone();
        r.stale = review_clone.stale;
        r.agent = review_clone.agent.clone();
    });

    state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found after restack: {pr}"),
    })
}

// ---------------------------------------------------------------------------
// GitHub PR import commands
// ---------------------------------------------------------------------------

/// Fetch open PRs authored by the current user from GitHub.
///
/// Runs `gh pr list --author=@me` in the configured repo path, fetches diffs
/// concurrently for each PR, builds [`Review`] objects, and stores them.
/// Returns the created reviews.
#[tauri::command]
pub async fn fetch_authored_prs(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<Review>, CommandError> {
    fetch_prs_by_filter(state, github::PrFilter::Authored).await
}

/// Fetch open PRs where the current user is requested for review.
///
/// Runs `gh pr list --search "review-requested:@me"` in the configured repo
/// path, fetches diffs concurrently, builds [`Review`] objects, and stores them.
/// Returns the created reviews.
#[tauri::command]
pub async fn fetch_review_requests(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<Review>, CommandError> {
    fetch_prs_by_filter(state, github::PrFilter::ReviewRequested).await
}

/// Shared implementation for fetching PRs by filter.
async fn fetch_prs_by_filter(
    state: State<'_, Arc<AppState>>,
    filter: github::PrFilter,
) -> Result<Vec<Review>, CommandError> {
    let config = Config::load()?;
    let repo_path = config.repo_path.unwrap_or_else(|| PathBuf::from("."));

    let prs = github::list_prs_filtered(&repo_path, filter)
        .await
        .map_err(|e| CommandError {
            message: format!("failed to list PRs: {e}"),
        })?;

    let mut reviews = Vec::with_capacity(prs.len());
    for pr in &prs {
        let diff = github::pr_diff_in(&repo_path, pr.number)
            .await
            .unwrap_or_default();
        let review = github::build_review_from_pr(pr, diff, &repo_path);
        state.reviews.insert(review.clone());
        reviews.push(review);
    }

    Ok(reviews)
}

// ---------------------------------------------------------------------------
// Plan file loading command
// ---------------------------------------------------------------------------

/// Load a project plan from a file path on disk.
///
/// Parses the plan document and stores it in the app state as a new
/// [`ProjectPlan`] in `Pending` state. The `path` argument is typically
/// selected by the user via the file dialog.
#[tauri::command]
pub fn load_plan_from_path(
    state: State<'_, Arc<AppState>>,
    path: String,
    project: String,
) -> Result<ProjectPlan, CommandError> {
    let raw = std::fs::read_to_string(&path)?;
    let doc: PlanDoc = plan_parser::parse(&raw)?;
    let plan = ProjectPlan {
        project: ProjectRef::new(project),
        doc,
        gate_state: GateState::Pending,
        comments: vec![],
        agent: None,
    };
    state.plan.set(plan.clone());
    Ok(plan)
}
