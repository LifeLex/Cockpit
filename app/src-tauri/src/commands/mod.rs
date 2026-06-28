//! Thin `#[tauri::command]` handlers that delegate to `cockpit-core`.
//!
//! Commands parse params, call core, and map results through
//! [`CommandError`](crate::error::CommandError). All logic lives in core.

use std::path::PathBuf;
use std::sync::Arc;

use tauri::State;

use cockpit_core::gate::Gated;
use cockpit_core::model::{
    Anchor, Comment, CommentId, CommentOrigin, DiffData, GateState, PrRef, Review,
};

use crate::error::CommandError;
use crate::state::AppState;

/// Return the cockpit-core crate version — trivial command to prove IPC round-trip.
#[tauri::command]
pub fn get_version() -> String {
    cockpit_core::VERSION.to_string()
}

/// List all reviews currently in the store.
#[tauri::command]
pub fn list_reviews(state: State<'_, Arc<AppState>>) -> Result<Vec<Review>, CommandError> {
    Ok(state.reviews.list())
}

/// Get the frontier: reviews safe for deep-review (not stale, not yet approved).
#[tauri::command]
pub fn get_frontier(state: State<'_, Arc<AppState>>) -> Result<Vec<Review>, CommandError> {
    let frontier = state
        .reviews
        .list()
        .into_iter()
        .filter(|r| !r.stale && r.gate_state != GateState::Approved)
        .collect();
    Ok(frontier)
}

/// Get a single review by PR reference string.
#[tauri::command]
pub fn get_review(state: State<'_, Arc<AppState>>, pr: String) -> Result<Review, CommandError> {
    let pr_ref = PrRef::new(&pr);
    state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {pr}"),
    })
}

/// Open a review for human review (`Pending | Reworked` -> `InReview`).
#[tauri::command]
pub fn open_review(state: State<'_, Arc<AppState>>, pr: String) -> Result<Review, CommandError> {
    let pr_ref = PrRef::new(&pr);
    state.reviews.update(&pr_ref, |review| {
        // Best-effort: if the transition fails, we silently ignore it.
        // The returned review will still reflect the unchanged state,
        // so the caller sees the truth.
        let _ = review.open();
    });
    state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {pr}"),
    })
}

/// Return the diff data for a review identified by PR reference.
#[tauri::command]
pub fn get_review_diff(
    state: State<'_, Arc<AppState>>,
    pr: String,
) -> Result<DiffData, CommandError> {
    let pr_ref = PrRef::new(&pr);
    let review = state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {pr}"),
    })?;
    Ok(review.diff.clone())
}

/// Add an anchored comment to a review at the diff gate.
///
/// Creates an ephemeral [`Comment`] with a `DiffLine` anchor and
/// appends it to the review's comment list.
#[tauri::command]
pub fn add_comment(
    state: State<'_, Arc<AppState>>,
    pr: String,
    file: String,
    line_start: u32,
    line_end: u32,
    body: String,
) -> Result<Review, CommandError> {
    let pr_ref = PrRef::new(&pr);

    let comment_id = CommentId::new(format!("c-{}", uuid::Uuid::new_v4()));
    let comment = Comment {
        id: comment_id,
        anchor: Anchor::DiffLine {
            path: PathBuf::from(&file),
            range: (line_start, line_end),
        },
        body,
        origin: CommentOrigin::Local,
    };

    let found = state.reviews.update(&pr_ref, |review| {
        review.comments.push(comment);
    });

    if !found {
        return Err(CommandError {
            message: format!("Review not found: {pr}"),
        });
    }

    state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {pr}"),
    })
}

/// Trigger the request-changes flow for a review (`InReview` -> `Dispatched`).
///
/// Requires at least one comment to be present. The actual agent dispatch
/// is handled by the `Gated::request_changes` transition; the agent spawn
/// is not yet wired (Phase 1 prerequisite).
#[tauri::command]
pub fn request_changes(
    state: State<'_, Arc<AppState>>,
    pr: String,
) -> Result<Review, CommandError> {
    let pr_ref = PrRef::new(&pr);

    let mut transition_err: Option<cockpit_core::gate::Error> = None;
    let found = state.reviews.update(&pr_ref, |review| {
        if let Err(e) = review.request_changes() {
            transition_err = Some(e);
        }
    });

    if !found {
        return Err(CommandError {
            message: format!("Review not found: {pr}"),
        });
    }

    if let Some(e) = transition_err {
        return Err(CommandError::from(e));
    }

    state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {pr}"),
    })
}
