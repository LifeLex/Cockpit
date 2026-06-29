//! Serializable command error for the IPC boundary.
//!
//! Converts from cockpit-core's typed `thiserror` enums into a flat
//! serializable format. Don't leak `anyhow` across IPC.

use serde::Serialize;

/// Command error that can cross the IPC boundary.
///
/// Each core error type gets a `From` impl so `?` works naturally
/// in command handlers.
#[derive(Debug, Serialize)]
pub struct CommandError {
    /// Human-readable error message.
    pub message: String,
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl From<cockpit_core::gate::Error> for CommandError {
    fn from(e: cockpit_core::gate::Error) -> Self {
        Self {
            message: e.to_string(),
        }
    }
}

impl From<cockpit_core::store::Error> for CommandError {
    fn from(e: cockpit_core::store::Error) -> Self {
        Self {
            message: e.to_string(),
        }
    }
}

impl From<cockpit_core::adapters::agent::Error> for CommandError {
    fn from(e: cockpit_core::adapters::agent::Error) -> Self {
        Self {
            message: e.to_string(),
        }
    }
}

impl From<cockpit_core::plan_parser::Error> for CommandError {
    fn from(e: cockpit_core::plan_parser::Error) -> Self {
        Self {
            message: e.to_string(),
        }
    }
}

impl From<cockpit_core::adapters::github::Error> for CommandError {
    fn from(e: cockpit_core::adapters::github::Error) -> Self {
        Self {
            message: e.to_string(),
        }
    }
}

impl From<std::io::Error> for CommandError {
    fn from(e: std::io::Error) -> Self {
        Self {
            message: e.to_string(),
        }
    }
}

impl From<cockpit_core::config::Error> for CommandError {
    fn from(e: cockpit_core::config::Error) -> Self {
        Self {
            message: e.to_string(),
        }
    }
}

impl From<cockpit_core::kickoff::Error> for CommandError {
    fn from(e: cockpit_core::kickoff::Error) -> Self {
        Self {
            message: e.to_string(),
        }
    }
}

impl From<cockpit_core::restack::Error> for CommandError {
    fn from(e: cockpit_core::restack::Error) -> Self {
        Self {
            message: e.to_string(),
        }
    }
}
