//! Tauri 2 shell — builder setup, state registration, and handler wiring.
//!
//! This is the entry point for the desktop app. It wires `cockpit-core`'s
//! domain into Tauri's command/event system behind `Arc<AppState>`.

mod commands;
mod error;
mod state;

use std::sync::Arc;

use tauri::Emitter;

use state::AppState;

/// Build and run the Tauri application.
///
/// Registers `AppState` (behind `Arc` for background-task access) and
/// all IPC command handlers via a single `generate_handler!` call.
/// Spawns a background task to forward hook-server `CompletionEvent`s
/// as Tauri events so the frontend can update live.
pub fn run() {
    let (completion_tx, completion_rx) = cockpit_core::hook_server::completion_channel();

    let app_state = Arc::new(AppState::new_with_completion_tx(completion_tx));

    tauri::Builder::default()
        .manage(app_state)
        .setup(|app| {
            let app_handle = app.handle().clone();
            let mut rx = completion_rx;

            // Forward CompletionEvents from the hook server to the frontend
            // via Tauri's event system. The frontend listens for
            // "agent-completed" and refreshes its state.
            tokio::spawn(async move {
                while let Ok(event) = rx.recv().await {
                    // Best-effort: if no frontend window is listening, the
                    // event is simply dropped.
                    let _ = app_handle.emit("agent-completed", &event);
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_version,
            commands::list_reviews,
            commands::get_frontier,
            commands::get_review,
            commands::open_review,
        ])
        .run(tauri::generate_context!())
        // INVARIANT: if Tauri fails to start there is nothing to recover —
        // the app cannot function without the event loop.
        .expect("error running tauri application");
}
