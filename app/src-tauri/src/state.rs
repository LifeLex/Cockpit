//! Shared application state, held behind `Arc` for thread safety.
//!
//! Background tasks (hook server, agent runs) access this from spawned tasks.

use tokio::sync::broadcast;

use cockpit_core::adapters::agent::SessionMap;
use cockpit_core::hook_server::CompletionEvent;
use cockpit_core::store::{PlanStore, ReviewStore};

/// Holds core handles shared across the Tauri app.
///
/// Wrapped in `Arc` at registration time so background tasks (hook server,
/// agent runs) can access it from spawned tasks without lifetime issues.
pub struct AppState {
    /// In-memory store of active reviews.
    pub reviews: ReviewStore,
    /// In-memory store for the optional project plan.
    pub plan: PlanStore,
    /// Maps agent session IDs to their reviewed objects.
    ///
    /// Used by the hook server and agent dispatch in later phases.
    #[allow(dead_code)]
    pub sessions: SessionMap,
    /// Sender side of the completion broadcast channel.
    ///
    /// The hook server sends events here; the Tauri setup listener forwards
    /// them to the frontend via Tauri events.
    #[allow(dead_code)]
    pub completion_tx: broadcast::Sender<CompletionEvent>,
}

impl AppState {
    /// Create a new `AppState` with the given completion channel sender.
    pub fn new_with_completion_tx(completion_tx: broadcast::Sender<CompletionEvent>) -> Self {
        Self {
            reviews: ReviewStore::new(),
            plan: PlanStore::new(),
            sessions: SessionMap::new(),
            completion_tx,
        }
    }
}
