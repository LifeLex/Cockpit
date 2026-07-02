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

use cockpit_core::gate::{AgentOutcome, Gated};
use cockpit_core::model::{AgentMode, GateState, PrRef, Project, ProjectId, Review};

use state::AppState;

/// Payload emitted on the `"agent-completed"` Tauri event after a completion is
/// reconciled against git HEAD.
///
/// Extends the raw [`CompletionEvent`](cockpit_core::hook_server::CompletionEvent)
/// fields the frontend already listens for with a git-HEAD-authoritative
/// `outcome` label, so the UI can tell rework that actually landed a commit
/// (`"reworked"`) from a failed/no-op run (`"failed"`) or a non-gate-advancing
/// artifact fill (`"completed"`). Hand-typed on the frontend (no `ts-rs`).
#[derive(Debug, Clone, serde::Serialize)]
struct AgentCompletedPayload {
    /// The session id that completed.
    session_id: String,
    /// UI key of the reviewed object (PR ref for reviews, project id for plans).
    object_id: String,
    /// Which agent mode ran.
    mode: AgentMode,
    /// Outcome label: `"reworked"`, `"failed"`, or `"completed"`.
    outcome: &'static str,
}

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

    // Restore persisted session state (D5) before any observer runs, so the
    // flush task's baseline revision reflects the loaded data. A missing or
    // corrupt file yields `None` (persist::load never panics — Invariant 1), so
    // a fresh start is the normal first-launch path.
    if let Ok(home) = cockpit_core::config::cockpit_home()
        && let Some(persisted) = cockpit_core::persist::load(&home)
    {
        app_state
            .reviews
            .hydrate(sanitize_loaded_reviews(persisted.reviews));
        app_state
            .projects
            .hydrate(sanitize_loaded_projects(persisted.projects));
    }

    // Clone the (Arc-backed) store handles for the background flush task before
    // app_state is moved into `.manage()`.
    let flush_reviews = app_state.reviews.clone();
    let flush_projects = app_state.projects.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
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

                    let outcome: &'static str = match event.mode {
                        AgentMode::Fix | AgentMode::Restack => {
                            // Reconcile the review against git HEAD, which — not
                            // agent stdout — decides whether rework landed.
                            reconcile_fix_completion(&app_state_ref, &event.object_id).await
                        }
                        AgentMode::Plan => {
                            // Two planner spawns land here (both AgentMode::Plan):
                            //   * initial generation — the plan is still
                            //     `Pending`; leave it `Pending` (artifact-fill)
                            //     so the user opens it when ready ("completed").
                            //   * rework — the plan is `Dispatched`; ingest the
                            //     planner's output and settle the gate: parsed
                            //     output → `Reworked` (clears comments), missing
                            //     or unparseable output → `InReview` (comments
                            //     preserved, "failed").
                            // Read/parse failure is non-fatal (Invariant 1) — we
                            // log and keep the prior doc rather than block.
                            //
                            // The session object_id is the project id (set at
                            // spawn), so this routes to the right project's plan.
                            let project_id = cockpit_core::model::ProjectId::new(&event.object_id);
                            ingest_plan_output(&app_state_ref, &project_id)
                        }
                        AgentMode::Implement => {
                            // An implementer finished building a review's
                            // initial code in its worktree. Clear the agent
                            // run and re-fetch the diff so the review is ready
                            // for human review — but do NOT auto-advance the
                            // gate: the review stays `Pending` until a human
                            // opens it (Invariant 5).
                            //
                            // Fan-out (kickoff::spawn_batch) keys the session by
                            // ReviewId, so resolve the review by id and act on
                            // its PrRef.
                            if let Some(pr_ref) =
                                resolve_review_pr(&app_state_ref, &event.object_id)
                            {
                                app_state_ref.reviews.update(&pr_ref, |review| {
                                    review.agent = None;
                                });
                                refresh_review_diff(&app_state_ref, &pr_ref).await;
                            }
                            "completed"
                        }
                        AgentMode::Review => {
                            // Advisory pre-pass reviewer finished: ingest its
                            // findings file onto the review. This NEVER touches
                            // the gate state — the pre-pass is read-only
                            // (Invariant 5).
                            ingest_review_findings(&app_state_ref, &event.object_id)
                        }
                    };

                    // Best-effort: if no frontend window is listening, the
                    // event is simply dropped.
                    let payload = AgentCompletedPayload {
                        session_id: event.session_id.clone(),
                        object_id: event.object_id.clone(),
                        mode: event.mode,
                        outcome,
                    };
                    let _ = app_handle.emit("agent-completed", &payload);
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

            // Background persistence flush (D5). Once per ~second, snapshot the
            // store revisions; when either changed since the last save, persist
            // the whole session to disk. `save_atomic` is blocking file IO, so
            // it runs on the blocking pool — the async loop never blocks
            // (Invariant 1). Save failures are logged and retried on the next
            // tick, never fatal.
            tauri::async_runtime::spawn(async move {
                let mut last = flush_reviews
                    .revision()
                    .wrapping_add(flush_projects.revision());
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

                    let current = flush_reviews
                        .revision()
                        .wrapping_add(flush_projects.revision());
                    if current == last {
                        continue;
                    }

                    let snapshot = cockpit_core::persist::PersistedState {
                        version: cockpit_core::persist::STATE_VERSION,
                        reviews: flush_reviews.list(),
                        projects: flush_projects.list(),
                    };

                    // Persist off the async runtime — save_atomic is sync IO.
                    let saved =
                        tokio::task::spawn_blocking(
                            move || match cockpit_core::config::cockpit_home() {
                                Ok(home) => cockpit_core::persist::save_atomic(&home, &snapshot)
                                    .map_err(|e| e.to_string()),
                                Err(e) => Err(e.to_string()),
                            },
                        )
                        .await;

                    match saved {
                        // Only advance the baseline on a durable save, so a
                        // failed flush is retried rather than silently skipped.
                        Ok(Ok(())) => last = current,
                        Ok(Err(msg)) => eprintln!("persist flush: save failed: {msg}"),
                        Err(e) => eprintln!("persist flush: save task panicked: {e}"),
                    }
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_reviews,
            commands::get_frontier,
            commands::get_review,
            commands::open_review,
            commands::get_review_diff,
            commands::get_interdiff,
            commands::get_evidence,
            commands::get_file_pair,
            commands::pre_review,
            commands::add_comment,
            commands::request_changes,
            commands::mirror_comments,
            commands::submit_github_review,
            commands::kill_agent,
            commands::ensure_review_worktree,
            commands::fetch_ci_checks,
            commands::list_ci_checks,
            commands::ci_run_logs_by_link,
            commands::fix_ci,
            commands::get_plan,
            commands::add_plan_comment,
            commands::generate_plan,
            commands::plan_request_changes,
            commands::plan_approve,
            commands::plan_open,
            commands::batch_status,
            commands::approve_review,
            commands::merge_review,
            commands::get_config,
            commands::save_config,
            commands::get_agent_prompt,
            commands::get_builtin_agent_prompt,
            commands::save_agent_prompt,
            commands::list_skills,
            commands::save_skill,
            commands::delete_skill,
            commands::sync_skills,
            commands::kickoff,
            commands::list_projects,
            commands::create_project,
            commands::attach_review,
            commands::restack_pr,
            commands::fetch_authored_prs,
            commands::fetch_review_requests,
            commands::shell::spawn_shell,
            commands::shell::shell_write,
            commands::shell::shell_resize,
            commands::shell::shell_kill,
            commands::open_in_editor,
            commands::start_lsp_bridge,
        ])
        .run(tauri::generate_context!())
        // INVARIANT: if Tauri fails to start there is nothing to recover --
        // the app cannot function without the event loop.
        .expect("error running tauri application");
}

/// Sanitize reviews loaded from disk before hydrating the store (D5).
///
/// Two fix-ups make persisted state safe to resume:
///   * The process that owned each agent handle is dead after a restart, so
///     every `agent` handle is dropped.
///   * A review left `Dispatched` at shutdown had an in-flight agent that will
///     never report back, so it is returned to `InReview` via
///     `mark_agent_failed`, which preserves its comments (Invariant 4) so the
///     pending rework can be re-dispatched.
///
/// The `mark_agent_failed` call is guarded by the `Dispatched` check, so its
/// only failure mode is unreachable here; the `Result` is ignored deliberately.
fn sanitize_loaded_reviews(reviews: Vec<Review>) -> Vec<Review> {
    reviews
        .into_iter()
        .map(|mut review| {
            review.agent = None;
            if review.gate_state == GateState::Dispatched {
                let _ = review.mark_agent_failed();
            }
            review
        })
        .collect()
}

/// Sanitize projects loaded from disk before hydrating the store (D5).
///
/// Mirrors [`sanitize_loaded_reviews`] for each project's optional plan: drop
/// the dead planner `agent` handle, and return a `Dispatched` plan to
/// `InReview` (comments preserved) so a pending plan rework can be re-dispatched.
fn sanitize_loaded_projects(projects: Vec<Project>) -> Vec<Project> {
    projects
        .into_iter()
        .map(|mut project| {
            if let Some(plan) = project.plan.as_mut() {
                plan.agent = None;
                if plan.gate_state == GateState::Dispatched {
                    let _ = plan.mark_agent_failed();
                }
            }
            project
        })
        .collect()
}

/// Reconcile a Fix/Restack agent completion against git HEAD and return the
/// outcome label for the `"agent-completed"` payload.
///
/// git HEAD — not agent stdout — is authoritative: an agent can report success
/// while committing nothing. The only trusted signal is whether the worktree
/// branch HEAD actually advanced.
///
/// The lock-across-await rule (CLAUDE.md §2) is honored by construction: the
/// worktree path is snapshotted out of the store (releasing the lock) before any
/// blocking git work; the blocking `git2` read runs on the blocking pool via
/// [`tokio::task::spawn_blocking`] (only the owned worktree path — all `Send` —
/// crosses the boundary); and the store is only re-locked afterwards to write
/// the result. [`Review::apply_agent_completion`] preserves comments on a Failed
/// outcome (Invariant 4).
async fn reconcile_fix_completion(state: &AppState, object_id: &str) -> &'static str {
    let pr_ref = PrRef::new(object_id);

    // A completion can arrive for a review that is no longer `Dispatched`: the
    // agent was killed (kill_agent already reconciled it to InReview) or a
    // duplicate completion (Stop hook + stream-end) already settled it. Applying
    // a transition now would be illegal and only log noise, so report the
    // review's already-settled outcome without touching it.
    let Some(review) = state.reviews.get(&pr_ref) else {
        // No stored review resolved for this object id — no rework can have
        // landed.
        return "failed";
    };
    match review.gate_state {
        GateState::Dispatched => {}
        GateState::Reworked => return "reworked",
        _ => return "failed",
    }

    // Snapshot the worktree path, dropping the store lock before the blocking
    // git read and the diff refresh below.
    let worktree = review.worktree;

    // `git2` reconcile is blocking; run it off the async runtime.
    let head =
        tokio::task::spawn_blocking(move || cockpit_core::adapters::git::reconcile(&worktree))
            .await;

    // Resolve the new HEAD SHA, or `None` (treated as "no progress") when the
    // reconcile failed or its task panicked. `None` routes to Failed, preserving
    // comments for re-dispatch.
    let new_head: Option<String> = match head {
        Ok(Ok(oid)) => Some(oid.to_string()),
        Ok(Err(e)) => {
            eprintln!("reconcile_fix_completion: reconcile failed for {object_id}: {e}");
            None
        }
        Err(e) => {
            eprintln!("reconcile_fix_completion: reconcile task panicked for {object_id}: {e}");
            None
        }
    };

    // Re-lock only to apply the git-authoritative outcome.
    let mut applied: Option<Result<AgentOutcome, cockpit_core::gate::Error>> = None;
    state.reviews.update(&pr_ref, |review| {
        applied = Some(review.apply_agent_completion(new_head));
    });

    match applied {
        Some(Ok(AgentOutcome::Reworked)) => {
            // Best-effort: re-fetch the diff so users review fresh code.
            refresh_review_diff(state, &pr_ref).await;
            "reworked"
        }
        Some(Ok(AgentOutcome::Failed)) => "failed",
        Some(Err(e)) => {
            eprintln!(
                "reconcile_fix_completion: apply_agent_completion failed for {object_id}: {e}"
            );
            "failed"
        }
        // Review vanished between the snapshot and the write-back.
        None => "failed",
    }
}

/// Settle a project's plan after a planner (`AgentMode::Plan`) completion and
/// return the outcome label for the `"agent-completed"` payload.
///
/// Clears the running agent, ingests the planner's written markdown (when a
/// `plan_path` is recorded and the file is present and non-empty) by parsing it
/// into the plan's `doc`, and settles the gate:
///   * `Dispatched` (rework) with parsed output -> `Reworked` (clears ephemeral
///     comments); returns `"reworked"`.
///   * `Dispatched` (rework) with missing/unparseable output -> `InReview` via
///     `mark_agent_failed` (comments preserved for re-dispatch); returns
///     `"failed"`.
///   * `Pending` (initial artifact-fill) stays `Pending`; returns `"completed"`.
///
/// Keyed by [`ProjectId`] (the completion event's `object_id`) so the correct
/// project's plan is updated. Read/parse failures are non-fatal (Invariant 1):
/// the prior doc is kept and the failure is logged rather than blocking the loop.
fn ingest_plan_output(state: &AppState, project_id: &ProjectId) -> &'static str {
    use cockpit_core::model::GateState;

    // Read + parse outside the store lock; only touch on-disk state here.
    let parsed = state.projects.plan(project_id).and_then(|plan| {
        let path = plan.plan_path.clone()?;
        match std::fs::read_to_string(&path) {
            Ok(raw) if !raw.trim().is_empty() => match cockpit_core::plan_parser::parse(&raw) {
                Ok(doc) => Some(doc),
                Err(e) => {
                    eprintln!(
                        "ingest_plan_output: parse failed for {}: {e}",
                        path.display()
                    );
                    None
                }
            },
            Ok(_) => None,
            Err(e) => {
                eprintln!(
                    "ingest_plan_output: read failed for {}: {e}",
                    path.display()
                );
                None
            }
        }
    });

    let parsed_ok = parsed.is_some();
    let mut outcome = "completed";
    state.projects.update_plan(project_id, |slot| {
        let Some(plan) = slot.as_mut() else {
            return;
        };
        plan.agent = None;
        if let Some(doc) = parsed {
            plan.doc = doc;
        }
        // Only a rework spawn (Dispatched) settles the gate; initial generation
        // (Pending) stays Pending as an artifact fill.
        if plan.gate_state == GateState::Dispatched {
            if parsed_ok {
                // `mark_reworked` clears ephemeral comments (Invariant 4). A
                // wrong starting state cannot occur here (guarded above), so the
                // error is ignored deliberately.
                let _ = plan.mark_reworked();
                outcome = "reworked";
            } else {
                // The planner produced no usable output — return the plan to
                // InReview with its comments preserved (failure-aware rework)
                // rather than falsely reporting rework. Guarded state as above.
                let _ = plan.mark_agent_failed();
                outcome = "failed";
            }
        }
    });
    outcome
}

/// Ingest the advisory reviewer's findings after an [`AgentMode::Review`]
/// completion and return the outcome label for the `"agent-completed"` payload.
///
/// The read-only pre-pass reviewer writes a JSON findings array to
/// [`config::findings_file_path`](cockpit_core::config::findings_file_path),
/// keyed by the PR ref used at spawn (the completion event's `object_id`). This
/// reads that file, parses it with
/// [`findings::parse_findings`](cockpit_core::findings::parse_findings), stores
/// the result on the review, and always clears the review's running agent handle.
///
/// Every failure mode is non-fatal (Invariant 1) and maps to `"failed"`: no
/// review resolves for the object id, the path cannot be resolved, the file is
/// missing or unreadable, or the parse returns
/// [`Error::NoArrayFound`](cockpit_core::findings::Error::NoArrayFound). A
/// successful parse (including a located-but-empty array) stores the findings and
/// returns `"completed"`.
///
/// INVARIANT: this NEVER touches `gate_state`. The advisory pre-pass is
/// read-only and never advances the gate (Invariant 5).
///
/// The findings file is a transport, not a store: it is deleted after ingest —
/// findings now live on the [`Review`] and in persistence.
fn ingest_review_findings(state: &AppState, object_id: &str) -> &'static str {
    let Some(pr_ref) = resolve_review_pr(state, object_id) else {
        // No stored review resolved for this object id — nothing to ingest.
        return "failed";
    };

    // Read + parse the reviewer's findings file (keyed by the PR ref used at
    // spawn). Any failure yields `None`; a successful parse yields the findings.
    let path = cockpit_core::config::findings_file_path(object_id);
    let parsed = match &path {
        Ok(path) => match std::fs::read_to_string(path) {
            Ok(raw) => match cockpit_core::findings::parse_findings(&raw) {
                Ok(findings) => Some(findings),
                Err(e) => {
                    eprintln!(
                        "ingest_review_findings: parse failed for {}: {e}",
                        path.display()
                    );
                    None
                }
            },
            Err(e) => {
                eprintln!(
                    "ingest_review_findings: read failed for {}: {e}",
                    path.display()
                );
                None
            }
        },
        Err(e) => {
            eprintln!("ingest_review_findings: findings path for {object_id}: {e}");
            None
        }
    };

    let outcome = if parsed.is_some() {
        "completed"
    } else {
        "failed"
    };

    // Always clear the running agent; store the findings on a successful parse.
    // INVARIANT: gate_state is never touched here (read-only pre-pass).
    state.reviews.update(&pr_ref, |r| {
        r.agent = None;
        if let Some(findings) = parsed {
            r.review_findings = findings;
        }
    });

    // The findings file is a transport, not a store — delete it after ingest.
    // Best-effort: a delete failure (other than a missing file) is logged but
    // never changes the outcome.
    if let Ok(path) = &path
        && let Err(e) = std::fs::remove_file(path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        eprintln!(
            "ingest_review_findings: remove {} failed: {e}",
            path.display()
        );
    }

    outcome
}

/// Resolve a review's [`PrRef`] from a completion event's `object_id`.
///
/// The object id may be a [`PrRef`] string (the Fix/Restack path keys sessions
/// by PR) or a `ReviewId` string (the implementer fan-out keys sessions by
/// review id). Tries the PR key first, then falls back to a scan by review id.
fn resolve_review_pr(state: &AppState, object_id: &str) -> Option<PrRef> {
    let pr_ref = PrRef::new(object_id);
    if state.reviews.get(&pr_ref).is_some() {
        return Some(pr_ref);
    }
    state
        .reviews
        .list()
        .into_iter()
        .find(|r| r.id.as_str() == object_id)
        .map(|r| r.pr)
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

    // The advisory findings were anchored to the previous diff/head; the head
    // has already moved by the time a refresh is requested, so drop them even
    // when the re-fetch fails (stale pins on changed content mislead).
    state.reviews.update(pr_ref, |review| {
        review.review_findings.clear();
    });

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
