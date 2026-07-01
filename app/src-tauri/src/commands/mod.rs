//! Thin `#[tauri::command]` handlers that delegate to `cockpit-core`.
//!
//! Commands parse params, call core, and map results through
//! [`CommandError`](crate::error::CommandError). All logic lives in core.

pub mod shell;

use std::path::PathBuf;
use std::sync::Arc;

use tauri::State;

use cockpit_core::adapters::agent::SpawnConfig;
use cockpit_core::adapters::github::{self, MirrorResult};
use cockpit_core::adapters::linear;
use cockpit_core::config::Config;
use cockpit_core::gate::Gated;
use cockpit_core::kickoff::{self, KickoffResult};
use cockpit_core::model::{
    AgentMode, Anchor, Artifact, Comment, CommentId, CommentOrigin, DiffData, GateState, PlanDoc,
    PrRef, Project, ProjectId, ProjectPlan, ProjectRef, Review,
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
/// Requires at least one comment to be present. After transitioning the gate
/// state, assembles a rework prompt from the review's issue, diff, and
/// comments, then spawns the fixer agent in the review's worktree. The
/// agent's stdout is streamed to the frontend via Tauri events.
#[tauri::command]
pub async fn request_changes(
    state: State<'_, Arc<AppState>>,
    app_handle: tauri::AppHandle,
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

    // Spawn the fixer agent via the shared Fix-loop path (no CI logs here).
    // Spawn failure is non-fatal — the gate transition already succeeded so we
    // return the Dispatched review regardless and log the spawn error.
    dispatch_fix_agent(&state, &app_handle, &pr, &pr_ref, None).await;

    state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {pr}"),
    })
}

/// Dispatch the diff-gate Fix loop for a review already transitioned to
/// `Dispatched`, spawning the fixer agent with a rework prompt.
///
/// This is the shared spawn path for both `request_changes` and `fix_ci`:
/// assemble the rework prompt (optionally carrying `ci_failures` verbatim),
/// spawn the fixer agent in the review's worktree, and attach the running
/// agent. Spawn failure is non-fatal — the gate transition already stands, so
/// the error is logged and surfaced as an `agent-event` rather than blocking
/// the loop (Invariant 1).
async fn dispatch_fix_agent(
    state: &State<'_, Arc<AppState>>,
    app_handle: &tauri::AppHandle,
    pr: &str,
    pr_ref: &PrRef,
    ci_failures: Option<&str>,
) {
    let Some(review) = state.reviews.get(pr_ref) else {
        return;
    };

    // Load config once so the per-mode custom preamble and agent command are
    // both honored. A load failure is non-fatal — fall back to defaults (no
    // preamble, builtin command) so rework still dispatches.
    let config = Config::load().ok();
    let preamble = config
        .as_ref()
        .and_then(|c| c.agent_prompts.for_mode(AgentMode::Fix).map(str::to_owned));
    // Skills relevant to the diff under review; discovery failures are non-fatal
    // (fall back to no skills — never block the loop).
    let skills = cockpit_core::skills::relevant_for_diff(&review.diff.raw);
    let artifact = Artifact::Diff(review.diff.clone());
    let rework_input = cockpit_core::prompt::ReworkInput {
        intent: review.issue.as_str(),
        custom_preamble: preamble.as_deref(),
        approved_plan: None,
        artifact: &artifact,
        comments: &review.comments,
        ci_failures,
        skills: &skills,
    };
    let assembled = cockpit_core::prompt::assemble_rework(&rework_input);

    match try_spawn_agent(state, app_handle, pr, pr_ref, &review.worktree, &assembled).await {
        Ok(agent_run) => {
            state.reviews.update(pr_ref, |r| {
                r.agent = Some(agent_run);
            });
        }
        Err(e) => {
            eprintln!("dispatch_fix_agent: agent spawn failed: {e}");
            use tauri::Emitter;
            let error_event = cockpit_core::adapters::agent_stream::Event::Error {
                message: format!("Agent spawn failed: {e}"),
            };
            let _ = app_handle.emit("agent-event", &error_event);
        }
    }
}

/// Attempt to spawn a fixer agent. Factored out so the caller can treat
/// failure as non-fatal.
async fn try_spawn_agent(
    state: &AppState,
    app_handle: &tauri::AppHandle,
    pr: &str,
    pr_ref: &PrRef,
    worktree: &std::path::Path,
    prompt: &cockpit_core::prompt::AssembledPrompt,
) -> Result<cockpit_core::model::AgentRun, String> {
    let config = Config::load().map_err(|e| format!("config: {e}"))?;
    let spawn_config = SpawnConfig::from_config(&config);
    let hook_url = format!("http://127.0.0.1:{}/hook/stop", config.hook_port);

    let spawn_result = cockpit_core::adapters::agent::spawn_agent(
        worktree,
        prompt,
        cockpit_core::model::AgentMode::Fix,
        pr_ref.as_str(),
        &state.sessions,
        &hook_url,
        &spawn_config,
    )
    .await
    .map_err(|e| format!("spawn: {e}"))?;

    let stream_ctx = crate::streaming::StreamContext {
        object_id: pr.to_string(),
        mode: cockpit_core::model::AgentMode::Fix,
        completion_tx: state.completion_tx.clone(),
    };
    Ok(crate::streaming::start_stream_forwarding(
        spawn_result,
        app_handle.clone(),
        stream_ctx,
    ))
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
// CI visibility + dispatch-to-fix
// ---------------------------------------------------------------------------

/// Parse a PR number from a review's PR reference.
///
/// Accepts URL form (`https://.../pull/42`) and `owner/repo#42` form; returns
/// `None` when no number can be parsed.
fn pr_number_from_ref(pr: &str) -> Option<u64> {
    if let Some(tail) = pr.rsplit('/').next()
        && let Ok(n) = tail.parse::<u64>()
    {
        return Some(n);
    }
    if let Some(tail) = pr.rsplit('#').next()
        && let Ok(n) = tail.parse::<u64>()
    {
        return Some(n);
    }
    None
}

/// Fetch CI checks for a review's PR and return their rollup (STATUS tier).
///
/// Reads checks via `gh pr checks` (using the review's `repo_slug` when set),
/// emits a `ci-updated` event carrying the full [`CiCheck`] list so the
/// frontend updates via events (not polling), and returns the [`CiSummary`]
/// rollup for the badge.
///
/// This never blocks the review loop and never mutates review state: any `gh`
/// error (including a PR with no checks) is treated as non-fatal and yields an
/// empty summary with an empty checks event (Invariant 1).
#[tauri::command]
pub async fn fetch_ci_checks(
    state: State<'_, Arc<AppState>>,
    app_handle: tauri::AppHandle,
    pr: String,
) -> Result<github::CiSummary, CommandError> {
    use tauri::Emitter;

    let pr_ref = PrRef::new(&pr);
    let repo_slug = state.reviews.get(&pr_ref).and_then(|r| r.repo_slug.clone());

    let empty = github::CiSummary {
        passed: 0,
        total: 0,
        failed: 0,
        pending: 0,
    };

    let Some(pr_number) = pr_number_from_ref(&pr) else {
        // No parseable PR number — nothing to fetch. Emit an empty update so
        // the badge clears rather than hanging.
        let _ = app_handle.emit("ci-updated", (&pr, Vec::<github::CiCheck>::new()));
        return Ok(empty);
    };

    match github::pr_checks(repo_slug.as_deref(), pr_number).await {
        Ok(checks) => {
            let summary = github::summarize(&checks);
            // Push the full checks list to the frontend via event (§4).
            let _ = app_handle.emit("ci-updated", (&pr, &checks));
            Ok(summary)
        }
        Err(e) => {
            // Non-fatal: a PR may legitimately have no checks (gh exits
            // non-zero). Report an empty summary; never block the loop.
            eprintln!("fetch_ci_checks: {e}");
            let _ = app_handle.emit("ci-updated", (&pr, Vec::<github::CiCheck>::new()));
            Ok(empty)
        }
    }
}

/// List the CI checks for a review's PR (STATUS tier, best-effort query).
///
/// Resolves the PR number and `repo_slug` from the stored review and reads
/// checks via `gh pr checks`. Unlike [`fetch_ci_checks`], this returns the full
/// [`CiCheck`] list directly (for the CI tab) and emits no event.
///
/// Per Invariant 1 this is a best-effort UI query that must never block the
/// review loop: any `gh` error (including a PR with no checks, or a
/// non-parseable PR reference) is treated as non-fatal and yields an EMPTY
/// list, with the error logged to stderr.
#[tauri::command]
pub async fn list_ci_checks(
    state: State<'_, Arc<AppState>>,
    pr: String,
) -> Result<Vec<github::CiCheck>, CommandError> {
    let pr_ref = PrRef::new(&pr);
    let repo_slug = state.reviews.get(&pr_ref).and_then(|r| r.repo_slug.clone());

    let Some(pr_number) = pr_number_from_ref(&pr) else {
        return Ok(Vec::new());
    };

    match github::pr_checks(repo_slug.as_deref(), pr_number).await {
        Ok(checks) => Ok(checks),
        Err(e) => {
            eprintln!("list_ci_checks: {e}");
            Ok(Vec::new())
        }
    }
}

/// Fetch the failed-job logs for a single CI run, identified by a check `link`
/// (LOG tier, best-effort, per-pipeline).
///
/// The `link` is a check's details URL (e.g.
/// `https://github.com/owner/repo/actions/runs/123/job/456`); the run id is
/// extracted server-side via [`github::run_id_from_link`] and the logs are read
/// via `gh run view <run-id> --log-failed`, scoped to the review's `repo_slug`.
///
/// Per Invariant 1 this is non-fatal: a `link` with no parseable run id, or any
/// `gh` error, yields an EMPTY string with the error logged to stderr, never a
/// hard failure that could block the UI. The CI panel calls this per pipeline,
/// so passing pipelines run no subprocess at all (only failed pipelines fetch).
#[tauri::command]
pub async fn ci_run_logs_by_link(
    state: State<'_, Arc<AppState>>,
    pr: String,
    link: String,
) -> Result<String, CommandError> {
    let pr_ref = PrRef::new(&pr);
    let repo_slug = state.reviews.get(&pr_ref).and_then(|r| r.repo_slug.clone());

    let Some(run_id) = github::run_id_from_link(&link) else {
        return Ok(String::new());
    };

    match github::run_logs(repo_slug.as_deref(), run_id).await {
        Ok(logs) => Ok(logs),
        Err(e) => {
            eprintln!("ci_run_logs_by_link: {e}");
            Ok(String::new())
        }
    }
}

/// Dispatch the Fix loop to address a PR's CI failures (LOG tier).
///
/// This is an EXPLICIT user action (Invariant 5): it never auto-fires. It
/// fetches the failed-CI logs on demand, reuses the diff-gate Fix loop
/// (`request_changes` spawn path via [`dispatch_fix_agent`]) — ensuring the
/// review is `InReview`, adding a synthetic local comment summarizing the CI
/// failure so the gate's ≥1-comment rule is met, transitioning
/// `request_changes` (→ `Dispatched`), and spawning the fixer agent with a
/// rework prompt carrying the CI logs verbatim.
///
/// The CI-log fetch is best-effort: a failure yields no logs but the Fix loop
/// still dispatches (the synthetic comment still tells the agent CI failed),
/// so a GitHub read never blocks the loop.
#[tauri::command]
pub async fn fix_ci(
    state: State<'_, Arc<AppState>>,
    app_handle: tauri::AppHandle,
    pr: String,
) -> Result<Review, CommandError> {
    let pr_ref = PrRef::new(&pr);
    let review = state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {pr}"),
    })?;

    // On-demand LOG-tier fetch. Best-effort: failure yields empty logs.
    let ci_logs = match pr_number_from_ref(&pr) {
        Some(n) => github::failed_ci_logs(review.repo_slug.as_deref(), n)
            .await
            .unwrap_or_else(|e| {
                eprintln!("fix_ci: failed_ci_logs: {e}");
                String::new()
            }),
        None => String::new(),
    };

    // Ensure the review is InReview before adding the comment + requesting
    // changes. Reworked -> InReview via open(); a wrong starting state surfaces
    // as a transition error below.
    let mut transition_err: Option<cockpit_core::gate::Error> = None;
    state.reviews.update(&pr_ref, |r| {
        if r.gate_state == GateState::Reworked
            && let Err(e) = r.open()
        {
            transition_err = Some(e);
            return;
        }
        if r.gate_state == GateState::Pending
            && let Err(e) = r.open()
        {
            transition_err = Some(e);
            return;
        }

        // Synthetic Local comment so the gate's ≥1-comment rule is satisfied.
        // Anchored to the PR (no specific line) via a zero-length diff anchor.
        let summary = if ci_logs.trim().is_empty() {
            "CI is failing on this PR. Investigate and fix the failing checks.".to_string()
        } else {
            "CI is failing on this PR. See the CI Failures section for the failed \
             job logs; fix the failing checks."
                .to_string()
        };
        r.comments.push(Comment {
            id: CommentId::new(format!("ci-{}", uuid::Uuid::new_v4())),
            anchor: Anchor::DiffLine {
                path: PathBuf::from("CI"),
                range: (0, 0),
            },
            body: summary,
            origin: CommentOrigin::Local,
        });

        if let Err(e) = r.request_changes() {
            transition_err = Some(e);
        }
    });

    if let Some(e) = transition_err {
        return Err(CommandError::from(e));
    }

    // Reuse the shared Fix-loop spawn path, carrying the CI logs into the
    // rework prompt verbatim.
    let ci_arg = if ci_logs.trim().is_empty() {
        None
    } else {
        Some(ci_logs.as_str())
    };
    dispatch_fix_agent(&state, &app_handle, &pr, &pr_ref, ci_arg).await;

    state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {pr}"),
    })
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
        plan_path: Some(PathBuf::from(file)),
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

/// Generate the initial project plan by spawning a planner agent.
///
/// This is an artifact-filling spawn: it does **not** move the gate. The plan
/// stays `Pending` while the planner (`AgentMode::Plan`) runs in the repo
/// working directory; on Stop-hook completion the plan is left `Pending` and
/// ready for the user to `plan_open`. Mirrors how implementers fill a review's
/// diff while the review stays `Pending`.
///
/// Requires a loaded plan (see `load_plan`/`kickoff`) and a configured repo
/// path. Spawn failure is surfaced as an error; the plan state is unchanged.
#[tauri::command]
pub async fn generate_plan(
    state: State<'_, Arc<AppState>>,
    app_handle: tauri::AppHandle,
) -> Result<ProjectPlan, CommandError> {
    let plan = state.plan.get().ok_or_else(|| CommandError {
        message: "No project plan loaded".into(),
    })?;

    // Resolve the on-disk destination the planner writes to, and ensure the
    // parent directory exists so the agent's write succeeds. This path is
    // read back and parsed on completion (see the Plan completion arm).
    let plan_path = cockpit_core::config::plan_file_path(plan.project.as_str())?;
    if let Some(parent) = plan_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Assemble the initial plan-generation prompt (intent = the project goal;
    // no comments — the plan does not exist yet). The Plan-mode custom preamble
    // is injected verbatim; a config load failure falls back to the builtin.
    let preamble = Config::load()
        .ok()
        .and_then(|c| c.agent_prompts.for_mode(AgentMode::Plan).map(str::to_owned));
    let intent = format!("Produce a project plan for {}.", plan.project);
    // Plan generation has no diff yet — surface universal (untagged) skills.
    // Discovery failures are non-fatal.
    let skills = cockpit_core::skills::relevant_for_diff("");
    let plan_input = cockpit_core::prompt::PlanInput {
        intent: &intent,
        custom_preamble: preamble.as_deref(),
        issues: &[],
        current_plan: Some(&plan.doc),
        output_path: Some(&plan_path),
        skills: &skills,
    };
    let assembled = cockpit_core::prompt::assemble_plan_prompt(&plan_input);

    let object_id = plan.project.as_str().to_string();
    let run = spawn_plan_agent(&state, &app_handle, &object_id, &assembled).await?;

    // Attach the running agent and record the write destination; the plan
    // stays Pending (artifact-fill).
    state.plan.update(|p| {
        p.agent = Some(run);
        p.plan_path = Some(plan_path);
    });

    state.plan.get().ok_or_else(|| CommandError {
        message: "Plan disappeared after update".into(),
    })
}

/// Transition the plan to `Dispatched` and spawn a planner agent for rework.
///
/// Requires that the plan is in `InReview` state with at least one comment.
/// The gate transition happens first; then a planner (`AgentMode::Plan`) is
/// spawned with the rework prompt (approved-plan-absent, artifact = the plan
/// doc, plus the gathered comments) — exactly like `request_changes` does for
/// reviews. Spawn failure is non-fatal: the gate already advanced, so the
/// `Dispatched` plan is returned and the error is logged.
///
/// This is an explicit user action (Invariant 5).
#[tauri::command]
pub async fn plan_request_changes(
    state: State<'_, Arc<AppState>>,
    app_handle: tauri::AppHandle,
) -> Result<ProjectPlan, CommandError> {
    let mut plan = state.plan.get().ok_or_else(|| CommandError {
        message: "No project plan loaded".into(),
    })?;

    plan.request_changes()?;

    // Resolve (and record) the on-disk destination for the revised plan so the
    // completion arm can read + parse it back. Reuse an existing path when the
    // plan already has one (e.g. loaded from a file); otherwise derive it.
    let plan_path = match plan.plan_path.clone() {
        Some(p) => p,
        None => cockpit_core::config::plan_file_path(plan.project.as_str())?,
    };
    if let Some(parent) = plan_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    plan.plan_path = Some(plan_path.clone());
    state.plan.set(plan.clone());

    // Assemble the rework prompt over the plan artifact + comments. Plan-mode
    // custom preamble injected verbatim (builtin fallback on load failure).
    let preamble = Config::load()
        .ok()
        .and_then(|c| c.agent_prompts.for_mode(AgentMode::Plan).map(str::to_owned));
    // Plan rework operates on the plan doc, not a file diff — surface universal
    // (untagged) skills. Discovery failures are non-fatal.
    let skills = cockpit_core::skills::relevant_for_diff("");
    let artifact = Artifact::Plan(plan.doc.clone());
    // Instruct the planner to persist the revised plan to the recorded path,
    // in the pinned format, so cockpit can parse it back on completion.
    let intent = format!(
        "Revise the project plan for {}. Write the finished plan as markdown to `{}` in the same pinned format.",
        plan.project,
        plan_path.display()
    );
    let rework_input = cockpit_core::prompt::ReworkInput {
        intent: &intent,
        custom_preamble: preamble.as_deref(),
        approved_plan: None,
        artifact: &artifact,
        comments: &plan.comments,
        ci_failures: None,
        skills: &skills,
    };
    let assembled = cockpit_core::prompt::assemble_rework(&rework_input);

    let object_id = plan.project.as_str().to_string();
    match spawn_plan_agent(&state, &app_handle, &object_id, &assembled).await {
        Ok(run) => {
            state.plan.update(|p| p.agent = Some(run));
        }
        Err(e) => {
            eprintln!("plan_request_changes: planner spawn failed: {e}");
            use tauri::Emitter;
            let error_event = cockpit_core::adapters::agent_stream::Event::Error {
                message: format!("Planner spawn failed: {e}"),
            };
            let _ = app_handle.emit("agent-event", &error_event);
        }
    }

    state.plan.get().ok_or_else(|| CommandError {
        message: "Plan disappeared after update".into(),
    })
}

/// Spawn a planner agent (`AgentMode::Plan`) in the repo working directory.
///
/// Factored out so both initial generation and rework share one spawn path.
/// Uses the configured repo path as the agent's working directory (the plan
/// is a document produced against the repo). Returns the [`AgentRun`] with
/// stdout streaming wired to the frontend.
async fn spawn_plan_agent(
    state: &AppState,
    app_handle: &tauri::AppHandle,
    object_id: &str,
    prompt: &cockpit_core::prompt::AssembledPrompt,
) -> Result<cockpit_core::model::AgentRun, CommandError> {
    let config = Config::load()?;
    let repo_path = config
        .repo_path
        .clone()
        .unwrap_or_else(|| PathBuf::from("."));
    let spawn_config = SpawnConfig::from_config(&config);
    let hook_url = format!("http://127.0.0.1:{}/hook/stop", config.hook_port);

    let spawn_result = cockpit_core::adapters::agent::spawn_agent(
        &repo_path,
        prompt,
        cockpit_core::model::AgentMode::Plan,
        object_id,
        &state.sessions,
        &hook_url,
        &spawn_config,
    )
    .await?;

    let stream_ctx = crate::streaming::StreamContext {
        object_id: object_id.to_string(),
        mode: cockpit_core::model::AgentMode::Plan,
        completion_tx: state.completion_tx.clone(),
    };
    Ok(crate::streaming::start_stream_forwarding(
        spawn_result,
        app_handle.clone(),
        stream_ctx,
    ))
}

/// Approve the plan (`InReview` -> `Approved`) and fan out implementers.
///
/// This is the guarded side effect of the plan gate (Invariant 5 / `SPEC.md`
/// §12): it only ever runs from this explicit user command, never from agent
/// output. On approval it spawns one implementer agent (`AgentMode::Implement`)
/// per frontier review of the plan's project — a dedicated worktree each,
/// bounded by `max_parallel_agents`. Each implementer builds the initial code
/// in its worktree; the reviews stay `Pending` (ready for human review) —
/// nothing auto-advances.
///
/// The gate transition happens first and is authoritative. Fan-out failure is
/// reported as an error but the plan remains `Approved`.
#[tauri::command]
pub async fn plan_approve(state: State<'_, Arc<AppState>>) -> Result<ProjectPlan, CommandError> {
    let mut plan = state.plan.get().ok_or_else(|| CommandError {
        message: "No project plan loaded".into(),
    })?;

    plan.approve()?;
    state.plan.set(plan.clone());

    // Fan out implementers for the project's frontier reviews. The plan's
    // ProjectRef and the first-class ProjectId share the same string (see
    // kickoff::project_from_linear), so reviews are selected by that id.
    let project_id = ProjectId::new(plan.project.as_str());
    // The approval stands regardless; a fan-out failure is surfaced to the
    // caller but does not roll back the (authoritative) gate transition.
    fan_out_implementers(&state, &plan.project, &project_id).await?;

    state.plan.get().ok_or_else(|| CommandError {
        message: "Plan disappeared after update".into(),
    })
}

/// Spawn implementer agents for a project's frontier reviews after approval.
///
/// Loads the project's reviews from the store, selects the frontier (roots of
/// the stack), and runs [`kickoff::spawn_batch`] with the configured
/// concurrency bound. Updates each spawned review's `base_sha` and `agent` in
/// the store; the reviews stay `Pending`.
async fn fan_out_implementers(
    state: &AppState,
    project: &ProjectRef,
    project_id: &ProjectId,
) -> Result<(), CommandError> {
    let config = Config::load()?;
    let repo_path = config
        .repo_path
        .clone()
        .unwrap_or_else(|| PathBuf::from("."));

    // Collect this project's reviews, then narrow to the frontier (roots).
    let mut reviews = cockpit_core::store::reviews_by_project(&state.reviews, Some(project_id));
    let frontier_ids = kickoff::select_frontier_reviews(&reviews);
    reviews.retain(|r| frontier_ids.contains(&r.id));

    if reviews.is_empty() {
        // Nothing to build (e.g. plan-only project); approval already stands.
        return Ok(());
    }

    // Phase 1 (synchronous): prepare worktrees. `git2::Repository` is not
    // `Send`, so it must not live across the `.await` below — scope it here so
    // it is dropped before spawning.
    // Implement-mode custom preamble, injected verbatim into every implementer
    // prompt (builtin fallback when unset).
    let implement_preamble = config
        .agent_prompts
        .for_mode(AgentMode::Implement)
        .map(str::to_owned);
    let prepared = {
        let repo = git2::Repository::discover(&repo_path).map_err(|e| CommandError {
            message: format!("could not open git repo at {}: {e}", repo_path.display()),
        })?;
        kickoff::prepare_batch_worktrees(
            &mut reviews,
            &repo,
            project,
            implement_preamble.as_deref(),
        )
        .map_err(CommandError::from)?
    };

    let spawn_config = SpawnConfig::from_config(&config);
    let hook_url = format!("http://127.0.0.1:{}/hook/stop", config.hook_port);

    let kickoff_config = kickoff::KickoffConfig {
        session_map: &state.sessions,
        hook_url: &hook_url,
        spawn_config: &spawn_config,
        max_parallel_agents: config.max_parallel_agents,
    };

    // Phase 2 (async): bounded agent fan-out. No repo handle in scope.
    kickoff::spawn_batch(&mut reviews, &prepared, &kickoff_config)
        .await
        .map_err(CommandError::from)?;

    // Persist the spawned agents + base SHAs back into the store.
    for review in &reviews {
        state.reviews.update(&review.pr, |r| {
            r.base_sha = review.base_sha.clone();
            r.agent = review.agent.clone();
        });
    }

    Ok(())
}

/// Return the [`BatchStatus`] for the plan's project (or ungrouped reviews).
///
/// Aggregates the project's reviews into building / ready / approved counts so
/// the frontend can show batch progress after a fan-out without polling each
/// review individually.
#[tauri::command]
pub fn batch_status(
    state: State<'_, Arc<AppState>>,
    project_id: Option<String>,
) -> Result<cockpit_core::store::BatchStatus, CommandError> {
    let id = project_id.map(ProjectId::new);
    Ok(cockpit_core::store::batch_status(
        &state.reviews,
        id.as_ref(),
    ))
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

/// Approve a single review by PR reference string (`InReview` -> `Approved`).
///
/// If the review is in `Reworked` state, it is first opened to `InReview`
/// before approving. The frontend calls this per review as an explicit user
/// action (Invariant 5: side effects require explicit confirmation).
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
// Agent prompt customization
// ---------------------------------------------------------------------------

/// Return the stored custom prompt override for `mode`, if any.
///
/// `None` means the mode uses its builtin default (see
/// [`get_builtin_agent_prompt`] for that text). Thin: reads config and returns
/// the [`AgentPrompts`](cockpit_core::config::AgentPrompts) entry.
#[tauri::command]
pub fn get_agent_prompt(mode: AgentMode) -> Result<Option<String>, CommandError> {
    let config = Config::load()?;
    Ok(config.agent_prompts.for_mode(mode).map(str::to_owned))
}

/// Return the builtin default prompt fragment for `mode`.
///
/// Used by the settings editor as the placeholder and the "reset to default"
/// value. This is the canonical builtin intent, never a stored override.
#[tauri::command]
pub fn get_builtin_agent_prompt(mode: AgentMode) -> Result<String, CommandError> {
    Ok(cockpit_core::prompt::builtin_intent(mode).to_string())
}

/// Persist a custom prompt override for `mode`.
///
/// An empty or whitespace-only `text` clears the override, resetting the mode
/// to its builtin default. The override is injected verbatim into that mode's
/// prompt at dispatch time.
#[tauri::command]
pub fn save_agent_prompt(mode: AgentMode, text: String) -> Result<(), CommandError> {
    let mut config = Config::load()?;
    config.agent_prompts.set_mode(mode, Some(text));
    config.save()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Skills commands
// ---------------------------------------------------------------------------

/// List installed skills from `<cockpit_home>/skills`.
///
/// Thin: delegates to [`cockpit_core::skills::discover_installed_skills`].
#[tauri::command]
pub fn list_skills() -> Result<Vec<cockpit_core::skills::Skill>, CommandError> {
    let skills = cockpit_core::skills::discover_installed_skills()?;
    Ok(skills)
}

/// Install or overwrite a local skill by name.
///
/// Writes `SKILL.md` (+ `.meta.json`) marking the skill [`SkillSource::Local`]
/// so a later GitHub sync never clobbers a hand edit. Explicit user action.
#[tauri::command]
pub fn save_skill(name: String, contents: String) -> Result<(), CommandError> {
    cockpit_core::skills::install_skill(
        &name,
        &contents,
        cockpit_core::skills::SkillSource::Local,
    )?;
    Ok(())
}

/// Delete an installed skill by name (Invariant 5: explicit user action).
#[tauri::command]
pub fn delete_skill(name: String) -> Result<(), CommandError> {
    cockpit_core::skills::delete_skill(&name)?;
    Ok(())
}

/// Sync skills from the configured GitHub source via the `gh` CLI.
///
/// Requires `[skills_github]` in config. Uses the user's `gh auth` (no PAT).
/// Returns a [`SyncReport`](cockpit_core::skills::SyncReport) of counts.
#[tauri::command]
pub async fn sync_skills() -> Result<cockpit_core::skills::SyncReport, CommandError> {
    let config = Config::load()?;
    let source = config.skills_github.ok_or_else(|| CommandError {
        message: "No skills GitHub source configured. Set it in Settings.".into(),
    })?;
    let report = cockpit_core::skills::sync_from_github(
        &source.owner,
        &source.repo,
        &source.branch,
        &source.path,
    )
    .await?;
    Ok(report)
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
            plan_path: None,
        };
        state.plan.set(plan);
    }

    // 4. Build reviews for frontier issues. Kickoff creates a first-class
    //    Linear-backed project that groups the reviews; worktrees live under
    //    the cockpit home (outside the managed repo) and are keyed via the
    //    unified `review_worktree_path` scheme so projects never collide.
    let cockpit_project = kickoff::project_from_linear(&project, format!("Project {project}"));

    let mut reviews = kickoff::build_reviews_for_frontier(
        &frontier,
        &data,
        &issue_dag,
        "main",
        Some(&cockpit_project.id),
    );
    for review in &mut reviews {
        review.worktree = kickoff::review_worktree_path(review)?;
    }

    // 5. Store the project and its reviews in the in-memory stores.
    state.projects.insert(cockpit_project);
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
// Project commands
// ---------------------------------------------------------------------------

/// List all first-class projects currently in the store.
#[tauri::command]
pub fn list_projects(state: State<'_, Arc<AppState>>) -> Result<Vec<Project>, CommandError> {
    Ok(state.projects.list())
}

/// Create a new ad-hoc project with the given name.
///
/// This is an explicit user action (Invariant 5): ad-hoc projects only ever
/// come from a deliberate UI action, never from agent output. Returns the
/// created [`Project`].
#[tauri::command]
pub fn create_project(
    state: State<'_, Arc<AppState>>,
    name: String,
) -> Result<Project, CommandError> {
    let project = kickoff::create_ad_hoc_project(name);
    state.projects.insert(project.clone());
    Ok(project)
}

/// Attach an existing review to a project.
///
/// Looks up the review by PR reference and sets its `project` field. Returns
/// the updated [`Review`]. Errors if either the review or the project is
/// unknown.
#[tauri::command]
pub fn attach_review(
    state: State<'_, Arc<AppState>>,
    pr: String,
    project_id: String,
) -> Result<Review, CommandError> {
    let pr_ref = PrRef::new(&pr);
    let project_id = ProjectId::new(&project_id);

    if state.projects.get(&project_id).is_none() {
        return Err(CommandError {
            message: format!("Project not found: {project_id}"),
        });
    }

    let updated = state.reviews.update(&pr_ref, |r| {
        r.project = Some(project_id.clone());
    });
    if !updated {
        return Err(CommandError {
            message: format!("Review not found: {pr}"),
        });
    }

    state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found after attach: {pr}"),
    })
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
    app_handle: tauri::AppHandle,
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
    let repo_path = config
        .repo_path
        .clone()
        .unwrap_or_else(|| PathBuf::from("."));
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
        let spawn_config = SpawnConfig::from_config(&config);
        let hook_url = format!("http://127.0.0.1:{}/hook/stop", config.hook_port);
        let worktree_path = review.worktree.clone();

        // Restack-mode custom preamble, injected verbatim (builtin fallback).
        let preamble = config
            .agent_prompts
            .for_mode(AgentMode::Restack)
            .map(str::to_owned);
        let prompt =
            restack::assemble_conflict_prompt(&review, &parent_branch, preamble.as_deref());
        let spawn_result = cockpit_core::adapters::agent::spawn_agent(
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

        // Start streaming agent stdout to the frontend.
        let stream_ctx = crate::streaming::StreamContext {
            object_id: review.id.as_str().to_string(),
            mode: cockpit_core::model::AgentMode::Restack,
            completion_tx: state.completion_tx.clone(),
        };
        let agent_run =
            crate::streaming::start_stream_forwarding(spawn_result, app_handle, stream_ctx);
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
///
/// When a review already exists in the store (matched by PR URL), only the
/// diff, branch, and base are refreshed — comments, gate state, agent run,
/// and stale flag are preserved. This prevents re-fetching from GitHub from
/// blowing away in-progress review work.
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

    let source = match filter {
        github::PrFilter::Authored => cockpit_core::model::ReviewSource::Authored,
        github::PrFilter::ReviewRequested => cockpit_core::model::ReviewSource::ReviewRequested,
    };

    let mut reviews = Vec::with_capacity(prs.len());
    for pr in &prs {
        let diff = if pr.repo_slug.is_empty() {
            github::pr_diff_in(&repo_path, pr.number)
                .await
                .unwrap_or_default()
        } else {
            github::pr_diff_by_repo(&pr.repo_slug, pr.number)
                .await
                .unwrap_or_default()
        };

        let pr_ref = PrRef::new(&pr.url);

        if state.reviews.get(&pr_ref).is_some() {
            let branch = pr.head_ref_name.clone();
            let base = pr.base_ref_name.clone();
            state.reviews.update(&pr_ref, |r| {
                r.diff = cockpit_core::model::DiffData { raw: diff };
                r.branch = branch;
                r.base = base;
            });
            if let Some(updated) = state.reviews.get(&pr_ref) {
                reviews.push(updated);
            }
        } else {
            let review = github::build_review_from_pr(pr, diff, &repo_path, source);
            state.reviews.insert(review.clone());
            reviews.push(review);
        }
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
        plan_path: Some(PathBuf::from(path)),
    };
    state.plan.set(plan.clone());
    Ok(plan)
}

// ---------------------------------------------------------------------------
// Open in editor
// ---------------------------------------------------------------------------

/// Open a file in the user's configured IDE/editor.
///
/// Uses the `ide_command` from config (e.g. "cursor", "code", "zed") to open
/// the given file path. For cross-repo PRs or branches not checked out
/// locally, fetches the branch into a worktree first.
#[tauri::command]
pub async fn open_in_editor(
    _state: State<'_, Arc<AppState>>,
    file_path: String,
    repo_slug: Option<String>,
    branch: Option<String>,
) -> Result<(), CommandError> {
    let config = Config::load()?;
    let ide = config
        .ide_command
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "code".to_string());
    let repo_path = config.repo_path.unwrap_or_else(|| PathBuf::from("."));

    let full_path = {
        let local = repo_path.join(&file_path);
        if local.exists() {
            local
        } else if let Some(ref branch) = branch {
            let root = cockpit_core::adapters::git::ensure_branch_checkout(
                &repo_path,
                branch,
                repo_slug.as_deref(),
            )
            .await
            .map_err(|e| CommandError {
                message: format!("failed to checkout branch for {file_path}: {e}"),
            })?;
            root.join(&file_path)
        } else {
            local
        }
    };

    tokio::process::Command::new(&ide)
        .arg(full_path.as_os_str())
        .spawn()
        .map_err(|e| CommandError {
            message: format!("failed to open {file_path} in {ide}: {e}"),
        })?;

    Ok(())
}

/// Start (or reuse) a Monaco LSP bridge for the given Monaco `languageId`.
///
/// Returns the localhost WebSocket URL the frontend language client should
/// connect to, or `None` when LSP is disabled or the language has no
/// configured server. The bridge is lazily started per language and cached in
/// [`AppState::lsp_bridges`], so repeated calls for the same language reuse the
/// existing bridge and URL.
///
/// The bridge only spawns the actual language-server child when the webview
/// opens a WebSocket, so an unavailable binary (pyright/tsserver not installed)
/// surfaces at connect time as a closed socket, not here — keeping this command
/// thin and non-fatal (Invariant 1).
#[tauri::command]
pub async fn start_lsp_bridge(
    state: State<'_, Arc<AppState>>,
    language_id: String,
) -> Result<Option<String>, CommandError> {
    use cockpit_core::adapters::lsp::LspBridge;
    use cockpit_core::config::LspLanguage;

    let Some(language) = LspLanguage::from_language_id(&language_id) else {
        return Ok(None);
    };

    let config = Config::load()?;
    if !config.lsp_servers.enabled {
        return Ok(None);
    }

    // Fast path: an existing bridge for this language. Take the URL under the
    // lock and drop it immediately — never hold the lock across `.await`.
    {
        let bridges = state.lsp_bridges.lock().map_err(|_| CommandError {
            message: "LSP bridge registry lock poisoned".to_string(),
        })?;
        if let Some(existing) = bridges.get(&language) {
            return Ok(Some(existing.url()));
        }
    }

    // Start a new bridge (async) with the lock released.
    let command = config.lsp_servers.command_for(language);
    let bridge = LspBridge::start(language, command).await?;
    let url = bridge.url();

    // Re-acquire the lock to insert. Double-check: another task may have
    // started one concurrently; if so, keep the first and drop ours (its
    // Drop aborts the just-started serve task, no orphan).
    let mut bridges = state.lsp_bridges.lock().map_err(|_| CommandError {
        message: "LSP bridge registry lock poisoned".to_string(),
    })?;
    if let Some(existing) = bridges.get(&language) {
        return Ok(Some(existing.url()));
    }
    bridges.insert(language, bridge);
    Ok(Some(url))
}
