//! Thin `#[tauri::command]` handlers that delegate to `cockpit-core`.
//!
//! Commands parse params, call core, and map results through
//! [`CommandError`](crate::error::CommandError). All logic lives in core.

pub mod shell;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tauri::State;

use cockpit_core::adapters::agent::SpawnConfig;
use cockpit_core::adapters::github::{self, MirrorResult, ReviewEvent, SubmitReviewResult};
use cockpit_core::adapters::linear;
use cockpit_core::config::{AgentPermissionMode, Config};
use cockpit_core::diff_signals::{EvidenceSummary, compute_diff_signals};
use cockpit_core::gate::Gated;
use cockpit_core::kickoff::{self, KickoffResult};
use cockpit_core::model::{
    AgentMode, Anchor, Artifact, CiSummary, Comment, CommentId, CommentOrigin, ConversationItem,
    DiffData, DiffSide, FilePair, GateState, PlanDoc, PrRef, Project, ProjectId, ProjectPlan,
    ProjectRef, ProjectSource, Review, ReviewSource,
};
use cockpit_core::plan_parser;
use cockpit_core::restack;
use cockpit_core::trajectory::{self, TrajectorySummary};

use crate::error::CommandError;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Agent permission requests
// ---------------------------------------------------------------------------

/// Hand-typed payload describing a pending tool-permission request.
///
/// Emitted on the `"permission-request"` Tauri event by the broker forwarder
/// (see `lib.rs`) and returned by [`list_pending_permissions`]. Both paths build
/// it from a core
/// [`PermissionRequest`](cockpit_core::hook_server::PermissionRequest) via
/// [`PendingPermission::from_request`] so the push and pull shapes never drift.
/// Hand-typed on the frontend (no `ts-rs`), mirroring
/// [`AgentCompletedPayload`](crate::AgentCompletedPayload).
#[derive(Debug, Clone, serde::Serialize)]
pub struct PendingPermission {
    /// Unique id used to resolve the request via [`resolve_permission`].
    pub id: String,
    /// UI key of the object the requesting agent is working on (PR ref or
    /// project id).
    pub object_id: String,
    /// The tool the agent is asking to run (e.g. `"Write"`, `"Bash"`).
    pub tool_name: String,
    /// One-line, human-glanceable summary derived from the tool input.
    pub summary: String,
    /// Wall-clock arrival time in epoch milliseconds.
    pub requested_at_epoch_ms: u64,
}

impl PendingPermission {
    /// Build a [`PendingPermission`] from a core
    /// [`PermissionRequest`](cockpit_core::hook_server::PermissionRequest),
    /// deriving the glanceable `summary` from the tool input.
    pub(crate) fn from_request(req: &cockpit_core::hook_server::PermissionRequest) -> Self {
        Self {
            id: req.id.clone(),
            object_id: req.object_id.clone(),
            tool_name: req.tool_name.clone(),
            summary: crate::permission_summary(&req.tool_name, &req.input),
            requested_at_epoch_ms: req.requested_at_epoch_ms,
        }
    }
}

/// Resolve a pending tool-permission request by id (Invariant 5: an explicit
/// human decision, never automatic or from agent output).
///
/// Delegates to the [`PermissionBroker`](cockpit_core::hook_server::PermissionBroker):
/// returns whether the decision landed (`false` means the request was already
/// resolved or timed out). Also emits a `"permission-resolved"` event
/// `{ id, allow }` — always, even when the decision did not land — so every UI
/// surface showing the request clears it, not just the one that acted.
#[tauri::command]
pub fn resolve_permission(
    state: State<'_, Arc<AppState>>,
    app_handle: tauri::AppHandle,
    id: String,
    allow: bool,
) -> Result<bool, CommandError> {
    use tauri::Emitter;

    let landed = state.permission_broker.resolve(&id, allow);

    // Broadcast the resolution so all surfaces clear the request. Hand-typed
    // payload (no ts-rs), mirroring the other event payloads in this crate.
    let _ = app_handle.emit(
        "permission-resolved",
        serde_json::json!({ "id": id, "allow": allow }),
    );

    Ok(landed)
}

/// List the currently-pending tool-permission requests.
///
/// Pull-path companion to the `"permission-request"` event: the frontend calls
/// this to reconcile its queue on mount or after a `Lagged` broadcast drop.
/// Maps the broker's snapshot into the same [`PendingPermission`] shape the
/// event forwarder emits.
#[tauri::command]
pub fn list_pending_permissions(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<PendingPermission>, CommandError> {
    Ok(state
        .permission_broker
        .pending()
        .iter()
        .map(PendingPermission::from_request)
        .collect())
}

/// Build the MCP `approve`-tool URL for `object_id` against the local hook
/// server.
///
/// The URL is baked into a spawned agent's `--mcp-config` so its permission
/// prompts route to cockpit for a human decision (see
/// [`SpawnConfig::apply_permission_mode`]). The object id (a PR ref or project
/// id) is percent-encoded as a single path segment via [`encode_path_segment`]
/// so it round-trips exactly through the axum `/mcp/{object_id}` route — PR refs
/// contain `/` and `#`, which must not be read as path separators. The decoded
/// path param the broker records as `object_id` therefore matches the reviewed
/// object's key exactly.
fn mcp_approve_url(hook_port: u16, object_id: &str) -> String {
    format!(
        "http://127.0.0.1:{hook_port}/mcp/{}",
        encode_path_segment(object_id)
    )
}

/// Percent-encode `s` as a single URL path segment (RFC 3986).
///
/// Encodes every byte outside the *unreserved* set (`A-Z a-z 0-9 - . _ ~`) as
/// `%XX` with uppercase hex, so reserved characters — notably `/` and `#`, which
/// appear in PR refs — are escaped rather than treated as path structure. axum
/// percent-decodes the `{object_id}` path param back to the original bytes, so
/// the encoded segment round-trips exactly (verified against axum 0.8's
/// `Path<String>` extractor).
///
/// A tiny hand-rolled encoder is used deliberately: the `percent-encoding` crate
/// is only a transitive dependency, and this avoids adding a direct one.
fn encode_path_segment(s: &str) -> String {
    /// Map a nibble (`0..=15`) to its uppercase-hex ASCII byte.
    fn hex(nibble: u8) -> u8 {
        match nibble {
            0..=9 => b'0' + nibble,
            _ => b'A' + (nibble - 10),
        }
    }

    let mut out = String::with_capacity(s.len());
    for &byte in s.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(byte));
            }
            _ => {
                out.push('%');
                out.push(char::from(hex(byte >> 4)));
                out.push(char::from(hex(byte & 0x0f)));
            }
        }
    }
    out
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
        .filter(|r| {
            !r.stale && r.gate_state != GateState::Approved && r.gate_state != GateState::Merged
        })
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

/// Return the interdiff for a review: the changes since the last dispatch (D10).
///
/// Diffs the review's [`DispatchSnapshot::reviewed_sha`] against the current
/// HEAD so a re-reviewer sees only what the rework changed, not the whole PR
/// again. Requires a recorded dispatch snapshot (typed error otherwise).
///
/// When the review has a cockpit-managed worktree present on disk, the diff is
/// computed locally with
/// [`git::diff_range`](cockpit_core::adapters::git::diff_range) (off the async
/// runtime, since `git2` is blocking). Otherwise it falls back to the GitHub
/// compare API between the snapshot SHA and the review's `head_sha` (requiring a
/// `repo_slug`). Returned in the same [`DiffData`] shape as
/// [`get_review_diff`].
#[tauri::command]
pub async fn get_interdiff(
    state: State<'_, Arc<AppState>>,
    pr: String,
) -> Result<DiffData, CommandError> {
    let pr_ref = PrRef::new(&pr);
    let review = state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {pr}"),
    })?;

    let snapshot = review.dispatch_snapshot.clone().ok_or_else(|| CommandError {
        message: format!(
            "Review {pr} has no dispatch snapshot; request changes first to establish an interdiff baseline"
        ),
    })?;

    let config = Config::load()?;
    let repo_path = config
        .repo_path
        .clone()
        .unwrap_or_else(|| PathBuf::from("."));

    // Prefer a truthful local diff when a managed worktree exists on disk.
    if is_managed_worktree(&review.worktree, &repo_path) && review.worktree.exists() {
        let worktree = review.worktree.clone();
        let from = snapshot.reviewed_sha.clone();
        let raw = tokio::task::spawn_blocking(move || {
            cockpit_core::adapters::git::diff_range(&worktree, &from, "HEAD")
        })
        .await
        .map_err(|e| CommandError {
            message: format!("interdiff task panicked: {e}"),
        })?
        .map_err(|e| CommandError {
            message: format!("interdiff failed: {e}"),
        })?;
        return Ok(DiffData { raw });
    }

    // Fall back to GitHub compare for imported PRs with no local worktree.
    let repo_slug = review.repo_slug.clone().ok_or_else(|| CommandError {
        message: format!(
            "Review {pr} has no local worktree and no repo slug; cannot compute an interdiff"
        ),
    })?;
    let raw = github::compare(&repo_slug, &snapshot.reviewed_sha, &review.head_sha).await?;
    Ok(DiffData { raw })
}

/// Fetch a PR's GitHub conversation (reviews, inline + issue comments) as
/// read-only context and store it on the review (E1).
///
/// Requires a `repo_slug` (typed error otherwise). Reads via
/// [`github::fetch_conversation`], overwrites the review's `conversation` with
/// the merged chronological list, and returns it. The frontend calls this on
/// opening a review-requested PR and from a refresh button.
///
/// This is a STATUS-tier read: it never advances the gate and never blocks the
/// loop. The stored conversation is external context, distinct from cockpit's
/// own ephemeral [`Comment`]s (Invariant 4).
#[tauri::command]
pub async fn fetch_conversation(
    state: State<'_, Arc<AppState>>,
    pr: String,
) -> Result<Vec<ConversationItem>, CommandError> {
    let pr_ref = PrRef::new(&pr);
    let review = state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {pr}"),
    })?;

    let repo_slug = review.repo_slug.clone().ok_or_else(|| CommandError {
        message: format!("Review {pr} has no repo slug; cannot fetch its GitHub conversation"),
    })?;
    let pr_number = pr_number_from_ref(&pr).ok_or_else(|| CommandError {
        message: format!("Could not parse a PR number from: {pr}"),
    })?;

    let items = github::fetch_conversation(&repo_slug, pr_number).await?;
    state.reviews.update(&pr_ref, |r| {
        r.conversation = items.clone();
    });
    Ok(items)
}

/// Return the review-time evidence bundle for a PR (B1 + D2).
///
/// Bundles the deterministic diff signals, the review's CI rollup, and the
/// commands the agent ran into one [`EvidenceSummary`] so the diff gate can show
/// what changed, whether CI is green, and what the agent executed without three
/// separate round-trips.
///
/// Both the diff walk ([`compute_diff_signals`]) and the trajectory read
/// ([`trajectory::load`]) are potentially expensive (a large diff string; file
/// IO), so they run together on the blocking pool via
/// [`tokio::task::spawn_blocking`] (only the owned diff string and PR key —
/// both `Send` — cross the boundary). `agent_ran` is filled from the persisted
/// trajectory summary; a missing/corrupt summary degrades to an empty list
/// (Invariant §0.1).
#[tauri::command]
pub async fn get_evidence(
    state: State<'_, Arc<AppState>>,
    pr: String,
) -> Result<EvidenceSummary, CommandError> {
    let pr_ref = PrRef::new(&pr);
    let review = state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {pr}"),
    })?;

    let ci = review.ci_summary;
    let raw = review.diff.raw;
    let (signals, agent_ran) = tokio::task::spawn_blocking(move || {
        let signals = compute_diff_signals(&raw);
        // Fold the agent's recorded commands in from its trajectory summary; a
        // missing or corrupt file yields no commands rather than an error.
        let agent_ran = trajectory::load(&pr)
            .map(|t| t.commands)
            .unwrap_or_default();
        (signals, agent_ran)
    })
    .await
    .map_err(|e| CommandError {
        message: format!("evidence task panicked: {e}"),
    })?;

    Ok(EvidenceSummary {
        signals,
        ci,
        agent_ran,
    })
}

/// Return the persisted agent trajectory summary for a PR, if one exists (D2).
///
/// Loads `<logs_dir>/<slug>.trajectory.json` — the compact rollup of the last
/// agent run keyed by the PR ref — so the UI can render "what did the agent
/// try?" without re-running or reparsing the raw log. The read is file IO, so
/// it runs on the blocking pool via [`tokio::task::spawn_blocking`].
///
/// Returns `Ok(None)` when no summary is present or it cannot be parsed
/// ([`trajectory::load`] never fails loudly — Invariant §0.1).
#[tauri::command]
pub async fn get_trajectory_summary(pr: String) -> Result<Option<TrajectorySummary>, CommandError> {
    let summary = tokio::task::spawn_blocking(move || trajectory::load(&pr))
        .await
        .map_err(|e| CommandError {
            message: format!("trajectory load task panicked: {e}"),
        })?;
    Ok(summary)
}

/// Return the full text of a single file on both sides of a review's diff (B4).
///
/// Feeds the diff gate's optional full-file view. The two revisions are resolved
/// preferring pinned SHAs (truthful across force-pushes): the base is
/// `base_sha` when set, else the base branch name; the head is `head_sha` when
/// set, else `HEAD` for a local read or the head branch name for a GitHub read.
///
/// Known limitation for imported PRs: their `base_sha` is empty (it is the
/// restack fork point, computed locally at kickoff — an import has none), so the
/// base side resolves by branch name and may drift past the PR's actual fork
/// point once the base branch advances. The New side is pinned to `head_sha` and
/// is authoritative.
///
/// Content is read locally with
/// [`git::file_at_rev`](cockpit_core::adapters::git::file_at_rev) (off the async
/// runtime, since `git2` is blocking) when a usable local repo dir exists — a
/// cockpit-managed worktree present on disk, or the shared checkout for a
/// same-repo PR (`repo_slug` absent). Otherwise, for an imported PR with a
/// `repo_slug`, it falls back to
/// [`github::contents_at`](cockpit_core::adapters::github::contents_at).
///
/// See [`combine_file_pair`] for how the two per-side results become the
/// returned [`FilePair`] and when `full` is `false`.
#[tauri::command]
pub async fn get_file_pair(
    state: State<'_, Arc<AppState>>,
    pr: String,
    path: String,
) -> Result<FilePair, CommandError> {
    let pr_ref = PrRef::new(&pr);
    let review = state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {pr}"),
    })?;

    let config = Config::load()?;
    let repo_path = config
        .repo_path
        .clone()
        .unwrap_or_else(|| PathBuf::from("."));

    // Resolve the base revision: a pinned SHA when known, else the base branch.
    let base_rev = if review.base_sha.is_empty() {
        review.base.clone()
    } else {
        review.base_sha.clone()
    };

    // A usable local repo dir: a managed worktree present on disk, or — only for
    // a same-repo PR (`repo_slug` absent) — the shared checkout. A cross-repo PR
    // (`repo_slug` set) whose worktree still points at `repo_path` is the wrong
    // repo, so it routes to the GitHub fallback instead.
    let local_dir: Option<PathBuf> = if is_managed_worktree(&review.worktree, &repo_path) {
        review.worktree.exists().then(|| review.worktree.clone())
    } else if review.repo_slug.is_none() && repo_path.exists() {
        Some(repo_path.clone())
    } else {
        None
    };

    if let Some(dir) = local_dir {
        // Head revision for a local read: a pinned SHA when known, else `HEAD`
        // (the worktree branch tip).
        let head_rev = if review.head_sha.is_empty() {
            "HEAD".to_string()
        } else {
            review.head_sha.clone()
        };
        let pair = tokio::task::spawn_blocking(move || {
            let original = cockpit_core::adapters::git::file_at_rev(&dir, &base_rev, &path);
            let modified = cockpit_core::adapters::git::file_at_rev(&dir, &head_rev, &path);
            combine_file_pair(original, modified)
        })
        .await
        .map_err(|e| CommandError {
            message: format!("file-pair task panicked: {e}"),
        })?;
        return Ok(pair);
    }

    // Imported PR with no usable local dir: read via GitHub contents.
    if let Some(repo_slug) = review.repo_slug.as_deref() {
        // Head ref for a GitHub read: a pinned SHA when known, else the head
        // branch name.
        let head_ref = if review.head_sha.is_empty() {
            review.branch.clone()
        } else {
            review.head_sha.clone()
        };
        let original = github::contents_at(repo_slug, &base_rev, &path).await;
        let modified = github::contents_at(repo_slug, &head_ref, &path).await;
        return Ok(combine_file_pair(original, modified));
    }

    // Neither a local dir nor a repo slug: nothing to read — fall back.
    Ok(FilePair {
        original: String::new(),
        modified: String::new(),
        full: false,
    })
}

/// Combine the two per-side file-content reads into a [`FilePair`].
///
/// `Err` on either side means the content could not be determined, so the pair
/// is reported as not-full (`full: false`) and the frontend falls back to the
/// diff fragments. `Ok(None)` on a side means the file is legitimately absent
/// there — an added or deleted file, or (indistinguishably) a blob past the
/// adapter's size cap — and maps to an empty string; but when BOTH sides are
/// absent there is nothing to show, so the pair is not-full. Any side that
/// loaded makes the pair `full`.
fn combine_file_pair<E>(
    original: Result<Option<String>, E>,
    modified: Result<Option<String>, E>,
) -> FilePair {
    let not_full = FilePair {
        original: String::new(),
        modified: String::new(),
        full: false,
    };
    match (original, modified) {
        (Ok(orig), Ok(modi)) => match (orig, modi) {
            // Both sides legitimately absent: nothing to render.
            (None, None) => not_full,
            (orig, modi) => FilePair {
                original: orig.unwrap_or_default(),
                modified: modi.unwrap_or_default(),
                full: true,
            },
        },
        // Could not determine one or both sides.
        _ => not_full,
    }
}

/// Add an anchored comment to a review at the diff gate.
///
/// Creates an ephemeral [`Comment`] with a `DiffLine` anchor and
/// appends it to the review's comment list. `side` selects which side of the
/// diff the range refers to; `None` defaults to [`DiffSide::New`] so existing
/// callers keep commenting on the post-change side unchanged (D12).
#[tauri::command]
pub fn add_comment(
    state: State<'_, Arc<AppState>>,
    pr: String,
    file: String,
    line_start: u32,
    line_end: u32,
    body: String,
    side: Option<DiffSide>,
) -> Result<Review, CommandError> {
    let pr_ref = PrRef::new(&pr);

    let comment_id = CommentId::new(format!("c-{}", uuid::Uuid::new_v4()));
    let comment = Comment {
        id: comment_id,
        anchor: Anchor::DiffLine {
            path: PathBuf::from(&file),
            range: (line_start, line_end),
            side: side.unwrap_or(DiffSide::New),
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
/// Requires at least one comment to be present. After transitioning the gate
/// state, assembles a rework prompt from the review's issue, diff, and
/// comments, then spawns the fixer agent in the review's worktree. The
/// agent's stdout is streamed to the frontend via Tauri events.
#[tauri::command]
pub async fn request_changes(
    state: State<'_, Arc<AppState>>,
    app_handle: tauri::AppHandle,
    pr: String,
) -> Result<Review, CommandError> {
    let pr_ref = PrRef::new(&pr);

    let mut transition_err: Option<cockpit_core::gate::Error> = None;
    let found = state.reviews.update(&pr_ref, |review| {
        // D10: snapshot the pre-rework head + the comments being dispatched
        // *before* advancing the gate, so the audit record reflects exactly
        // what this cycle asked for. Guarded to the states where the transition
        // will succeed so a rejected transition never leaves a bogus snapshot.
        if review.gate_state == GateState::InReview && !review.comments.is_empty() {
            review.snapshot_dispatch();
        }
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

    // D7: dispatching rework makes this review's descendants stale — once its
    // HEAD advances they sit on an old base and need a restack. Mark them at
    // dispatch time (SPEC §7), immediately after the successful transition.
    if let Some(id) = state.reviews.get(&pr_ref).map(|r| r.id) {
        state.reviews.mark_descendants_stale(&id);
    }

    // Spawn the fixer agent via the shared Fix-loop path (no CI logs here).
    // Spawn failure is non-fatal — the gate transition already succeeded so we
    // return the Dispatched review regardless and log the spawn error.
    dispatch_fix_agent(&state, &app_handle, &pr, &pr_ref, None).await;

    state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {pr}"),
    })
}

/// Maximum number of PR-body characters carried into a rework intent.
///
/// A very long PR description would otherwise dominate the prompt; truncating
/// keeps the fixer focused while still giving it the leading context.
const MAX_INTENT_BODY_CHARS: usize = 8000;

/// Compose the rework intent from a review's PR title, body, and issue ref.
///
/// Lays out the parts as `<title>` / `<body>` / `Issue: <ref>` separated by
/// blank lines, skipping any empty part so there are no stray blank lines. The
/// body is truncated to [`MAX_INTENT_BODY_CHARS`] characters on a `char`
/// boundary (D4).
fn compose_fix_intent(title: &str, body: &str, issue: &str) -> String {
    let mut parts: Vec<String> = Vec::new();

    let title = title.trim();
    if !title.is_empty() {
        parts.push(title.to_string());
    }

    let body = truncate_on_char_boundary(body.trim(), MAX_INTENT_BODY_CHARS);
    if !body.is_empty() {
        parts.push(body.to_string());
    }

    let issue = issue.trim();
    if !issue.is_empty() {
        parts.push(format!("Issue: {issue}"));
    }

    parts.join("\n\n")
}

/// Return the prefix of `s` holding at most `max_chars` characters.
///
/// Slices on a `char` boundary (never mid-codepoint), so the result is always
/// valid UTF-8.
fn truncate_on_char_boundary(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

/// Dispatch the diff-gate Fix loop for a review already transitioned to
/// `Dispatched`, spawning the fixer agent with a rework prompt.
///
/// This is the shared spawn path for both `request_changes` and `fix_ci`:
/// assemble the rework prompt (optionally carrying `ci_failures` verbatim),
/// spawn the fixer agent in the review's worktree, and attach the running
/// agent. Spawn failure is non-fatal — the gate transition already stands, so
/// the error is logged and surfaced as an `agent-event` rather than blocking
/// the loop (Invariant 1).
async fn dispatch_fix_agent(
    state: &State<'_, Arc<AppState>>,
    app_handle: &tauri::AppHandle,
    pr: &str,
    pr_ref: &PrRef,
    ci_failures: Option<&str>,
) {
    let Some(review) = state.reviews.get(pr_ref) else {
        return;
    };

    // D12: ensure a usable worktree before spawning. An imported same-repo PR
    // points at the user's shared checkout; materialize a dedicated branch
    // worktree so the fixer commits on the PR branch and the HEAD-based outcome
    // detection is truthful. Failure is non-fatal (Invariant 1): surface it and
    // bail rather than spawning against the wrong tree.
    let worktree = match ensure_worktree_for_review(state, pr_ref).await {
        Ok(w) => w,
        Err(e) => {
            eprintln!("dispatch_fix_agent: ensure worktree failed: {e}");
            let error_event = cockpit_core::adapters::agent_stream::Event::Error {
                message: format!("Preparing worktree failed: {e}"),
            };
            crate::streaming::emit_agent_event(app_handle, pr, error_event);
            return;
        }
    };

    // Load config once so the per-mode custom preamble and agent command are
    // both honored. A load failure is non-fatal — fall back to defaults (no
    // preamble, builtin command) so rework still dispatches.
    let config = Config::load().ok();
    let preamble = config
        .as_ref()
        .and_then(|c| c.agent_prompts.for_mode(AgentMode::Fix).map(str::to_owned));
    // Skills relevant to the diff under review; discovery failures are non-fatal
    // (fall back to no skills — never block the loop).
    let skills = cockpit_core::skills::relevant_for_diff(&review.diff.raw);
    let artifact = Artifact::Diff(review.diff.clone());
    // Compose the rework intent from the PR title/body/issue so the fixer has
    // the full context, not just the bare issue ref (D4).
    let intent = compose_fix_intent(&review.title, &review.body, review.issue.as_str());
    // D8/SPEC §9: thread the project's approved plan into the rework prompt as
    // the contract, but only for reviews whose project has an approved plan.
    let approved_plan = review.project.as_ref().and_then(|pid| {
        state
            .projects
            .plan(pid)
            .filter(|p| p.gate_state == GateState::Approved)
            .map(|p| p.doc)
    });
    let rework_input = cockpit_core::prompt::ReworkInput {
        intent: &intent,
        custom_preamble: preamble.as_deref(),
        approved_plan: approved_plan.as_ref(),
        artifact: &artifact,
        comments: &review.comments,
        ci_failures,
        skills: &skills,
    };
    let assembled = cockpit_core::prompt::assemble_rework(&rework_input);

    match try_spawn_agent(state, app_handle, pr, pr_ref, &worktree, &assembled).await {
        Ok(agent_run) => {
            state.reviews.update(pr_ref, |r| {
                r.agent = Some(agent_run);
            });
        }
        Err(e) => {
            eprintln!("dispatch_fix_agent: agent spawn failed: {e}");
            // Surface the spawn failure to the agent panel, keyed by the
            // review's PR ref so the frontend attributes it to the right object.
            let error_event = cockpit_core::adapters::agent_stream::Event::Error {
                message: format!("Agent spawn failed: {e}"),
            };
            crate::streaming::emit_agent_event(app_handle, pr, error_event);
        }
    }
}

/// Attempt to spawn a fixer agent. Factored out so the caller can treat
/// failure as non-fatal.
async fn try_spawn_agent(
    state: &AppState,
    app_handle: &tauri::AppHandle,
    pr: &str,
    pr_ref: &PrRef,
    worktree: &std::path::Path,
    prompt: &cockpit_core::prompt::AssembledPrompt,
) -> Result<cockpit_core::model::AgentRun, String> {
    let config = Config::load().map_err(|e| format!("config: {e}"))?;
    // Apply the configured permission mode, routing prompts to cockpit's MCP
    // `approve` tool keyed by this review's PR ref. No blanket write allow is
    // added: the fixer edits inside its own worktree (its cwd), and empirically
    // those edits pass under the default (Approve) policy without ever routing
    // through the Approve queue.
    let approve_url = mcp_approve_url(config.hook_port, pr_ref.as_str());
    let spawn_config = SpawnConfig::from_config(&config)
        .apply_permission_mode(config.agent_permission_mode, Some(&approve_url));
    let hook_url = format!("http://127.0.0.1:{}/hook/stop", config.hook_port);

    let spawn_result = cockpit_core::adapters::agent::spawn_agent(
        worktree,
        prompt,
        cockpit_core::model::AgentMode::Fix,
        pr_ref.as_str(),
        &state.sessions,
        &hook_url,
        &spawn_config,
    )
    .await
    .map_err(|e| format!("spawn: {e}"))?;

    let stream_ctx = crate::streaming::StreamContext {
        object_id: pr.to_string(),
        mode: cockpit_core::model::AgentMode::Fix,
        completion_tx: state.completion_tx.clone(),
    };
    Ok(crate::streaming::start_stream_forwarding(
        spawn_result,
        app_handle.clone(),
        stream_ctx,
    ))
}

/// Run the advisory read-only pre-pass reviewer over a PR's diff (B2).
///
/// This is an explicit user action. It spawns an [`AgentMode::Review`] agent
/// that inspects the diff against the PR intent and writes a JSON findings array
/// to [`config::findings_file_path`](cockpit_core::config::findings_file_path);
/// the Stop-hook completion handler ingests that file onto the review (see
/// `ingest_review_findings` in `lib.rs`).
///
/// The pre-pass is advisory: it NEVER touches the gate state (Review mode never
/// transitions). It refuses to start when an agent is already attached so a
/// reviewer cannot race an in-flight fix/restack/implement agent. An imported
/// PR's worktree is materialized first via [`ensure_worktree_for_review`]. On a
/// successful spawn the running agent is attached and any stale findings from a
/// previous pre-pass are cleared so the UI shows "agent working" cleanly.
#[tauri::command]
pub async fn pre_review(
    state: State<'_, Arc<AppState>>,
    app_handle: tauri::AppHandle,
    pr: String,
) -> Result<Review, CommandError> {
    let pr_ref = PrRef::new(&pr);
    let review = state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {pr}"),
    })?;

    // Refuse to start a second agent for this review: the advisory reviewer must
    // not race an in-flight fix/restack/implement agent.
    if review.agent.is_some() {
        return Err(CommandError {
            message: format!("Review {pr} already has a running agent; wait for it to finish"),
        });
    }

    // Materialize a usable worktree for imported PRs (a no-op for managed ones).
    let worktree = ensure_worktree_for_review(&state, &pr_ref).await?;

    // Re-read after materialization, which may have updated the worktree path.
    let review = state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {pr}"),
    })?;

    // Resolve (and ensure the parent dir of) the findings output path so the
    // reviewer's write succeeds; the completion handler reads it back.
    let findings_path = cockpit_core::config::findings_file_path(&pr)?;
    if let Some(parent) = findings_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Assemble the review prompt: intent from the PR title/body/issue, the
    // Review-mode custom preamble, and skills relevant to the diff — mirroring
    // dispatch_fix_agent's structure. A config load failure falls back to no
    // preamble (builtin instruction).
    let preamble = Config::load().ok().and_then(|c| {
        c.agent_prompts
            .for_mode(AgentMode::Review)
            .map(str::to_owned)
    });
    let skills = cockpit_core::skills::relevant_for_diff(&review.diff.raw);
    let review_input = cockpit_core::prompt::ReviewInput {
        title: &review.title,
        body: &review.body,
        issue: review.issue.as_str(),
        custom_preamble: preamble.as_deref(),
        diff: &review.diff,
        output_path: Some(&findings_path),
        skills: &skills,
    };
    let assembled = cockpit_core::prompt::assemble_review_prompt(&review_input);

    // Spawn the reviewer (keyed by PR ref so completion ingests the right
    // review). On success, attach the agent and clear any stale findings from a
    // previous pre-pass — atomically, so the UI never shows old findings under a
    // running agent. The gate state is left UNTOUCHED (Invariant 5).
    let agent_run =
        try_spawn_review_agent(&state, &app_handle, &pr, &pr_ref, &worktree, &assembled)
            .await
            .map_err(|e| CommandError {
                message: format!("failed to spawn reviewer agent: {e}"),
            })?;
    state.reviews.update(&pr_ref, |r| {
        r.review_findings.clear();
        r.agent = Some(agent_run);
    });

    state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {pr}"),
    })
}

/// Attempt to spawn the advisory pre-pass reviewer ([`AgentMode::Review`]).
///
/// Mirrors [`try_spawn_agent`] but in Review mode: spawns the agent in the
/// review's worktree, keyed by the PR ref, and wires stdout streaming to the
/// frontend. Factored out so the caller can map a spawn failure to a
/// [`CommandError`].
async fn try_spawn_review_agent(
    state: &AppState,
    app_handle: &tauri::AppHandle,
    pr: &str,
    pr_ref: &PrRef,
    worktree: &std::path::Path,
    prompt: &cockpit_core::prompt::AssembledPrompt,
) -> Result<cockpit_core::model::AgentRun, String> {
    let config = Config::load().map_err(|e| format!("config: {e}"))?;
    // The findings file lives outside the worktree; a headless session can't
    // be granted that write interactively, so pre-authorize it at spawn — both
    // as an accessible dir (`with_extra_dir`) and as a scoped write auto-approval
    // (`allow_write_under`) so the reviewer's findings write never routes through
    // the Approve queue.
    let findings_dir =
        cockpit_core::config::findings_dir().map_err(|e| format!("findings dir: {e}"))?;
    let approve_url = mcp_approve_url(config.hook_port, pr_ref.as_str());
    let spawn_config = SpawnConfig::from_config(&config)
        .apply_permission_mode(config.agent_permission_mode, Some(&approve_url))
        .with_extra_dir(&findings_dir)
        .allow_write_under(&findings_dir);
    let hook_url = format!("http://127.0.0.1:{}/hook/stop", config.hook_port);

    let spawn_result = cockpit_core::adapters::agent::spawn_agent(
        worktree,
        prompt,
        AgentMode::Review,
        pr_ref.as_str(),
        &state.sessions,
        &hook_url,
        &spawn_config,
    )
    .await
    .map_err(|e| format!("spawn: {e}"))?;

    let stream_ctx = crate::streaming::StreamContext {
        object_id: pr.to_string(),
        mode: AgentMode::Review,
        completion_tx: state.completion_tx.clone(),
    };
    Ok(crate::streaming::start_stream_forwarding(
        spawn_result,
        app_handle.clone(),
        stream_ctx,
    ))
}

// ---------------------------------------------------------------------------
// Worktree materialization
// ---------------------------------------------------------------------------

/// Whether `worktree` is a cockpit-managed worktree rather than the user's
/// shared repo checkout at `repo_path`.
///
/// Managed worktrees live under the cockpit worktrees dir (kickoff reviews) or a
/// per-repo clone (cross-repo fix worktrees); either way they are never the
/// configured `repo_path`, which is where imported same-repo PRs point until a
/// fix materializes a dedicated worktree.
fn is_managed_worktree(worktree: &Path, repo_path: &Path) -> bool {
    worktree != repo_path
}

/// Ensure a review has a usable worktree on disk, materializing one for imported
/// PRs, and return its path (D12).
///
/// Kickoff reviews (and previously-materialized ones) already live in a
/// cockpit-managed worktree — this returns it unchanged. A GitHub-imported
/// same-repo PR instead points at the user's shared repo checkout; its PR branch
/// is checked out into a dedicated worktree via
/// [`git::ensure_branch_checkout`](cockpit_core::adapters::git::ensure_branch_checkout),
/// recorded on the review, so rework and interdiff operate on the PR branch and
/// HEAD-based outcome detection is truthful.
///
/// Shared by the [`ensure_review_worktree`] command and [`dispatch_fix_agent`].
async fn ensure_worktree_for_review(
    state: &AppState,
    pr_ref: &PrRef,
) -> Result<PathBuf, CommandError> {
    let review = state.reviews.get(pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {pr_ref}"),
    })?;

    let config = Config::load()?;
    let repo_path = config
        .repo_path
        .clone()
        .unwrap_or_else(|| PathBuf::from("."));

    // Already a managed worktree: use it as-is.
    if is_managed_worktree(&review.worktree, &repo_path) {
        return Ok(review.worktree.clone());
    }

    // Imported same-repo PR pointing at the shared checkout: materialize a
    // dedicated branch worktree and record it on the review.
    let new_path = cockpit_core::adapters::git::ensure_branch_checkout(
        &repo_path,
        &review.branch,
        review.repo_slug.as_deref(),
    )
    .await
    .map_err(|e| CommandError {
        message: format!(
            "failed to check out branch `{}` for {pr_ref}: {e}",
            review.branch
        ),
    })?;

    state.reviews.update(pr_ref, |r| {
        r.worktree = new_path.clone();
    });

    Ok(new_path)
}

/// Ensure a review has a checked-out worktree on disk, returning its path (D12).
///
/// A no-op returning the existing path for kickoff reviews; for a GitHub-imported
/// same-repo PR it checks the PR branch out into a dedicated worktree (recorded
/// on the review) so rework and interdiff operate on the PR branch rather than
/// the user's shared checkout. This is an explicit, idempotent user action.
#[tauri::command]
pub async fn ensure_review_worktree(
    state: State<'_, Arc<AppState>>,
    pr: String,
) -> Result<String, CommandError> {
    let pr_ref = PrRef::new(&pr);
    let path = ensure_worktree_for_review(&state, &pr_ref).await?;
    Ok(path.to_string_lossy().into_owned())
}

// ---------------------------------------------------------------------------
// Comment mirroring
// ---------------------------------------------------------------------------

/// Mirror local comments for a review to GitHub.
///
/// Only mirrors comments with [`CommentOrigin::Local`] origin to avoid
/// duplicating comments that came from GitHub. This is an explicit user
/// action (Invariant 5: mirroring comments to a public GitHub thread
/// never happens automatically).
#[tauri::command]
pub async fn mirror_comments(
    state: State<'_, Arc<AppState>>,
    pr: String,
) -> Result<MirrorResult, CommandError> {
    let pr_ref = PrRef::new(&pr);
    let review = state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {pr}"),
    })?;

    let result = github::mirror_comments(&pr_ref, &review.comments)
        .await
        .map_err(|e| CommandError {
            message: e.to_string(),
        })?;

    Ok(result)
}

/// Submit a GitHub PR review (approve / request changes / comment) carrying the
/// review's inline Local comments (D9).
///
/// This is a guarded outward side effect (Invariant 5): it publishes to a public
/// GitHub thread and must only ever be invoked from an explicit user action in
/// the UI, never automatically or from agent output.
///
/// Requires a `repo_slug` (typed error otherwise). The review's Local-origin
/// comments are submitted inline; [`github::submit_review`] pre-validates each
/// against the PR diff, recording any whose anchored line is not part of the
/// diff in [`SubmitReviewResult::skipped`] rather than failing the whole review.
///
/// On success:
/// - `Approve` on a review-requested PR also advances the local gate to
///   `Approved` (opening from `Pending`/`Reworked` first, like [`approve_review`]).
///   No local agent is ever dispatched here.
/// - `RequestChanges`/`Comment` clear the Local comments that were actually
///   submitted (they now live on GitHub; keeping local copies would
///   double-report), leaving skipped ones in place so the user can fix and retry.
#[tauri::command]
pub async fn submit_github_review(
    state: State<'_, Arc<AppState>>,
    pr: String,
    event: ReviewEvent,
    body: Option<String>,
) -> Result<SubmitReviewResult, CommandError> {
    let pr_ref = PrRef::new(&pr);
    let review = state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {pr}"),
    })?;

    let repo_slug = review.repo_slug.clone().ok_or_else(|| CommandError {
        message: format!("Review {pr} has no repo slug; cannot submit a GitHub review"),
    })?;
    let pr_number = pr_number_from_ref(&pr).ok_or_else(|| CommandError {
        message: format!("Could not parse a PR number from: {pr}"),
    })?;

    // `submit_review` filters to Local-origin comments and validates each against
    // the diff, so pass the full comment list plus the review's diff.
    let result = github::submit_review(
        &repo_slug,
        pr_number,
        event,
        &review.comments,
        &body.unwrap_or_default(),
        &review.diff.raw,
    )
    .await?;

    match event {
        ReviewEvent::Approve => {
            // Mirror the GitHub approval locally only for review-requested PRs
            // (cockpit's own authored PRs use approve_review + merge). Best-effort:
            // the GitHub review already succeeded, so a local transition error is
            // ignored rather than surfaced.
            if review.source == ReviewSource::ReviewRequested {
                state.reviews.update(&pr_ref, |r| {
                    if matches!(r.gate_state, GateState::Pending | GateState::Reworked) {
                        let _ = r.open();
                    }
                    let _ = r.approve();
                });
            }
        }
        ReviewEvent::RequestChanges | ReviewEvent::Comment => {
            // Clear the Local comments that were submitted; keep skipped ones so
            // the user can fix and retry. Non-Local (GitHub-mirrored) comments
            // are always kept.
            let skipped: Vec<CommentId> = result.skipped.iter().map(|(id, _)| id.clone()).collect();
            state.reviews.update(&pr_ref, |r| {
                r.comments
                    .retain(|c| c.origin != CommentOrigin::Local || skipped.contains(&c.id));
            });
        }
    }

    // Mark the revision this GitHub review covered so a later "changes since your
    // review" interdiff (E2) can diff against it. Any verdict counts. Only when
    // the head SHA is known (imported PRs pin it at fetch).
    if !review.head_sha.is_empty() {
        state.reviews.update(&pr_ref, |r| {
            r.last_reviewed_sha = Some(r.head_sha.clone());
        });
    }

    Ok(result)
}

/// Return the interdiff for a teammate's PR since the user's last GitHub review
/// (E2): the changes a review-requested PR received after the user reviewed it.
///
/// Requires a recorded `last_reviewed_sha` (set by [`submit_github_review`]), a
/// `repo_slug`, and a current `head_sha` that is non-empty and differs from the
/// last-reviewed revision (typed errors otherwise). Diffs
/// `last_reviewed_sha..head_sha` via the GitHub compare API so the re-reviewer
/// sees only what changed, not the whole PR again. Returned in the same
/// [`DiffData`] shape as [`get_review_diff`].
#[tauri::command]
pub async fn get_teammate_interdiff(
    state: State<'_, Arc<AppState>>,
    pr: String,
) -> Result<DiffData, CommandError> {
    let pr_ref = PrRef::new(&pr);
    let review = state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {pr}"),
    })?;

    let last = review
        .last_reviewed_sha
        .clone()
        .ok_or_else(|| CommandError {
            message: format!(
                "Review {pr} has no prior review recorded; submit a GitHub review first"
            ),
        })?;
    let repo_slug = review.repo_slug.clone().ok_or_else(|| CommandError {
        message: format!("Review {pr} has no repo slug; cannot compute a teammate interdiff"),
    })?;

    // A non-empty head that differs from the last-reviewed revision is the whole
    // point: without new commits there is nothing to re-review.
    if review.head_sha.is_empty() || review.head_sha == last {
        return Err(CommandError {
            message: format!("Review {pr} has no new commits since your last review"),
        });
    }

    let raw = github::compare(&repo_slug, &last, &review.head_sha).await?;
    Ok(DiffData { raw })
}

// ---------------------------------------------------------------------------
// CI visibility + dispatch-to-fix
// ---------------------------------------------------------------------------

/// Parse a PR number from a review's PR reference.
///
/// Accepts URL form (`https://.../pull/42`) and `owner/repo#42` form; returns
/// `None` when no number can be parsed.
fn pr_number_from_ref(pr: &str) -> Option<u64> {
    if let Some(tail) = pr.rsplit('/').next()
        && let Ok(n) = tail.parse::<u64>()
    {
        return Some(n);
    }
    if let Some(tail) = pr.rsplit('#').next()
        && let Ok(n) = tail.parse::<u64>()
    {
        return Some(n);
    }
    None
}

/// Fetch CI checks for a review's PR and return their rollup (STATUS tier).
///
/// Reads checks via `gh pr checks` (using the review's `repo_slug` when set),
/// emits a `ci-updated` event carrying the full [`CiCheck`] list so the
/// frontend updates via events (not polling), and returns the [`CiSummary`]
/// rollup for the badge.
///
/// This never blocks the review loop and never mutates review state: any `gh`
/// error (including a PR with no checks) is treated as non-fatal and yields an
/// empty summary with an empty checks event (Invariant 1).
#[tauri::command]
pub async fn fetch_ci_checks(
    state: State<'_, Arc<AppState>>,
    app_handle: tauri::AppHandle,
    pr: String,
) -> Result<CiSummary, CommandError> {
    use tauri::Emitter;

    let pr_ref = PrRef::new(&pr);
    let repo_slug = state.reviews.get(&pr_ref).and_then(|r| r.repo_slug.clone());

    let empty = CiSummary {
        passed: 0,
        total: 0,
        failed: 0,
        pending: 0,
    };

    let Some(pr_number) = pr_number_from_ref(&pr) else {
        // No parseable PR number — nothing to fetch. Emit an empty update so
        // the badge clears rather than hanging.
        let _ = app_handle.emit("ci-updated", (&pr, Vec::<github::CiCheck>::new()));
        return Ok(empty);
    };

    match github::pr_checks(repo_slug.as_deref(), pr_number).await {
        Ok(checks) => {
            let summary = github::summarize(&checks);
            // Push the full checks list to the frontend via event (§4).
            let _ = app_handle.emit("ci-updated", (&pr, &checks));
            Ok(summary)
        }
        Err(e) => {
            // Non-fatal: a PR may legitimately have no checks (gh exits
            // non-zero). Report an empty summary; never block the loop.
            eprintln!("fetch_ci_checks: {e}");
            let _ = app_handle.emit("ci-updated", (&pr, Vec::<github::CiCheck>::new()));
            Ok(empty)
        }
    }
}

/// List the CI checks for a review's PR (STATUS tier, best-effort query).
///
/// Resolves the PR number and `repo_slug` from the stored review and reads
/// checks via `gh pr checks`. Unlike [`fetch_ci_checks`], this returns the full
/// [`CiCheck`] list directly (for the CI tab) and emits no event.
///
/// Per Invariant 1 this is a best-effort UI query that must never block the
/// review loop: any `gh` error (including a PR with no checks, or a
/// non-parseable PR reference) is treated as non-fatal and yields an EMPTY
/// list, with the error logged to stderr.
#[tauri::command]
pub async fn list_ci_checks(
    state: State<'_, Arc<AppState>>,
    pr: String,
) -> Result<Vec<github::CiCheck>, CommandError> {
    let pr_ref = PrRef::new(&pr);
    let repo_slug = state.reviews.get(&pr_ref).and_then(|r| r.repo_slug.clone());

    let Some(pr_number) = pr_number_from_ref(&pr) else {
        return Ok(Vec::new());
    };

    match github::pr_checks(repo_slug.as_deref(), pr_number).await {
        Ok(checks) => Ok(checks),
        Err(e) => {
            eprintln!("list_ci_checks: {e}");
            Ok(Vec::new())
        }
    }
}

/// Fetch the failed-job logs for a single CI run, identified by a check `link`
/// (LOG tier, best-effort, per-pipeline).
///
/// The `link` is a check's details URL (e.g.
/// `https://github.com/owner/repo/actions/runs/123/job/456`); the run id is
/// extracted server-side via [`github::run_id_from_link`] and the logs are read
/// via `gh run view <run-id> --log-failed`, scoped to the review's `repo_slug`.
///
/// Per Invariant 1 this is non-fatal: a `link` with no parseable run id, or any
/// `gh` error, yields an EMPTY string with the error logged to stderr, never a
/// hard failure that could block the UI. The CI panel calls this per pipeline,
/// so passing pipelines run no subprocess at all (only failed pipelines fetch).
#[tauri::command]
pub async fn ci_run_logs_by_link(
    state: State<'_, Arc<AppState>>,
    pr: String,
    link: String,
) -> Result<String, CommandError> {
    let pr_ref = PrRef::new(&pr);
    let repo_slug = state.reviews.get(&pr_ref).and_then(|r| r.repo_slug.clone());

    let Some(run_id) = github::run_id_from_link(&link) else {
        return Ok(String::new());
    };

    match github::run_logs(repo_slug.as_deref(), run_id).await {
        Ok(logs) => Ok(logs),
        Err(e) => {
            eprintln!("ci_run_logs_by_link: {e}");
            Ok(String::new())
        }
    }
}

/// Dispatch the Fix loop to address a PR's CI failures (LOG tier).
///
/// This is an EXPLICIT user action (Invariant 5): it never auto-fires. It
/// fetches the failed-CI logs on demand, reuses the diff-gate Fix loop
/// (`request_changes` spawn path via [`dispatch_fix_agent`]) — ensuring the
/// review is `InReview`, adding a synthetic local comment summarizing the CI
/// failure so the gate's ≥1-comment rule is met, transitioning
/// `request_changes` (→ `Dispatched`), and spawning the fixer agent with a
/// rework prompt carrying the CI logs verbatim.
///
/// The CI-log fetch is best-effort: a failure yields no logs but the Fix loop
/// still dispatches (the synthetic comment still tells the agent CI failed),
/// so a GitHub read never blocks the loop.
#[tauri::command]
pub async fn fix_ci(
    state: State<'_, Arc<AppState>>,
    app_handle: tauri::AppHandle,
    pr: String,
) -> Result<Review, CommandError> {
    let pr_ref = PrRef::new(&pr);
    let review = state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {pr}"),
    })?;

    // On-demand LOG-tier fetch. Best-effort: failure yields empty logs.
    let ci_logs = match pr_number_from_ref(&pr) {
        Some(n) => github::failed_ci_logs(review.repo_slug.as_deref(), n)
            .await
            .unwrap_or_else(|e| {
                eprintln!("fix_ci: failed_ci_logs: {e}");
                String::new()
            }),
        None => String::new(),
    };

    // Ensure the review is InReview before adding the comment + requesting
    // changes. Reworked -> InReview via open(); a wrong starting state surfaces
    // as a transition error below.
    let mut transition_err: Option<cockpit_core::gate::Error> = None;
    state.reviews.update(&pr_ref, |r| {
        // Auto-open from Pending/Reworked so the gate can dispatch. A failing
        // open bails before we touch comments.
        if matches!(r.gate_state, GateState::Pending | GateState::Reworked)
            && let Err(e) = r.open()
        {
            transition_err = Some(e);
            return;
        }

        // Guard: only mutate the review when it is in a state `request_changes`
        // will accept (InReview). Pushing the synthetic comment first and only
        // then discovering the transition is illegal would leave a stray CI
        // comment behind, so bail here without mutating.
        if r.gate_state != GateState::InReview {
            transition_err = Some(cockpit_core::gate::Error::IllegalTransition {
                from: r.gate_state,
                event: "request_changes",
            });
            return;
        }

        // Synthetic Local comment so the gate's ≥1-comment rule is satisfied.
        // Anchored to the PR (no specific line) via a zero-length diff anchor.
        let summary = if ci_logs.trim().is_empty() {
            "CI is failing on this PR. Investigate and fix the failing checks.".to_string()
        } else {
            "CI is failing on this PR. See the CI Failures section for the failed \
             job logs; fix the failing checks."
                .to_string()
        };
        let comment_id = CommentId::new(format!("ci-{}", uuid::Uuid::new_v4()));
        r.comments.push(Comment {
            id: comment_id.clone(),
            anchor: Anchor::DiffLine {
                path: PathBuf::from("CI"),
                range: (0, 0),
                side: DiffSide::New,
            },
            body: summary,
            origin: CommentOrigin::Local,
        });

        // D10: snapshot the pre-rework head + the synthetic CI comment being
        // dispatched, before the gate advances to Dispatched. State is InReview
        // by the guard above.
        r.snapshot_dispatch();

        if let Err(e) = r.request_changes() {
            // Roll back the synthetic comment so a rejected transition never
            // leaves a stray CI comment on the review.
            r.comments.retain(|c| c.id != comment_id);
            transition_err = Some(e);
        }
    });

    if let Some(e) = transition_err {
        return Err(CommandError::from(e));
    }

    // D7: same as request_changes — dispatching rework staled the descendants.
    if let Some(id) = state.reviews.get(&pr_ref).map(|r| r.id) {
        state.reviews.mark_descendants_stale(&id);
    }

    // Reuse the shared Fix-loop spawn path, carrying the CI logs into the
    // rework prompt verbatim.
    let ci_arg = if ci_logs.trim().is_empty() {
        None
    } else {
        Some(ci_logs.as_str())
    };
    dispatch_fix_agent(&state, &app_handle, &pr, &pr_ref, ci_arg).await;

    state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {pr}"),
    })
}

// ---------------------------------------------------------------------------
// Plan gate commands
// ---------------------------------------------------------------------------

/// Return the plan for the given project, if one exists.
#[tauri::command]
pub fn get_plan(
    state: State<'_, Arc<AppState>>,
    project_id: String,
) -> Result<Option<ProjectPlan>, CommandError> {
    Ok(state.projects.plan(&ProjectId::new(&project_id)))
}

/// Add a comment anchored to a plan step or file for the given project's plan.
///
/// The `anchor` string is parsed by `cockpit-core`'s plan anchor parser
/// (format: `"step:N"` or `"file:path"`).
#[tauri::command]
pub fn add_plan_comment(
    state: State<'_, Arc<AppState>>,
    project_id: String,
    anchor: String,
    body: String,
) -> Result<ProjectPlan, CommandError> {
    let id = ProjectId::new(&project_id);
    let parsed_anchor: Anchor = plan_parser::parse_plan_anchor(&anchor)?;
    let comment = Comment {
        id: CommentId::new(uuid::Uuid::new_v4().to_string()),
        anchor: parsed_anchor,
        body,
        origin: CommentOrigin::Local,
    };

    let mut had_plan = false;
    state.projects.update_plan(&id, |slot| {
        if let Some(plan) = slot.as_mut() {
            plan.comments.push(comment);
            had_plan = true;
        }
    });

    if !had_plan {
        return Err(CommandError {
            message: format!("No plan for project: {project_id}"),
        });
    }

    plan_for(&state, &id)
}

/// Return the plan for a project, or a "not found" [`CommandError`].
///
/// Small shared helper: every plan command re-reads the project's plan after
/// mutating it so the frontend receives the current state.
fn plan_for(state: &AppState, id: &ProjectId) -> Result<ProjectPlan, CommandError> {
    state.projects.plan(id).ok_or_else(|| CommandError {
        message: format!("No plan for project: {id}"),
    })
}

/// Best-effort plan-prompt issue lines for a project's Linear issues (E3).
///
/// For a Linear-backed project, fetches its issues and formats each into a
/// bullet line via [`kickoff::plan_issue_line`] — the identifier, title, and a
/// capped description gist (the full description reaches the reviewer via each
/// review's intent; the planner only needs enough to scope the work).
///
/// Returns an empty list for an ad-hoc project, a missing project, no API key,
/// or any Linear failure, so plan generation never blocks on an external
/// round-trip (Invariant 1) — an empty list renders identically to the pre-E3
/// "No issues listed." plan prompt.
async fn plan_issue_lines(state: &Arc<AppState>, id: &ProjectId) -> Vec<String> {
    let Some(project) = state.projects.get(id) else {
        return Vec::new();
    };
    let ProjectSource::Linear(linear_id) = &project.source else {
        return Vec::new();
    };
    let Some(api_key) = Config::load().ok().and_then(|c| c.linear_api_key) else {
        return Vec::new();
    };

    // A stalled Linear API must not hang the caller indefinitely; errors
    // (including timeout) degrade to the empty/default path.
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
    {
        Ok(c) => c,
        Err(_) => reqwest::Client::new(),
    };
    let project_ref = ProjectRef::new(linear_id);
    match linear::fetch_project_issues(&client, &api_key, &project_ref).await {
        Ok(data) => data.issues.iter().map(kickoff::plan_issue_line).collect(),
        Err(e) => {
            eprintln!("generate_plan: Linear issue fetch failed for {linear_id}: {e}");
            Vec::new()
        }
    }
}

/// Generate the plan for a project by spawning a planner agent.
///
/// This is an artifact-filling spawn: it does **not** move the gate. A `Pending`
/// [`ProjectPlan`] is created on the project (if it has none yet), its
/// `plan_path` set to [`config::plan_file_path`], and the planner
/// (`AgentMode::Plan`) is spawned in the repo working directory keyed by the
/// project id so completion routes back to this project. On Stop-hook completion
/// the plan is left `Pending` and ready for `plan_open`. Mirrors how implementers
/// fill a review's diff while the review stays `Pending`.
///
/// Requires a known project and a configured repo path. Spawn failure is
/// surfaced as an error.
#[tauri::command]
pub async fn generate_plan(
    state: State<'_, Arc<AppState>>,
    app_handle: tauri::AppHandle,
    project_id: String,
) -> Result<ProjectPlan, CommandError> {
    let id = ProjectId::new(&project_id);

    if state.projects.get(&id).is_none() {
        return Err(CommandError {
            message: format!("Project not found: {project_id}"),
        });
    }

    // Resolve the on-disk destination the planner writes to, and ensure the
    // parent directory exists so the agent's write succeeds. This path is
    // read back and parsed on completion (see the Plan completion arm). The
    // plan's ProjectRef mirrors the project id so completion can route back.
    let plan_path = cockpit_core::config::plan_file_path(&project_id)?;
    if let Some(parent) = plan_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Ensure a Pending plan exists on the project, recording the write path.
    // An existing plan is reused (its current doc seeds the prompt below).
    let existed = state.projects.update_plan(&id, |slot| {
        let plan = slot.get_or_insert_with(|| ProjectPlan {
            project: ProjectRef::new(&project_id),
            doc: cockpit_core::model::PlanDoc {
                summary: String::new(),
                steps: vec![],
                files: vec![],
                risks: vec![],
                raw: String::new(),
            },
            gate_state: GateState::Pending,
            comments: vec![],
            agent: None,
            plan_path: None,
        });
        plan.plan_path = Some(plan_path.clone());
    });
    if !existed {
        return Err(CommandError {
            message: format!("Project not found: {project_id}"),
        });
    }
    let plan = plan_for(&state, &id)?;

    // Assemble the initial plan-generation prompt (intent = the project goal;
    // no comments — the plan does not exist yet). The Plan-mode custom preamble
    // is injected verbatim; a config load failure falls back to the builtin.
    let preamble = Config::load()
        .ok()
        .and_then(|c| c.agent_prompts.for_mode(AgentMode::Plan).map(str::to_owned));
    let intent = format!("Produce a project plan for {}.", plan.project);
    // Plan generation has no diff yet — surface universal (untagged) skills.
    // Discovery failures are non-fatal.
    let skills = cockpit_core::skills::relevant_for_diff("");
    // Give the planner the project's issues (with a description gist each) when
    // they can be fetched; best-effort, so an ad-hoc project or a Linear failure
    // falls back to an empty list without blocking plan generation (E3).
    let issue_lines = plan_issue_lines(state.inner(), &id).await;
    let plan_input = cockpit_core::prompt::PlanInput {
        intent: &intent,
        custom_preamble: preamble.as_deref(),
        issues: &issue_lines,
        current_plan: Some(&plan.doc),
        output_path: Some(&plan_path),
        skills: &skills,
    };
    let assembled = cockpit_core::prompt::assemble_plan_prompt(&plan_input);

    // The session object_id MUST be the project id so the Plan completion arm
    // updates the right project's plan.
    let run = spawn_plan_agent(&state, &app_handle, &project_id, &assembled).await?;

    // Attach the running agent; the plan stays Pending (artifact-fill).
    state.projects.update_plan(&id, |slot| {
        if let Some(p) = slot.as_mut() {
            p.agent = Some(run);
        }
    });

    plan_for(&state, &id)
}

/// Transition the plan to `Dispatched` and spawn a planner agent for rework.
///
/// Requires that the plan is in `InReview` state with at least one comment.
/// The gate transition happens first; then a planner (`AgentMode::Plan`) is
/// spawned with the rework prompt (approved-plan-absent, artifact = the plan
/// doc, plus the gathered comments) — exactly like `request_changes` does for
/// reviews. Spawn failure is non-fatal: the gate already advanced, so the
/// `Dispatched` plan is returned and the error is logged.
///
/// This is an explicit user action (Invariant 5).
#[tauri::command]
pub async fn plan_request_changes(
    state: State<'_, Arc<AppState>>,
    app_handle: tauri::AppHandle,
    project_id: String,
) -> Result<ProjectPlan, CommandError> {
    let id = ProjectId::new(&project_id);
    let mut plan = plan_for(&state, &id)?;

    plan.request_changes()?;

    // Resolve (and record) the on-disk destination for the revised plan so the
    // completion arm can read + parse it back. Reuse an existing path when the
    // plan already has one; otherwise derive it from the project id.
    let plan_path = match plan.plan_path.clone() {
        Some(p) => p,
        None => cockpit_core::config::plan_file_path(&project_id)?,
    };
    if let Some(parent) = plan_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    plan.plan_path = Some(plan_path.clone());
    let plan_snapshot = plan.clone();
    state.projects.update_plan(&id, |slot| {
        *slot = Some(plan_snapshot);
    });

    // Assemble the rework prompt over the plan artifact + comments. Plan-mode
    // custom preamble injected verbatim (builtin fallback on load failure).
    let preamble = Config::load()
        .ok()
        .and_then(|c| c.agent_prompts.for_mode(AgentMode::Plan).map(str::to_owned));
    // Plan rework operates on the plan doc, not a file diff — surface universal
    // (untagged) skills. Discovery failures are non-fatal.
    let skills = cockpit_core::skills::relevant_for_diff("");
    let artifact = Artifact::Plan(plan.doc.clone());
    // Instruct the planner to persist the revised plan to the recorded path,
    // in the pinned format, so cockpit can parse it back on completion.
    let intent = format!(
        "Revise the project plan for {}. Write the finished plan as markdown to `{}` in the same pinned format.",
        plan.project,
        plan_path.display()
    );
    let rework_input = cockpit_core::prompt::ReworkInput {
        intent: &intent,
        custom_preamble: preamble.as_deref(),
        approved_plan: None,
        artifact: &artifact,
        comments: &plan.comments,
        ci_failures: None,
        skills: &skills,
    };
    let assembled = cockpit_core::prompt::assemble_rework(&rework_input);

    // The session object_id MUST be the project id so completion routes back.
    match spawn_plan_agent(&state, &app_handle, &project_id, &assembled).await {
        Ok(run) => {
            state.projects.update_plan(&id, |slot| {
                if let Some(p) = slot.as_mut() {
                    p.agent = Some(run);
                }
            });
        }
        Err(e) => {
            eprintln!("plan_request_changes: planner spawn failed: {e}");
            // Surface the spawn failure to the agent panel, keyed by the
            // project id so the frontend attributes it to the right plan.
            let error_event = cockpit_core::adapters::agent_stream::Event::Error {
                message: format!("Planner spawn failed: {e}"),
            };
            crate::streaming::emit_agent_event(&app_handle, &project_id, error_event);
        }
    }

    plan_for(&state, &id)
}

/// Spawn a planner agent (`AgentMode::Plan`) in the repo working directory.
///
/// Factored out so both initial generation and rework share one spawn path.
/// Uses the configured repo path as the agent's working directory (the plan
/// is a document produced against the repo). Returns the [`AgentRun`] with
/// stdout streaming wired to the frontend.
async fn spawn_plan_agent(
    state: &AppState,
    app_handle: &tauri::AppHandle,
    object_id: &str,
    prompt: &cockpit_core::prompt::AssembledPrompt,
) -> Result<cockpit_core::model::AgentRun, CommandError> {
    let config = Config::load()?;
    let repo_path = config
        .repo_path
        .clone()
        .unwrap_or_else(|| PathBuf::from("."));
    // The plan document lives outside the worktree; pre-authorize the plans
    // dir so the headless planner's Write isn't silently blocked — both as an
    // accessible dir (`with_extra_dir`) and as a scoped write auto-approval
    // (`allow_write_under`) so the plan write never routes through Approve.
    let plans_dir = cockpit_core::config::plans_dir()?;
    let approve_url = mcp_approve_url(config.hook_port, object_id);
    let spawn_config = SpawnConfig::from_config(&config)
        .apply_permission_mode(config.agent_permission_mode, Some(&approve_url))
        .with_extra_dir(&plans_dir)
        .allow_write_under(&plans_dir);
    let hook_url = format!("http://127.0.0.1:{}/hook/stop", config.hook_port);

    let spawn_result = cockpit_core::adapters::agent::spawn_agent(
        &repo_path,
        prompt,
        cockpit_core::model::AgentMode::Plan,
        object_id,
        &state.sessions,
        &hook_url,
        &spawn_config,
    )
    .await?;

    let stream_ctx = crate::streaming::StreamContext {
        object_id: object_id.to_string(),
        mode: cockpit_core::model::AgentMode::Plan,
        completion_tx: state.completion_tx.clone(),
    };
    Ok(crate::streaming::start_stream_forwarding(
        spawn_result,
        app_handle.clone(),
        stream_ctx,
    ))
}

/// Approve the plan (`InReview` -> `Approved`) and fan out implementers.
///
/// This is the guarded side effect of the plan gate (Invariant 5 / `SPEC.md`
/// §12): it only ever runs from this explicit user command, never from agent
/// output. On approval it spawns one implementer agent (`AgentMode::Implement`)
/// per frontier review of the plan's project — a dedicated worktree each,
/// bounded by `max_parallel_agents`. Each implementer builds the initial code
/// in its worktree; the reviews stay `Pending` (ready for human review) —
/// nothing auto-advances.
///
/// The gate transition happens first and is authoritative. Fan-out failure is
/// reported as an error but the plan remains `Approved`.
#[tauri::command]
pub async fn plan_approve(
    state: State<'_, Arc<AppState>>,
    app_handle: tauri::AppHandle,
    project_id: String,
) -> Result<ProjectPlan, CommandError> {
    let id = ProjectId::new(&project_id);
    let mut plan = plan_for(&state, &id)?;

    plan.approve()?;
    let approved = plan.clone();
    state.projects.update_plan(&id, |slot| {
        *slot = Some(approved);
    });

    // D8: prepare worktrees synchronously (git2 is not `Send`), then fan out
    // implementers on a background task so this command returns the Approved
    // plan immediately instead of blocking until every implementer finishes.
    // The approval already stands; a fan-out failure is surfaced only via the
    // agent-event streams and never rolls back the (authoritative) gate
    // transition (Invariant 1/5).
    let state_arc: Arc<AppState> = state.inner().clone();
    spawn_background_fan_out(state_arc, &app_handle, &plan.project, &id, plan.doc.clone());

    plan_for(&state, &id)
}

/// Prepare worktrees and launch the background implementer fan-out for a
/// project's frontier reviews after plan approval (D8).
///
/// Worktree creation needs the non-`Send` `git2::Repository`, so it runs
/// synchronously here (the repo is dropped before the task spawn). The bounded
/// agent fan-out then runs on a background task ([`run_fan_out`]) so the calling
/// command does not block. Every failure mode is non-fatal: the plan approval is
/// authoritative and already applied, so a prep failure is logged and surfaced
/// as a per-project agent-event rather than rolled back (Invariant 1/5).
fn spawn_background_fan_out(
    state: Arc<AppState>,
    app_handle: &tauri::AppHandle,
    project: &ProjectRef,
    project_id: &ProjectId,
    approved_plan: PlanDoc,
) {
    let config = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("plan_approve fan-out: config load failed: {e}");
            emit_fan_out_error(app_handle, project_id, &format!("config load failed: {e}"));
            return;
        }
    };
    let repo_path = config
        .repo_path
        .clone()
        .unwrap_or_else(|| PathBuf::from("."));
    // Implement-mode custom preamble, injected verbatim into every implementer
    // prompt (builtin fallback when unset).
    let implement_preamble = config
        .agent_prompts
        .for_mode(AgentMode::Implement)
        .map(str::to_owned);

    // Collect this project's reviews, then narrow to the frontier (roots).
    let mut reviews = cockpit_core::store::reviews_by_project(&state.reviews, Some(project_id));
    let frontier_ids = kickoff::select_frontier_reviews(&reviews);
    reviews.retain(|r| frontier_ids.contains(&r.id));
    if reviews.is_empty() {
        // Nothing to build (e.g. a plan-only project); approval already stands.
        return;
    }

    // Phase 1 (synchronous, non-Send git2): create the worktrees and record each
    // review's base_sha. The prompts `prepare_batch_worktrees` builds carry no
    // plan, so they are discarded — the per-review prompts are rebuilt below to
    // thread the approved plan in.
    {
        let repo = match git2::Repository::discover(&repo_path) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("plan_approve fan-out: could not open git repo: {e}");
                emit_fan_out_error(
                    app_handle,
                    project_id,
                    &format!("could not open git repo: {e}"),
                );
                return;
            }
        };
        if let Err(e) = kickoff::prepare_batch_worktrees(
            &mut reviews,
            &repo,
            project,
            implement_preamble.as_deref(),
        ) {
            eprintln!("plan_approve fan-out: prepare worktrees failed: {e}");
            emit_fan_out_error(
                app_handle,
                project_id,
                &format!("prepare worktrees failed: {e}"),
            );
            return;
        }
    }

    // Persist each review's base_sha now that its worktree exists.
    for review in &reviews {
        state.reviews.update(&review.pr, |r| {
            r.base_sha = review.base_sha.clone();
        });
    }

    // Build the per-review implement prompt + spawn job, threading the approved
    // plan into each (SPEC §9). Reuses kickoff's exact prompt text.
    let jobs: Vec<(Review, cockpit_core::prompt::AssembledPrompt)> = reviews
        .iter()
        .map(|review| {
            let prompt = kickoff::assemble_implement_prompt(
                review,
                project,
                Some(&approved_plan),
                implement_preamble.as_deref(),
            );
            (review.clone(), prompt)
        })
        .collect();

    // Base spawn config without a permission mode: the per-review approve URL
    // (keyed by each review's PR ref) is applied inside `run_fan_out`, since one
    // config is reused across reviews with distinct object ids. The Stop-hook
    // URL is derived from `hook_port` inside `run_fan_out`.
    let spawn_config = SpawnConfig::from_config(&config);
    let hook_port = config.hook_port;
    let permission_mode = config.agent_permission_mode;
    let max_parallel = config.max_parallel_agents.max(1) as usize;
    let app_handle = app_handle.clone();

    // Phase 2 (async, no repo handle in scope): bounded agent fan-out.
    tauri::async_runtime::spawn(async move {
        run_fan_out(
            state,
            app_handle,
            jobs,
            max_parallel,
            spawn_config,
            hook_port,
            permission_mode,
        )
        .await;
    });
}

/// Run the bounded implementer fan-out in waves of at most `max_parallel` agents.
///
/// Each review is spawned via the same streaming path the diff-gate Fix loop
/// uses (`spawn_agent` + `start_stream_forwarding`), keyed by its PR ref, with
/// its running agent recorded in the store as it spawns. A wave is not started
/// until the previous wave's agents have completed — completions arrive on the
/// broadcast channel that `start_stream_forwarding` fires when each process
/// exits — which is the concurrency bound. A spawn failure is surfaced as a
/// per-review agent-event and simply not awaited (Invariant 1). The reviews stay
/// `Pending`; nothing auto-advances (Invariant 5).
async fn run_fan_out(
    state: Arc<AppState>,
    app_handle: tauri::AppHandle,
    jobs: Vec<(Review, cockpit_core::prompt::AssembledPrompt)>,
    max_parallel: usize,
    spawn_config: SpawnConfig,
    hook_port: u16,
    permission_mode: AgentPermissionMode,
) {
    use tokio::sync::broadcast::error::RecvError;

    let hook_url = format!("http://127.0.0.1:{hook_port}/hook/stop");

    // Subscribe before spawning so no completion can be missed.
    let mut completions = state.completion_tx.subscribe();

    for wave in jobs.chunks(max_parallel) {
        // PR refs we must see complete before starting the next wave.
        let mut pending: HashSet<String> = HashSet::new();

        for (review, prompt) in wave {
            // Per-review permission wiring: route this implementer's prompts to
            // cockpit's MCP `approve` tool keyed by its own PR ref. No blanket
            // write allow — implementers edit inside their own worktree (cwd),
            // and those edits empirically pass under the default (Approve)
            // policy without routing through the Approve queue.
            let approve_url = mcp_approve_url(hook_port, review.pr.as_str());
            let review_spawn_config = spawn_config
                .clone()
                .apply_permission_mode(permission_mode, Some(&approve_url));
            let spawn_result = cockpit_core::adapters::agent::spawn_agent(
                &review.worktree,
                prompt,
                AgentMode::Implement,
                review.pr.as_str(),
                &state.sessions,
                &hook_url,
                &review_spawn_config,
            )
            .await;

            match spawn_result {
                Ok(spawn_result) => {
                    let stream_ctx = crate::streaming::StreamContext {
                        object_id: review.pr.as_str().to_string(),
                        mode: AgentMode::Implement,
                        completion_tx: state.completion_tx.clone(),
                    };
                    let run = crate::streaming::start_stream_forwarding(
                        spawn_result,
                        app_handle.clone(),
                        stream_ctx,
                    );
                    state.reviews.update(&review.pr, |r| {
                        r.agent = Some(run);
                    });
                    pending.insert(review.pr.as_str().to_string());
                }
                Err(e) => {
                    eprintln!("plan_approve fan-out: spawn failed for {}: {e}", review.pr);
                    let error_event = cockpit_core::adapters::agent_stream::Event::Error {
                        message: format!("Implementer spawn failed: {e}"),
                    };
                    crate::streaming::emit_agent_event(
                        &app_handle,
                        review.pr.as_str(),
                        error_event,
                    );
                }
            }
        }

        // Wait for this wave's implementers to finish before starting the next.
        //
        // Completions arrive on a shared, bounded broadcast that can drop
        // messages: a `Lagged` error means we already missed at least one, and a
        // completion can even be missed with no detected lag (another subscriber
        // drains its slot first). Either way a PR could sit in `pending` forever
        // and stall every later wave. The store — not the channel — is the
        // durable source of truth for "still building": the global completion
        // consumer clears a review's `agent` handle once it applies the
        // completion. So on detected lag reconcile `pending` against the store,
        // and add a periodic safety tick so even a silently-missed completion
        // eventually unblocks the wave.
        let mut safety_tick = tokio::time::interval(std::time::Duration::from_secs(15));
        // The first tick fires immediately; skip it so we do not reconcile before
        // any agent has had a chance to run.
        safety_tick.tick().await;

        while !pending.is_empty() {
            tokio::select! {
                recv = completions.recv() => match recv {
                    Ok(event) => {
                        if event.mode == AgentMode::Implement {
                            pending.remove(&event.object_id);
                        }
                    }
                    // Detected lag: at least one completion was dropped.
                    // Reconcile so a lost completion cannot pin a PR in `pending`.
                    Err(RecvError::Lagged(_)) => reconcile_pending(&state, &mut pending),
                    // Channel closed (app shutting down): stop the fan-out.
                    Err(RecvError::Closed) => return,
                },
                // Safety tick: catch completions missed without a detected lag.
                _ = safety_tick.tick() => reconcile_pending(&state, &mut pending),
            }
        }
    }
}

/// Drop from `pending` any PR whose review no longer has a running agent (or no
/// longer exists), reconciling the wave gate against the store.
///
/// [`run_fan_out`] waits on a shared, bounded completion broadcast that can drop
/// messages, so it cannot rely on seeing every completion. The store's `agent`
/// handle — cleared by the global completion consumer once a completion is
/// applied — is the durable signal for whether an implementer is still running,
/// so it is what the wave gate reconciles against.
fn reconcile_pending(state: &AppState, pending: &mut HashSet<String>) {
    pending.retain(|pr| {
        state
            .reviews
            .get(&PrRef::new(pr.as_str()))
            .is_some_and(|review| review.agent.is_some())
    });
}

/// Surface an implementer fan-out preparation failure to the project's agent
/// panel, keyed by the project id so the frontend attributes it correctly.
fn emit_fan_out_error(app_handle: &tauri::AppHandle, project_id: &ProjectId, message: &str) {
    let event = cockpit_core::adapters::agent_stream::Event::Error {
        message: format!("Implementer fan-out failed: {message}"),
    };
    crate::streaming::emit_agent_event(app_handle, project_id.as_str(), event);
}

/// Return the [`BatchStatus`] for a project's reviews.
///
/// Aggregates the project's reviews into building / ready / approved counts so
/// the frontend can show per-project batch progress after a fan-out without
/// polling each review individually.
#[tauri::command]
pub fn batch_status(
    state: State<'_, Arc<AppState>>,
    project_id: String,
) -> Result<cockpit_core::store::BatchStatus, CommandError> {
    let id = ProjectId::new(&project_id);
    Ok(cockpit_core::store::batch_status(&state.reviews, Some(&id)))
}

/// Open the given project's plan for review (`Pending | Reworked` -> `InReview`).
#[tauri::command]
pub fn plan_open(
    state: State<'_, Arc<AppState>>,
    project_id: String,
) -> Result<ProjectPlan, CommandError> {
    let id = ProjectId::new(&project_id);
    let mut plan = plan_for(&state, &id)?;

    plan.open()?;
    let opened = plan;
    state.projects.update_plan(&id, |slot| {
        *slot = Some(opened);
    });

    plan_for(&state, &id)
}

/// Approve a single review by PR reference string (`InReview` -> `Approved`).
///
/// If the review is in `Reworked` state, it is first opened to `InReview`
/// before approving. The frontend calls this per review as an explicit user
/// action (Invariant 5: side effects require explicit confirmation).
#[tauri::command]
pub fn approve_review(state: State<'_, Arc<AppState>>, pr: String) -> Result<Review, CommandError> {
    let pr_ref = PrRef::new(&pr);

    let mut transition_err: Option<cockpit_core::gate::Error> = None;
    let found = state.reviews.update(&pr_ref, |review| {
        // Transition Reworked -> InReview first if needed.
        if review.gate_state == GateState::Reworked
            && let Err(e) = review.open()
        {
            transition_err = Some(e);
            return;
        }
        if let Err(e) = review.approve() {
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

/// Merge an approved review's PR (`Approved` -> `Merged`) and GC its worktree.
///
/// This is a guarded side effect (Invariant 5 / `SPEC.md` §9): it only ever
/// runs from this explicit user command, never automatically or from agent
/// output. It requires the review to be [`GateState::Approved`] and refuses
/// [`ReviewSource::ReviewRequested`] PRs — cockpit merges the user's own work,
/// not teammates' review requests.
///
/// On a successful `gh pr merge` (squash) it advances the gate to `Merged`,
/// marks the review's descendants stale (they now sit on an old base and need a
/// restack), then GCs the worktree — but only when it lives under the
/// cockpit-managed worktrees directory, never the user's main checkout. The
/// prune is best-effort: a failure is logged, not surfaced as an error
/// (Invariant 1).
#[tauri::command]
pub async fn merge_review(
    state: State<'_, Arc<AppState>>,
    pr: String,
) -> Result<Review, CommandError> {
    let pr_ref = PrRef::new(&pr);
    let review = state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {pr}"),
    })?;

    // Guard: only an approved review may merge.
    if review.gate_state != GateState::Approved {
        return Err(CommandError {
            message: format!(
                "Review {pr} is not approved (state: {:?}); approve it before merging",
                review.gate_state
            ),
        });
    }

    // Guard: never merge a teammate's review-requested PR.
    if review.source == ReviewSource::ReviewRequested {
        return Err(CommandError {
            message: format!(
                "Review {pr} is a review-requested PR; cockpit does not merge teammates' PRs"
            ),
        });
    }

    let Some(pr_number) = pr_number_from_ref(&pr) else {
        return Err(CommandError {
            message: format!("Could not parse a PR number from: {pr}"),
        });
    };

    // Guarded side effect: merge via `gh` (squash). A failure here leaves the
    // review Approved and is surfaced to the caller.
    github::merge_pr(
        review.repo_slug.as_deref(),
        pr_number,
        github::MergeMethod::Squash,
    )
    .await?;

    // Advance the gate to Merged (Approved -> Merged is the only legal edge).
    let mut transition_err: Option<cockpit_core::gate::Error> = None;
    state.reviews.update(&pr_ref, |r| {
        if let Err(e) = r.mark_merged() {
            transition_err = Some(e);
        }
    });
    if let Some(e) = transition_err {
        return Err(CommandError::from(e));
    }

    // Descendants now sit on an old base — mark them stale so the UI prompts a
    // restack.
    state.reviews.mark_descendants_stale(&review.id);

    // Worktree GC: prune the merged review's worktree, but ONLY when it lives
    // under the cockpit-managed worktrees dir (never the user's main checkout).
    // Best-effort (Invariant 1): any failure is logged, never surfaced.
    prune_merged_worktree(&pr, &review.worktree, &review.branch);

    state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {pr}"),
    })
}

/// Best-effort GC of a merged review's git worktree.
///
/// Only prunes when `worktree` is under [`cockpit_core::config::worktrees_dir`]
/// so the user's main checkout is never touched. `git2::Repository` is not
/// `Send`; this helper is synchronous and holds no handle across an `.await`,
/// so callers must invoke it outside any await span. Every failure mode (no
/// worktrees dir, worktree outside it, repo won't open, prune errors) is logged
/// and swallowed — merge already succeeded, so GC must never fail the command.
fn prune_merged_worktree(pr: &str, worktree: &std::path::Path, branch: &str) {
    let Ok(worktrees_dir) = cockpit_core::config::worktrees_dir() else {
        eprintln!("merge_review: could not resolve worktrees dir; skipping prune for {pr}");
        return;
    };
    if !worktree.starts_with(&worktrees_dir) {
        // Not a cockpit-managed worktree (e.g. an imported PR pointing at the
        // main checkout) — leave it alone.
        return;
    }

    let repo_path = Config::load()
        .ok()
        .and_then(|c| c.repo_path)
        .unwrap_or_else(|| PathBuf::from("."));
    match git2::Repository::discover(&repo_path) {
        Ok(repo) => {
            if let Err(e) = cockpit_core::adapters::git::prune_worktree(&repo, branch) {
                eprintln!("merge_review: prune_worktree failed for {pr}: {e}");
            }
        }
        Err(e) => {
            eprintln!(
                "merge_review: could not open repo at {} for prune: {e}",
                repo_path.display()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Agent control
// ---------------------------------------------------------------------------

/// Kill the running agent attached to a review (D11).
///
/// Sends SIGTERM to the agent process, removes its session from the session map
/// so a straggling Stop-hook / stream-end completion cannot double-fire, then
/// applies a no-progress completion: the review returns to `InReview` with its
/// comments preserved and the agent handle cleared (git HEAD, not the killed
/// agent, is authoritative — Invariant 4). This is an explicit user action.
///
/// The killed process still emits a stream-end completion when it exits; the
/// completion handler tolerates the review already being non-`Dispatched` and
/// settles it without an illegal transition (see `reconcile_fix_completion`).
#[tauri::command]
pub async fn kill_agent(
    state: State<'_, Arc<AppState>>,
    pr: String,
) -> Result<Review, CommandError> {
    let pr_ref = PrRef::new(&pr);
    let review = state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {pr}"),
    })?;

    let pid = review
        .agent
        .as_ref()
        .map(|a| a.pid)
        .ok_or_else(|| CommandError {
            message: format!("Review {pr} has no running agent to kill"),
        })?;

    // Send SIGTERM to the agent process.
    cockpit_core::adapters::agent::kill_agent(pid).await?;

    // Remove the session so a straggling completion cannot double-fire against a
    // review we are about to return to InReview. The fix path keys sessions by
    // PR ref; fall back to the review id for defensiveness.
    let session_id = state
        .sessions
        .find_by_object(pr_ref.as_str())
        .or_else(|| state.sessions.find_by_object(review.id.as_str()));
    if let Some(sid) = session_id {
        state.sessions.remove(&sid);
    }

    // Apply a no-progress completion. On a Dispatched review this returns it to
    // InReview (comments preserved, agent cleared); for a non-Dispatched agent
    // (e.g. a Pending implementer) the transition is a no-op but the agent handle
    // is still cleared, which is what the UI needs — so the transition error is
    // ignored deliberately.
    state.reviews.update(&pr_ref, |r| {
        let _ = r.apply_agent_completion(None);
    });

    state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {pr}"),
    })
}

// ---------------------------------------------------------------------------
// Settings / Config commands
// ---------------------------------------------------------------------------

/// Load the application configuration from `~/.cockpit/config.toml`.
///
/// Returns the default configuration if the file does not exist.
#[tauri::command]
pub fn get_config() -> Result<Config, CommandError> {
    let config = Config::load()?;
    Ok(config)
}

/// Save the application configuration to `~/.cockpit/config.toml`.
///
/// Creates the `~/.cockpit/` directory if it does not already exist.
#[tauri::command]
pub fn save_config(config: Config) -> Result<(), CommandError> {
    config.save()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Agent prompt customization
// ---------------------------------------------------------------------------

/// Return the stored custom prompt override for `mode`, if any.
///
/// `None` means the mode uses its builtin default (see
/// [`get_builtin_agent_prompt`] for that text). Thin: reads config and returns
/// the [`AgentPrompts`](cockpit_core::config::AgentPrompts) entry.
#[tauri::command]
pub fn get_agent_prompt(mode: AgentMode) -> Result<Option<String>, CommandError> {
    let config = Config::load()?;
    Ok(config.agent_prompts.for_mode(mode).map(str::to_owned))
}

/// Return the builtin default prompt fragment for `mode`.
///
/// Used by the settings editor as the placeholder and the "reset to default"
/// value. This is the canonical builtin intent, never a stored override.
#[tauri::command]
pub fn get_builtin_agent_prompt(mode: AgentMode) -> Result<String, CommandError> {
    Ok(cockpit_core::prompt::builtin_intent(mode).to_string())
}

/// Persist a custom prompt override for `mode`.
///
/// An empty or whitespace-only `text` clears the override, resetting the mode
/// to its builtin default. The override is injected verbatim into that mode's
/// prompt at dispatch time.
#[tauri::command]
pub fn save_agent_prompt(mode: AgentMode, text: String) -> Result<(), CommandError> {
    let mut config = Config::load()?;
    config.agent_prompts.set_mode(mode, Some(text));
    config.save()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Skills commands
// ---------------------------------------------------------------------------

/// List installed skills from `<cockpit_home>/skills`.
///
/// Thin: delegates to [`cockpit_core::skills::discover_installed_skills`].
#[tauri::command]
pub fn list_skills() -> Result<Vec<cockpit_core::skills::Skill>, CommandError> {
    let skills = cockpit_core::skills::discover_installed_skills()?;
    Ok(skills)
}

/// Install or overwrite a local skill by name.
///
/// Writes `SKILL.md` (+ `.meta.json`) marking the skill [`SkillSource::Local`]
/// so a later GitHub sync never clobbers a hand edit. Explicit user action.
#[tauri::command]
pub fn save_skill(name: String, contents: String) -> Result<(), CommandError> {
    cockpit_core::skills::install_skill(
        &name,
        &contents,
        cockpit_core::skills::SkillSource::Local,
    )?;
    Ok(())
}

/// Delete an installed skill by name (Invariant 5: explicit user action).
#[tauri::command]
pub fn delete_skill(name: String) -> Result<(), CommandError> {
    cockpit_core::skills::delete_skill(&name)?;
    Ok(())
}

/// Sync skills from the configured GitHub source via the `gh` CLI.
///
/// Requires `[skills_github]` in config. Uses the user's `gh auth` (no PAT).
/// Returns a [`SyncReport`](cockpit_core::skills::SyncReport) of counts.
#[tauri::command]
pub async fn sync_skills() -> Result<cockpit_core::skills::SyncReport, CommandError> {
    let config = Config::load()?;
    let source = config.skills_github.ok_or_else(|| CommandError {
        message: "No skills GitHub source configured. Set it in Settings.".into(),
    })?;
    let report = cockpit_core::skills::sync_from_github(
        &source.owner,
        &source.repo,
        &source.branch,
        &source.path,
    )
    .await?;
    Ok(report)
}

// ---------------------------------------------------------------------------
// Kickoff command
// ---------------------------------------------------------------------------

/// Kick off a Linear project: fetch issues, optionally plan, then create
/// reviews for each frontier issue.
///
/// This is an explicit user action (Invariant 5). If `skip_plan` is false,
/// a project plan is created in `Pending` state for the user to review
/// before the batch is spawned.
///
/// Returns a [`KickoffResult`] with the created reviews and frontier.
#[tauri::command]
pub async fn kickoff(
    state: State<'_, Arc<AppState>>,
    project_id: String,
    skip_plan: bool,
) -> Result<KickoffResult, CommandError> {
    let config = Config::load()?;

    let api_key = config.linear_api_key.ok_or_else(|| CommandError {
        message: "Linear API key not configured. Set it in Settings.".into(),
    })?;

    let project = ProjectRef::new(&project_id);
    // A stalled Linear API must not hang the caller indefinitely; errors
    // (including timeout) degrade to the empty/default path.
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
    {
        Ok(c) => c,
        Err(_) => reqwest::Client::new(),
    };

    // 1. Fetch issues and compute the frontier.
    let (data, frontier) = kickoff::fetch_and_compute_frontier(&client, &api_key, &project).await?;

    if frontier.is_empty() {
        return Err(CommandError::from(kickoff::Error::EmptyFrontier));
    }

    // 2. Build the issue DAG for parent/child wiring.
    let issue_dag = linear::build_issue_dag(&data);

    // 3. Build the first-class Linear-backed project that groups the reviews;
    //    worktrees live under the cockpit home (outside the managed repo) and
    //    are keyed via the unified `review_worktree_path` scheme so projects
    //    never collide.
    let mut cockpit_project = kickoff::project_from_linear(&project, format!("Project {project}"));

    // 4. Handle plan gate decision: attach a Pending plan to the project itself
    //    (per-project plan scoping), not a global slot.
    if !skip_plan {
        cockpit_project.plan = Some(ProjectPlan {
            project: project.clone(),
            doc: cockpit_core::model::PlanDoc {
                summary: format!("Plan for project {project}"),
                steps: vec![],
                files: vec![],
                risks: vec![],
                raw: String::new(),
            },
            gate_state: GateState::Pending,
            comments: vec![],
            agent: None,
            plan_path: None,
        });
    }

    let mut reviews = kickoff::build_reviews_for_frontier(
        &frontier,
        &data,
        &issue_dag,
        "main",
        Some(&cockpit_project.id),
    );
    for review in &mut reviews {
        review.worktree = kickoff::review_worktree_path(review)?;
    }

    // 5. Store the project and its reviews in the in-memory stores. Snapshot the
    //    project's plan first (it moves into the store on insert).
    let plan = cockpit_project.plan.clone();
    state.projects.insert(cockpit_project);
    for review in &reviews {
        state.reviews.insert(review.clone());
    }

    let result = KickoffResult {
        reviews,
        plan,
        issue_count: data.issues.len(),
        frontier,
    };

    Ok(result)
}

// ---------------------------------------------------------------------------
// Project commands
// ---------------------------------------------------------------------------

/// List all first-class projects currently in the store.
#[tauri::command]
pub fn list_projects(state: State<'_, Arc<AppState>>) -> Result<Vec<Project>, CommandError> {
    Ok(state.projects.list())
}

/// Create a new ad-hoc project with the given name.
///
/// This is an explicit user action (Invariant 5): ad-hoc projects only ever
/// come from a deliberate UI action, never from agent output. Returns the
/// created [`Project`].
#[tauri::command]
pub fn create_project(
    state: State<'_, Arc<AppState>>,
    name: String,
) -> Result<Project, CommandError> {
    let project = kickoff::create_ad_hoc_project(name);
    state.projects.insert(project.clone());
    Ok(project)
}

/// Attach an existing review to a project.
///
/// Looks up the review by PR reference and sets its `project` field. Returns
/// the updated [`Review`]. Errors if either the review or the project is
/// unknown.
#[tauri::command]
pub fn attach_review(
    state: State<'_, Arc<AppState>>,
    pr: String,
    project_id: String,
) -> Result<Review, CommandError> {
    let pr_ref = PrRef::new(&pr);
    let project_id = ProjectId::new(&project_id);

    if state.projects.get(&project_id).is_none() {
        return Err(CommandError {
            message: format!("Project not found: {project_id}"),
        });
    }

    let updated = state.reviews.update(&pr_ref, |r| {
        r.project = Some(project_id.clone());
    });
    if !updated {
        return Err(CommandError {
            message: format!("Review not found: {pr}"),
        });
    }

    state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found after attach: {pr}"),
    })
}

// ---------------------------------------------------------------------------
// Restack command
// ---------------------------------------------------------------------------

/// Outcome of restacking a single review via [`restack_one`].
///
/// Drives the per-child progress event and the halt decision in
/// [`restack_stack`]: a [`RestackStep::Conflict`] hands the branch to a
/// conflict-resolver agent and stops the sequence, while [`RestackStep::Clean`]
/// lets it continue to the next descendant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RestackStep {
    /// Rebase completed cleanly; the review's stale flag was cleared.
    Clean,
    /// Rebase hit conflicts; the conflict-resolver agent was dispatched.
    Conflict,
}

/// Restack one review onto `parent_branch`, dispatching the conflict-resolver on
/// failure, and persist the result to the store.
///
/// The shared core of both [`restack_pr`] (single review) and [`restack_stack`]
/// (whole dependency-ordered stack). The git work runs synchronously (via
/// [`restack::restack_review`]) and the `git2::Repository` — which is not `Send`
/// — is dropped before any `.await`, so this future stays `Send` and can be
/// awaited from a spawned background task.
///
/// On a clean rebase the stale flag is cleared and `base_sha` advances; on
/// conflict the conflict-resolver agent (`AgentMode::Restack`) is spawned in the
/// review's worktree with stdout streamed to the frontend, keyed by the PR ref
/// so the Restack completion handler matches it. Either way `base_sha`, `stale`,
/// and the agent handle are written back to the store.
async fn restack_one(
    state: &AppState,
    app_handle: &tauri::AppHandle,
    config: &Config,
    repo_path: &Path,
    pr_ref: &PrRef,
    parent_branch: &str,
) -> Result<RestackStep, CommandError> {
    let mut review = state.reviews.get(pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {pr_ref}"),
    })?;

    // Phase 1: synchronous git restack. git2::Repository is not Send, so we
    // must not hold it across an .await point.
    let repo = git2::Repository::discover(repo_path).map_err(|e| CommandError {
        message: format!(
            "not inside a git repository at {}: {e}",
            repo_path.display()
        ),
    })?;

    let clean =
        restack::restack_review(&repo, &mut review, parent_branch).map_err(|e| CommandError {
            message: format!("restack failed: {e}"),
        })?;

    // Drop the repo before any .await to satisfy Send requirements.
    drop(repo);

    // Phase 2: if conflicts, spawn the conflict-resolver agent (async).
    if !clean {
        // Apply the permission mode, routing prompts to cockpit's MCP `approve`
        // tool keyed by this review's PR ref. No blanket write allow: the
        // resolver edits inside its own worktree (cwd), and those edits
        // empirically pass under the default (Approve) policy.
        let approve_url = mcp_approve_url(config.hook_port, review.pr.as_str());
        let spawn_config = SpawnConfig::from_config(config)
            .apply_permission_mode(config.agent_permission_mode, Some(&approve_url));
        let hook_url = format!("http://127.0.0.1:{}/hook/stop", config.hook_port);
        let worktree_path = review.worktree.clone();

        // Restack-mode custom preamble, injected verbatim (builtin fallback).
        let preamble = config
            .agent_prompts
            .for_mode(AgentMode::Restack)
            .map(str::to_owned);
        let prompt = restack::assemble_conflict_prompt(&review, parent_branch, preamble.as_deref());
        // Key the session + stream by the PR ref (not the ReviewId): the
        // Restack completion handler resolves reviews by PrRef, so keying by
        // ReviewId here would leave restack completions unmatched. Mirrors the
        // Fix path (see `try_spawn_agent`).
        let spawn_result = cockpit_core::adapters::agent::spawn_agent(
            &worktree_path,
            &prompt,
            AgentMode::Restack,
            review.pr.as_str(),
            &state.sessions,
            &hook_url,
            &spawn_config,
        )
        .await
        .map_err(|e| CommandError {
            message: format!("failed to spawn conflict-resolver agent: {e}"),
        })?;

        // Start streaming agent stdout to the frontend.
        let stream_ctx = crate::streaming::StreamContext {
            object_id: review.pr.as_str().to_string(),
            mode: AgentMode::Restack,
            completion_tx: state.completion_tx.clone(),
        };
        let agent_run =
            crate::streaming::start_stream_forwarding(spawn_result, app_handle.clone(), stream_ctx);
        review.agent = Some(agent_run);
    }

    // Persist the updated review back to the in-memory store.
    let review_clone = review.clone();
    state.reviews.update(pr_ref, |r| {
        r.base_sha = review_clone.base_sha.clone();
        r.stale = review_clone.stale;
        r.agent = review_clone.agent.clone();
    });

    Ok(if clean {
        RestackStep::Clean
    } else {
        RestackStep::Conflict
    })
}

/// Restack a stale PR onto its parent's new head.
///
/// If the rebase is clean, clears the stale flag and returns the updated
/// review. If there are conflicts, spawns the conflict-resolver agent and
/// returns the review with the agent run attached.
///
/// This is an explicit user action (Invariant 5).
///
/// Delegates the git + spawn work to the shared [`restack_one`] helper, which
/// runs the git operations synchronously before any async agent spawn so that
/// `git2::Repository` (not `Send`) never lives across an `.await` boundary.
#[tauri::command]
pub async fn restack_pr(
    state: State<'_, Arc<AppState>>,
    app_handle: tauri::AppHandle,
    pr: String,
) -> Result<Review, CommandError> {
    let pr_ref = PrRef::new(&pr);

    let review = state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {pr}"),
    })?;

    if !review.stale {
        return Err(CommandError {
            message: format!("PR {pr} is not stale; nothing to restack"),
        });
    }

    let config = Config::load()?;
    let repo_path = config
        .repo_path
        .clone()
        .unwrap_or_else(|| PathBuf::from("."));
    let parent_branch = review.base.clone();

    restack_one(
        &state,
        &app_handle,
        &config,
        &repo_path,
        &pr_ref,
        &parent_branch,
    )
    .await?;

    state.reviews.get(&pr_ref).ok_or_else(|| CommandError {
        message: format!("Review not found after restack: {pr}"),
    })
}

/// Progress event payload for a [`restack_stack`] run.
///
/// Emitted on the hand-typed `"restack-progress"` Tauri event so the frontend
/// can track a whole-stack restack live. `current`/`total` are 1-based over the
/// dependency-ordered descendants; `status` is `"restacking"` before a child,
/// its outcome afterwards (`"clean"`/`"conflict"`/`"error"`), and `"done"` once
/// the entire sequence completes cleanly. Hand-typed on the frontend (no
/// `ts-rs`).
#[derive(Debug, Clone, serde::Serialize)]
struct RestackProgressPayload {
    /// PR ref of the stack root the run was requested for.
    root_pr: String,
    /// 1-based index of the child currently being restacked (0 on a load error).
    current: u32,
    /// Total number of descendants in the restack order.
    total: u32,
    /// PR ref of the child this event is about (empty on the final `"done"`).
    current_pr: String,
    /// Phase of the run: `restacking` | `clean` | `conflict` | `done` | `error`.
    status: &'static str,
    /// Human-readable reason for a halt (currently only the TOCTOU agent-guard
    /// `"error"`); omitted from the wire when absent so the optional frontend
    /// field stays `undefined` rather than `null`.
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

/// Emit a `"restack-progress"` event, swallowing any emit failure.
///
/// `detail` carries an optional human-readable reason (used by the TOCTOU
/// agent-guard halt); pass `None` for the ordinary progress transitions.
fn emit_restack_progress(
    app_handle: &tauri::AppHandle,
    root_pr: &str,
    current: u32,
    total: u32,
    current_pr: &str,
    status: &'static str,
    detail: Option<String>,
) {
    use tauri::Emitter;
    let payload = RestackProgressPayload {
        root_pr: root_pr.to_string(),
        current,
        total,
        current_pr: current_pr.to_string(),
        status,
        detail,
    };
    let _ = app_handle.emit("restack-progress", &payload);
}

/// Restack a whole stack: rebase every descendant of `root_pr` onto its parent
/// in dependency order, one at a time (D3).
///
/// This is an explicit user action (Invariant 5). It computes the
/// dependency-ordered descendants of the root (via [`restack::dependency_order`],
/// scoped to the root's project like the fan-out) and drives them SEQUENTIALLY
/// on a background task, emitting `"restack-progress"` events as it goes. The
/// command itself returns immediately once the sequence is launched.
///
/// Guards (typed errors, checked before launching):
/// - an empty order (the root has no descendants) — nothing to restack;
/// - any review in the order already has an agent attached — someone is
///   mid-rework and restacking underneath them would clobber their branch.
///
/// Per-child semantics mirror [`restack_pr`]: a clean rebase clears stale and
/// the sequence continues; a conflict dispatches the conflict-resolver agent and
/// HALTS the sequence (the remaining descendants stay stale until the resolver
/// lands and the user restacks again); a hard error also halts.
#[tauri::command]
pub async fn restack_stack(
    state: State<'_, Arc<AppState>>,
    app_handle: tauri::AppHandle,
    root_pr: String,
) -> Result<(), CommandError> {
    let root_ref = PrRef::new(&root_pr);
    let root = state.reviews.get(&root_ref).ok_or_else(|| CommandError {
        message: format!("Review not found: {root_pr}"),
    })?;

    // Scope the review set to the root's project (as the fan-out does) and
    // compute the dependency-ordered descendants.
    let reviews = cockpit_core::store::reviews_by_project(&state.reviews, root.project.as_ref());
    let order = restack::dependency_order(&reviews, &root.id);

    if order.is_empty() {
        return Err(CommandError {
            message: format!("Stack rooted at {root_pr} has nothing to restack"),
        });
    }

    // Guard: refuse if the root itself is mid-rework. The root is NOT part of
    // `order` (only its descendants are), so it needs its own check —
    // restacking descendants onto a root whose branch an agent is actively
    // rewriting would build on an unstable base.
    if root.agent.is_some() {
        return Err(CommandError {
            message: format!(
                "Stack root {root_pr} is mid-rework (agent attached); wait for it to finish before restacking the stack"
            ),
        });
    }

    // Guard: refuse if any review in the order is mid-rework. Restacking a branch
    // out from under a running agent would clobber its work.
    for (id, _) in &order {
        if let Some(r) = reviews.iter().find(|r| &r.id == id)
            && r.agent.is_some()
        {
            return Err(CommandError {
                message: format!(
                    "Review {} is mid-rework (agent attached); wait for it to finish before restacking the stack",
                    r.pr
                ),
            });
        }
    }

    // Resolve the ordered ids to (PrRef, parent_branch) pairs up front from the
    // snapshot, so the background task does not need the id -> PrRef mapping.
    // dependency_order skips dangling ids, so every id resolves here.
    let steps: Vec<(PrRef, String)> = order
        .into_iter()
        .filter_map(|(id, parent_branch)| {
            reviews
                .iter()
                .find(|r| r.id == id)
                .map(|r| (r.pr.clone(), parent_branch))
        })
        .collect();

    // Launch the sequence on a background task so the command returns promptly.
    let state_arc: Arc<AppState> = state.inner().clone();
    let app_handle = app_handle.clone();
    tauri::async_runtime::spawn(async move {
        run_restack_sequence(state_arc, app_handle, root_pr, steps).await;
    });

    Ok(())
}

/// Drive a dependency-ordered restack sequence to completion (or first halt).
///
/// Runs on a background task spawned by [`restack_stack`]. Loads config once,
/// then restacks each `(PrRef, parent_branch)` pair in order via [`restack_one`],
/// emitting `"restack-progress"` before and after each child. A conflict or a
/// hard error halts the sequence (leaving later descendants stale); a fully
/// clean run emits a final `"done"`. Every failure mode is non-fatal to the task
/// (Invariant §0.1): errors are logged and surfaced as progress events, never a
/// panic.
async fn run_restack_sequence(
    state: Arc<AppState>,
    app_handle: tauri::AppHandle,
    root_pr: String,
    steps: Vec<(PrRef, String)>,
) {
    // The step count cannot realistically exceed u32::MAX; saturate defensively
    // rather than silence the conversion with `as`.
    let total = u32::try_from(steps.len()).unwrap_or(u32::MAX);
    let root_ref = PrRef::new(&root_pr);

    // Load config + repo path once for the whole sequence. A load failure aborts
    // before touching any branch.
    let config = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("restack_stack: config load failed: {e}");
            emit_restack_progress(&app_handle, &root_pr, 0, total, "", "error", None);
            return;
        }
    };
    let repo_path = config
        .repo_path
        .clone()
        .unwrap_or_else(|| PathBuf::from("."));

    for (idx, (pr_ref, parent_branch)) in steps.iter().enumerate() {
        // 1-based; saturate rather than silence the conversion with `as`.
        let current = u32::try_from(idx).unwrap_or(u32::MAX).saturating_add(1);
        let current_pr = pr_ref.as_str().to_string();

        // TOCTOU guard: the launch-time agent check ran once, over a snapshot of
        // the whole order. Since then an agent could have attached to this
        // not-yet-processed child — or to the ROOT, which is never in `order` —
        // and restacking a branch out from under a running agent would clobber
        // its work. Re-read both from the store immediately before the restack
        // and halt if either is now mid-rework. This closes the TOCTOU window;
        // the check is best-effort (a race within one child's restack remains
        // theoretically possible, but restack_one aborts on conflict and only
        // moves the ref on a clean success).
        if let Some(reason) = restack_blocked_reason(&state, &root_ref, pr_ref) {
            emit_restack_progress(
                &app_handle,
                &root_pr,
                current,
                total,
                &current_pr,
                "error",
                Some(reason),
            );
            return;
        }

        emit_restack_progress(
            &app_handle,
            &root_pr,
            current,
            total,
            &current_pr,
            "restacking",
            None,
        );

        match restack_one(
            &state,
            &app_handle,
            &config,
            &repo_path,
            pr_ref,
            parent_branch,
        )
        .await
        {
            Ok(RestackStep::Clean) => {
                emit_restack_progress(
                    &app_handle,
                    &root_pr,
                    current,
                    total,
                    &current_pr,
                    "clean",
                    None,
                );
            }
            Ok(RestackStep::Conflict) => {
                // The conflict-resolver agent now owns this branch; stop here so
                // later descendants are not restacked onto an unresolved base.
                emit_restack_progress(
                    &app_handle,
                    &root_pr,
                    current,
                    total,
                    &current_pr,
                    "conflict",
                    None,
                );
                return;
            }
            Err(e) => {
                eprintln!(
                    "restack_stack: restack failed for {current_pr}: {}",
                    e.message
                );
                emit_restack_progress(
                    &app_handle,
                    &root_pr,
                    current,
                    total,
                    &current_pr,
                    "error",
                    None,
                );
                return;
            }
        }
    }

    // The whole sequence completed cleanly.
    emit_restack_progress(&app_handle, &root_pr, total, total, "", "done", None);
}

/// Re-read the child and root reviews from the store and report which one, if
/// any, now has an agent attached.
///
/// Closes the [`run_restack_sequence`] TOCTOU window: the launch-time guard in
/// [`restack_stack`] checks the whole order once, but a Fix/Review/Restack agent
/// can attach to a not-yet-processed child — or to the ROOT, which is never in
/// the order — before its turn arrives. Returns a human-readable reason when
/// either is mid-rework, otherwise `None`.
fn restack_blocked_reason(state: &AppState, root_ref: &PrRef, child_ref: &PrRef) -> Option<String> {
    if state
        .reviews
        .get(child_ref)
        .is_some_and(|r| r.agent.is_some())
    {
        return Some(format!(
            "{child_ref} is now mid-rework (agent attached); halted the stack restack here"
        ));
    }
    if state
        .reviews
        .get(root_ref)
        .is_some_and(|r| r.agent.is_some())
    {
        return Some(format!(
            "stack root {root_ref} is now mid-rework (agent attached); halted the stack restack"
        ));
    }
    None
}

// ---------------------------------------------------------------------------
// GitHub PR import commands
// ---------------------------------------------------------------------------

/// Fetch open PRs authored by the current user from GitHub.
///
/// Runs `gh pr list --author=@me` in the configured repo path, fetches diffs
/// concurrently for each PR, builds [`Review`] objects, and stores them.
/// Returns the created reviews.
#[tauri::command]
pub async fn fetch_authored_prs(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<Review>, CommandError> {
    fetch_prs_by_filter(&state, github::PrFilter::Authored).await
}

/// Fetch open PRs where the current user is requested for review.
///
/// Runs `gh pr list --search "review-requested:@me"` in the configured repo
/// path, fetches diffs concurrently, builds [`Review`] objects, and stores them.
/// Returns the created reviews.
#[tauri::command]
pub async fn fetch_review_requests(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<Review>, CommandError> {
    fetch_prs_by_filter(&state, github::PrFilter::ReviewRequested).await
}

/// Shared implementation for fetching PRs by filter.
///
/// When a review already exists in the store (matched by PR URL), the diff,
/// branch, base, CI summary, and pinned base/head SHAs are refreshed from
/// GitHub — comments, gate state, agent run, and stale flag are preserved so a
/// re-fetch never blows away in-progress review work. The head SHA is held back
/// while a rework is in flight (an attached agent or `Dispatched` state): the
/// local worktree HEAD leads GitHub's last-reported OID then, so adopting the
/// fetched head would point the diff/full-file view at a stale revision.
///
/// Takes `&AppState` (not `State`) so both the thin commands and the background
/// notify poller (D4) share this exact fetch + no-clobber refresh path.
pub(crate) async fn fetch_prs_by_filter(
    state: &AppState,
    filter: github::PrFilter,
) -> Result<Vec<Review>, CommandError> {
    let config = Config::load()?;
    let repo_path = config.repo_path.unwrap_or_else(|| PathBuf::from("."));

    let prs = github::list_prs_filtered(&repo_path, filter)
        .await
        .map_err(|e| CommandError {
            message: format!("failed to list PRs: {e}"),
        })?;

    let source = match filter {
        github::PrFilter::Authored => cockpit_core::model::ReviewSource::Authored,
        github::PrFilter::ReviewRequested => cockpit_core::model::ReviewSource::ReviewRequested,
    };

    let mut reviews = Vec::with_capacity(prs.len());
    for pr in &prs {
        let diff = if pr.repo_slug.is_empty() {
            github::pr_diff_in(&repo_path, pr.number)
                .await
                .unwrap_or_default()
        } else {
            github::pr_diff_by_repo(&pr.repo_slug, pr.number)
                .await
                .unwrap_or_default()
        };

        let pr_ref = PrRef::new(&pr.url);

        if state.reviews.get(&pr_ref).is_some() {
            let branch = pr.head_ref_name.clone();
            let base = pr.base_ref_name.clone();
            let head_sha = pr.head_ref_oid.clone();
            let ci_summary = github::rollup_to_summary(&pr.status_check_rollup);
            state.reviews.update(&pr_ref, |r| {
                r.diff = cockpit_core::model::DiffData { raw: diff };
                r.branch = branch;
                r.base = base;

                // Refresh CI + the pinned head SHA from GitHub. A failed per-PR
                // enrichment falls back to an empty rollup / empty OID; treat
                // those as "no fresh data" and keep what we already have rather
                // than degrading the diff resolution to a branch-name lookup.
                //
                // `base_sha` is deliberately NOT refreshed here: it is the restack
                // fork point (see `Review::base_sha`), not the base branch tip, so
                // the kickoff-computed value must never be clobbered by a GitHub
                // read (that would break restack once the base advances).
                if let Some(ci) = ci_summary {
                    r.ci_summary = Some(ci);
                }
                // The head SHA is authoritative locally while a rework is in
                // flight: a review with an attached agent, in `Dispatched`, or in
                // `Reworked` has a worktree HEAD that leads what GitHub last
                // reported, so adopting the fetched head here would point the
                // diff/full-file view at a stale revision. `Reworked` in
                // particular: after `apply_agent_completion` the local worktree
                // HEAD leads GitHub until the agent's push is visible, and
                // reverting it would make the interdiff read empty. Only take
                // GitHub's head OID when no rework owns the branch.
                let rework_in_flight = r.agent.is_some()
                    || r.gate_state == GateState::Dispatched
                    || r.gate_state == GateState::Reworked;
                if !rework_in_flight && !head_sha.is_empty() {
                    r.head_sha = head_sha;
                }
            });
            if let Some(updated) = state.reviews.get(&pr_ref) {
                reviews.push(updated);
            }
        } else {
            let review = github::build_review_from_pr(pr, diff, &repo_path, source);
            state.reviews.insert(review.clone());
            reviews.push(review);
        }
    }

    Ok(reviews)
}

// ---------------------------------------------------------------------------
// Open in editor
// ---------------------------------------------------------------------------

/// Open a file in the user's configured IDE/editor.
///
/// Uses the `ide_command` from config (e.g. "cursor", "code", "zed") to open
/// the given file path. For cross-repo PRs or branches not checked out
/// locally, fetches the branch into a worktree first.
#[tauri::command]
pub async fn open_in_editor(
    _state: State<'_, Arc<AppState>>,
    file_path: String,
    repo_slug: Option<String>,
    branch: Option<String>,
) -> Result<(), CommandError> {
    let config = Config::load()?;
    let ide = config
        .ide_command
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "code".to_string());
    let repo_path = config.repo_path.unwrap_or_else(|| PathBuf::from("."));

    let full_path = {
        let local = repo_path.join(&file_path);
        if local.exists() {
            local
        } else if let Some(ref branch) = branch {
            let root = cockpit_core::adapters::git::ensure_branch_checkout(
                &repo_path,
                branch,
                repo_slug.as_deref(),
            )
            .await
            .map_err(|e| CommandError {
                message: format!("failed to checkout branch for {file_path}: {e}"),
            })?;
            root.join(&file_path)
        } else {
            local
        }
    };

    tokio::process::Command::new(&ide)
        .arg(full_path.as_os_str())
        .spawn()
        .map_err(|e| CommandError {
            message: format!("failed to open {file_path} in {ide}: {e}"),
        })?;

    Ok(())
}

/// Start (or reuse) a Monaco LSP bridge for the given Monaco `languageId`.
///
/// Returns the localhost WebSocket URL the frontend language client should
/// connect to, or `None` when LSP is disabled or the language has no
/// configured server. The bridge is lazily started per language and cached in
/// [`AppState::lsp_bridges`], so repeated calls for the same language reuse the
/// existing bridge and URL.
///
/// The bridge only spawns the actual language-server child when the webview
/// opens a WebSocket, so an unavailable binary (pyright/tsserver not installed)
/// surfaces at connect time as a closed socket, not here — keeping this command
/// thin and non-fatal (Invariant 1).
#[tauri::command]
pub async fn start_lsp_bridge(
    state: State<'_, Arc<AppState>>,
    language_id: String,
) -> Result<Option<String>, CommandError> {
    use cockpit_core::adapters::lsp::LspBridge;
    use cockpit_core::config::LspLanguage;

    let Some(language) = LspLanguage::from_language_id(&language_id) else {
        return Ok(None);
    };

    let config = Config::load()?;
    if !config.lsp_servers.enabled {
        return Ok(None);
    }

    // Fast path: an existing bridge for this language. Take the URL under the
    // lock and drop it immediately — never hold the lock across `.await`.
    {
        let bridges = state.lsp_bridges.lock().map_err(|_| CommandError {
            message: "LSP bridge registry lock poisoned".to_string(),
        })?;
        if let Some(existing) = bridges.get(&language) {
            return Ok(Some(existing.url()));
        }
    }

    // Start a new bridge (async) with the lock released.
    let command = config.lsp_servers.command_for(language);
    let bridge = LspBridge::start(language, command).await?;
    let url = bridge.url();

    // Re-acquire the lock to insert. Double-check: another task may have
    // started one concurrently; if so, keep the first and drop ours (its
    // Drop aborts the just-started serve task, no orphan).
    let mut bridges = state.lsp_bridges.lock().map_err(|_| CommandError {
        message: "LSP bridge registry lock poisoned".to_string(),
    })?;
    if let Some(existing) = bridges.get(&language) {
        return Ok(Some(existing.url()));
    }
    bridges.insert(language, bridge);
    Ok(Some(url))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Percent-decode a path segment: the inverse of [`encode_path_segment`],
    /// matching axum's `Path<String>` decoding semantics. Test-only, used to
    /// prove the encode/decode round-trip stays exact without pulling in the
    /// `percent-encoding` crate.
    fn decode_path_segment(s: &str) -> String {
        fn val(byte: u8) -> Option<u8> {
            match byte {
                b'0'..=b'9' => Some(byte - b'0'),
                b'a'..=b'f' => Some(byte - b'a' + 10),
                b'A'..=b'F' => Some(byte - b'A' + 10),
                _ => None,
            }
        }
        let bytes = s.as_bytes();
        let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%'
                && i + 2 < bytes.len()
                && let (Some(hi), Some(lo)) = (val(bytes[i + 1]), val(bytes[i + 2]))
            {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
            out.push(bytes[i]);
            i += 1;
        }
        String::from_utf8(out).expect("round-trip produces valid UTF-8")
    }

    #[test]
    fn encode_path_segment_escapes_reserved_chars() {
        assert_eq!(encode_path_segment("owner/repo#42"), "owner%2Frepo%2342");
        assert_eq!(
            encode_path_segment("https://github.com/o/r/pull/42"),
            "https%3A%2F%2Fgithub.com%2Fo%2Fr%2Fpull%2F42"
        );
        // A space encodes as %20.
        assert_eq!(encode_path_segment("a b"), "a%20b");
    }

    #[test]
    fn encode_path_segment_preserves_unreserved() {
        // RFC 3986 unreserved set passes through untouched.
        let unreserved = "AZaz09-._~";
        assert_eq!(encode_path_segment(unreserved), unreserved);
    }

    #[test]
    fn encode_path_segment_round_trips_through_decode() {
        // Decoding the encoded segment (as axum does) reproduces the input
        // exactly, so the broker's recorded object_id matches the reviewed
        // object's key.
        for object_id in [
            "owner/repo#42",
            "https://github.com/o/r/pull/42",
            "plain-project-id",
            "proj:with spaces & symbols/#",
            "a.b_c~d-e",
        ] {
            let encoded = encode_path_segment(object_id);
            assert_eq!(
                decode_path_segment(&encoded),
                object_id,
                "round-trip failed for {object_id:?} (encoded: {encoded:?})"
            );
        }
    }

    #[test]
    fn mcp_approve_url_composes_encoded_segment() {
        assert_eq!(
            mcp_approve_url(19876, "owner/repo#7"),
            "http://127.0.0.1:19876/mcp/owner%2Frepo%237"
        );
    }
}
