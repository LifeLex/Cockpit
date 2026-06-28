//! cockpit -- thin CLI over `cockpit-core`; the validation surface for phases 0-2.
//!
//! Subcommands arrive with the plan: `project`/`ingest` (T0.7), `comment`/
//! `request-changes` (T1.4), `kickoff` (T2.3).

use std::env;

use anyhow::{Context, Result};
use clap::Parser;

use cockpit_core::adapters::{github, linear};
use cockpit_core::dag;
use cockpit_core::model::{IssueRef, ProjectRef};

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
