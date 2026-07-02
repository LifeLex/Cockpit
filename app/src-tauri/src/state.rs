//! Shared application state, held behind `Arc` for thread safety.
//!
//! Background tasks (hook server, agent runs) access this from spawned tasks.

use std::collections::HashMap;
use std::sync::Mutex;

use tokio::sync::broadcast;

use cockpit_core::adapters::agent::SessionMap;
use cockpit_core::adapters::lsp::LspBridge;
use cockpit_core::config::LspLanguage;
use cockpit_core::hook_server::{CompletionEvent, PermissionBroker};
use cockpit_core::store::{ProjectStore, ReviewStore};

/// Holds core handles shared across the Tauri app.
///
/// Wrapped in `Arc` at registration time so background tasks (hook server,
/// agent runs) can access it from spawned tasks without lifetime issues.
pub struct AppState {
    /// In-memory store of active reviews.
    pub reviews: ReviewStore,
    /// In-memory store of first-class projects that group reviews.
    ///
    /// Each project owns its optional plan; the plan gate operates per project.
    pub projects: ProjectStore,
    /// Maps agent session IDs to their reviewed objects.
    ///
    /// Shared with the hook server to look up which review an agent
    /// completion callback belongs to.
    pub sessions: SessionMap,
    /// Sender side of the completion broadcast channel.
    ///
    /// The hook server sends events here; the Tauri setup listener forwards
    /// them to the frontend via Tauri events.
    pub completion_tx: broadcast::Sender<CompletionEvent>,

    /// Routes headless-agent tool-permission requests to a human decision.
    ///
    /// Created once at startup and shared (cloned) with the hook server's MCP
    /// `approve` endpoint. Commands resolve pending requests and enumerate the
    /// queue through this handle; the setup hook subscribes to it to forward
    /// requests to the frontend. Cloneable — all clones share one registry.
    pub permission_broker: PermissionBroker,

    /// Running Monaco LSP bridges, one per language, started lazily.
    ///
    /// Held here so their lifetime is tied to the app: dropping `AppState`
    /// drops each [`LspBridge`], aborting its serve task and killing any
    /// spawned language-server child (no orphan pids). The `std::sync::Mutex`
    /// is only ever held for trivial map lookups/inserts — never across an
    /// `.await`.
    pub lsp_bridges: Mutex<HashMap<LspLanguage, LspBridge>>,
}

impl AppState {
    /// Create a new `AppState` with the given completion channel sender and
    /// permission broker.
    pub fn new_with_completion_tx(
        completion_tx: broadcast::Sender<CompletionEvent>,
        permission_broker: PermissionBroker,
    ) -> Self {
        Self {
            reviews: ReviewStore::new(),
            projects: ProjectStore::new(),
            sessions: SessionMap::new(),
            completion_tx,
            permission_broker,
            lsp_bridges: Mutex::new(HashMap::new()),
        }
    }
}
