//! External-system adapters: git, GitHub, Linear, agent.
//!
//! Each adapter has its own typed error enum. Adapters are stateless
//! functions; the caller (the loop, the CLI) owns the `Repository` handle.

pub mod git;
pub mod github;
