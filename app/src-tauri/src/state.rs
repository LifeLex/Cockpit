//! Shared application state, held behind `Arc` for thread safety.
//!
//! Background tasks (hook server, agent runs) access this from spawned tasks.

use cockpit_core::adapters::agent::SessionMap;
use cockpit_core::store::ReviewStore;

/// Holds core handles shared across the Tauri app.
///
/// Wrapped in `Arc` at registration time so background tasks (hook server,
/// agent runs) can access it from spawned tasks without lifetime issues.
pub struct AppState {
    /// In-memory store of active reviews.
    pub reviews: ReviewStore,
    /// Maps agent session IDs to their reviewed objects.
    ///
    /// Used by the hook server and agent dispatch in later phases.
    #[allow(dead_code)]
    pub sessions: SessionMap,
}

impl AppState {
    /// Create a new `AppState` with empty stores.
    pub fn new() -> Self {
        Self {
            reviews: ReviewStore::new(),
            sessions: SessionMap::new(),
        }
    }
}
