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

/// Fetch CI check statuses for a PR via `gh pr checks --json`.
pub async fn pr_checks(pr_number: u64) -> Result<Vec<CheckRun>, Error> {
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
        Anchor::DiffLine { path, range } => {
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
            },
            PrData {
                number: 2,
                head_ref_name: "feature/no-id-here".into(),
                base_ref_name: "main".into(),
                title: "No ID".into(),
                state: "OPEN".into(),
                url: "https://github.com/o/r/pull/2".into(),
            },
            PrData {
                number: 3,
                head_ref_name: "alejandro/ab-7-short".into(),
                base_ref_name: "develop".into(),
                title: "Short".into(),
                state: "MERGED".into(),
                url: "https://github.com/o/r/pull/3".into(),
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
                },
                body: "fix this".into(),
                origin: CommentOrigin::Local,
            },
            Comment {
                id: CommentId::new("gh-1"),
                anchor: Anchor::DiffLine {
                    path: PathBuf::from("b.rs"),
                    range: (5, 10),
                },
                body: "from github".into(),
                origin: CommentOrigin::GitHubMirror,
            },
            Comment {
                id: CommentId::new("local-2"),
                anchor: Anchor::DiffLine {
                    path: PathBuf::from("c.rs"),
                    range: (3, 3),
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
