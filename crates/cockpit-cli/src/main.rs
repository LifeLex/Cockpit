//! cockpit -- thin CLI over `cockpit-core`; the validation surface for phases 0-2.
//!
//! Subcommands arrive with the plan: `project`/`ingest` (T0.7), `comment`/
//! `request-changes`/`start` (T1.4), `kickoff` (T2.3).

use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Parser;

use cockpit_core::adapters::agent::{SessionMap, SpawnConfig};
use cockpit_core::adapters::{github, linear};
use cockpit_core::batch;
use cockpit_core::dag;
use cockpit_core::gate::Gated;
use cockpit_core::hook_server::{self, HookState};
use cockpit_core::model::{
    Anchor, Artifact, Comment, CommentId, CommentOrigin, DiffData, GateState, IssueRef, PrRef,
    ProjectPlan, ProjectRef, Review, ReviewId, ReviewSource,
};
use cockpit_core::plan_parser;
use cockpit_core::prompt::{self, ReworkInput};
use cockpit_core::restack;
use cockpit_core::store::{self, PlanStore, ReviewStore};

// ---------------------------------------------------------------------------
// CLI structure
// ---------------------------------------------------------------------------

/// Review cockpit for Linear projects.
#[derive(Parser)]
#[command(name = "cockpit", about = "Review cockpit for Linear projects")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// Top-level subcommands.
#[derive(clap::Subcommand)]
enum Command {
    /// Read a Linear project, build the issue DAG, and print the frontier.
    Project {
        /// Linear project ID.
        id: String,
    },
    /// List existing PRs and link them to Linear issues.
    Ingest,
    /// Add a comment to a PR under review.
    Comment {
        #[command(subcommand)]
        action: CommentAction,
    },
    /// Gather comments, assemble prompt, spawn fixer agent, transition to Dispatched.
    RequestChanges {
        /// PR number.
        pr: u64,
    },
    /// Start the review loop: ingest PRs, start hook server, wait.
    Start {
        /// Hook server port.
        #[arg(long, default_value = "19876")]
        port: u16,
    },
    /// Manage the project plan (plan gate).
    Plan {
        #[command(subcommand)]
        action: PlanAction,
    },
    /// Restack a stale PR onto its parent's new head.
    Restack {
        /// PR number to restack.
        pr: u64,
    },
    /// Mirror local comments for a PR to GitHub (explicit side effect per Invariant 5).
    Mirror {
        /// PR number.
        pr: u64,
        /// Show what would be posted without actually posting.
        #[arg(long)]
        dry_run: bool,
    },
    /// Kick off a Linear project: fetch issues, optionally plan, then spawn
    /// implementer agents for each frontier issue.
    Kickoff {
        /// Linear project ID.
        project_id: String,
        /// Skip the plan gate and go directly to batch implementation.
        #[arg(long)]
        skip_plan: bool,
        /// Hook server port for agent completion callbacks.
        #[arg(long, default_value = "19876")]
        port: u16,
    },
    /// Evaluate frontier reviews for batch approval.
    ///
    /// By default shows a dry-run table of reviews and their verdicts.
    /// Pass `--confirm` to actually approve all eligible reviews.
    /// Per CLAUDE.md Invariant 5: batch-approve NEVER fires automatically.
    BatchApprove {
        /// Show what would be approved without approving (default).
        #[arg(long)]
        dry_run: bool,
        /// Actually approve all eligible reviews (explicit user action).
        #[arg(long)]
        confirm: bool,
    },
}

/// Subcommands for the `comment` verb.
#[derive(clap::Subcommand)]
enum CommentAction {
    /// Add an anchored comment to a PR.
    Add {
        /// PR number.
        pr: u64,
        /// File path in the diff.
        #[arg(long)]
        file: String,
        /// Line range (e.g. "10-15" or "42").
        #[arg(long)]
        line: String,
        /// Comment body.
        body: String,
    },
}

/// Subcommands for the `plan` verb.
#[derive(clap::Subcommand)]
enum PlanAction {
    /// Load a plan document from a file.
    Load {
        /// Path to the plan markdown file.
        file: String,
        /// Linear project ID for the plan.
        #[arg(long, default_value = "default")]
        project: String,
    },
    /// Show the current plan.
    Show,
    /// Add a comment to the plan.
    Comment {
        /// Anchor: "step:N" for a plan step, "file:path" for a file.
        #[arg(long)]
        anchor: String,
        /// Comment body.
        body: String,
    },
    /// Request changes on the plan (dispatch planner agent).
    RequestChanges,
    /// Approve the current plan.
    Approve,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Project { id } => run_project(&id).await,
        Command::Ingest => run_ingest().await,
        Command::Comment { action } => run_comment(action).await,
        Command::RequestChanges { pr } => run_request_changes(pr).await,
        Command::Start { port } => run_start(port).await,
        Command::Plan { action } => run_plan(action).await,
        Command::Mirror { pr, dry_run } => run_mirror(pr, dry_run).await,
        Command::Restack { pr } => run_restack(pr).await,
        Command::Kickoff {
            project_id,
            skip_plan,
            port,
        } => run_kickoff(&project_id, skip_plan, port).await,
        Command::BatchApprove { dry_run, confirm } => run_batch_approve(dry_run, confirm).await,
    }
}

// ---------------------------------------------------------------------------
// `cockpit project <id>`
// ---------------------------------------------------------------------------

/// Fetch issues from Linear, build the dependency DAG, and print the frontier.
async fn run_project(project_id: &str) -> Result<()> {
    let api_key = env::var("LINEAR_API_KEY").context(
        "LINEAR_API_KEY env var is required.\n\
         Set it to your Linear personal API key:\n  \
         export LINEAR_API_KEY=lin_api_...",
    )?;

    let project_ref = ProjectRef::new(project_id);
    let client = reqwest::Client::new();

    let data = linear::fetch_project_issues(&client, &api_key, &project_ref)
        .await
        .context("failed to fetch project issues from Linear")?;

    let issue_count = data.issues.len();
    let dag = linear::build_issue_dag(&data);

    let dep_count: usize = dag.values().map(|deps| deps.len()).sum();
    let frontier = dag::compute_frontier(&dag);

    println!("Project: {project_ref}");
    println!("Issues:  {issue_count}");
    println!("Dependencies: {dep_count}");
    println!();

    println!("Frontier ({} issues ready):", frontier.len());
    if frontier.is_empty() {
        println!("  (none)");
    } else {
        for issue in &frontier {
            print_issue_detail(issue, &data);
        }
    }

    println!();
    println!("All issues:");
    for node in &data.issues {
        let issue_ref = IssueRef::new(&node.identifier);
        let deps = dag.get(&issue_ref).map_or(0, |d| d.len());
        let marker = if frontier.contains(&issue_ref) {
            " [frontier]"
        } else {
            ""
        };
        println!(
            "  {} — {} (deps: {}){}",
            node.identifier, node.title, deps, marker
        );
    }

    Ok(())
}

/// Print a single frontier issue with its title (looked up from the project data).
fn print_issue_detail(issue: &IssueRef, data: &linear::ProjectData) {
    let title = data
        .issues
        .iter()
        .find(|n| n.identifier == issue.as_str())
        .map_or("(unknown)", |n| n.title.as_str());

    println!("  {} — {}", issue, title);
}

// ---------------------------------------------------------------------------
// `cockpit ingest`
// ---------------------------------------------------------------------------

/// List PRs from GitHub and link each to its Linear issue (if parseable from the branch).
async fn run_ingest() -> Result<()> {
    let prs = github::list_prs()
        .await
        .context("failed to list PRs via `gh`")?;

    let linked = github::link_prs_to_issues(&prs);

    println!("PRs found: {}", prs.len());
    println!();

    let mut linked_count = 0u64;
    let mut unlinked_count = 0u64;

    for (pr_ref, issue_ref) in &linked {
        match issue_ref {
            Some(issue) => {
                linked_count += 1;
                println!("  [linked]   {} -> {}", pr_ref, issue);
            }
            None => {
                unlinked_count += 1;
                println!("  [unlinked] {}", pr_ref);
            }
        }
    }

    println!();
    println!("Linked:   {linked_count}");
    println!("Unlinked: {unlinked_count}");

    Ok(())
}

// ---------------------------------------------------------------------------
// `cockpit start --port <port>`
// ---------------------------------------------------------------------------

/// Ingest PRs, create reviews, start the hook server, and wait.
async fn run_start(port: u16) -> Result<()> {
    // 1. Ingest PRs from GitHub.
    let prs = github::list_prs()
        .await
        .context("failed to list PRs via `gh`")?;

    let linked = github::link_prs_to_issues(&prs);

    // 2. Create Reviews for each PR that has a linked issue.
    let store = ReviewStore::new();
    let mut review_count = 0u64;

    for (pr_data, (pr_ref, issue_ref)) in prs.iter().zip(linked.iter()) {
        let Some(issue) = issue_ref else { continue };

        let review = Review {
            id: ReviewId::new(format!("r-{}", pr_data.number)),
            issue: issue.clone(),
            pr: pr_ref.clone(),
            branch: pr_data.head_ref_name.clone(),
            base: pr_data.base_ref_name.clone(),
            base_sha: String::new(),
            source: ReviewSource::Authored,
            worktree: PathBuf::from(format!(".cockpit/worktrees/{}", pr_data.number)),
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

        store.insert(review);
        review_count += 1;
    }

    // 3. Save state to file.
    let state_path = Path::new(store::STATE_FILE);
    store::save_to_file(&store, state_path).context("failed to save state file")?;

    println!("Reviews loaded: {review_count}");
    println!("State file: {}", state_path.display());
    println!();

    // 4. Start the hook server.
    let session_map = SessionMap::new();
    let (completion_tx, _rx) = hook_server::completion_channel();
    let hook_state = HookState {
        session_map,
        completion_tx,
    };

    let hook_url = format!("http://127.0.0.1:{port}/hook/stop");
    println!("Hook server: {hook_url}");
    println!("Waiting for agent completions...");

    hook_server::serve(hook_state, port)
        .await
        .context("hook server error")?;

    Ok(())
}

// ---------------------------------------------------------------------------
// `cockpit comment add <pr> --file <path> --line <range> "<body>"`
// ---------------------------------------------------------------------------

/// Parse a line range string into an inclusive `(start, end)` pair.
///
/// Accepts `"N"` for a single line or `"N-M"` for a range.
fn parse_line_range(s: &str) -> Result<(u32, u32)> {
    if let Some((start_str, end_str)) = s.split_once('-') {
        let start: u32 = start_str
            .parse()
            .with_context(|| format!("invalid line range start: {start_str:?}"))?;
        let end: u32 = end_str
            .parse()
            .with_context(|| format!("invalid line range end: {end_str:?}"))?;
        if start > end {
            bail!("line range start ({start}) must be <= end ({end})");
        }
        Ok((start, end))
    } else {
        let line: u32 = s
            .parse()
            .with_context(|| format!("invalid line number: {s:?}"))?;
        Ok((line, line))
    }
}

/// Add a comment to a PR.
async fn run_comment(action: CommentAction) -> Result<()> {
    match action {
        CommentAction::Add {
            pr,
            file,
            line,
            body,
        } => {
            let range = parse_line_range(&line)?;
            let pr_ref = PrRef::new(format!("owner/repo#{pr}"));

            // Load state from file.
            let state_path = Path::new(store::STATE_FILE);
            let store = store::load_from_file(state_path).context("failed to load state file")?;

            // Look up the review.
            let review = store
                .get(&pr_ref)
                .ok_or_else(|| anyhow::anyhow!("no review found for PR #{pr}"))?;

            // Create the comment.
            let comment = Comment {
                id: CommentId::new(format!("c-{}-{}", pr, review.comments.len() + 1)),
                anchor: Anchor::DiffLine {
                    path: PathBuf::from(&file),
                    range,
                },
                body: body.clone(),
                origin: CommentOrigin::Local,
            };

            // Add comment and transition to InReview if Pending.
            store.update(&pr_ref, |r| {
                r.comments.push(comment);
                if r.gate_state == GateState::Pending {
                    // INVARIANT: Pending -> InReview is always a legal transition.
                    r.gate_state = GateState::InReview;
                }
            });

            // Persist.
            store::save_to_file(&store, state_path).context("failed to save state file")?;

            let review = store
                .get(&pr_ref)
                .expect("review was just updated; must exist");
            println!(
                "Comment added to PR #{pr} at {file}:{}-{}",
                range.0, range.1
            );
            println!("State: {:?}", review.gate_state);
            println!("Total comments: {}", review.comments.len());

            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// `cockpit request-changes <pr>`
// ---------------------------------------------------------------------------

/// Gather comments, assemble the rework prompt, spawn the fixer agent, and
/// transition the review to Dispatched.
async fn run_request_changes(pr_num: u64) -> Result<()> {
    let pr_ref = PrRef::new(format!("owner/repo#{pr_num}"));

    // Load state from file.
    let state_path = Path::new(store::STATE_FILE);
    let store = store::load_from_file(state_path).context("failed to load state file")?;

    // Look up the review.
    let mut review = store
        .get(&pr_ref)
        .ok_or_else(|| anyhow::anyhow!("no review found for PR #{pr_num}"))?;

    // Transition InReview -> Dispatched (enforces >= 1 comment).
    review
        .request_changes()
        .map_err(|e| anyhow::anyhow!("cannot request changes: {e}"))?;

    // Assemble the rework prompt.
    let artifact = Artifact::Diff(review.diff.clone());
    let input = ReworkInput {
        intent: review.issue.as_str(),
        approved_plan: None,
        artifact: &artifact,
        comments: &review.comments,
        skills: &[],
    };
    let prompt = prompt::assemble_rework(&input);

    println!("Prompt hash: {}", prompt.hash);

    // Spawn the fixer agent.
    let session_map = SessionMap::new();
    let hook_url = "http://127.0.0.1:19876/hook/stop";
    let config = SpawnConfig::default();

    let spawn_result = cockpit_core::adapters::agent::spawn_agent(
        &review.worktree,
        &prompt,
        cockpit_core::model::AgentMode::Fix,
        review.id.as_str(),
        &session_map,
        hook_url,
        &config,
    )
    .await
    .context("failed to spawn fixer agent")?;

    println!("Agent PID: {}", spawn_result.run.pid);

    // Store the agent run on the review.
    review.agent = Some(spawn_result.run);

    // Persist the updated review.
    store.update(&pr_ref, |r| {
        r.gate_state = review.gate_state;
        r.comments = review.comments.clone();
        r.agent = review.agent.clone();
    });
    store::save_to_file(&store, state_path).context("failed to save state file")?;

    println!("Dispatched -- waiting for agent to complete");

    Ok(())
}

// ---------------------------------------------------------------------------
// `cockpit plan <action>`
// ---------------------------------------------------------------------------

/// Dispatch plan subcommands.
async fn run_plan(action: PlanAction) -> Result<()> {
    match action {
        PlanAction::Load { file, project } => run_plan_load(&file, &project).await,
        PlanAction::Show => run_plan_show().await,
        PlanAction::Comment { anchor, body } => run_plan_comment(&anchor, &body).await,
        PlanAction::RequestChanges => run_plan_request_changes().await,
        PlanAction::Approve => run_plan_approve().await,
    }
}

/// Load a plan document from a file, parse it, and store it.
async fn run_plan_load(file: &str, project_id: &str) -> Result<()> {
    let raw = std::fs::read_to_string(file)
        .with_context(|| format!("failed to read plan file: {file}"))?;

    let doc = plan_parser::parse(&raw).context("failed to parse plan document")?;

    let plan = ProjectPlan {
        project: ProjectRef::new(project_id),
        doc: doc.clone(),
        gate_state: GateState::Pending,
        comments: vec![],
        agent: None,
    };

    let plan_store = PlanStore::new();
    plan_store.set(plan);

    let state_path = Path::new(store::PLAN_STATE_FILE);
    store::save_plan_to_file(&plan_store, state_path).context("failed to save plan state")?;

    println!("Plan loaded: {}", doc.summary);
    println!("Steps:  {}", doc.steps.len());
    println!("Files:  {}", doc.files.len());
    println!("Risks:  {}", doc.risks.len());
    println!("State:  {:?}", GateState::Pending);

    Ok(())
}

/// Show the current plan.
async fn run_plan_show() -> Result<()> {
    let state_path = Path::new(store::PLAN_STATE_FILE);
    let plan_store = store::load_plan_from_file(state_path).context("failed to load plan state")?;

    let plan = plan_store
        .get()
        .ok_or_else(|| anyhow::anyhow!("no plan loaded; use `cockpit plan load <file>` first"))?;

    println!("Plan: {}", plan.doc.summary);
    println!("State: {:?}", plan.gate_state);
    println!("Project: {}", plan.project);
    println!();

    println!("Steps ({}):", plan.doc.steps.len());
    for step in &plan.doc.steps {
        println!("  {}. {}", step.index + 1, step.title);
        if !step.description.is_empty() {
            for line in step.description.lines() {
                println!("     {line}");
            }
        }
    }

    println!();
    println!("Files ({}):", plan.doc.files.len());
    for file in &plan.doc.files {
        println!("  - {}", file.display());
    }

    println!();
    println!("Risks ({}):", plan.doc.risks.len());
    if plan.doc.risks.is_empty() {
        println!("  (none)");
    } else {
        for risk in &plan.doc.risks {
            println!("  - {risk}");
        }
    }

    if !plan.comments.is_empty() {
        println!();
        println!("Comments ({}):", plan.comments.len());
        for (i, comment) in plan.comments.iter().enumerate() {
            let anchor_text = prompt::render_anchor(&comment.anchor, Some(&plan.doc));
            println!("  {}. [{}] {}", i + 1, anchor_text, comment.body);
        }
    }

    Ok(())
}

/// Add a comment to the plan.
async fn run_plan_comment(anchor_str: &str, body: &str) -> Result<()> {
    let anchor =
        plan_parser::parse_plan_anchor(anchor_str).context("failed to parse anchor string")?;

    let state_path = Path::new(store::PLAN_STATE_FILE);
    let plan_store = store::load_plan_from_file(state_path).context("failed to load plan state")?;

    if plan_store.get().is_none() {
        bail!("no plan loaded; use `cockpit plan load <file>` first");
    }

    plan_store.update(|plan| {
        let comment_num = plan.comments.len() + 1;
        let comment = Comment {
            id: CommentId::new(format!("pc-{comment_num}")),
            anchor,
            body: body.to_string(),
            origin: CommentOrigin::Local,
        };
        plan.comments.push(comment);

        // Transition Pending -> InReview if needed.
        if plan.gate_state == GateState::Pending {
            // INVARIANT: Pending -> InReview is always a legal transition.
            plan.gate_state = GateState::InReview;
        }
    });

    store::save_plan_to_file(&plan_store, state_path).context("failed to save plan state")?;

    let plan = plan_store.get().expect("plan was just updated; must exist");
    let anchor_text = prompt::render_anchor(
        &plan.comments.last().expect("just pushed a comment").anchor,
        Some(&plan.doc),
    );
    println!("Comment added: [{anchor_text}] {body}");
    println!("State: {:?}", plan.gate_state);
    println!("Total comments: {}", plan.comments.len());

    Ok(())
}

/// Request changes on the plan: assemble prompt and spawn planner agent.
async fn run_plan_request_changes() -> Result<()> {
    let state_path = Path::new(store::PLAN_STATE_FILE);
    let plan_store = store::load_plan_from_file(state_path).context("failed to load plan state")?;

    let mut plan = plan_store
        .get()
        .ok_or_else(|| anyhow::anyhow!("no plan loaded; use `cockpit plan load <file>` first"))?;

    // Transition InReview -> Dispatched (enforces >= 1 comment).
    plan.request_changes()
        .map_err(|e| anyhow::anyhow!("cannot request changes: {e}"))?;

    // Assemble the rework prompt (plan gate: no approved_plan, artifact is Plan).
    let artifact = Artifact::Plan(plan.doc.clone());
    let input = ReworkInput {
        intent: plan.project.as_str(),
        approved_plan: None,
        artifact: &artifact,
        comments: &plan.comments,
        skills: &[],
    };
    let assembled_prompt = prompt::assemble_rework(&input);

    println!("Prompt hash: {}", assembled_prompt.hash);

    // Spawn the planner agent.
    let session_map = SessionMap::new();
    let hook_url = "http://127.0.0.1:19876/hook/stop";
    let config = SpawnConfig::default();

    // Use the current directory as the worktree for plan-gate agents.
    let worktree = env::current_dir().context("failed to get current directory")?;

    let spawn_result = cockpit_core::adapters::agent::spawn_agent(
        &worktree,
        &assembled_prompt,
        cockpit_core::model::AgentMode::Plan,
        plan.project.as_str(),
        &session_map,
        hook_url,
        &config,
    )
    .await
    .context("failed to spawn planner agent")?;

    println!("Agent PID: {}", spawn_result.run.pid);

    // Store the agent run on the plan.
    plan.agent = Some(spawn_result.run);

    // Persist the updated plan.
    let updated_store = PlanStore::new();
    updated_store.set(plan);
    store::save_plan_to_file(&updated_store, state_path).context("failed to save plan state")?;

    println!("Dispatched -- waiting for planner agent to complete");

    Ok(())
}

/// Approve the current plan.
async fn run_plan_approve() -> Result<()> {
    let state_path = Path::new(store::PLAN_STATE_FILE);
    let plan_store = store::load_plan_from_file(state_path).context("failed to load plan state")?;

    let mut plan = plan_store
        .get()
        .ok_or_else(|| anyhow::anyhow!("no plan loaded; use `cockpit plan load <file>` first"))?;

    plan.approve()
        .map_err(|e| anyhow::anyhow!("cannot approve plan: {e}"))?;

    let updated_store = PlanStore::new();
    updated_store.set(plan);
    store::save_plan_to_file(&updated_store, state_path).context("failed to save plan state")?;

    println!("Plan approved -- ready for batch implementation");

    Ok(())
}

// ---------------------------------------------------------------------------
// `cockpit mirror <pr> [--dry-run]`
// ---------------------------------------------------------------------------

/// Mirror local comments for a PR to GitHub.
///
/// This is an explicit user action (Invariant 5): mirroring comments to a
/// public GitHub thread never happens automatically.
async fn run_mirror(pr_num: u64, dry_run: bool) -> Result<()> {
    let pr_ref = PrRef::new(format!("owner/repo#{pr_num}"));

    // Load state from file.
    let state_path = Path::new(store::STATE_FILE);
    let store = store::load_from_file(state_path).context("failed to load state file")?;

    // Look up the review.
    let review = store
        .get(&pr_ref)
        .ok_or_else(|| anyhow::anyhow!("no review found for PR #{pr_num}"))?;

    // Filter to local-only comments.
    let local_comments: Vec<&Comment> = review
        .comments
        .iter()
        .filter(|c| c.origin == CommentOrigin::Local)
        .collect();

    if local_comments.is_empty() {
        println!("No local comments to mirror for PR #{pr_num}.");
        return Ok(());
    }

    println!(
        "{} local comment(s) to mirror for PR #{pr_num}:",
        local_comments.len()
    );
    for comment in &local_comments {
        let formatted = github::format_comment_body(comment);
        println!();
        println!("  --- Comment {} ---", comment.id);
        for line in formatted.lines() {
            println!("  {line}");
        }
    }

    if dry_run {
        println!();
        println!("(dry run -- nothing posted)");
        return Ok(());
    }

    println!();
    println!("Posting to GitHub...");

    let result = github::mirror_comments(&pr_ref, &review.comments)
        .await
        .context("failed to mirror comments")?;

    println!("Posted:  {}", result.posted);
    if !result.failed.is_empty() {
        println!("Failed:  {}", result.failed.len());
        for (comment_id, reason) in &result.failed {
            println!("  {comment_id}: {reason}");
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// `cockpit restack <pr>`
// ---------------------------------------------------------------------------

/// Restack a stale PR onto its parent's new head, spawning the
/// conflict-resolver agent if the rebase has conflicts.
async fn run_restack(pr_num: u64) -> Result<()> {
    let pr_ref = PrRef::new(format!("owner/repo#{pr_num}"));

    // Load state from file.
    let state_path = Path::new(store::STATE_FILE);
    let store = store::load_from_file(state_path).context("failed to load state file")?;

    // Look up the review.
    let mut review = store
        .get(&pr_ref)
        .ok_or_else(|| anyhow::anyhow!("no review found for PR #{pr_num}"))?;

    if !review.stale {
        bail!("PR #{pr_num} is not stale; nothing to restack");
    }

    // Determine parent branch from the review's `base` field.
    let parent_branch = review.base.clone();
    let worktree_path = review.worktree.clone();

    // Open the repo (current directory).
    let repo = git2::Repository::discover(".").context("not inside a git repository")?;

    let session_map = SessionMap::new();
    let hook_url = "http://127.0.0.1:19876/hook/stop";
    let config = SpawnConfig::default();

    let outcome = restack::restack_or_resolve(
        &repo,
        &mut review,
        &parent_branch,
        &worktree_path,
        &session_map,
        hook_url,
        &config,
    )
    .await
    .context("restack failed")?;

    match outcome {
        restack::Outcome::Clean => {
            println!("PR #{pr_num} restacked cleanly onto {parent_branch}");
            println!("Stale flag cleared");
        }
        restack::Outcome::ConflictDispatched => {
            let agent = review.agent.as_ref().expect(
                // INVARIANT: ConflictDispatched always sets review.agent.
                "agent must be set after ConflictDispatched",
            );
            println!("PR #{pr_num} has conflicts restacking onto {parent_branch}");
            println!("Conflict-resolver agent spawned (PID: {})", agent.pid);
        }
    }

    // Persist the updated review.
    store.update(&pr_ref, |r| {
        r.base_sha = review.base_sha.clone();
        r.stale = review.stale;
        r.agent = review.agent.clone();
    });
    store::save_to_file(&store, state_path).context("failed to save state file")?;

    Ok(())
}

// ---------------------------------------------------------------------------
// `cockpit kickoff <project-id>`
// ---------------------------------------------------------------------------

/// Kick off a Linear project: fetch issues, optionally run the plan gate,
/// then spawn implementer agents for each frontier issue.
async fn run_kickoff(project_id: &str, skip_plan: bool, port: u16) -> Result<()> {
    let api_key = env::var("LINEAR_API_KEY").context(
        "LINEAR_API_KEY env var is required.\n\
         Set it to your Linear personal API key:\n  \
         export LINEAR_API_KEY=lin_api_...",
    )?;

    let project = ProjectRef::new(project_id);
    let client = reqwest::Client::new();

    // 1. Fetch issues and compute the frontier.
    println!("Fetching issues from Linear...");
    let (data, frontier) =
        cockpit_core::kickoff::fetch_and_compute_frontier(&client, &api_key, &project)
            .await
            .context("failed to fetch project issues from Linear")?;

    println!("Project: {project}");
    println!("Issues:  {}", data.issues.len());
    println!("Frontier: {} issues ready", frontier.len());

    if frontier.is_empty() {
        bail!("no frontier issues found — all issues have unmet dependencies");
    }

    for issue in &frontier {
        print_issue_detail(issue, &data);
    }

    // 2. Build the DAG for parent/child wiring.
    let issue_dag = linear::build_issue_dag(&data);

    // 3. Handle the plan gate.
    if skip_plan {
        println!();
        println!("Plan gate: skipped (--skip-plan)");
    } else {
        println!();
        println!("Plan gate: creating project plan...");

        // Create a plan in Pending state. The user must approve it
        // via `cockpit plan approve` before the batch is built
        // (Invariant 5: side effects require explicit confirmation).
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

        let plan_store = cockpit_core::store::PlanStore::new();
        plan_store.set(plan);

        let plan_path = Path::new(cockpit_core::store::PLAN_STATE_FILE);
        cockpit_core::store::save_plan_to_file(&plan_store, plan_path)
            .context("failed to save plan state")?;

        println!("Plan created in Pending state.");
        println!("Review the plan with: cockpit plan show");
        println!("Approve with:         cockpit plan approve");
        println!();
        println!("After plan approval, re-run: cockpit kickoff {project_id} --skip-plan");
        return Ok(());
    }

    // 4. Build reviews for frontier issues.
    let worktree_base = PathBuf::from(".cockpit/worktrees");
    let reviews = cockpit_core::kickoff::build_reviews_for_frontier(
        &frontier,
        &data,
        &issue_dag,
        &worktree_base,
        "main",
    );

    println!();
    println!("Reviews to create: {}", reviews.len());
    for review in &reviews {
        let parent_info = if review.parents.is_empty() {
            String::from("base: main")
        } else {
            format!("base: {} (stacked)", review.base)
        };
        println!("  {} -> {} [{}]", review.issue, review.branch, parent_info);
    }

    // 5. Save reviews to the store.
    let state_path = Path::new(cockpit_core::store::STATE_FILE);
    let store =
        cockpit_core::store::load_from_file(state_path).context("failed to load state file")?;

    for review in &reviews {
        store.insert(review.clone());
    }

    cockpit_core::store::save_to_file(&store, state_path).context("failed to save state file")?;

    println!();
    println!("State saved to: {}", state_path.display());

    // 6. Create worktrees and spawn agents.
    let repo = git2::Repository::discover(".").context("not inside a git repository")?;
    let session_map = cockpit_core::adapters::agent::SessionMap::new();
    let spawn_config = cockpit_core::adapters::agent::SpawnConfig::default();
    let hook_url = format!("http://127.0.0.1:{port}/hook/stop");

    let kickoff_config = cockpit_core::kickoff::KickoffConfig {
        http_client: &client,
        api_key: &api_key,
        worktree_base: &worktree_base,
        repo: &repo,
        session_map: &session_map,
        hook_url: &hook_url,
        spawn_config: &spawn_config,
    };

    println!();
    println!("Creating worktrees and spawning implementer agents...");

    let mut mutable_reviews = reviews;
    cockpit_core::kickoff::spawn_batch(&mut mutable_reviews, &kickoff_config, &project)
        .await
        .context("failed to spawn batch")?;

    // 7. Update store with agent runs.
    for review in &mutable_reviews {
        store.update(&review.pr, |r| {
            r.base_sha = review.base_sha.clone();
            r.agent = review.agent.clone();
        });
    }

    cockpit_core::store::save_to_file(&store, state_path)
        .context("failed to save state file after spawning agents")?;

    println!();
    println!("Batch kickoff complete!");
    println!("Reviews created: {}", mutable_reviews.len());
    for review in &mutable_reviews {
        if let Some(agent) = &review.agent {
            println!(
                "  {} -> PID {} ({})",
                review.issue, agent.pid, review.branch
            );
        }
    }

    println!();
    println!("Hook server: http://127.0.0.1:{port}/hook/stop");
    println!("Start the hook server with: cockpit start --port {port}");

    Ok(())
}

// ---------------------------------------------------------------------------
// `cockpit batch-approve [--dry-run] [--confirm]`
// ---------------------------------------------------------------------------

/// Evaluate frontier reviews for batch approval and optionally approve them.
///
/// By default (or with `--dry-run`) this prints a table of verdicts without
/// side effects. The `--confirm` flag is an explicit user action that
/// approves all eligible reviews (Invariant 5: side effects require explicit
/// confirmation).
async fn run_batch_approve(dry_run: bool, confirm: bool) -> Result<()> {
    if dry_run && confirm {
        bail!("--dry-run and --confirm are mutually exclusive");
    }

    // Load state from file.
    let state_path = Path::new(store::STATE_FILE);
    let store = store::load_from_file(state_path).context("failed to load state file")?;

    let config = batch::Config::default();
    let results = batch::evaluate_frontier(&store, &config);

    if results.is_empty() {
        println!("No reviews in the frontier to evaluate.");
        return Ok(());
    }

    // Print the verdict table.
    println!("{:<30} {:<12} {:<10} Reasons", "PR", "State", "Verdict");
    println!("{}", "-".repeat(80));

    let mut eligible_count: usize = 0;
    let mut ineligible_count: usize = 0;

    for (review, verdict) in &results {
        let (verdict_label, reasons) = match verdict {
            batch::Verdict::Eligible { reasons } => {
                eligible_count += 1;
                ("ELIGIBLE", reasons.join("; "))
            }
            batch::Verdict::Ineligible { reasons } => {
                ineligible_count += 1;
                ("INELIGIBLE", reasons.join("; "))
            }
        };

        println!(
            "{:<30} {:<12} {:<10} {}",
            review.pr,
            format!("{:?}", review.gate_state),
            verdict_label,
            reasons
        );
    }

    println!();
    println!(
        "Total: {} reviewed, {} eligible, {} ineligible",
        results.len(),
        eligible_count,
        ineligible_count
    );

    // If --confirm, approve all eligible reviews.
    if confirm {
        if eligible_count == 0 {
            println!("No eligible reviews to approve.");
            return Ok(());
        }

        println!();
        println!("Approving {eligible_count} eligible review(s)...");

        let mut approved = 0usize;
        for (review, verdict) in &results {
            if !verdict.is_eligible() {
                continue;
            }

            // Transition through open() if Reworked, then approve().
            let mut transition_err: Option<String> = None;
            store.update(&review.pr, |r| {
                // If Reworked, open first to get to InReview.
                if r.gate_state == GateState::Reworked {
                    if let Err(e) = r.open() {
                        transition_err = Some(format!("open failed for {}: {e}", review.pr));
                        return;
                    }
                }
                if let Err(e) = r.approve() {
                    transition_err = Some(format!("approve failed for {}: {e}", review.pr));
                    return;
                }
                approved += 1;
            });

            if let Some(err_msg) = transition_err {
                eprintln!("  Warning: {err_msg}");
            } else {
                println!("  Approved: {}", review.pr);
            }
        }

        // Persist the updated state.
        store::save_to_file(&store, state_path).context("failed to save state file")?;

        println!();
        println!("Approved {approved}/{eligible_count} reviews.");
    } else {
        println!();
        println!("Dry run -- no changes made. Pass --confirm to approve eligible reviews.");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use cockpit_core::gate::Gated;
    use cockpit_core::model::{
        Anchor, Comment, CommentId, CommentOrigin, DiffData, GateState, IssueRef, PlanDoc,
        PlanStep, PrRef, ProjectPlan, ProjectRef, Review, ReviewId, ReviewSource,
    };
    use cockpit_core::store::{PlanStore, ReviewStore};

    /// Build a minimal `Review` at the given state.
    fn make_review(pr_num: u64, state: GateState) -> Review {
        Review {
            id: ReviewId::new(format!("r-{pr_num}")),
            issue: IssueRef::new(format!("ISSUE-{pr_num}")),
            pr: PrRef::new(format!("owner/repo#{pr_num}")),
            branch: format!("alejandro/test-{pr_num}"),
            base: "main".into(),
            base_sha: "000".into(),
            source: ReviewSource::Frontier,
            worktree: PathBuf::from(format!("/tmp/wt-{pr_num}")),
            gate_state: state,
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

    fn make_comment(id: &str) -> Comment {
        Comment {
            id: CommentId::new(id),
            anchor: Anchor::DiffLine {
                path: PathBuf::from("src/main.rs"),
                range: (10, 15),
            },
            body: "fix this".into(),
            origin: CommentOrigin::Local,
        }
    }

    // -- parse_line_range tests --

    #[test]
    fn parse_line_range_single() {
        let (start, end) = parse_line_range("42").unwrap();
        assert_eq!((start, end), (42, 42));
    }

    #[test]
    fn parse_line_range_pair() {
        let (start, end) = parse_line_range("10-15").unwrap();
        assert_eq!((start, end), (10, 15));
    }

    #[test]
    fn parse_line_range_invalid() {
        assert!(parse_line_range("abc").is_err());
    }

    #[test]
    fn parse_line_range_reversed() {
        assert!(
            parse_line_range("15-10").is_err(),
            "reversed range should fail"
        );
    }

    // -- comment add flow --

    #[test]
    fn add_comment_stores_in_review() {
        let store = ReviewStore::new();
        let review = make_review(1, GateState::InReview);
        let pr = review.pr.clone();
        store.insert(review);

        // Simulate adding a comment.
        let comment = make_comment("c-1");
        store.update(&pr, |r| {
            r.comments.push(comment);
        });

        let got = store.get(&pr).unwrap();
        assert_eq!(got.comments.len(), 1);
        assert_eq!(got.comments[0].body, "fix this");
    }

    #[test]
    fn add_comment_transitions_pending_to_in_review() {
        let store = ReviewStore::new();
        let review = make_review(2, GateState::Pending);
        let pr = review.pr.clone();
        store.insert(review);

        // Simulate adding a comment with auto-transition.
        let comment = make_comment("c-2");
        store.update(&pr, |r| {
            r.comments.push(comment);
            if r.gate_state == GateState::Pending {
                r.gate_state = GateState::InReview;
            }
        });

        let got = store.get(&pr).unwrap();
        assert_eq!(got.gate_state, GateState::InReview);
        assert_eq!(got.comments.len(), 1);
    }

    // -- request-changes flow --

    #[test]
    fn request_changes_transitions_to_dispatched() {
        let mut review = make_review(3, GateState::InReview);
        review.comments.push(make_comment("c-3"));

        review.request_changes().unwrap();
        assert_eq!(review.gate_state, GateState::Dispatched);
    }

    #[test]
    fn request_changes_no_comments_fails() {
        let mut review = make_review(4, GateState::InReview);

        let err = review.request_changes().unwrap_err();
        assert!(
            matches!(err, cockpit_core::gate::Error::NoComments),
            "expected NoComments, got {err:?}"
        );
    }

    // ---------------------------------------------------------------
    // Plan gate tests
    // ---------------------------------------------------------------

    fn make_plan_doc() -> PlanDoc {
        PlanDoc {
            summary: "Build a thing".into(),
            steps: vec![
                PlanStep {
                    index: 0,
                    title: "Step one".into(),
                    description: "Do something".into(),
                },
                PlanStep {
                    index: 1,
                    title: "Step two".into(),
                    description: "Do more".into(),
                },
            ],
            files: vec![
                PathBuf::from("src/lib.rs"),
                PathBuf::from("src/main.rs"),
            ],
            risks: vec!["migration needed".into()],
            raw: "# Plan: Build a thing\n\n## Steps\n\n1. Step one\n   Do something\n\n2. Step two\n   Do more\n\n## Files\n\n- src/lib.rs\n- src/main.rs\n\n## Risks\n\n- migration needed\n".into(),
        }
    }

    fn make_project_plan(state: GateState) -> ProjectPlan {
        ProjectPlan {
            project: ProjectRef::new("proj-1"),
            doc: make_plan_doc(),
            gate_state: state,
            comments: vec![],
            agent: None,
        }
    }

    fn make_plan_comment(id: &str, anchor: Anchor) -> Comment {
        Comment {
            id: CommentId::new(id),
            anchor,
            body: "fix this step".into(),
            origin: CommentOrigin::Local,
        }
    }

    #[test]
    fn plan_gate_full_cycle() {
        let mut plan = make_project_plan(GateState::Pending);

        // Pending -> InReview
        plan.open().unwrap();
        assert_eq!(plan.gate_state(), GateState::InReview);

        // Add a comment
        plan.comments
            .push(make_plan_comment("pc-1", Anchor::PlanStep(0)));
        assert_eq!(plan.comments().len(), 1);

        // InReview -> Dispatched
        plan.request_changes().unwrap();
        assert_eq!(plan.gate_state(), GateState::Dispatched);

        // Dispatched -> Reworked (comments cleared per Invariant 4)
        plan.mark_reworked().unwrap();
        assert_eq!(plan.gate_state(), GateState::Reworked);
        assert!(plan.comments().is_empty(), "comments cleared on Reworked");

        // Reworked -> InReview
        plan.open().unwrap();
        assert_eq!(plan.gate_state(), GateState::InReview);

        // InReview -> Approved
        plan.approve().unwrap();
        assert_eq!(plan.gate_state(), GateState::Approved);
    }

    #[test]
    fn plan_comment_anchors() {
        let plan = make_project_plan(GateState::InReview);

        let step_comment = make_plan_comment("pc-1", Anchor::PlanStep(0));
        let file_comment = make_plan_comment("pc-2", Anchor::PlanFile(PathBuf::from("src/lib.rs")));

        // Verify anchors render correctly.
        let step_anchor = prompt::render_anchor(&step_comment.anchor, Some(&plan.doc));
        assert_eq!(step_anchor, "plan step 0: Step one");

        let file_anchor = prompt::render_anchor(&file_comment.anchor, Some(&plan.doc));
        assert_eq!(file_anchor, "plan file: src/lib.rs");
    }

    #[test]
    fn plan_prompt_assembly() {
        let mut plan = make_project_plan(GateState::InReview);
        plan.comments
            .push(make_plan_comment("pc-1", Anchor::PlanStep(0)));
        plan.comments.push(Comment {
            id: CommentId::new("pc-2"),
            anchor: Anchor::PlanFile(PathBuf::from("src/lib.rs")),
            body: "consider splitting".into(),
            origin: CommentOrigin::Local,
        });

        let artifact = Artifact::Plan(plan.doc.clone());
        let input = ReworkInput {
            intent: "Build a reusable widget framework",
            approved_plan: None, // plan gate: no approved plan
            artifact: &artifact,
            comments: &plan.comments,
            skills: &[],
        };

        let result = prompt::assemble_rework(&input);

        // No "Approved Plan" section in plan-gate prompts.
        assert!(
            !result.text.contains("## Approved Plan"),
            "plan-gate prompt should not have an Approved Plan section"
        );

        // Artifact section should contain the plan raw text.
        assert!(
            result.text.contains("## Current Artifact"),
            "prompt should have Current Artifact section"
        );
        assert!(
            result.text.contains(&plan.doc.raw),
            "prompt should contain the plan raw text"
        );

        // Comments section should contain rendered anchors.
        assert!(
            result.text.contains("plan step 0: Step one"),
            "prompt should contain rendered step anchor"
        );
        assert!(
            result.text.contains("plan file: src/lib.rs"),
            "prompt should contain rendered file anchor"
        );

        // Should have a scope guard.
        assert!(
            result.text.contains("## Scope Guard"),
            "prompt should have Scope Guard section"
        );

        // Hash should be valid.
        assert_eq!(result.hash.len(), 64);
    }

    #[test]
    fn plan_approve_from_in_review() {
        let mut plan = make_project_plan(GateState::InReview);

        // Can approve directly from InReview (no comments needed for approval).
        plan.approve().unwrap();
        assert_eq!(plan.gate_state(), GateState::Approved);
    }

    #[test]
    fn plan_store_round_trip_with_comments() {
        let plan_store = PlanStore::new();
        let mut plan = make_project_plan(GateState::InReview);
        plan.comments
            .push(make_plan_comment("pc-1", Anchor::PlanStep(0)));
        plan_store.set(plan);

        let dir = tempfile::tempdir().expect("should create temp dir");
        let path = dir.path().join("plan.json");

        store::save_plan_to_file(&plan_store, &path).expect("save should succeed");
        let loaded = store::load_plan_from_file(&path).expect("load should succeed");

        let got = loaded.get().expect("plan should be present");
        assert_eq!(got.gate_state, GateState::InReview);
        assert_eq!(got.comments.len(), 1);
        assert_eq!(got.doc.steps.len(), 2);
    }
}
