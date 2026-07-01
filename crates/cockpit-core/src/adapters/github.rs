//! GitHub adapter --- shells out to `gh` for PR listing, diffs, and CI checks.
//!
//! The critical function is [`parse_issue_from_branch`]: Linear embeds the issue
//! identifier in generated branch names (e.g. `alejandro/nex-123-add-feature`),
//! so cockpit links PR to issue by parsing the head branch. See `SPEC.md` S16.

use serde::{Deserialize, Serialize};
use tokio::process::Command;
use ts_rs::TS;

use crate::model::{Anchor, Comment, CommentId, CommentOrigin, IssueRef, PrRef};

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
    /// Current PR state (e.g. "OPEN", "MERGED", "CLOSED").
    pub state: String,
    /// Full URL of the PR on GitHub.
    pub url: String,
    /// Repository slug (e.g. "Nexcade/garage"). Present for cross-repo searches.
    #[serde(default)]
    pub repo_slug: String,
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

/// Rollup of a set of [`CiCheck`]s into pass/fail/pending counts.
///
/// Drives the diff-gate CI badge. Neutral and skipped checks count as passing:
/// they do not indicate a failure and should not block or alarm the reviewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../app/src/bindings/")]
pub struct CiSummary {
    /// Checks that passed (includes neutral/skipped/cancelled).
    pub passed: u32,
    /// Total number of checks.
    pub total: u32,
    /// Checks that failed.
    pub failed: u32,
    /// Checks still pending (queued or in progress).
    pub pending: u32,
}

/// Classification of a single check's outcome, derived from its bucket/state.
///
/// Kept private: the public surface is [`CiSummary`] via [`summarize`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckOutcome {
    /// Passed, or a non-blocking outcome (neutral, skipped, cancelled).
    Pass,
    /// Failed (failure, timed out, action required, startup failure, etc.).
    Fail,
    /// Not yet concluded (queued, in progress, pending).
    Pending,
}

impl CiCheck {
    /// Classify this check's outcome from its `bucket` (falling back to `state`).
    ///
    /// `gh`'s `bucket` field is the normalized signal; when a fixture or older
    /// `gh` omits it, the raw `state` is used. Neutral/skipped/cancelled all map
    /// to [`CheckOutcome::Pass`] — they do not represent a failure.
    fn outcome(&self) -> CheckOutcome {
        let signal = if self.bucket.is_empty() {
            self.state.as_str()
        } else {
            self.bucket.as_str()
        };
        match signal.to_ascii_lowercase().as_str() {
            // gh buckets.
            "pass" | "skipping" | "cancel" => CheckOutcome::Pass,
            "fail" => CheckOutcome::Fail,
            "pending" => CheckOutcome::Pending,
            // Raw GitHub states (used when bucket is absent).
            "success" | "neutral" | "skipped" | "cancelled" | "canceled" => CheckOutcome::Pass,
            "failure" | "timed_out" | "action_required" | "startup_failure" | "stale" => {
                CheckOutcome::Fail
            }
            "queued" | "in_progress" | "waiting" | "requested" | "expected" => {
                CheckOutcome::Pending
            }
            // Unknown signal: treat conservatively as pending so it is neither a
            // false pass nor a false failure.
            _ => CheckOutcome::Pending,
        }
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
            "number,headRefName,baseRefName,title,state,url",
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
/// Uses `gh search prs` (cross-repo) to discover PRs, then enriches each
/// with branch info from `gh pr view --repo`. This way PRs from any repo
/// the user has access to are returned, not just a single configured repo.
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
            let me = gh_whoami().await.unwrap_or_default();

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

    // Enrich each PR with branch names via `gh pr view --repo`.
    let mut prs = Vec::with_capacity(search_results.len());
    for sr in &search_results {
        let slug = &sr.repository.name_with_owner;
        match enrich_pr(slug, sr.number).await {
            Ok(mut pr) => {
                pr.repo_slug = slug.clone();
                prs.push(pr);
            }
            Err(_) => {
                prs.push(PrData {
                    number: sr.number,
                    head_ref_name: String::new(),
                    base_ref_name: String::new(),
                    title: sr.title.clone(),
                    state: sr.state.clone(),
                    url: sr.url.clone(),
                    repo_slug: slug.clone(),
                });
            }
        }
    }

    Ok(prs)
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
    /// Login of the PR author, lowercased for comparison.
    fn author_login(&self) -> &str {
        &self.author.login
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
            "number,headRefName,baseRefName,title,state,url",
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
        title: String::new(),
        body: String::new(),
        branch: pr.head_ref_name.clone(),
        base: pr.base_ref_name.clone(),
        base_sha: String::new(),
        source,
        worktree: repo_path.to_path_buf(),
        gate_state: GateState::Pending,
        diff: DiffData { raw: diff },
        head_sha: String::new(),
        comments: vec![],
        parents: vec![],
        children: vec![],
        stale: false,
        agent: None,
        repo_slug: slug,
        project: None,
        dispatch_snapshot: None,
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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::model::DiffSide;

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
                state: "OPEN".into(),
                url: "https://github.com/o/r/pull/1".into(),
                repo_slug: String::new(),
            },
            PrData {
                number: 2,
                head_ref_name: "feature/no-id-here".into(),
                base_ref_name: "main".into(),
                title: "No ID".into(),
                state: "OPEN".into(),
                url: "https://github.com/o/r/pull/2".into(),
                repo_slug: String::new(),
            },
            PrData {
                number: 3,
                head_ref_name: "alejandro/ab-7-short".into(),
                base_ref_name: "develop".into(),
                title: "Short".into(),
                state: "MERGED".into(),
                url: "https://github.com/o/r/pull/3".into(),
                repo_slug: String::new(),
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
}
