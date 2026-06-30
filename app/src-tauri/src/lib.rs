//! Tauri 2 shell -- builder setup, state registration, and handler wiring.
//!
//! This is the entry point for the desktop app. It wires `cockpit-core`'s
//! domain into Tauri's command/event system behind `Arc<AppState>`.

mod commands;
mod error;
mod state;
mod streaming;

use std::sync::Arc;

use tauri::{Emitter, Manager};

use cockpit_core::gate::Gated;
use cockpit_core::model::{AgentMode, PrRef};

use state::AppState;

/// Set the macOS dock icon from an embedded PNG.
///
/// During `cargo tauri dev` there is no `.app` bundle, so macOS shows a
/// default blank icon. Tauri 2 has no built-in API for this (see
/// tauri-apps/tauri#2985). The standard workaround is to call
/// `NSApplication.setApplicationIconImage` at startup.
#[cfg(target_os = "macos")]
fn set_macos_dock_icon() {
    use objc2::AnyThread;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    let icon_bytes = include_bytes!("../icons/128x128@2x.png");

    // SAFETY: the Tauri setup hook runs on the main thread.
    let mtm = unsafe { MainThreadMarker::new_unchecked() };

    let data = NSData::with_bytes(icon_bytes);
    if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
        let app = NSApplication::sharedApplication(mtm);
        // SAFETY: image is a valid NSImage from well-formed PNG data.
        unsafe {
            app.setApplicationIconImage(Some(&image));
        }
    }
}

/// Build and run the Tauri application.
///
/// Registers `AppState` (behind `Arc` for background-task access) and
/// all IPC command handlers via a single `generate_handler!` call.
/// Spawns a background task to forward hook-server `CompletionEvent`s
/// as Tauri events so the frontend can update live.
pub fn run() {
    let (completion_tx, completion_rx) = cockpit_core::hook_server::completion_channel();

    let app_state = Arc::new(AppState::new_with_completion_tx(completion_tx));
    let shell_sessions = commands::shell::ShellSessions::default();

    // Clone handles needed by the setup hook before app_state is moved
    // into `.manage()` (which takes ownership of the Arc).
    let hook_sessions = app_state.sessions.clone();
    let hook_completion_tx = app_state.completion_tx.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .manage(app_state)
        .manage(shell_sessions)
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            set_macos_dock_icon();

            let app_handle = app.handle().clone();
            let mut rx = completion_rx;

            // Reconcile completed agent sessions and forward
            // CompletionEvents to the frontend via Tauri's event system.
            // The frontend listens for "agent-completed" to refresh state.
            tauri::async_runtime::spawn(async move {
                while let Ok(event) = rx.recv().await {
                    let app_state_ref: tauri::State<'_, Arc<AppState>> = app_handle.state();

                    match event.mode {
                        AgentMode::Fix | AgentMode::Restack => {
                            // Look up the review by PrRef (stored as
                            // object_id), clear the agent run, transition
                            // to Reworked, and re-fetch the diff.
                            let pr_ref = PrRef::new(&event.object_id);
                            app_state_ref.reviews.update(&pr_ref, |review| {
                                review.agent = None;
                                let _ = review.mark_reworked();
                            });

                            // Best-effort: re-fetch the diff so users
                            // review fresh code, not stale diffs.
                            refresh_review_diff(&app_state_ref, &pr_ref).await;
                        }
                        AgentMode::Plan => {
                            // Clear the plan's agent run and transition
                            // to Reworked so the user can re-review.
                            app_state_ref.plan.update(|plan| {
                                plan.agent = None;
                                let _ = plan.mark_reworked();
                            });
                        }
                        AgentMode::Implement => {
                            // TODO: implementation agent completion
                            // reconciliation (Phase 2).
                        }
                    }

                    // Best-effort: if no frontend window is listening, the
                    // event is simply dropped.
                    let _ = app_handle.emit("agent-completed", &event);
                }
            });

            // Start the hook server for agent completion callbacks.
            {
                let hook_state = cockpit_core::hook_server::HookState {
                    session_map: hook_sessions,
                    completion_tx: hook_completion_tx,
                };
                let config = cockpit_core::config::Config::load().unwrap_or_default();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) =
                        cockpit_core::hook_server::serve(hook_state, config.hook_port).await
                    {
                        eprintln!("hook server error: {e}");
                    }
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_version,
            commands::list_reviews,
            commands::get_frontier,
            commands::get_review,
            commands::open_review,
            commands::get_review_diff,
            commands::add_comment,
            commands::request_changes,
            commands::mirror_comments,
            commands::get_plan,
            commands::load_plan,
            commands::add_plan_comment,
            commands::plan_request_changes,
            commands::plan_approve,
            commands::plan_open,
            commands::batch_approve_preview,
            commands::approve_review,
            commands::get_config,
            commands::save_config,
            commands::kickoff,
            commands::restack_pr,
            commands::load_plan_from_path,
            commands::fetch_authored_prs,
            commands::fetch_review_requests,
            commands::shell::spawn_shell,
            commands::shell::shell_write,
            commands::shell::shell_resize,
            commands::shell::shell_kill,
            commands::open_in_editor,
        ])
        .run(tauri::generate_context!())
        // INVARIANT: if Tauri fails to start there is nothing to recover --
        // the app cannot function without the event loop.
        .expect("error running tauri application");
}

/// Re-fetch the PR diff from GitHub after an agent completion.
///
/// Snapshots the `repo_slug` and PR number from the review, then calls
/// the GitHub adapter to fetch a fresh diff. Errors are silently ignored
/// since this is a best-effort background refresh.
async fn refresh_review_diff(state: &AppState, pr_ref: &PrRef) {
    use cockpit_core::adapters::github;
    use cockpit_core::model::DiffData;

    let (repo_slug, pr_number) = {
        let review = state.reviews.get(pr_ref);
        match review {
            Some(r) => {
                let number = extract_pr_number(r.pr.as_str());
                (r.repo_slug.clone(), number)
            }
            None => return,
        }
    };

    let Some(pr_number) = pr_number else {
        return;
    };

    let diff_result = match &repo_slug {
        Some(slug) => github::pr_diff_by_repo(slug, pr_number).await,
        None => github::pr_diff(pr_number).await,
    };

    if let Ok(raw_diff) = diff_result {
        state.reviews.update(pr_ref, |review| {
            review.diff = DiffData { raw: raw_diff };
        });
    }
}

/// Extract the PR number from a PR URL or reference string.
///
/// Handles formats like `https://github.com/owner/repo/pull/42` and
/// `owner/repo#42`. Returns `None` if no number can be parsed.
fn extract_pr_number(pr_str: &str) -> Option<u64> {
    // Try URL format: .../pull/42
    if let Some(tail) = pr_str.rsplit('/').next()
        && let Ok(n) = tail.parse::<u64>()
    {
        return Some(n);
    }

    // Try ref format: owner/repo#42
    if let Some(after_hash) = pr_str.rsplit('#').next()
        && let Ok(n) = after_hash.parse::<u64>()
    {
        return Some(n);
    }

    None
}
