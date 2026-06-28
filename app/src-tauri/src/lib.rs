//! Tauri 2 shell — builder setup, state registration, and handler wiring.
//!
//! This is the entry point for the desktop app. It wires `cockpit-core`'s
//! domain into Tauri's command/event system behind `Arc<AppState>`.

mod commands;
mod error;
mod state;

use std::sync::Arc;

use state::AppState;

/// Build and run the Tauri application.
///
/// Registers `AppState` (behind `Arc` for background-task access) and
/// all IPC command handlers via a single `generate_handler!` call.
pub fn run() {
    let app_state = Arc::new(AppState::new());

    tauri::Builder::default()
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            commands::get_version,
            commands::list_reviews,
        ])
        .run(tauri::generate_context!())
        // INVARIANT: if Tauri fails to start there is nothing to recover —
        // the app cannot function without the event loop.
        .expect("error running tauri application");
}
