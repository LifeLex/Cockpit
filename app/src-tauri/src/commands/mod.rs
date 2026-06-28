//! Thin `#[tauri::command]` handlers that delegate to `cockpit-core`.
//!
//! Commands parse params, call core, and map results through
//! [`CommandError`](crate::error::CommandError). All logic lives in core.

use std::sync::Arc;

use tauri::State;

use crate::error::CommandError;
use crate::state::AppState;

/// Return the cockpit-core crate version — trivial command to prove IPC round-trip.
#[tauri::command]
pub fn get_version() -> String {
    cockpit_core::VERSION.to_string()
}

/// List all reviews currently in the store.
#[tauri::command]
pub fn list_reviews(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<cockpit_core::model::Review>, CommandError> {
    Ok(state.reviews.list())
}
