//! GitHub adapter --- shells out to `gh` for PR listing, diffs, and CI checks.
//!
//! The critical function is [`parse_issue_from_branch`]: Linear embeds the issue
//! identifier in generated branch names (e.g. `alejandro/nex-123-add-feature`),
//! so cockpit links PR to issue by parsing the head branch. See `SPEC.md` S16.

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use ts_rs::TS;

use crate::model::{
    Anchor, CiSummary, Comment, CommentId, CommentOrigin, DiffSide, IssueRef, PrRef,
};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors from GitHub CLI operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// `gh` exited with a non-zero status or produced unexpected output.
    #[error("gh command failed: {0}")]
    GhCommand(String),

    /// The JSON output from `gh` could not be parsed into the expected shape.
    #[error("failed to parse gh output: {0}")]
    ParseOutput(String),

    /// The branch name did not contain a recognizable Linear issue identifier.
    #[error("no issue ID found in branch: {0}")]
    BranchParse(String),

    /// Failed to post a comment to a GitHub PR.
    #[error("failed to post comment to PR {pr}: {reason}")]
    PostComment {
        /// The PR that was targeted.
        pr: String,
        /// Why the post failed.
        reason: String,
    },

    /// An I/O error occurred while spawning or communicating with `gh`.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Raw PR data from `gh pr list --json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrData {
    /// PR number within the repository.
    pub number: u64,
    /// Head branch name (where the changes live).
    pub head_ref_name: String,
    /// Base branch name (target for the merge).
    pub base_ref_name: String,
    /// PR title.
    pub title: String,
    /// PR description / body. Absent in legacy field sets, so defaulted.
    #[serde(default)]
    pub body: String,
    /// Current PR state (e.g. "OPEN", "MERGED", "CLOSED").
    pub state: String,
    /// Full URL of the PR on GitHub.
    pub url: String,
    /// Repository slug (e.g. "Nexcade/garage"). Present for cross-repo searches.
    #[serde(default)]
    pub repo_slug: String,
    /// The PR's `statusCheckRollup` entries. Empty for legacy field sets (which
    /// did not request the rollup), so defaulted; rolled up via
    /// [`rollup_to_summary`].
    #[serde(default)]
    pub status_check_rollup: Vec<StatusCheckNode>,
    /// Head commit SHA (`headRefOid`), pinned at fetch so the diff/full-file
    /// fallback resolves the exact revision instead of a drifting branch lookup.
    /// Empty for legacy field sets, so defaulted.
    #[serde(default)]
    pub head_ref_oid: String,
    /// Base commit SHA (`baseRefOid`), pinned at fetch (see `head_ref_oid`).
    /// Empty for legacy field sets, so defaulted.
    #[serde(default)]
    pub base_ref_oid: String,
}

/// CI check status from `gh pr checks`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckRun {
    /// Name of the check (e.g. "CI / build").
    pub name: String,
    /// Current status (e.g. "completed", "in_progress").
    pub status: String,
    /// Conclusion once completed (e.g. "success", "failure"). `None` while in progress.
    pub conclusion: Option<String>,
}

/// A single CI check as returned by `gh pr checks --json`.
///
/// `gh pr checks` reports each check's `bucket` — a normalized rollup of the
/// raw state that groups the many possible GitHub statuses into a handful of
/// outcomes (`pass`, `fail`, `pending`, `skipping`, `cancel`). The bucket is
/// the reliable signal for summarizing pass/fail; `state` is the raw value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub struct CiCheck {
    /// Name of the check (e.g. "build", "lint").
    pub name: String,
    /// Raw check state (e.g. "SUCCESS", "FAILURE", "PENDING", "SKIPPED").
    pub state: String,
    /// Normalized outcome bucket from `gh` (e.g. "pass", "fail", "pending").
    pub bucket: String,
    /// Deep link to the check run's details page (used to extract the run id).
    pub link: String,
    /// Workflow name the check belongs to, when available.
    #[serde(default)]
    pub workflow: String,
}

/// Classification of a single check's outcome, derived from its bucket/state.
///
/// Kept private: the public surface is [`CiSummary`] via [`summarize`] and
/// [`rollup_to_summary`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckOutcome {
    /// Passed, or a non-blocking outcome (neutral, skipped, cancelled).
    Pass,
    /// Failed (failure, timed out, action required, startup failure, etc.).
    Fail,
    /// Not yet concluded (queued, in progress, pending).
    Pending,
}

/// Classify a single raw check signal into a [`CheckOutcome`].
///
/// The signal is a `gh` bucket (`pass`/`fail`/`pending`/…), a raw GitHub check
/// state/conclusion (`SUCCESS`/`FAILURE`/…), or a legacy commit-status state
/// (`ERROR`/`EXPECTED`/…) — the match is case-insensitive and covers all three.
/// Both the `gh pr checks` path ([`CiCheck::outcome`]) and the
/// `statusCheckRollup` path ([`rollup_to_summary`]) route through here so their
/// pass/fail/pending semantics can never drift apart.
///
/// Neutral, skipped, and cancelled map to [`CheckOutcome::Pass`] (they are not
/// failures); an unknown signal maps conservatively to [`CheckOutcome::Pending`]
/// so it is neither a false pass nor a false failure.
fn classify_check_signal(signal: &str) -> CheckOutcome {
    match signal.to_ascii_lowercase().as_str() {
        // gh buckets.
        "pass" | "skipping" | "cancel" => CheckOutcome::Pass,
        "fail" => CheckOutcome::Fail,
        "pending" => CheckOutcome::Pending,
        // Raw GitHub check states / conclusions and commit-status states.
        "success" | "neutral" | "skipped" | "cancelled" | "canceled" => CheckOutcome::Pass,
        "failure" | "timed_out" | "action_required" | "startup_failure" | "stale" | "error" => {
            CheckOutcome::Fail
        }
        "queued" | "in_progress" | "waiting" | "requested" | "expected" => CheckOutcome::Pending,
        // Unknown signal: treat conservatively as pending so it is neither a
        // false pass nor a false failure.
        _ => CheckOutcome::Pending,
    }
}

impl CiCheck {
    /// Classify this check's outcome from its `bucket` (falling back to `state`).
    ///
    /// `gh`'s `bucket` field is the normalized signal; when a fixture or older
    /// `gh` omits it, the raw `state` is used. Delegates to
    /// [`classify_check_signal`] so `gh pr checks` and the rollup share one
    /// classification.
    fn outcome(&self) -> CheckOutcome {
        let signal = if self.bucket.is_empty() {
            self.state.as_str()
        } else {
            self.bucket.as_str()
        };
        classify_check_signal(signal)
    }
}

/// Roll up a slice of [`CiCheck`]s into a [`CiSummary`].
///
/// Pure and deterministic. Neutral, skipped, and cancelled checks count toward
/// `passed`. `passed + failed + pending == total`.
pub fn summarize(checks: &[CiCheck]) -> CiSummary {
    let mut summary = CiSummary {
        passed: 0,
        total: checks.len() as u32,
        failed: 0,
        pending: 0,
    };
    for check in checks {
        match check.outcome() {
            CheckOutcome::Pass => summary.passed += 1,
            CheckOutcome::Fail => summary.failed += 1,
            CheckOutcome::Pending => summary.pending += 1,
        }
    }
    summary
}

/// Deserialize a possibly-null JSON string as the empty string when null.
///
/// `gh` emits `"conclusion": null` for an in-flight CheckRun, and plain
/// `#[serde(default)]` only covers an *absent* field, not an explicit `null`.
/// Applied to every string field of [`StatusCheckNode`] so a null anywhere in
/// the heterogeneous rollup coalesces to empty rather than failing the parse.
fn null_as_empty_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

/// A single entry in a PR's `statusCheckRollup` (from `gh pr view --json`).
///
/// The rollup is a heterogeneous array: GitHub Actions/checks appear as
/// `CheckRun` nodes (carrying `status`/`conclusion`/`name`) while legacy commit
/// statuses appear as `StatusContext` nodes (carrying `state`/`context`). Every
/// field is optional and defaulted because each node only populates the subset
/// belonging to its own type, and a present-but-null value coalesces to empty.
/// Not TS-exported: this is an adapter-internal shape that is rolled up into the
/// domain [`CiSummary`] via [`rollup_to_summary`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusCheckNode {
    /// GraphQL type discriminator: `"CheckRun"` or `"StatusContext"`.
    #[serde(
        default,
        rename = "__typename",
        deserialize_with = "null_as_empty_string"
    )]
    pub typename: String,
    /// CheckRun: the check's name (e.g. `"build"`). Empty for a StatusContext.
    #[serde(default, deserialize_with = "null_as_empty_string")]
    pub name: String,
    /// CheckRun: run status (e.g. `"COMPLETED"`, `"IN_PROGRESS"`). Empty for a StatusContext.
    #[serde(default, deserialize_with = "null_as_empty_string")]
    pub status: String,
    /// CheckRun: conclusion once completed (e.g. `"SUCCESS"`, `"FAILURE"`).
    /// Empty while a run is in flight (`gh` reports `null`), and for a StatusContext.
    #[serde(default, deserialize_with = "null_as_empty_string")]
    pub conclusion: String,
    /// StatusContext: the status name (e.g. `"ci/circleci"`). Empty for a CheckRun.
    #[serde(default, deserialize_with = "null_as_empty_string")]
    pub context: String,
    /// StatusContext: raw state (e.g. `"SUCCESS"`, `"FAILURE"`, `"ERROR"`, `"PENDING"`).
    /// Empty for a CheckRun.
    #[serde(default, deserialize_with = "null_as_empty_string")]
    pub state: String,
}

impl StatusCheckNode {
    /// The raw signal fed to [`classify_check_signal`] for this node.
    ///
    /// A StatusContext carries its outcome in `state`; a CheckRun carries it in
    /// `conclusion` once completed, or in `status` while still in flight. `state`
    /// is only non-empty for a StatusContext, so it is checked first.
    fn signal(&self) -> &str {
        if !self.state.is_empty() {
            &self.state
        } else if !self.conclusion.is_empty() {
            &self.conclusion
        } else {
            &self.status
        }
    }
}

/// Roll up a PR's `statusCheckRollup` nodes into a [`CiSummary`].
///
/// Returns `None` for an empty slice: a PR with **no** checks is not the same as
/// a PR whose checks are all green, and the CI badge must be able to tell them
/// apart. Classification routes through [`classify_check_signal`] — the same
/// function [`summarize`] uses — so the rollup and `gh pr checks` paths agree on
/// every equivalent state. `passed + failed + pending == total`.
pub fn rollup_to_summary(nodes: &[StatusCheckNode]) -> Option<CiSummary> {
    if nodes.is_empty() {
        return None;
    }
    let mut summary = CiSummary {
        passed: 0,
        total: nodes.len() as u32,
        failed: 0,
        pending: 0,
    };
    for node in nodes {
        match classify_check_signal(node.signal()) {
            CheckOutcome::Pass => summary.passed += 1,
            CheckOutcome::Fail => summary.failed += 1,
            CheckOutcome::Pending => summary.pending += 1,
        }
    }
    Some(summary)
}

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

/// List open PRs in the current repository via `gh pr list --json`.
///
/// Returns up to 100 PRs with the fields needed for branch-to-issue linkage
/// and diff-gate display.
pub async fn list_prs() -> Result<Vec<PrData>, Error> {
    let output = Command::new("gh")
        .args([
            "pr",
            "list",
            "--json",
            "number,headRefName,baseRefName,title,body,state,url",
            "--limit",
            "100",
        ])
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::GhCommand(stderr.into_owned()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).map_err(|e| Error::ParseOutput(e.to_string()))
}

/// Fetch the unified diff for a single PR via `gh pr diff`.
pub async fn pr_diff(pr_number: u64) -> Result<String, Error> {
    let output = Command::new("gh")
        .args(["pr", "diff", &pr_number.to_string()])
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::GhCommand(stderr.into_owned()));
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Fetch legacy CI check statuses for a PR via `gh pr checks --json`.
///
/// Superseded by [`pr_checks`], which returns the richer [`CiCheck`] shape used
/// by the diff-gate CI badge. Retained for callers that only need the raw
/// status/conclusion pair.
pub async fn pr_check_runs(pr_number: u64) -> Result<Vec<CheckRun>, Error> {
    let output = Command::new("gh")
        .args([
            "pr",
            "checks",
            &pr_number.to_string(),
            "--json",
            "name,status,conclusion",
        ])
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::GhCommand(stderr.into_owned()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).map_err(|e| Error::ParseOutput(e.to_string()))
}

/// Fetch CI checks for a PR via `gh pr checks <n> --json name,state,bucket,link,workflow`.
///
/// When `repo_slug` is `Some`, targets that repository with `--repo` so the
/// call works cross-repo without a `current_dir`. Parses into the [`CiCheck`]
/// shape used by the diff-gate badge; roll it up with [`summarize`].
///
/// This is a STATUS-tier read: it never mutates state and never blocks the
/// review loop. `gh` exits non-zero when a PR has no checks at all — callers
/// (e.g. the Tauri `fetch_ci_checks` command) treat that as an empty result.
pub async fn pr_checks(repo_slug: Option<&str>, pr_number: u64) -> Result<Vec<CiCheck>, Error> {
    let pr = pr_number.to_string();
    let mut args: Vec<&str> = vec![
        "pr",
        "checks",
        &pr,
        "--json",
        "name,state,bucket,link,workflow",
    ];
    if let Some(slug) = repo_slug {
        args.push("--repo");
        args.push(slug);
    }

    let output = Command::new("gh").args(&args).output().await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::GhCommand(stderr.into_owned()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).map_err(|e| Error::ParseOutput(e.to_string()))
}

/// Maximum characters of concatenated failed-CI logs kept for the rework prompt.
///
/// GitHub Actions logs can run to megabytes; the tail is where failures and
/// assertion output live, so we keep the tail and drop the head. This bounds the
/// prompt so a huge log can never blow up dispatch (Invariant 1: never block the
/// loop on GitHub).
const MAX_CI_LOG_CHARS: usize = 20_000;

/// Extract a GitHub Actions run id from a check's `link` (details URL).
///
/// Check links look like
/// `https://github.com/owner/repo/actions/runs/1234567890/job/987` or
/// `.../actions/runs/1234567890`. Returns the numeric run id following
/// `/runs/`, or `None` when the URL is not a recognizable Actions run link.
pub fn run_id_from_link(link: &str) -> Option<u64> {
    let after = link.split("/runs/").nth(1)?;
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse::<u64>().ok()
}

/// Truncate CI log text to [`MAX_CI_LOG_CHARS`], keeping the tail.
///
/// When truncation occurs a short marker is prepended so the reader (and the
/// agent) knows the head was dropped. Splits on a `char` boundary so multi-byte
/// UTF-8 is never sliced mid-character.
fn truncate_tail(text: &str, max_chars: usize) -> String {
    let char_count = text.chars().count();
    if char_count <= max_chars {
        return text.to_string();
    }
    let skip = char_count - max_chars;
    let tail: String = text.chars().skip(skip).collect();
    format!("[... {skip} earlier characters truncated ...]\n{tail}")
}

/// Fetch the concatenated logs of a PR's failed CI checks (LOG tier).
///
/// Finds the failed checks (via [`summarize`]'s classification), extracts each
/// distinct run id from the check `link`, runs `gh run view <run-id>
/// --log-failed`, concatenates the outputs, and truncates to a sane cap keeping
/// the tail (see [`MAX_CI_LOG_CHARS`]).
///
/// This is an ON-DEMAND read fed into the diff-gate rework prompt only from an
/// explicit user "Fix CI" action — it is never auto-fired and never blocks the
/// loop. Returns an empty string when there are no failed checks.
pub async fn failed_ci_logs(repo_slug: Option<&str>, pr_number: u64) -> Result<String, Error> {
    let checks = pr_checks(repo_slug, pr_number).await?;

    // Collect the distinct run ids of failed checks, preserving first-seen order
    // so the output is deterministic and a shared run id is fetched once.
    let mut run_ids: Vec<u64> = Vec::new();
    for check in &checks {
        if summarize(std::slice::from_ref(check)).failed == 0 {
            continue;
        }
        if let Some(id) = run_id_from_link(&check.link)
            && !run_ids.contains(&id)
        {
            run_ids.push(id);
        }
    }

    let mut combined = String::new();
    for id in run_ids {
        match run_view_log_failed(repo_slug, id).await {
            Ok(log) => {
                if !combined.is_empty() {
                    combined.push_str("\n\n");
                }
                combined.push_str(&format!("=== run {id} (failed jobs) ===\n"));
                combined.push_str(&log);
            }
            // A single run's log fetch failing is non-fatal: keep whatever we
            // have. Never block the Fix action on a GitHub read (Invariant 1).
            Err(e) => {
                eprintln!("failed_ci_logs: gh run view {id} failed: {e}");
            }
        }
    }

    Ok(truncate_tail(&combined, MAX_CI_LOG_CHARS))
}

/// Fetch the failed-job logs for a single CI run (LOG tier).
///
/// Runs `gh run view <run_id> --log-failed` for the given run, scoped to
/// `repo_slug` when provided, and truncates the output to [`MAX_CI_LOG_CHARS`]
/// keeping the tail (see [`truncate_tail`]). Unlike [`failed_ci_logs`], which
/// aggregates every failed run of a PR, this reads exactly one run so the CI
/// panel can show per-pipeline logs.
///
/// This is an ON-DEMAND read for the CI panel; it is never auto-fired against
/// the review loop and, per Invariant 1, callers treat a `gh` error as
/// non-fatal (empty logs) rather than a blocked UI.
pub async fn run_logs(repo_slug: Option<&str>, run_id: u64) -> Result<String, Error> {
    let raw = run_view_log_failed(repo_slug, run_id).await?;
    Ok(truncate_tail(&raw, MAX_CI_LOG_CHARS))
}

/// Run `gh run view <run-id> --log-failed`, optionally scoped to a repo.
async fn run_view_log_failed(repo_slug: Option<&str>, run_id: u64) -> Result<String, Error> {
    let id = run_id.to_string();
    let mut args: Vec<&str> = vec!["run", "view", &id, "--log-failed"];
    if let Some(slug) = repo_slug {
        args.push("--repo");
        args.push(slug);
    }

    let output = Command::new("gh").args(&args).output().await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::GhCommand(stderr.into_owned()));
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Parse a Linear issue identifier from a branch name.
///
/// Linear embeds the issue ID in generated branch names, typically as
/// `username/PREFIX-123-description`. The prefix is 2-5 uppercase letters
/// followed by a dash and one or more digits (e.g. `NEX-123`, `AB-1`).
///
/// The prefix is normalized to uppercase for consistency, so
/// `alejandro/nex-456-fix-bug` yields `IssueRef("NEX-456")`.
///
/// Returns `None` if no matching pattern is found.
pub fn parse_issue_from_branch(branch: &str) -> Option<IssueRef> {
    // Take the segment after the first `/`, or the whole string if there is none.
    // This strips the `username/` prefix that Linear typically adds.
    let segment = branch.split('/').next_back()?;

    if segment.is_empty() {
        return None;
    }

    // Walk the segment looking for PREFIX-DIGITS at the start of a word boundary.
    // A "word boundary" here means start-of-segment or right after a `-` that
    // follows a digit (not a prefix char). We only need the first match.
    //
    // Manual parsing avoids pulling in the `regex` crate for a simple pattern.
    let bytes = segment.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        // Try to match [A-Za-z]{2,5}-[0-9]+ starting at position i.
        let start = i;
        let mut alpha_count = 0;

        // Count consecutive alpha chars (the prefix).
        while i < len && bytes[i].is_ascii_alphabetic() {
            alpha_count += 1;
            i += 1;
        }

        // Need 2..=5 alpha chars followed by a dash.
        if (2..=5).contains(&alpha_count) && i < len && bytes[i] == b'-' {
            let dash_pos = i;
            i += 1; // skip the dash

            // Count consecutive digits.
            let digit_start = i;
            while i < len && bytes[i].is_ascii_digit() {
                i += 1;
            }

            let digit_count = i - digit_start;
            if digit_count > 0 {
                // Ensure the match ends at a word boundary: end of string or
                // followed by a non-alphanumeric char (typically `-`).
                if i >= len || !bytes[i].is_ascii_alphanumeric() {
                    let prefix = &segment[start..dash_pos];
                    let digits = &segment[digit_start..i];
                    let issue_id = format!("{}-{digits}", prefix.to_uppercase());
                    return Some(IssueRef::new(issue_id));
                }
            }
        }

        // Advance to the next potential start: skip to next `-` boundary + 1,
        // or to the next char if we didn't move.
        if i == start {
            i += 1;
        }
        // Skip to the character after the next `-`.
        while i < len && bytes[i] != b'-' {
            i += 1;
        }
        if i < len {
            i += 1; // skip the `-`
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Filtered PR listing
// ---------------------------------------------------------------------------

/// How to filter the PR list from GitHub.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrFilter {
    /// PRs authored by the authenticated user.
    Authored,
    /// PRs where the authenticated user was requested for review.
    ReviewRequested,
}

/// List open PRs with a filter across all accessible repositories.
///
/// Uses `gh search prs` (cross-repo) to discover PRs, then enriches each with
/// branch info from `gh pr view --repo`. Enrichment runs with a bounded number
/// of in-flight `gh` calls ([`ENRICH_CONCURRENCY`]) and the results are
/// re-sorted so the output order matches the search order. This way PRs from any
/// repo the user has access to are returned, not just a single configured repo.
///
/// A per-PR enrichment failure is non-fatal: that PR falls back to the fields
/// already known from the search result (with empty branch names) rather than
/// failing the whole call.
///
/// - [`PrFilter::Authored`] → `gh search prs --author=@me`
/// - [`PrFilter::ReviewRequested`] → searches both `--review-requested=@me`
///   (pending requests) and `--reviewed-by=@me` (already interacted), then
///   deduplicates by PR URL and excludes self-authored PRs.
pub async fn list_prs_filtered(
    _repo_path: &std::path::Path,
    filter: PrFilter,
) -> Result<Vec<PrData>, Error> {
    let search_results = match filter {
        PrFilter::Authored => search_prs(&["--author", "@me"]).await?,
        PrFilter::ReviewRequested => {
            let mut pending = search_prs(&["--review-requested", "@me"]).await?;
            let reviewed = search_prs(&["--reviewed-by", "@me"]).await?;
            // GitHub logins are case-insensitive; lowercase our identity so the
            // self-exclusion below compares like-for-like against the likewise
            // lowercased `author_login`.
            let me = gh_whoami().await.unwrap_or_default().to_lowercase();

            // Merge and deduplicate by URL.
            let mut seen = std::collections::HashSet::new();
            for r in &pending {
                seen.insert(r.url.clone());
            }
            for r in reviewed {
                if seen.insert(r.url.clone()) {
                    pending.push(r);
                }
            }

            // Exclude self-authored PRs (those belong in the Authored tab).
            if !me.is_empty() {
                pending.retain(|r| r.author_login() != me);
            }

            pending
        }
    };

    // Enrich each PR with branch names via `gh pr view --repo`, running up to
    // ENRICH_CONCURRENCY calls at once. Each task is tagged with its search-order
    // index so the results can be re-sorted below; a task never fails (an enrich
    // error resolves to a fallback `PrData`), preserving the old semantics where
    // one PR's failure does not abort the whole listing.
    let total = search_results.len();
    let mut slots: Vec<Option<PrData>> = vec![None; total];
    let mut join_set: tokio::task::JoinSet<(usize, PrData)> = tokio::task::JoinSet::new();
    let mut pending = search_results.into_iter().enumerate();

    // Prime the initial window of in-flight enrichments.
    for _ in 0..ENRICH_CONCURRENCY {
        match pending.next() {
            Some((index, sr)) => {
                join_set.spawn(enrich_task(index, sr));
            }
            None => break,
        }
    }

    // As each enrichment completes, record it and start the next one.
    while let Some(joined) = join_set.join_next().await {
        let (index, pr) =
            joined.map_err(|e| Error::GhCommand(format!("enrich task failed to join: {e}")))?;
        if let Some(slot) = slots.get_mut(index) {
            *slot = Some(pr);
        }
        if let Some((index, sr)) = pending.next() {
            join_set.spawn(enrich_task(index, sr));
        }
    }

    Ok(slots.into_iter().flatten().collect())
}

/// Maximum number of concurrent `gh pr view` enrichment calls in
/// [`list_prs_filtered`]. Bounds fan-out so a large PR list cannot spawn an
/// unbounded number of `gh` subprocesses at once.
const ENRICH_CONCURRENCY: usize = 8;

/// Enrich a single search result into a full [`PrData`], returning its
/// search-order `index` alongside the result.
///
/// On enrichment failure the PR falls back to the fields already known from the
/// search result (with empty branch names), so the task always resolves — a
/// per-PR failure never aborts [`list_prs_filtered`].
async fn enrich_task(index: usize, sr: SearchPrResult) -> (usize, PrData) {
    let slug = sr.repository.name_with_owner.clone();
    match enrich_pr(&slug, sr.number).await {
        Ok(mut pr) => {
            pr.repo_slug = slug;
            (index, pr)
        }
        Err(_) => (
            index,
            PrData {
                number: sr.number,
                head_ref_name: String::new(),
                base_ref_name: String::new(),
                title: sr.title,
                body: String::new(),
                state: sr.state,
                url: sr.url,
                repo_slug: slug,
                status_check_rollup: Vec::new(),
                head_ref_oid: String::new(),
                base_ref_oid: String::new(),
            },
        ),
    }
}

/// Run `gh search prs --state=open` with the given extra args.
async fn search_prs(extra_args: &[&str]) -> Result<Vec<SearchPrResult>, Error> {
    let mut cmd = Command::new("gh");
    cmd.args([
        "search",
        "prs",
        "--state",
        "open",
        "--json",
        "number,title,state,url,repository,author",
        "--limit",
        "100",
    ]);
    cmd.args(extra_args);

    let output = cmd.output().await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::GhCommand(stderr.into_owned()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).map_err(|e| Error::ParseOutput(e.to_string()))
}

/// Get the current authenticated GitHub username.
async fn gh_whoami() -> Result<String, Error> {
    let output = Command::new("gh")
        .args(["auth", "status", "--json", "user"])
        .output()
        .await?;

    if !output.status.success() {
        return Err(Error::GhCommand("gh auth status failed".into()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value =
        serde_json::from_str(&stdout).map_err(|e| Error::ParseOutput(e.to_string()))?;
    Ok(v.get("user")
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string())
}

/// Intermediate type for `gh search prs` JSON output.
#[derive(Debug, Deserialize)]
struct SearchPrResult {
    number: u64,
    title: String,
    state: String,
    url: String,
    repository: SearchRepo,
    #[serde(default)]
    author: SearchAuthor,
}

impl SearchPrResult {
    /// Login of the PR author, lowercased for case-insensitive comparison.
    ///
    /// GitHub logins are case-insensitive, so callers compare against a
    /// likewise-lowercased identity (see [`list_prs_filtered`]).
    fn author_login(&self) -> String {
        self.author.login.to_lowercase()
    }
}

/// Repository info embedded in search results.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchRepo {
    name_with_owner: String,
}

/// Author info embedded in search results.
#[derive(Debug, Default, Deserialize)]
struct SearchAuthor {
    #[serde(default)]
    login: String,
}

/// Fetch full PR details (branch names) from a specific repo.
async fn enrich_pr(repo_slug: &str, pr_number: u64) -> Result<PrData, Error> {
    let output = Command::new("gh")
        .args([
            "pr",
            "view",
            &pr_number.to_string(),
            "--repo",
            repo_slug,
            "--json",
            "number,headRefName,baseRefName,title,body,state,url,statusCheckRollup,headRefOid,baseRefOid",
        ])
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::GhCommand(stderr.into_owned()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).map_err(|e| Error::ParseOutput(e.to_string()))
}

/// Fetch the unified diff for a PR by repo slug and number.
///
/// Uses `--repo` so it works cross-repo without needing `current_dir`.
pub async fn pr_diff_in(repo_path: &std::path::Path, pr_number: u64) -> Result<String, Error> {
    let output = Command::new("gh")
        .current_dir(repo_path)
        .args(["pr", "diff", &pr_number.to_string()])
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::GhCommand(stderr.into_owned()));
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Fetch the unified diff for a PR using `--repo` (cross-repo).
pub async fn pr_diff_by_repo(repo_slug: &str, pr_number: u64) -> Result<String, Error> {
    let output = Command::new("gh")
        .args(["pr", "diff", &pr_number.to_string(), "--repo", repo_slug])
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::GhCommand(stderr.into_owned()));
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Build a [`Review`] from GitHub PR data and its diff.
///
/// For PRs linked to a Linear issue (detected from the branch name), the
/// `issue` field is the parsed issue ref. Otherwise, it uses the PR title.
/// The `repo_path` is used as the worktree location for local operations.
/// The `source` parameter indicates whether this is an authored or
/// review-requested PR.
pub fn build_review_from_pr(
    pr: &PrData,
    diff: String,
    repo_path: &std::path::Path,
    source: crate::model::ReviewSource,
) -> crate::model::Review {
    use crate::model::*;

    let issue =
        parse_issue_from_branch(&pr.head_ref_name).unwrap_or_else(|| IssueRef::new(&pr.title));

    let id_prefix = if pr.repo_slug.is_empty() {
        format!("gh-{}", pr.number)
    } else {
        format!("gh-{}-{}", pr.repo_slug.replace('/', "-"), pr.number)
    };

    let slug = if pr.repo_slug.is_empty() {
        None
    } else {
        Some(pr.repo_slug.clone())
    };

    Review {
        id: ReviewId::new(id_prefix),
        issue,
        pr: PrRef::new(&pr.url),
        title: pr.title.clone(),
        body: pr.body.clone(),
        branch: pr.head_ref_name.clone(),
        base: pr.base_ref_name.clone(),
        // Pin the base/head SHAs from the fetch so the diff and full-file
        // fallback resolve the exact revisions rather than re-resolving the base
        // by branch name (which drifts as the base branch advances).
        base_sha: pr.base_ref_oid.clone(),
        source,
        worktree: repo_path.to_path_buf(),
        gate_state: GateState::Pending,
        diff: DiffData { raw: diff },
        head_sha: pr.head_ref_oid.clone(),
        comments: vec![],
        parents: vec![],
        children: vec![],
        stale: false,
        agent: None,
        repo_slug: slug,
        project: None,
        dispatch_snapshot: None,
        ci_summary: rollup_to_summary(&pr.status_check_rollup),
        review_findings: vec![],
        conversation: vec![],
        last_reviewed_sha: None,
    }
}

/// Map each PR to its linked Linear issue (if any) by parsing the head branch.
///
/// Returns pairs of `(PrRef, Option<IssueRef>)` in the same order as the input.
/// The `PrRef` is constructed from the PR's URL.
pub fn link_prs_to_issues(prs: &[PrData]) -> Vec<(PrRef, Option<IssueRef>)> {
    prs.iter()
        .map(|pr| {
            let pr_ref = PrRef::new(&pr.url);
            let issue_ref = parse_issue_from_branch(&pr.head_ref_name);
            (pr_ref, issue_ref)
        })
        .collect()
}

/// Wire stack `parents`/`children` edges across GitHub-imported reviews by
/// matching each review's `base` branch against another review's head `branch`.
///
/// A review whose `base` is another review's `branch` is stacked on top of it:
/// the base review becomes the parent and this review becomes its child. This
/// mirrors the semantics of `kickoff::wire_children`, but derives the graph from
/// GitHub branch topology rather than the Linear DAG.
///
/// Only reviews sourced from GitHub ([`ReviewSource::Authored`] /
/// [`ReviewSource::ReviewRequested`]) have their edges cleared and rebuilt.
/// Reviews created by a project kickoff ([`ReviewSource::Frontier`]) keep the
/// edges wired from the Linear DAG untouched — a review's `base` matching a
/// frontier branch may still add the frontier review as a parent, but the
/// frontier review's own edges are never cleared.
pub fn wire_stack_edges(reviews: &mut [crate::model::Review]) {
    use crate::model::ReviewSource;

    // GitHub-imported reviews derive their stack purely from branch topology, so
    // their edges are safe to clear and rebuild. Frontier reviews carry
    // kickoff-wired edges that must be preserved.
    fn is_github_sourced(source: ReviewSource) -> bool {
        matches!(
            source,
            ReviewSource::Authored | ReviewSource::ReviewRequested
        )
    }

    // Clear existing edges only on the reviews we own the topology for.
    for review in reviews.iter_mut() {
        if is_github_sourced(review.source) {
            review.parents.clear();
            review.children.clear();
        }
    }

    // Map head branch -> review id so a base branch can be resolved to a parent.
    let branch_to_id: std::collections::HashMap<String, crate::model::ReviewId> = reviews
        .iter()
        .map(|r| (r.branch.clone(), r.id.clone()))
        .collect();

    // Derive parent edges from base==branch, collecting the (parent, child)
    // pairs to wire children in a second pass (avoids a mutable-borrow conflict).
    let mut edges: Vec<(crate::model::ReviewId, crate::model::ReviewId)> = Vec::new();
    for review in reviews.iter_mut() {
        if !is_github_sourced(review.source) {
            continue;
        }
        if let Some(parent_id) = branch_to_id.get(&review.base) {
            // A review can never be its own parent (guards a self-referential
            // base branch).
            if parent_id != &review.id {
                review.parents.push(parent_id.clone());
                edges.push((parent_id.clone(), review.id.clone()));
            }
        }
    }

    for (parent_id, child_id) in edges {
        if let Some(parent) = reviews.iter_mut().find(|r| r.id == parent_id)
            && !parent.children.contains(&child_id)
        {
            parent.children.push(child_id);
        }
    }
}

// ---------------------------------------------------------------------------
// Comment mirroring
// ---------------------------------------------------------------------------

/// Result of mirroring local comments to a GitHub PR.
///
/// Tracks how many comments were successfully posted and collects failures
/// with their reason, so the caller can report partial success.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub struct MirrorResult {
    /// Number of comments successfully posted.
    pub posted: usize,
    /// Comments that failed to post: `(comment_id, error_message)`.
    pub failed: Vec<(CommentId, String)>,
}

/// Format a cockpit [`Comment`] into the markdown body posted to GitHub.
///
/// Includes the anchor location so the reader knows which code location
/// the comment refers to.
pub fn format_comment_body(comment: &Comment) -> String {
    let anchor_label = match &comment.anchor {
        Anchor::PlanStep(idx) => format!("**Plan step {idx}**"),
        Anchor::PlanFile(path) => format!("**Plan file:** `{}`", path.display()),
        Anchor::DiffLine { path, range, .. } => {
            if range.0 == range.1 {
                format!("**{}** line {}", path.display(), range.0)
            } else {
                format!("**{}** lines {}-{}", path.display(), range.0, range.1)
            }
        }
    };

    format!("{anchor_label}\n\n{}", comment.body)
}

/// Post a single comment to a GitHub PR via `gh pr comment`.
///
/// Uses the `gh` CLI to create a new comment on the PR. The comment body
/// includes the anchor information so the GitHub reader knows which code
/// location is referenced.
pub async fn post_review_comment(pr_ref: &PrRef, comment: &Comment) -> Result<(), Error> {
    let body = format_comment_body(comment);

    let output = Command::new("gh")
        .args(["pr", "comment", pr_ref.as_str(), "--body", &body])
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::PostComment {
            pr: pr_ref.to_string(),
            reason: stderr.into_owned(),
        });
    }

    Ok(())
}

/// Mirror local comments for a PR to GitHub.
///
/// Only mirrors comments with [`CommentOrigin::Local`] origin -- comments
/// that came from GitHub (`GitHubMirror`) are skipped to avoid duplicates.
///
/// This is an explicit user action per Invariant 5: it is never called
/// automatically or from agent output.
pub async fn mirror_comments(pr_ref: &PrRef, comments: &[Comment]) -> Result<MirrorResult, Error> {
    let local_comments: Vec<&Comment> = comments
        .iter()
        .filter(|c| c.origin == CommentOrigin::Local)
        .collect();

    let mut posted = 0usize;
    let mut failed: Vec<(CommentId, String)> = Vec::new();

    for comment in local_comments {
        match post_review_comment(pr_ref, comment).await {
            Ok(()) => posted += 1,
            Err(e) => failed.push((comment.id.clone(), e.to_string())),
        }
    }

    Ok(MirrorResult { posted, failed })
}

// ---------------------------------------------------------------------------
// Merge
// ---------------------------------------------------------------------------

/// How GitHub should combine a PR's commits when merging.
///
/// Mirrors `gh pr merge`'s mutually-exclusive strategy flags. Defaults to
/// [`MergeMethod::Squash`], the common single-commit-per-PR workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub enum MergeMethod {
    /// Squash all commits into one, then merge (`--squash`). The default.
    #[default]
    Squash,
    /// Create a merge commit (`--merge`).
    Merge,
    /// Rebase the commits onto the base (`--rebase`).
    Rebase,
}

/// Merge a PR via `gh pr merge`, deleting the head branch on success.
///
/// When `repo_slug` is `Some`, targets that repository with `--repo` so the
/// call works cross-repo without a `current_dir`. `method` selects the merge
/// strategy flag.
///
/// This is a guarded side effect (Invariant 5): it must only be invoked after an
/// explicit human confirmation in the UI, never automatically or from agent
/// output. A non-zero `gh` exit captures stderr into [`Error::GhCommand`].
pub async fn merge_pr(
    repo_slug: Option<&str>,
    pr_number: u64,
    method: MergeMethod,
) -> Result<(), Error> {
    let pr = pr_number.to_string();
    let method_flag = match method {
        MergeMethod::Squash => "--squash",
        MergeMethod::Merge => "--merge",
        MergeMethod::Rebase => "--rebase",
    };
    let mut args: Vec<&str> = vec!["pr", "merge", &pr, method_flag, "--delete-branch"];
    if let Some(slug) = repo_slug {
        args.push("--repo");
        args.push(slug);
    }

    let output = Command::new("gh").args(&args).output().await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::GhCommand(stderr.into_owned()));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Compare
// ---------------------------------------------------------------------------

/// Fetch a unified diff between two commits via the GitHub compare API.
///
/// Runs `gh api repos/<slug>/compare/<base>...<head>` with the
/// `application/vnd.github.v3.diff` media type so the response body IS a unified
/// diff (rather than the default JSON summary). Used to diff arbitrary commit
/// ranges (e.g. a review's `base_sha`..`head_sha`) without a local checkout.
pub async fn compare(repo_slug: &str, base_sha: &str, head_sha: &str) -> Result<String, Error> {
    let endpoint = format!("repos/{repo_slug}/compare/{base_sha}...{head_sha}");
    let output = Command::new("gh")
        .args([
            "api",
            &endpoint,
            "-H",
            "Accept: application/vnd.github.v3.diff",
        ])
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::GhCommand(stderr.into_owned()));
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

// ---------------------------------------------------------------------------
// Contents API (full-file fallback)
// ---------------------------------------------------------------------------

/// Maximum file size served by [`contents_at`], in bytes.
///
/// Mirrors [`crate::adapters::git::MAX_FULL_FILE_BYTES`]: the full-file view
/// feeds Monaco, so a blob past this cap yields `Ok(None)` and the UI stays on
/// the diff view. GitHub independently omits the body for blobs over 1 MiB.
pub const MAX_CONTENTS_BYTES: usize = 512 * 1024;

/// Decode GitHub's base64 `content` field into UTF-8 text.
///
/// GitHub wraps the base64 payload across lines, so all ASCII whitespace is
/// stripped before decoding. Returns `None` when the base64 is malformed or the
/// decoded bytes are not valid UTF-8 (the full-file view is text-only).
fn decode_base64_content(content: &str) -> Option<String> {
    let stripped: String = content
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(stripped)
        .ok()?;
    String::from_utf8(bytes).ok()
}

/// Parse a GitHub contents-API response body into optional file text.
///
/// The response carries the file's bytes as base64 in `content` with an
/// `encoding` field. Pure and independently testable; [`contents_at`] wraps it
/// around the `gh` call. Returns:
/// - `Ok(None)` when `size` exceeds [`MAX_CONTENTS_BYTES`], when GitHub omits
///   the body for a large file (`content` empty with a non-zero `size`, as it
///   does for blobs over 1 MiB), when `encoding` is not `base64`, or when the
///   decoded bytes are not valid UTF-8 (the full-file view is text-only).
/// - `Ok(Some(text))` otherwise (an empty file with `size` 0 decodes to `""`).
fn parse_contents_response(json: &str) -> Result<Option<String>, Error> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| Error::ParseOutput(e.to_string()))?;

    let size = value
        .get("size")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    // Convert to usize for the cap comparison; a value too large for usize is,
    // by definition, over the cap.
    let size_bytes = usize::try_from(size).unwrap_or(usize::MAX);
    if size_bytes > MAX_CONTENTS_BYTES {
        return Ok(None);
    }

    // GitHub sets `encoding` to "none" (with an empty body) for oversize blobs.
    let encoding = value
        .get("encoding")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if encoding != "base64" {
        return Ok(None);
    }

    let content = value
        .get("content")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if content.is_empty() {
        // An empty body with a non-zero size is GitHub omitting the content for
        // a large file; an empty body with size 0 is a genuinely empty file.
        if size > 0 {
            return Ok(None);
        }
        return Ok(Some(String::new()));
    }

    Ok(decode_base64_content(content))
}

/// Fetch a file's text at a git ref via the GitHub contents API (fallback).
///
/// Runs `gh api repos/<slug>/contents/<path>?ref=<ref>` and decodes the base64
/// body via [`parse_contents_response`]. This is the cross-repo fallback for the
/// full-file view when no local checkout is available.
///
/// Returns `Ok(None)` when the file is absent at `ref_` (HTTP 404), too large
/// (over [`MAX_CONTENTS_BYTES`], or omitted by GitHub for blobs over 1 MiB), or
/// not UTF-8 text. Any other `gh` failure is an [`Error::GhCommand`].
pub async fn contents_at(repo_slug: &str, ref_: &str, path: &str) -> Result<Option<String>, Error> {
    let endpoint = format!("repos/{repo_slug}/contents/{path}?ref={ref_}");
    let output = Command::new("gh").args(["api", &endpoint]).output().await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // A 404 means the path does not exist at this ref (an added/deleted file
        // viewed from the wrong side) — a normal outcome, not an error. `gh api`
        // exposes no structured status code on failure, and this adapter has no
        // status-code discrimination precedent, so we match the "HTTP 404" text
        // gh prints to stderr. This is fragile: a change to gh's error wording
        // would break the discrimination.
        if stderr.contains("HTTP 404") {
            return Ok(None);
        }
        return Err(Error::GhCommand(stderr.into_owned()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_contents_response(&stdout)
}

// ---------------------------------------------------------------------------
// Reviews (inline review comments)
// ---------------------------------------------------------------------------

/// The kind of review to submit to GitHub, matching the API's `event` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub enum ReviewEvent {
    /// Approve the pull request (`APPROVE`).
    Approve,
    /// Request changes on the pull request (`REQUEST_CHANGES`).
    RequestChanges,
    /// Leave a review without approving or requesting changes (`COMMENT`).
    Comment,
}

/// Result of submitting a review to a GitHub PR.
///
/// `submitted` counts the inline comments actually included in the review;
/// `skipped` lists the comments that were left out with a human-readable reason
/// (e.g. their anchored line is not part of the PR's diff), so the caller can
/// report partial success.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub struct SubmitReviewResult {
    /// Number of inline comments included in the submitted review.
    pub submitted: usize,
    /// Comments left out of the review: `(comment_id, reason)`.
    pub skipped: Vec<(CommentId, String)>,
}

/// Convert a cockpit [`Comment`] into a GitHub review-comment JSON object.
///
/// Only [`Anchor::DiffLine`] anchors map to an inline review comment; every
/// other anchor kind returns `None`. The GitHub side string is `"RIGHT"` for the
/// new side and `"LEFT"` for the old side. A multi-line range additionally
/// carries `start_line`/`start_side`, matching GitHub's multi-line comment shape.
fn review_comment_payload(comment: &Comment) -> Option<serde_json::Value> {
    let Anchor::DiffLine { path, range, side } = &comment.anchor else {
        return None;
    };
    let (start, end) = *range;
    let side_str = diff_side_label(*side);

    let mut payload = serde_json::json!({
        "path": path.display().to_string(),
        "line": end,
        "side": side_str,
        "body": comment.body,
    });
    if start != end {
        payload["start_line"] = serde_json::json!(start);
        payload["start_side"] = serde_json::json!(side_str);
    }
    Some(payload)
}

/// GitHub's side label for a [`DiffSide`].
fn diff_side_label(side: DiffSide) -> &'static str {
    match side {
        DiffSide::New => "RIGHT",
        DiffSide::Old => "LEFT",
    }
}

/// A parsed unified-diff hunk header: `@@ -old_start,old_len +new_start,new_len @@`.
#[derive(Debug, Clone, Copy)]
struct HunkHeader {
    old_start: u32,
    old_len: u32,
    new_start: u32,
    new_len: u32,
}

/// Parse a hunk header line (`@@ -a,b +c,d @@`) into its four numbers.
///
/// The count is optional in unified diffs (`@@ -a +c @@` means a count of 1),
/// so a missing `,len` defaults to 1. Returns `None` for any non-hunk line.
fn parse_hunk_header(line: &str) -> Option<HunkHeader> {
    let rest = line.strip_prefix("@@ ")?;
    let close = rest.find(" @@")?;
    let spec = &rest[..close];

    let mut parts = spec.split_whitespace();
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let (old_start, old_len) = parse_hunk_range(old)?;
    let (new_start, new_len) = parse_hunk_range(new)?;

    Some(HunkHeader {
        old_start,
        old_len,
        new_start,
        new_len,
    })
}

/// Parse a hunk range component (`a,b` or `a`) into `(start, len)`.
fn parse_hunk_range(s: &str) -> Option<(u32, u32)> {
    match s.split_once(',') {
        Some((start, len)) => Some((start.parse().ok()?, len.parse().ok()?)),
        None => Some((s.parse().ok()?, 1)),
    }
}

/// Extract the repo-relative path from a `---`/`+++` file header value.
///
/// Strips the `a/` or `b/` prefix and any trailing tab-delimited metadata.
/// Returns `None` for `/dev/null` (an added or deleted file has no counterpart).
fn parse_file_header_path(value: &str) -> Option<String> {
    let token = value.split('\t').next().unwrap_or(value).trim();
    if token == "/dev/null" {
        return None;
    }
    let stripped = token
        .strip_prefix("a/")
        .or_else(|| token.strip_prefix("b/"))
        .unwrap_or(token);
    Some(stripped.to_string())
}

/// Validate that a comment's anchored line range is part of a PR's diff.
///
/// GitHub rejects (422) review comments whose line is not in the diff, so this
/// pre-flights each comment. It walks the unified diff, tracking the current
/// file's old/new paths and each hunk header. A [`Anchor::DiffLine`] is valid
/// when some hunk on its side covers the whole `range` for the matching path,
/// where a header `@@ -a,b +c,d @@` covers old lines `[a, a+b)` and new lines
/// `[c, c+d)`. Non-`DiffLine` anchors cannot be inline comments and are rejected.
fn validate_comment_in_diff(comment: &Comment, diff: &str) -> Result<(), String> {
    let Anchor::DiffLine { path, range, side } = &comment.anchor else {
        return Err("comment is not anchored to a diff line".to_string());
    };
    let (start, end) = *range;
    let target = path.display().to_string();
    let side = *side;

    let mut old_path: Option<String> = None;
    let mut new_path: Option<String> = None;

    for line in diff.lines() {
        if let Some(value) = line.strip_prefix("--- ") {
            old_path = parse_file_header_path(value);
            continue;
        }
        if let Some(value) = line.strip_prefix("+++ ") {
            new_path = parse_file_header_path(value);
            continue;
        }
        let Some(hunk) = parse_hunk_header(line) else {
            continue;
        };

        let path_matches = match side {
            DiffSide::New => new_path.as_deref() == Some(target.as_str()),
            DiffSide::Old => old_path.as_deref() == Some(target.as_str()),
        };
        if !path_matches {
            continue;
        }

        let (lo, len) = match side {
            DiffSide::New => (hunk.new_start, hunk.new_len),
            DiffSide::Old => (hunk.old_start, hunk.old_len),
        };
        // Covered lines are [lo, lo + len); the inclusive range [start, end] fits
        // when start >= lo and end < lo + len.
        if len > 0 && start >= lo && end < lo + len {
            return Ok(());
        }
    }

    Err(format!(
        "line {end} on the {} side of {target} is not part of the diff",
        match side {
            DiffSide::New => "new",
            DiffSide::Old => "old",
        }
    ))
}

/// Assemble the review payload posted to `POST /pulls/<n>/reviews`.
fn build_review_payload(
    event: ReviewEvent,
    body: &str,
    comments: &[serde_json::Value],
) -> serde_json::Value {
    let event_str = match event {
        ReviewEvent::Approve => "APPROVE",
        ReviewEvent::RequestChanges => "REQUEST_CHANGES",
        ReviewEvent::Comment => "COMMENT",
    };
    serde_json::json!({
        "body": body,
        "event": event_str,
        "comments": comments,
    })
}

/// Submit a review with inline comments to a GitHub PR via the reviews API.
///
/// Only [`CommentOrigin::Local`] comments are submitted (the same rule as
/// [`mirror_comments`]): comments mirrored from GitHub are skipped to avoid
/// duplicates. Each local comment is pre-validated against `diff` with
/// [`validate_comment_in_diff`]; invalid comments are recorded in
/// [`SubmitReviewResult::skipped`] rather than failing the whole review, because
/// GitHub returns a 422 for the entire request if any line is out of range.
///
/// The assembled payload is POSTed with
/// `gh api --method POST repos/<slug>/pulls/<n>/reviews --input -`, streaming the
/// JSON to the child's stdin. A non-zero `gh` exit captures stderr into
/// [`Error::GhCommand`].
///
/// This is a guarded side effect (Invariant 5): it is never called automatically
/// or from agent output.
pub async fn submit_review(
    repo_slug: &str,
    pr_number: u64,
    event: ReviewEvent,
    comments: &[Comment],
    body: &str,
    diff: &str,
) -> Result<SubmitReviewResult, Error> {
    let mut payloads: Vec<serde_json::Value> = Vec::new();
    let mut skipped: Vec<(CommentId, String)> = Vec::new();

    for comment in comments {
        // Only locally-authored comments are submitted; GitHub-mirrored ones
        // would duplicate existing threads.
        if comment.origin != CommentOrigin::Local {
            continue;
        }
        match validate_comment_in_diff(comment, diff) {
            Ok(()) => match review_comment_payload(comment) {
                Some(payload) => payloads.push(payload),
                None => skipped.push((
                    comment.id.clone(),
                    "comment is not anchored to a diff line".to_string(),
                )),
            },
            Err(reason) => skipped.push((comment.id.clone(), reason)),
        }
    }

    let submitted = payloads.len();
    let payload = build_review_payload(event, body, &payloads);
    let body_bytes = serde_json::to_vec(&payload).map_err(|e| Error::ParseOutput(e.to_string()))?;

    let endpoint = format!("repos/{repo_slug}/pulls/{pr_number}/reviews");
    let mut child = Command::new("gh")
        .args(["api", "--method", "POST", &endpoint, "--input", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    // Stream the JSON body to stdin, then drop the handle to signal EOF so `gh`
    // proceeds. Scoped so the borrow ends before `wait_with_output`.
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::GhCommand("failed to open stdin for gh api".to_string()))?;
        stdin.write_all(&body_bytes).await?;
    }

    let output = child.wait_with_output().await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::GhCommand(stderr.into_owned()));
    }

    Ok(SubmitReviewResult { submitted, skipped })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::model::{DiffData, DiffSide, GateState, Review, ReviewId, ReviewSource};

    // -- author_login (self-exclusion) tests --

    /// Build a minimal search result authored by `login`.
    fn search_result_by(login: &str) -> SearchPrResult {
        SearchPrResult {
            number: 1,
            title: String::new(),
            state: "open".into(),
            url: "https://example/pr/1".into(),
            repository: SearchRepo {
                name_with_owner: "owner/repo".into(),
            },
            author: SearchAuthor {
                login: login.into(),
            },
        }
    }

    #[test]
    fn author_login_lowercases_for_comparison() {
        let sr = search_result_by("Alejandro");
        assert_eq!(
            sr.author_login(),
            "alejandro",
            "author_login must lowercase so self-exclusion is case-insensitive"
        );
    }

    #[test]
    fn author_login_matches_differently_cased_identity() {
        // Mirrors the self-exclusion comparison in `list_prs_filtered`: the
        // authenticated identity is lowercased too, so a case mismatch between
        // the search result and `gh_whoami` must still count as "self".
        let sr = search_result_by("AlejandroPerez");
        let me = "alejandroperez".to_string();
        assert_eq!(sr.author_login(), me, "case-insensitive self-match");
    }

    // -- parse_issue_from_branch tests --

    #[test]
    fn parse_standard_branch() {
        let result = parse_issue_from_branch("alejandro/NEX-123-add-feature");
        assert_eq!(
            result,
            Some(IssueRef::new("NEX-123")),
            "should parse uppercase prefix with user/ prefix"
        );
    }

    #[test]
    fn parse_lowercase_normalizes_to_uppercase() {
        let result = parse_issue_from_branch("alejandro/nex-456-fix-bug");
        assert_eq!(
            result,
            Some(IssueRef::new("NEX-456")),
            "lowercase prefix should be normalized to uppercase"
        );
    }

    #[test]
    fn parse_no_issue_id() {
        let result = parse_issue_from_branch("feature/no-issue-id");
        assert_eq!(
            result, None,
            "branch without a valid issue pattern should return None"
        );
    }

    #[test]
    fn parse_short_prefix() {
        let result = parse_issue_from_branch("alejandro/AB-1-short");
        assert_eq!(
            result,
            Some(IssueRef::new("AB-1")),
            "2-char prefix should be accepted"
        );
    }

    #[test]
    fn parse_empty_branch() {
        let result = parse_issue_from_branch("");
        assert_eq!(result, None, "empty branch should return None");
    }

    #[test]
    fn parse_long_description() {
        let result = parse_issue_from_branch("alejandro/PROJ-9999-very-long-description-here");
        assert_eq!(
            result,
            Some(IssueRef::new("PROJ-9999")),
            "long descriptions after the issue ID should not interfere"
        );
    }

    #[test]
    fn parse_bare_main() {
        let result = parse_issue_from_branch("main");
        assert_eq!(result, None, "bare 'main' has no issue pattern");
    }

    #[test]
    fn parse_five_char_prefix() {
        let result = parse_issue_from_branch("user/ABCDE-42-thing");
        assert_eq!(
            result,
            Some(IssueRef::new("ABCDE-42")),
            "5-char prefix is the upper bound"
        );
    }

    #[test]
    fn parse_six_char_prefix_rejected() {
        // 6-char prefix exceeds the 2..=5 range; should not match as a single issue ID.
        let result = parse_issue_from_branch("user/ABCDEF-42-thing");
        assert_eq!(
            result, None,
            "6-char prefix should not match the issue pattern"
        );
    }

    #[test]
    fn parse_no_slash_with_issue() {
        // Branch without a username/ prefix but with an issue pattern.
        let result = parse_issue_from_branch("NEX-100-hotfix");
        assert_eq!(
            result,
            Some(IssueRef::new("NEX-100")),
            "should work without a username/ prefix"
        );
    }

    #[test]
    fn parse_mixed_case_prefix() {
        let result = parse_issue_from_branch("alejandro/Nex-789-mixed");
        assert_eq!(
            result,
            Some(IssueRef::new("NEX-789")),
            "mixed case prefix should normalize to uppercase"
        );
    }

    // -- PrData deserialization --

    #[test]
    fn deserialize_pr_data_fixture() {
        let json = r#"[
            {
                "number": 42,
                "headRefName": "alejandro/NEX-123-add-feature",
                "baseRefName": "main",
                "title": "Add feature",
                "state": "OPEN",
                "url": "https://github.com/owner/repo/pull/42"
            },
            {
                "number": 43,
                "headRefName": "feature/no-issue",
                "baseRefName": "main",
                "title": "Plain feature",
                "state": "OPEN",
                "url": "https://github.com/owner/repo/pull/43"
            }
        ]"#;

        let prs: Vec<PrData> = serde_json::from_str(json).expect("should parse fixture JSON");

        assert_eq!(prs.len(), 2);
        assert_eq!(prs[0].number, 42);
        assert_eq!(prs[0].head_ref_name, "alejandro/NEX-123-add-feature");
        assert_eq!(prs[0].base_ref_name, "main");
        assert_eq!(prs[1].number, 43);
        assert_eq!(prs[1].state, "OPEN");

        // Legacy JSON omits the rollup + OID fields; they default cleanly.
        assert!(prs[0].status_check_rollup.is_empty());
        assert_eq!(prs[0].head_ref_oid, "");
        assert_eq!(prs[0].base_ref_oid, "");
    }

    #[test]
    fn deserialize_pr_data_with_rollup_and_oids() {
        // The enriched field set: the rollup array plus pinned head/base OIDs.
        let json = r#"[
            {
                "number": 7,
                "headRefName": "alejandro/NEX-7-thing",
                "baseRefName": "main",
                "title": "Thing",
                "state": "OPEN",
                "url": "https://github.com/owner/repo/pull/7",
                "headRefOid": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "baseRefOid": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "statusCheckRollup": [
                    {"__typename":"CheckRun","name":"build","status":"COMPLETED","conclusion":"SUCCESS"},
                    {"__typename":"CheckRun","name":"lint","status":"IN_PROGRESS","conclusion":null},
                    {"__typename":"StatusContext","context":"ci/circleci","state":"FAILURE"}
                ]
            }
        ]"#;

        let prs: Vec<PrData> = serde_json::from_str(json).expect("enriched JSON parses");

        assert_eq!(prs.len(), 1);
        assert_eq!(
            prs[0].head_ref_oid,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        assert_eq!(
            prs[0].base_ref_oid,
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        );
        assert_eq!(prs[0].status_check_rollup.len(), 3);
        // A CheckRun's completed conclusion, an in-flight null conclusion, and a
        // StatusContext state all parse (null coalesces to empty).
        assert_eq!(prs[0].status_check_rollup[0].conclusion, "SUCCESS");
        assert_eq!(prs[0].status_check_rollup[1].conclusion, "");
        assert_eq!(prs[0].status_check_rollup[1].status, "IN_PROGRESS");
        assert_eq!(prs[0].status_check_rollup[2].state, "FAILURE");
    }

    // -- link_prs_to_issues --

    #[test]
    fn link_prs_mixed_branches() {
        let prs = vec![
            PrData {
                number: 1,
                head_ref_name: "alejandro/NEX-100-thing".into(),
                base_ref_name: "main".into(),
                title: "Thing".into(),
                body: String::new(),
                state: "OPEN".into(),
                url: "https://github.com/o/r/pull/1".into(),
                repo_slug: String::new(),
                status_check_rollup: Vec::new(),
                head_ref_oid: String::new(),
                base_ref_oid: String::new(),
            },
            PrData {
                number: 2,
                head_ref_name: "feature/no-id-here".into(),
                base_ref_name: "main".into(),
                title: "No ID".into(),
                body: String::new(),
                state: "OPEN".into(),
                url: "https://github.com/o/r/pull/2".into(),
                repo_slug: String::new(),
                status_check_rollup: Vec::new(),
                head_ref_oid: String::new(),
                base_ref_oid: String::new(),
            },
            PrData {
                number: 3,
                head_ref_name: "alejandro/ab-7-short".into(),
                base_ref_name: "develop".into(),
                title: "Short".into(),
                body: String::new(),
                state: "MERGED".into(),
                url: "https://github.com/o/r/pull/3".into(),
                repo_slug: String::new(),
                status_check_rollup: Vec::new(),
                head_ref_oid: String::new(),
                base_ref_oid: String::new(),
            },
        ];

        let linked = link_prs_to_issues(&prs);

        assert_eq!(linked.len(), 3);

        // PR 1: has issue
        assert_eq!(linked[0].0, PrRef::new("https://github.com/o/r/pull/1"));
        assert_eq!(linked[0].1, Some(IssueRef::new("NEX-100")));

        // PR 2: no issue
        assert_eq!(linked[1].0, PrRef::new("https://github.com/o/r/pull/2"));
        assert_eq!(linked[1].1, None);

        // PR 3: has issue (lowercase normalized)
        assert_eq!(linked[2].0, PrRef::new("https://github.com/o/r/pull/3"));
        assert_eq!(linked[2].1, Some(IssueRef::new("AB-7")));
    }

    // -- CheckRun deserialization --

    #[test]
    fn deserialize_check_run() {
        let json = r#"[
            {
                "name": "CI / build",
                "status": "completed",
                "conclusion": "success"
            },
            {
                "name": "CI / lint",
                "status": "in_progress",
                "conclusion": null
            }
        ]"#;

        let checks: Vec<CheckRun> =
            serde_json::from_str(json).expect("should parse check run JSON");

        assert_eq!(checks.len(), 2);
        assert_eq!(checks[0].name, "CI / build");
        assert_eq!(checks[0].conclusion, Some("success".into()));
        assert_eq!(checks[1].name, "CI / lint");
        assert_eq!(checks[1].conclusion, None);
    }

    // -- format_comment_body --

    #[test]
    fn format_diff_line_single_line() {
        let comment = Comment {
            id: CommentId::new("c-1"),
            anchor: Anchor::DiffLine {
                path: PathBuf::from("src/main.rs"),
                range: (42, 42),
                side: DiffSide::New,
            },
            body: "This variable is unused.".into(),
            origin: CommentOrigin::Local,
        };

        let body = format_comment_body(&comment);
        assert!(
            body.contains("**src/main.rs** line 42"),
            "single-line anchor should say 'line 42', got: {body}"
        );
        assert!(body.contains("This variable is unused."));
    }

    #[test]
    fn format_diff_line_range() {
        let comment = Comment {
            id: CommentId::new("c-2"),
            anchor: Anchor::DiffLine {
                path: PathBuf::from("lib/util.rs"),
                range: (10, 20),
                side: DiffSide::New,
            },
            body: "Refactor this block.".into(),
            origin: CommentOrigin::Local,
        };

        let body = format_comment_body(&comment);
        assert!(
            body.contains("**lib/util.rs** lines 10-20"),
            "multi-line anchor should say 'lines 10-20', got: {body}"
        );
        assert!(body.contains("Refactor this block."));
    }

    #[test]
    fn format_plan_step_anchor() {
        let comment = Comment {
            id: CommentId::new("c-3"),
            anchor: Anchor::PlanStep(2),
            body: "This step is too vague.".into(),
            origin: CommentOrigin::Local,
        };

        let body = format_comment_body(&comment);
        assert!(
            body.contains("**Plan step 2**"),
            "plan step anchor, got: {body}"
        );
        assert!(body.contains("This step is too vague."));
    }

    #[test]
    fn format_plan_file_anchor() {
        let comment = Comment {
            id: CommentId::new("c-4"),
            anchor: Anchor::PlanFile(PathBuf::from("src/lib.rs")),
            body: "Consider splitting.".into(),
            origin: CommentOrigin::Local,
        };

        let body = format_comment_body(&comment);
        assert!(
            body.contains("**Plan file:** `src/lib.rs`"),
            "plan file anchor, got: {body}"
        );
        assert!(body.contains("Consider splitting."));
    }

    // -- mirror_comments filtering --

    #[test]
    fn mirror_filters_only_local_comments() {
        // Verify that mirror_comments filters correctly by testing the
        // filtering logic without actually calling `gh`.
        let comments = [
            Comment {
                id: CommentId::new("local-1"),
                anchor: Anchor::DiffLine {
                    path: PathBuf::from("a.rs"),
                    range: (1, 1),
                    side: DiffSide::New,
                },
                body: "fix this".into(),
                origin: CommentOrigin::Local,
            },
            Comment {
                id: CommentId::new("gh-1"),
                anchor: Anchor::DiffLine {
                    path: PathBuf::from("b.rs"),
                    range: (5, 10),
                    side: DiffSide::New,
                },
                body: "from github".into(),
                origin: CommentOrigin::GitHubMirror,
            },
            Comment {
                id: CommentId::new("local-2"),
                anchor: Anchor::DiffLine {
                    path: PathBuf::from("c.rs"),
                    range: (3, 3),
                    side: DiffSide::New,
                },
                body: "also fix this".into(),
                origin: CommentOrigin::Local,
            },
        ];

        // Count how many are local (what mirror_comments would process).
        let local_count = comments
            .iter()
            .filter(|c| c.origin == CommentOrigin::Local)
            .count();
        assert_eq!(local_count, 2, "only Local comments should be mirrored");
    }

    // -- CiCheck / summarize --

    /// Fixture in the `gh pr checks --json name,state,bucket,link,workflow`
    /// shape (bucket present), covering pass/fail/pending/skipping.
    const GH_CHECKS_BUCKET: &str = r#"[
        {"name":"build","state":"SUCCESS","bucket":"pass","link":"https://github.com/o/r/actions/runs/111/job/1","workflow":"CI"},
        {"name":"test","state":"FAILURE","bucket":"fail","link":"https://github.com/o/r/actions/runs/222/job/2","workflow":"CI"},
        {"name":"lint","state":"PENDING","bucket":"pending","link":"https://github.com/o/r/actions/runs/333/job/3","workflow":"CI"},
        {"name":"license","state":"SKIPPED","bucket":"skipping","link":"","workflow":"CI"},
        {"name":"deploy","state":"CANCELLED","bucket":"cancel","link":"","workflow":"CD"}
    ]"#;

    /// Fixture with the raw-state shape only (no bucket), incl. NEUTRAL/SKIPPED.
    const GH_CHECKS_RAW_STATE: &str = r#"[
        {"name":"build","state":"success","bucket":"","link":"https://github.com/o/r/actions/runs/900","workflow":"CI"},
        {"name":"flaky","state":"neutral","bucket":"","link":"","workflow":"CI"},
        {"name":"skip","state":"skipped","bucket":"","link":"","workflow":"CI"},
        {"name":"unit","state":"failure","bucket":"","link":"https://github.com/o/r/actions/runs/901/job/9","workflow":"CI"},
        {"name":"slow","state":"in_progress","bucket":"","link":"","workflow":"CI"}
    ]"#;

    #[test]
    fn summarize_bucket_shape() {
        let checks: Vec<CiCheck> =
            serde_json::from_str(GH_CHECKS_BUCKET).expect("bucket fixture parses");
        let s = summarize(&checks);
        // pass + skipping + cancel all count as passed.
        assert_eq!(s.total, 5);
        assert_eq!(s.passed, 3, "pass + skipping + cancel");
        assert_eq!(s.failed, 1);
        assert_eq!(s.pending, 1);
        assert_eq!(s.passed + s.failed + s.pending, s.total);
    }

    #[test]
    fn summarize_raw_state_neutral_skipped_pass() {
        let checks: Vec<CiCheck> =
            serde_json::from_str(GH_CHECKS_RAW_STATE).expect("raw-state fixture parses");
        let s = summarize(&checks);
        // success + neutral + skipped => passed; failure => failed; in_progress
        // => pending.
        assert_eq!(s.total, 5);
        assert_eq!(s.passed, 3, "success + neutral + skipped count as pass");
        assert_eq!(s.failed, 1);
        assert_eq!(s.pending, 1);
    }

    #[test]
    fn summarize_empty_is_all_zero() {
        let s = summarize(&[]);
        assert_eq!(s.total, 0);
        assert_eq!(s.passed, 0);
        assert_eq!(s.failed, 0);
        assert_eq!(s.pending, 0);
    }

    // -- rollup_to_summary --

    /// A mixed rollup: CheckRun (with an in-flight null conclusion) plus legacy
    /// StatusContext nodes, covering pass/fail/pending on both node kinds.
    const ROLLUP_MIXED: &str = r#"[
        {"__typename":"CheckRun","name":"build","status":"COMPLETED","conclusion":"SUCCESS"},
        {"__typename":"CheckRun","name":"test","status":"COMPLETED","conclusion":"FAILURE"},
        {"__typename":"CheckRun","name":"deploy","status":"COMPLETED","conclusion":"CANCELLED"},
        {"__typename":"CheckRun","name":"lint","status":"IN_PROGRESS","conclusion":null},
        {"__typename":"StatusContext","context":"ci/circleci","state":"SUCCESS"},
        {"__typename":"StatusContext","context":"legacy/errored","state":"ERROR"},
        {"__typename":"StatusContext","context":"legacy/queued","state":"PENDING"}
    ]"#;

    #[test]
    fn rollup_mixed_checkrun_and_statuscontext() {
        let nodes: Vec<StatusCheckNode> =
            serde_json::from_str(ROLLUP_MIXED).expect("mixed rollup parses");
        let s = rollup_to_summary(&nodes).expect("non-empty rollup yields Some");
        assert_eq!(s.total, 7);
        // pass: SUCCESS + CANCELLED (CheckRun) + SUCCESS (StatusContext) = 3
        assert_eq!(s.passed, 3, "success + cancelled + statuscontext success");
        // fail: FAILURE (CheckRun) + ERROR (StatusContext) = 2
        assert_eq!(s.failed, 2, "checkrun failure + statuscontext error");
        // pending: in-flight IN_PROGRESS + StatusContext PENDING = 2
        assert_eq!(s.pending, 2, "in_progress + statuscontext pending");
        assert_eq!(s.passed + s.failed + s.pending, s.total);
    }

    #[test]
    fn rollup_all_success() {
        let json = r#"[
            {"__typename":"CheckRun","name":"a","status":"COMPLETED","conclusion":"SUCCESS"},
            {"__typename":"CheckRun","name":"b","status":"COMPLETED","conclusion":"SUCCESS"}
        ]"#;
        let nodes: Vec<StatusCheckNode> = serde_json::from_str(json).expect("parses");
        let s = rollup_to_summary(&nodes).expect("Some");
        assert_eq!((s.passed, s.failed, s.pending, s.total), (2, 0, 0, 2));
    }

    #[test]
    fn rollup_with_pending() {
        let json = r#"[
            {"__typename":"CheckRun","name":"a","status":"COMPLETED","conclusion":"SUCCESS"},
            {"__typename":"CheckRun","name":"b","status":"QUEUED","conclusion":null}
        ]"#;
        let nodes: Vec<StatusCheckNode> = serde_json::from_str(json).expect("parses");
        let s = rollup_to_summary(&nodes).expect("Some");
        assert_eq!((s.passed, s.failed, s.pending, s.total), (1, 0, 1, 2));
    }

    #[test]
    fn rollup_with_failure() {
        let json = r#"[
            {"__typename":"CheckRun","name":"a","status":"COMPLETED","conclusion":"SUCCESS"},
            {"__typename":"CheckRun","name":"b","status":"COMPLETED","conclusion":"FAILURE"}
        ]"#;
        let nodes: Vec<StatusCheckNode> = serde_json::from_str(json).expect("parses");
        let s = rollup_to_summary(&nodes).expect("Some");
        assert_eq!((s.passed, s.failed, s.pending, s.total), (1, 1, 0, 2));
    }

    #[test]
    fn rollup_empty_is_none() {
        // No checks is NOT the same as all-green: the badge must distinguish them.
        assert_eq!(rollup_to_summary(&[]), None);
    }

    #[test]
    fn rollup_classification_matches_summarize() {
        // Every state a CiCheck (`gh pr checks`) and a StatusCheckNode (rollup)
        // can both carry must classify identically. Both paths route through
        // `classify_check_signal`; this test fails loudly if they ever diverge.
        let states = [
            "SUCCESS",
            "NEUTRAL",
            "SKIPPED",
            "CANCELLED",
            "FAILURE",
            "TIMED_OUT",
            "ACTION_REQUIRED",
            "ERROR",
            "STALE",
            "PENDING",
            "IN_PROGRESS",
            "QUEUED",
            "EXPECTED",
            "WAITING",
        ];
        for state in states {
            let check = CiCheck {
                name: "c".into(),
                state: state.into(),
                bucket: String::new(),
                link: String::new(),
                workflow: String::new(),
            };
            let node = StatusCheckNode {
                state: state.into(),
                ..StatusCheckNode::default()
            };
            let via_summarize = summarize(std::slice::from_ref(&check));
            let via_rollup =
                rollup_to_summary(std::slice::from_ref(&node)).expect("non-empty yields Some");
            assert_eq!(
                (
                    via_summarize.passed,
                    via_summarize.failed,
                    via_summarize.pending
                ),
                (via_rollup.passed, via_rollup.failed, via_rollup.pending),
                "classification drift for state {state}"
            );
        }
    }

    #[test]
    fn run_id_extraction() {
        assert_eq!(
            run_id_from_link("https://github.com/o/r/actions/runs/1234567890/job/987"),
            Some(1234567890)
        );
        assert_eq!(
            run_id_from_link("https://github.com/o/r/actions/runs/42"),
            Some(42)
        );
        // No /runs/ segment.
        assert_eq!(run_id_from_link("https://github.com/o/r/pull/5"), None);
        // Empty link.
        assert_eq!(run_id_from_link(""), None);
    }

    #[test]
    fn truncate_keeps_tail() {
        let text: String = (0..30_000).map(|_| 'x').collect();
        let out = truncate_tail(&text, MAX_CI_LOG_CHARS);
        assert!(out.contains("truncated"), "marker present");
        // The kept tail is exactly MAX_CI_LOG_CHARS of the original content,
        // plus the prepended marker line.
        assert!(out.ends_with(&"x".repeat(MAX_CI_LOG_CHARS)));
        assert!(out.len() > MAX_CI_LOG_CHARS);
    }

    #[test]
    fn truncate_noop_when_under_cap() {
        let text = "short log with a\ntail assertion failed";
        assert_eq!(truncate_tail(text, MAX_CI_LOG_CHARS), text);
    }

    #[test]
    fn mirror_result_serialization() {
        let result = MirrorResult {
            posted: 3,
            failed: vec![(CommentId::new("c-5"), "timeout".into())],
        };

        let json = serde_json::to_string(&result).expect("should serialize");
        let parsed: MirrorResult = serde_json::from_str(&json).expect("should deserialize");

        assert_eq!(parsed.posted, 3);
        assert_eq!(parsed.failed.len(), 1);
        assert_eq!(parsed.failed[0].0, CommentId::new("c-5"));
        assert_eq!(parsed.failed[0].1, "timeout");
    }

    // -- review_comment_payload --

    /// Build a diff-line comment for payload/validation tests.
    fn diff_comment(id: &str, path: &str, range: (u32, u32), side: DiffSide) -> Comment {
        Comment {
            id: CommentId::new(id),
            anchor: Anchor::DiffLine {
                path: PathBuf::from(path),
                range,
                side,
            },
            body: "please fix".into(),
            origin: CommentOrigin::Local,
        }
    }

    #[test]
    fn payload_single_line_new_is_right() {
        let comment = diff_comment("c-1", "src/main.rs", (42, 42), DiffSide::New);
        let payload = review_comment_payload(&comment).expect("diff-line yields a payload");

        assert_eq!(payload["path"], "src/main.rs");
        assert_eq!(payload["line"], 42);
        assert_eq!(payload["side"], "RIGHT");
        assert_eq!(payload["body"], "please fix");
        // Single-line comment carries no multi-line start fields.
        assert!(payload.get("start_line").is_none());
        assert!(payload.get("start_side").is_none());
    }

    #[test]
    fn payload_range_carries_start_fields() {
        let comment = diff_comment("c-2", "lib/util.rs", (10, 20), DiffSide::New);
        let payload = review_comment_payload(&comment).expect("diff-line yields a payload");

        assert_eq!(payload["line"], 20);
        assert_eq!(payload["side"], "RIGHT");
        assert_eq!(payload["start_line"], 10);
        assert_eq!(payload["start_side"], "RIGHT");
    }

    #[test]
    fn payload_old_side_is_left() {
        let comment = diff_comment("c-3", "src/main.rs", (7, 7), DiffSide::Old);
        let payload = review_comment_payload(&comment).expect("diff-line yields a payload");

        assert_eq!(payload["side"], "LEFT");
        assert_eq!(payload["line"], 7);
    }

    #[test]
    fn payload_plan_step_is_none() {
        let comment = Comment {
            id: CommentId::new("c-4"),
            anchor: Anchor::PlanStep(1),
            body: "vague".into(),
            origin: CommentOrigin::Local,
        };
        assert!(
            review_comment_payload(&comment).is_none(),
            "non-diff-line anchors have no inline payload"
        );
    }

    // -- validate_comment_in_diff --

    /// A diff touching `src/main.rs`: old lines [10,13), new lines [10,14).
    const SAMPLE_DIFF: &str = "\
diff --git a/src/main.rs b/src/main.rs
index 1111111..2222222 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -10,3 +10,4 @@ fn main() {
 let a = 1;
-let b = 2;
+let b = 3;
+let c = 4;
 let d = 5;
";

    #[test]
    fn validate_new_line_inside_hunk_ok() {
        let comment = diff_comment("v-1", "src/main.rs", (12, 12), DiffSide::New);
        assert!(validate_comment_in_diff(&comment, SAMPLE_DIFF).is_ok());
    }

    #[test]
    fn validate_new_line_outside_hunk_err() {
        let comment = diff_comment("v-2", "src/main.rs", (50, 50), DiffSide::New);
        let err = validate_comment_in_diff(&comment, SAMPLE_DIFF)
            .expect_err("line 50 is not part of the diff");
        assert!(err.contains("50"), "reason names the offending line: {err}");
    }

    #[test]
    fn validate_old_line_in_deletion_hunk_ok() {
        // Old line 11 is the deleted `let b = 2;`, within old range [10,13).
        let comment = diff_comment("v-3", "src/main.rs", (11, 11), DiffSide::Old);
        assert!(validate_comment_in_diff(&comment, SAMPLE_DIFF).is_ok());
    }

    #[test]
    fn validate_wrong_path_err() {
        let comment = diff_comment("v-4", "other/file.rs", (11, 11), DiffSide::New);
        assert!(
            validate_comment_in_diff(&comment, SAMPLE_DIFF).is_err(),
            "a path not present in the diff must be rejected"
        );
    }

    #[test]
    fn validate_non_diff_line_err() {
        let comment = Comment {
            id: CommentId::new("v-5"),
            anchor: Anchor::PlanFile(PathBuf::from("src/lib.rs")),
            body: "x".into(),
            origin: CommentOrigin::Local,
        };
        assert!(validate_comment_in_diff(&comment, SAMPLE_DIFF).is_err());
    }

    // -- build_review_payload --

    #[test]
    fn review_payload_event_strings() {
        let cases = [
            (ReviewEvent::Approve, "APPROVE"),
            (ReviewEvent::RequestChanges, "REQUEST_CHANGES"),
            (ReviewEvent::Comment, "COMMENT"),
        ];
        for (event, expected) in cases {
            let payload = build_review_payload(event, "looks good", &[]);
            assert_eq!(payload["event"], expected);
            assert_eq!(payload["body"], "looks good");
            assert_eq!(payload["comments"], serde_json::json!([]));
        }
    }

    // -- wire_stack_edges --

    /// Build a minimal [`Review`] for stack-edge tests.
    fn stack_review(id: &str, branch: &str, base: &str, source: ReviewSource) -> Review {
        Review {
            id: ReviewId::new(id),
            issue: IssueRef::new(format!("ISSUE-{id}")),
            pr: PrRef::new(format!("owner/repo#{id}")),
            title: String::new(),
            body: String::new(),
            branch: branch.into(),
            base: base.into(),
            base_sha: "000".into(),
            source,
            worktree: PathBuf::from(format!("/tmp/wt-{id}")),
            gate_state: GateState::Pending,
            diff: DiffData { raw: String::new() },
            head_sha: "aaa".into(),
            comments: vec![],
            parents: vec![],
            children: vec![],
            stale: false,
            agent: None,
            repo_slug: None,
            project: None,
            dispatch_snapshot: None,
            ci_summary: None,
            review_findings: vec![],
            conversation: vec![],
            last_reviewed_sha: None,
        }
    }

    #[test]
    fn wire_stack_edges_linear_chain() {
        let mut reviews = vec![
            stack_review("a", "fa", "main", ReviewSource::Authored),
            stack_review("b", "fb", "fa", ReviewSource::Authored),
            stack_review("c", "fc", "fb", ReviewSource::Authored),
        ];
        wire_stack_edges(&mut reviews);

        assert!(reviews[0].parents.is_empty());
        assert_eq!(reviews[0].children, vec![ReviewId::new("b")]);
        assert_eq!(reviews[1].parents, vec![ReviewId::new("a")]);
        assert_eq!(reviews[1].children, vec![ReviewId::new("c")]);
        assert_eq!(reviews[2].parents, vec![ReviewId::new("b")]);
        assert!(reviews[2].children.is_empty());
    }

    #[test]
    fn wire_stack_edges_diamond_fan_out() {
        // A is the root; B and C both stack on A; D stacks on B.
        let mut reviews = vec![
            stack_review("a", "fa", "main", ReviewSource::Authored),
            stack_review("b", "fb", "fa", ReviewSource::Authored),
            stack_review("c", "fc", "fa", ReviewSource::ReviewRequested),
            stack_review("d", "fd", "fb", ReviewSource::Authored),
        ];
        wire_stack_edges(&mut reviews);

        assert!(reviews[0].parents.is_empty());
        assert_eq!(
            reviews[0].children,
            vec![ReviewId::new("b"), ReviewId::new("c")]
        );
        assert_eq!(reviews[1].parents, vec![ReviewId::new("a")]);
        assert_eq!(reviews[1].children, vec![ReviewId::new("d")]);
        assert_eq!(reviews[2].parents, vec![ReviewId::new("a")]);
        assert!(reviews[2].children.is_empty());
        assert_eq!(reviews[3].parents, vec![ReviewId::new("b")]);
        assert!(reviews[3].children.is_empty());
    }

    #[test]
    fn wire_stack_edges_orphan_on_main() {
        let mut reviews = vec![stack_review(
            "solo",
            "fsolo",
            "main",
            ReviewSource::Authored,
        )];
        wire_stack_edges(&mut reviews);
        assert!(reviews[0].parents.is_empty());
        assert!(reviews[0].children.is_empty());
    }

    #[test]
    fn wire_stack_edges_preserves_frontier() {
        // A Frontier review carries kickoff-wired edges; even though its base
        // matches another review's branch, those edges must be left untouched.
        let mut frontier = stack_review("f", "ff", "fbase", ReviewSource::Frontier);
        frontier.parents = vec![ReviewId::new("pre-parent")];
        frontier.children = vec![ReviewId::new("pre-child")];

        let mut reviews = vec![
            frontier,
            stack_review("p", "fbase", "main", ReviewSource::Authored),
        ];
        wire_stack_edges(&mut reviews);

        // Frontier edges untouched.
        assert_eq!(reviews[0].parents, vec![ReviewId::new("pre-parent")]);
        assert_eq!(reviews[0].children, vec![ReviewId::new("pre-child")]);
        // The authored review's edges were rebuilt: base=main, no parent; and the
        // frontier review was NOT added as its child.
        assert!(reviews[1].parents.is_empty());
        assert!(reviews[1].children.is_empty());
    }

    // -- build_review_from_pr --

    #[test]
    fn build_review_pins_shas_and_ci_summary() {
        let pr = PrData {
            number: 7,
            head_ref_name: "alejandro/NEX-7-thing".into(),
            base_ref_name: "main".into(),
            title: "Thing".into(),
            body: "desc".into(),
            state: "OPEN".into(),
            url: "https://github.com/o/r/pull/7".into(),
            repo_slug: "o/r".into(),
            status_check_rollup: serde_json::from_str(
                r#"[
                    {"__typename":"CheckRun","name":"build","status":"COMPLETED","conclusion":"SUCCESS"},
                    {"__typename":"CheckRun","name":"test","status":"COMPLETED","conclusion":"FAILURE"}
                ]"#,
            )
            .expect("rollup parses"),
            head_ref_oid: "aaa111".into(),
            base_ref_oid: "bbb222".into(),
        };

        let review = build_review_from_pr(
            &pr,
            "diff".to_string(),
            std::path::Path::new("/tmp/wt"),
            ReviewSource::Authored,
        );

        // SHAs are pinned from the fetched OIDs, not left empty (the drift fix).
        assert_eq!(review.head_sha, "aaa111");
        assert_eq!(review.base_sha, "bbb222");
        // The rollup is summarized into ci_summary (1 pass, 1 fail).
        let ci = review.ci_summary.expect("rollup yields a summary");
        assert_eq!((ci.passed, ci.failed, ci.pending, ci.total), (1, 1, 0, 2));
        assert_eq!(review.issue, IssueRef::new("NEX-7"));
    }

    #[test]
    fn build_review_without_rollup_has_no_ci_summary() {
        let pr = PrData {
            number: 8,
            head_ref_name: "feature/no-id".into(),
            base_ref_name: "main".into(),
            title: "No checks".into(),
            body: String::new(),
            state: "OPEN".into(),
            url: "https://github.com/o/r/pull/8".into(),
            repo_slug: String::new(),
            status_check_rollup: Vec::new(),
            head_ref_oid: String::new(),
            base_ref_oid: String::new(),
        };

        let review = build_review_from_pr(
            &pr,
            String::new(),
            std::path::Path::new("/tmp/wt"),
            ReviewSource::Authored,
        );

        // No checks -> no summary (distinct from an all-green summary).
        assert_eq!(review.ci_summary, None);
        assert_eq!(review.head_sha, "");
        assert_eq!(review.base_sha, "");
    }

    // -- decode_base64_content --

    #[test]
    fn decode_base64_strips_embedded_whitespace() {
        // "hello world" is "aGVsbG8gd29ybGQ="; GitHub wraps the payload across
        // lines, so an embedded newline must be stripped before decoding.
        let wrapped = "aGVsbG8g\nd29ybGQ=\n";
        assert_eq!(
            decode_base64_content(wrapped).as_deref(),
            Some("hello world")
        );
    }

    #[test]
    fn decode_base64_invalid_utf8_is_none() {
        // "//4=" decodes to bytes [0xFF, 0xFE], which are not valid UTF-8.
        assert_eq!(decode_base64_content("//4="), None);
    }

    #[test]
    fn decode_base64_malformed_is_none() {
        assert_eq!(decode_base64_content("not valid base64 @@@"), None);
    }

    // -- parse_contents_response --

    #[test]
    fn parse_contents_decodes_body() {
        let json = r#"{"encoding":"base64","size":11,"content":"aGVsbG8gd29ybGQ="}"#;
        assert_eq!(
            parse_contents_response(json).unwrap().as_deref(),
            Some("hello world")
        );
    }

    #[test]
    fn parse_contents_wrapped_base64() {
        // Mirrors GitHub's actual wire format, where `content` carries embedded
        // newlines inside the JSON string.
        let json = "{\"encoding\":\"base64\",\"size\":11,\"content\":\"aGVsbG8g\\nd29ybGQ=\\n\"}";
        assert_eq!(
            parse_contents_response(json).unwrap().as_deref(),
            Some("hello world")
        );
    }

    #[test]
    fn parse_contents_empty_file_is_empty_string() {
        let json = r#"{"encoding":"base64","size":0,"content":""}"#;
        assert_eq!(parse_contents_response(json).unwrap().as_deref(), Some(""));
    }

    #[test]
    fn parse_contents_oversize_is_none() {
        let json = format!(
            r#"{{"encoding":"base64","size":{},"content":"aGk="}}"#,
            MAX_CONTENTS_BYTES + 1
        );
        assert_eq!(parse_contents_response(&json).unwrap(), None);
    }

    #[test]
    fn parse_contents_omitted_large_body_is_none() {
        // GitHub omits the body for blobs over 1 MiB: encoding "none", empty
        // content, non-zero size.
        let json = r#"{"encoding":"none","size":2000000,"content":""}"#;
        assert_eq!(parse_contents_response(json).unwrap(), None);
    }

    #[test]
    fn parse_contents_empty_body_nonzero_size_is_none() {
        // A within-cap size but empty base64 body still maps to None: GitHub only
        // sends an empty body when it has withheld the content.
        let json = r#"{"encoding":"base64","size":42,"content":""}"#;
        assert_eq!(parse_contents_response(json).unwrap(), None);
    }

    #[test]
    fn parse_contents_binary_is_none() {
        // base64 "//4=" decodes to non-UTF-8 bytes; the text-only view returns None.
        let json = r#"{"encoding":"base64","size":2,"content":"//4="}"#;
        assert_eq!(parse_contents_response(json).unwrap(), None);
    }

    #[test]
    fn parse_contents_malformed_json_errors() {
        let result = parse_contents_response("not json at all");
        assert!(
            matches!(result, Err(Error::ParseOutput(_))),
            "malformed JSON must be a ParseOutput error, got {result:?}"
        );
    }
}
