//! cockpit — thin CLI over `cockpit-core`; the validation surface for phases 0–2.
//!
//! Subcommands arrive with the plan: `project`/`ingest` (T0.7), `comment`/
//! `request-changes` (T1.4), `kickoff` (T2.3).

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    println!(
        "cockpit {} — scaffold ready. Next task: T0.2 (see IMPLEMENTATION_PLAN.md).",
        cockpit_core::VERSION
    );
    Ok(())
}
