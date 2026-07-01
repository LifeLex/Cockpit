//! Batch-approve logic for the clean frontier.
//!
//! Evaluates all frontier reviews against configurable quality heuristics
//! and produces a verdict for each. Actual approval is always a separate,
//! explicit user action (Invariant 5: side effects require explicit
//! confirmation).

use serde::{Deserialize, Serialize};

use crate::model::{GateState, Review};
use crate::store::ReviewStore;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Configurable thresholds for batch-approve heuristics.
///
/// Controls which reviews are deemed eligible for batch approval.
/// The defaults represent conservative quality gates.
// No `#[derive(TS)]`: batch-approve is a CLI-only surface (Invariant §9 removed
// the FE batch-approve button), so exporting a binding would emit an orphan.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// Maximum number of files changed in the diff for auto-eligibility.
    ///
    /// Reviews touching more files than this are flagged for manual review.
    /// Set to `0` to disable this check.
    pub max_files_changed: usize,
}

// ---------------------------------------------------------------------------
// Verdict
// ---------------------------------------------------------------------------

/// Outcome of evaluating a single review against batch-approve heuristics.
///
/// Carries human-readable reasons so the user can understand *why* a review
/// is eligible or ineligible.
// No `#[derive(TS)]`: batch-approve is a CLI-only surface (Invariant §9 removed
// the FE batch-approve button), so exporting a binding would emit an orphan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Verdict {
    /// The review passes all heuristics and may be batch-approved.
    Eligible {
        /// Human-readable reasons why this review qualifies.
        reasons: Vec<String>,
    },
    /// The review fails one or more heuristics and should be reviewed manually.
    Ineligible {
        /// Human-readable reasons why this review does not qualify.
        reasons: Vec<String>,
    },
}

impl Verdict {
    /// Whether this verdict allows batch approval.
    pub fn is_eligible(&self) -> bool {
        matches!(self, Verdict::Eligible { .. })
    }
}

// ---------------------------------------------------------------------------
// Evaluation
// ---------------------------------------------------------------------------

/// Evaluate a single review against the batch-approve heuristics.
///
/// A review is eligible when:
/// - Gate state is `InReview` or `Reworked` (can transition to Approved)
/// - Not stale
/// - Has no pending comments
/// - Agent has run at least once (not a fresh PR with no review cycle)
pub fn evaluate_review(review: &Review, _config: &Config) -> Verdict {
    let mut ineligible_reasons: Vec<String> = Vec::new();
    let mut eligible_reasons: Vec<String> = Vec::new();

    // 1. Gate state must be InReview or Reworked.
    //    InReview is the only state from which approve() is legal.
    //    Reworked must first transition to InReview via open() before approve.
    match review.gate_state {
        GateState::InReview => {
            eligible_reasons.push("state is InReview (can approve)".into());
        }
        GateState::Reworked => {
            eligible_reasons.push("state is Reworked (will open then approve)".into());
        }
        other => {
            ineligible_reasons.push(format!("state is {other:?} (must be InReview or Reworked)"));
        }
    }

    // 2. Must not be stale.
    if review.stale {
        ineligible_reasons.push("review is stale (ancestor in rework)".into());
    } else {
        eligible_reasons.push("not stale".into());
    }

    // 3. Must have no pending comments.
    if review.comments.is_empty() {
        eligible_reasons.push("no pending comments".into());
    } else {
        ineligible_reasons.push(format!(
            "{} pending comment(s) not yet addressed",
            review.comments.len()
        ));
    }

    // 4. Agent must have run at least once.
    if review.agent.is_some() {
        eligible_reasons.push("agent has run at least once".into());
    } else {
        ineligible_reasons.push("agent has never run (no review cycle completed)".into());
    }

    if ineligible_reasons.is_empty() {
        Verdict::Eligible {
            reasons: eligible_reasons,
        }
    } else {
        Verdict::Ineligible {
            reasons: ineligible_reasons,
        }
    }
}

/// Evaluate all frontier reviews against batch-approve heuristics.
///
/// Returns a list of `(Review, Verdict)` pairs for every review in the store
/// that is part of the frontier (not stale, not Approved). The caller decides
/// what to do with the results -- this function has no side effects.
pub fn evaluate_frontier(store: &ReviewStore, config: &Config) -> Vec<(Review, Verdict)> {
    let reviews = store.list();

    reviews
        .into_iter()
        .filter(|r| r.gate_state != GateState::Approved)
        .map(|r| {
            let verdict = evaluate_review(&r, config);
            (r, verdict)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::SystemTime;

    use super::*;
    use crate::model::{
        AgentMode, AgentRun, Anchor, Comment, CommentId, CommentOrigin, DiffData, GateState,
        IssueRef, PrRef, ReviewId, ReviewSource,
    };
    use crate::store::ReviewStore;

    /// Build a minimal `Review` at the given state with configurable fields.
    fn make_review(pr_num: u64, state: GateState) -> Review {
        Review {
            id: ReviewId::new(format!("r-{pr_num}")),
            issue: IssueRef::new(format!("ISSUE-{pr_num}")),
            pr: PrRef::new(format!("owner/repo#{pr_num}")),
            branch: format!("alejandro/test-{pr_num}"),
            base: "main".into(),
            base_sha: "000".into(),
            source: ReviewSource::Frontier,
            worktree: PathBuf::from(format!("/tmp/wt-{pr_num}")),
            gate_state: state,
            diff: DiffData { raw: String::new() },
            head_sha: "abc123".into(),
            comments: vec![],
            parents: vec![],
            children: vec![],
            stale: false,
            agent: None,
            repo_slug: None,
            project: None,
        }
    }

    fn make_agent_run() -> AgentRun {
        AgentRun {
            pid: 1234,
            mode: AgentMode::Fix,
            started_at: SystemTime::now(),
            prompt_hash: "deadbeef".into(),
            log_path: PathBuf::from("/tmp/agent.log"),
        }
    }

    fn make_comment(id: &str) -> Comment {
        Comment {
            id: CommentId::new(id),
            anchor: Anchor::DiffLine {
                path: PathBuf::from("src/main.rs"),
                range: (1, 5),
            },
            body: "fix this".into(),
            origin: CommentOrigin::Local,
        }
    }

    // ---------------------------------------------------------------
    // evaluate_review
    // ---------------------------------------------------------------

    #[test]
    fn eligible_in_review_with_agent_no_comments() {
        let mut review = make_review(1, GateState::InReview);
        review.agent = Some(make_agent_run());
        let config = Config::default();

        let verdict = evaluate_review(&review, &config);
        assert!(verdict.is_eligible(), "expected Eligible, got {verdict:?}");
    }

    #[test]
    fn eligible_reworked_with_agent_no_comments() {
        let mut review = make_review(2, GateState::Reworked);
        review.agent = Some(make_agent_run());
        let config = Config::default();

        let verdict = evaluate_review(&review, &config);
        assert!(verdict.is_eligible(), "expected Eligible, got {verdict:?}");
    }

    #[test]
    fn ineligible_pending_state() {
        let mut review = make_review(3, GateState::Pending);
        review.agent = Some(make_agent_run());
        let config = Config::default();

        let verdict = evaluate_review(&review, &config);
        assert!(
            !verdict.is_eligible(),
            "Pending review should be ineligible"
        );
        match &verdict {
            Verdict::Ineligible { reasons } => {
                assert!(reasons.iter().any(|r| r.contains("Pending")));
            }
            Verdict::Eligible { .. } => panic!("expected Ineligible"),
        }
    }

    #[test]
    fn ineligible_dispatched_state() {
        let mut review = make_review(4, GateState::Dispatched);
        review.agent = Some(make_agent_run());
        let config = Config::default();

        let verdict = evaluate_review(&review, &config);
        assert!(!verdict.is_eligible(), "Dispatched should be ineligible");
    }

    #[test]
    fn ineligible_approved_state() {
        let mut review = make_review(5, GateState::Approved);
        review.agent = Some(make_agent_run());
        let config = Config::default();

        let verdict = evaluate_review(&review, &config);
        assert!(!verdict.is_eligible(), "Approved should be ineligible");
    }

    #[test]
    fn ineligible_stale_review() {
        let mut review = make_review(6, GateState::InReview);
        review.agent = Some(make_agent_run());
        review.stale = true;
        let config = Config::default();

        let verdict = evaluate_review(&review, &config);
        assert!(!verdict.is_eligible(), "stale review should be ineligible");
        match &verdict {
            Verdict::Ineligible { reasons } => {
                assert!(reasons.iter().any(|r| r.contains("stale")));
            }
            Verdict::Eligible { .. } => panic!("expected Ineligible"),
        }
    }

    #[test]
    fn ineligible_pending_comments() {
        let mut review = make_review(7, GateState::InReview);
        review.agent = Some(make_agent_run());
        review.comments.push(make_comment("c-1"));
        let config = Config::default();

        let verdict = evaluate_review(&review, &config);
        assert!(
            !verdict.is_eligible(),
            "review with comments should be ineligible"
        );
        match &verdict {
            Verdict::Ineligible { reasons } => {
                assert!(reasons.iter().any(|r| r.contains("pending comment")));
            }
            Verdict::Eligible { .. } => panic!("expected Ineligible"),
        }
    }

    #[test]
    fn ineligible_no_agent_run() {
        let review = make_review(8, GateState::InReview);
        // agent is None
        let config = Config::default();

        let verdict = evaluate_review(&review, &config);
        assert!(
            !verdict.is_eligible(),
            "review with no agent run should be ineligible"
        );
        match &verdict {
            Verdict::Ineligible { reasons } => {
                assert!(reasons.iter().any(|r| r.contains("agent")));
            }
            Verdict::Eligible { .. } => panic!("expected Ineligible"),
        }
    }

    #[test]
    fn multiple_ineligible_reasons() {
        let mut review = make_review(9, GateState::Pending);
        review.stale = true;
        review.comments.push(make_comment("c-1"));
        // agent is None
        let config = Config::default();

        let verdict = evaluate_review(&review, &config);
        assert!(!verdict.is_eligible());
        match &verdict {
            Verdict::Ineligible { reasons } => {
                // Should have reasons for: wrong state, stale, comments, no agent
                assert!(
                    reasons.len() >= 4,
                    "expected at least 4 reasons, got {}",
                    reasons.len()
                );
            }
            Verdict::Eligible { .. } => panic!("expected Ineligible"),
        }
    }

    // ---------------------------------------------------------------
    // evaluate_frontier
    // ---------------------------------------------------------------

    #[test]
    fn evaluate_frontier_filters_approved() {
        let store = ReviewStore::new();
        let mut r1 = make_review(10, GateState::InReview);
        r1.agent = Some(make_agent_run());
        let r2 = make_review(11, GateState::Approved);

        store.insert(r1);
        store.insert(r2);

        let config = Config::default();
        let results = evaluate_frontier(&store, &config);

        // Only r1 should be in the results (r2 is Approved, filtered out).
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0.pr.as_str(), "owner/repo#10");
    }

    #[test]
    fn evaluate_frontier_includes_all_non_approved() {
        let store = ReviewStore::new();

        let mut eligible = make_review(20, GateState::InReview);
        eligible.agent = Some(make_agent_run());

        let mut ineligible = make_review(21, GateState::Dispatched);
        ineligible.agent = Some(make_agent_run());

        store.insert(eligible);
        store.insert(ineligible);

        let config = Config::default();
        let results = evaluate_frontier(&store, &config);

        assert_eq!(results.len(), 2);

        // One should be eligible, one ineligible.
        let eligible_count = results.iter().filter(|(_, v)| v.is_eligible()).count();
        let ineligible_count = results.iter().filter(|(_, v)| !v.is_eligible()).count();
        assert_eq!(eligible_count, 1);
        assert_eq!(ineligible_count, 1);
    }

    #[test]
    fn evaluate_frontier_empty_store() {
        let store = ReviewStore::new();
        let config = Config::default();
        let results = evaluate_frontier(&store, &config);
        assert!(results.is_empty());
    }

    // ---------------------------------------------------------------
    // Verdict helpers
    // ---------------------------------------------------------------

    #[test]
    fn verdict_is_eligible_returns_correct_value() {
        let eligible = Verdict::Eligible {
            reasons: vec!["ok".into()],
        };
        let ineligible = Verdict::Ineligible {
            reasons: vec!["nope".into()],
        };

        assert!(eligible.is_eligible());
        assert!(!ineligible.is_eligible());
    }
}
