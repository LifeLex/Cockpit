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
use cockpit_core::dag;
use cockpit_core::gate::Gated;
use cockpit_core::hook_server::{self, HookState};
use cockpit_core::model::{
    Anchor, Artifact, Comment, CommentId, CommentOrigin, DiffData, GateState, IssueRef, PrRef,
    ProjectRef, Review, ReviewId,
};
use cockpit_core::prompt::{self, ReworkInput};
use cockpit_core::store::{self, ReviewStore};

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
            worktree: PathBuf::from(format!(".cockpit/worktrees/{}", pr_data.number)),
            gate_state: GateState::Pending,
            diff: DiffData { raw: String::new() },
            head_sha: String::new(),
            comments: vec![],
            parents: vec![],
            children: vec![],
            stale: false,
            agent: None,
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
    };
    let prompt = prompt::assemble_rework(&input);

    println!("Prompt hash: {}", prompt.hash);

    // Spawn the fixer agent.
    let session_map = SessionMap::new();
    let hook_url = "http://127.0.0.1:19876/hook/stop";
    let config = SpawnConfig::default();

    let agent_run = cockpit_core::adapters::agent::spawn_agent(
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

    println!("Agent PID: {}", agent_run.pid);

    // Store the agent run on the review.
    review.agent = Some(agent_run);

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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use cockpit_core::gate::Gated;
    use cockpit_core::model::{
        Anchor, Comment, CommentId, CommentOrigin, DiffData, GateState, IssueRef, PrRef, Review,
        ReviewId,
    };
    use cockpit_core::store::ReviewStore;

    /// Build a minimal `Review` at the given state.
    fn make_review(pr_num: u64, state: GateState) -> Review {
        Review {
            id: ReviewId::new(format!("r-{pr_num}")),
            issue: IssueRef::new(format!("ISSUE-{pr_num}")),
            pr: PrRef::new(format!("owner/repo#{pr_num}")),
            branch: format!("alejandro/test-{pr_num}"),
            base: "main".into(),
            base_sha: "000".into(),
            worktree: PathBuf::from(format!("/tmp/wt-{pr_num}")),
            gate_state: state,
            diff: DiffData { raw: String::new() },
            head_sha: "abc123".into(),
            comments: vec![],
            parents: vec![],
            children: vec![],
            stale: false,
            agent: None,
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
}
