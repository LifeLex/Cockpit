//! cockpit-core — headless domain model, the `Gated` review loop, and adapters.
//!
//! See `SPEC.md` (what) and `CLAUDE.md` (how). This crate has NO UI dependencies:
//! no `tauri`, no DOM, no framework. It must be fully exercisable headlessly:
//! the integration tests drive the real loop against local git and the hook
//! server. If a feature can't be exercised that way, it does not belong here yet.

#![forbid(unsafe_code)]

/// Crate version, surfaced for diagnostics.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod config;
pub mod model;

pub mod adapters;
pub mod dag;
pub mod gate;
pub mod kickoff;
pub mod persist;
pub mod plan_parser;
pub mod prompt;
pub mod restack;
pub mod skills;
pub mod store;
pub mod workflow;

pub mod hook_server;

#[cfg(test)]
mod tests {
    #[test]
    fn smoke() {
        assert!(!super::VERSION.is_empty());
    }
}
