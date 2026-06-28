//! cockpit-core — headless domain model, the `Gated` review loop, and adapters.
//!
//! See `SPEC.md` (what) and `CLAUDE.md` (how). This crate has NO UI dependencies:
//! no `tauri`, no DOM, no framework. If a feature can't be driven from
//! `cockpit-cli`, it does not belong here yet.

#![forbid(unsafe_code)]

/// Crate version, surfaced for diagnostics.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod model;

pub mod adapters;
pub mod batch;
pub mod dag;
pub mod gate;
pub mod kickoff;
pub mod plan_parser;
pub mod prompt;
pub mod restack;
pub mod store;

pub mod hook_server;

#[cfg(test)]
mod tests {
    #[test]
    fn smoke() {
        assert!(!super::VERSION.is_empty());
    }
}
