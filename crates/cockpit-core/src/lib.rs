//! cockpit-core — headless domain model, the `Gated` review loop, and adapters.
//!
//! See `SPEC.md` (what) and `CLAUDE.md` (how). This crate has NO UI dependencies:
//! no `tauri`, no DOM, no framework. If a feature can't be driven from
//! `cockpit-cli`, it does not belong here yet.

#![forbid(unsafe_code)]

/// Crate version, surfaced for diagnostics.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

// Modules land here as the plan progresses:
//   pub mod model;       // T0.2 — domain types + newtypes
//   pub mod gate;        // T0.3 — the Gated trait + state machine
//   pub mod adapters;    // T0.4–T0.6 — linear, github, git, agent
//   pub mod prompt;      // T1.1 — deterministic prompt assembly
//   pub mod hook_server; // T1.3 — axum Stop-hook listener

#[cfg(test)]
mod tests {
    #[test]
    fn smoke() {
        assert!(!super::VERSION.is_empty());
    }
}
