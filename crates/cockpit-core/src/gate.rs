//! The shared review loop — the `Gated` trait and its state-transition logic.
//!
//! Both [`crate::model::ProjectPlan`] and [`crate::model::Review`] implement
//! [`Gated`]. The pure state transitions (`open`, `request_changes`, `approve`,
//! `mark_reworked`, `mark_agent_failed`) are default methods that enforce
//! `SPEC.md` §7's transition table. Implementors supply only the accessor
//! plumbing and the effectful `dispatch`/`reconcile`.

use crate::model::{AgentRun, Comment, DispatchSnapshot, GateState, ProjectPlan, Review};

/// Errors from gate state transitions.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A transition was attempted from a state that does not allow it.
    #[error("illegal transition from {from:?} on event `{event}`")]
    IllegalTransition {
        /// The state the object was in when the transition was attempted.
        from: GateState,
        /// Human-readable name of the event that was attempted.
        event: &'static str,
    },

    /// `request_changes` requires at least one comment.
    #[error("request changes requires at least one comment")]
    NoComments,

    /// Placeholder for operations not yet wired to real adapters.
    #[error("operation not yet implemented")]
    NotImplemented,
}

/// The outcome of applying an agent's completion to a [`Review`].
///
/// Distinct from a raw `bool` so callers branch on intent, not truthiness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentOutcome {
    /// The agent advanced HEAD; the review moved `Dispatched → Reworked`.
    Reworked,
    /// The agent produced no new commit; the review returned `Dispatched →
    /// InReview` with its comments preserved for re-dispatch.
    Failed,
}

/// The shared review loop, implemented by both [`ProjectPlan`] and [`Review`].
///
/// The pure state-transition methods (`open`, `request_changes`, `approve`,
/// `mark_reworked`, `mark_agent_failed`) are provided as default methods that
/// enforce the transition table from `SPEC.md` §7. Implementors supply the
/// accessor plumbing and the effectful `dispatch`/`reconcile`.
pub trait Gated {
    /// Current gate state.
    fn gate_state(&self) -> GateState;

    /// Ephemeral comments for the current review cycle.
    fn comments(&self) -> &[Comment];

    /// Mutable access to the gate state — implementation detail for default
    /// transition methods. Use the named transition methods instead.
    fn gate_state_mut(&mut self) -> &mut GateState;

    /// Mutable access to the comments vec — implementation detail for default
    /// transition methods. Use the named transition methods instead.
    fn comments_mut(&mut self) -> &mut Vec<Comment>;

    /// Assemble the prompt from gathered comments and spawn the agent in its
    /// worktree. Implementors wire this to the agent adapter in later phases.
    fn dispatch(&mut self) -> Result<AgentRun, Error>;

    /// After the Stop hook fires: re-read the artifact, then transition to
    /// `Reworked`. Implementors wire this to git/artifact reconciliation.
    fn reconcile(&mut self) -> Result<(), Error>;

    // ------------------------------------------------------------------
    // Pure state transitions (default implementations)
    // ------------------------------------------------------------------

    /// `Pending | Reworked → InReview`.
    fn open(&mut self) -> Result<(), Error> {
        let state = self.gate_state();
        match state {
            GateState::Pending | GateState::Reworked => {
                *self.gate_state_mut() = GateState::InReview;
                Ok(())
            }
            _ => Err(Error::IllegalTransition {
                from: state,
                event: "open",
            }),
        }
    }

    /// `InReview → Dispatched`. Requires at least one comment.
    fn request_changes(&mut self) -> Result<(), Error> {
        let state = self.gate_state();
        if state != GateState::InReview {
            return Err(Error::IllegalTransition {
                from: state,
                event: "request_changes",
            });
        }
        if self.comments().is_empty() {
            return Err(Error::NoComments);
        }
        *self.gate_state_mut() = GateState::Dispatched;
        Ok(())
    }

    /// `InReview → Approved`.
    fn approve(&mut self) -> Result<(), Error> {
        let state = self.gate_state();
        if state != GateState::InReview {
            return Err(Error::IllegalTransition {
                from: state,
                event: "approve",
            });
        }
        *self.gate_state_mut() = GateState::Approved;
        Ok(())
    }

    /// `Dispatched → Reworked`. Clears ephemeral comments (Invariant 4).
    fn mark_reworked(&mut self) -> Result<(), Error> {
        let state = self.gate_state();
        if state != GateState::Dispatched {
            return Err(Error::IllegalTransition {
                from: state,
                event: "mark_reworked",
            });
        }
        self.comments_mut().clear();
        *self.gate_state_mut() = GateState::Reworked;
        Ok(())
    }

    /// `Dispatched → InReview`. Agent failed or produced no change;
    /// comments are preserved so they can be re-dispatched.
    fn mark_agent_failed(&mut self) -> Result<(), Error> {
        let state = self.gate_state();
        if state != GateState::Dispatched {
            return Err(Error::IllegalTransition {
                from: state,
                event: "mark_agent_failed",
            });
        }
        *self.gate_state_mut() = GateState::InReview;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Gated impl for Review
// ---------------------------------------------------------------------------

impl Gated for Review {
    fn gate_state(&self) -> GateState {
        self.gate_state
    }

    fn comments(&self) -> &[Comment] {
        &self.comments
    }

    fn gate_state_mut(&mut self) -> &mut GateState {
        &mut self.gate_state
    }

    fn comments_mut(&mut self) -> &mut Vec<Comment> {
        &mut self.comments
    }

    fn dispatch(&mut self) -> Result<AgentRun, Error> {
        Err(Error::NotImplemented)
    }

    fn reconcile(&mut self) -> Result<(), Error> {
        Err(Error::NotImplemented)
    }
}

// ---------------------------------------------------------------------------
// Gated impl for ProjectPlan
// ---------------------------------------------------------------------------

impl Gated for ProjectPlan {
    fn gate_state(&self) -> GateState {
        self.gate_state
    }

    fn comments(&self) -> &[Comment] {
        &self.comments
    }

    fn gate_state_mut(&mut self) -> &mut GateState {
        &mut self.gate_state
    }

    fn comments_mut(&mut self) -> &mut Vec<Comment> {
        &mut self.comments
    }

    fn dispatch(&mut self) -> Result<AgentRun, Error> {
        Err(Error::NotImplemented)
    }

    fn reconcile(&mut self) -> Result<(), Error> {
        Err(Error::NotImplemented)
    }
}

// ---------------------------------------------------------------------------
// Stale flag logic (Review-specific, not part of the loop)
// ---------------------------------------------------------------------------

impl Review {
    /// Mark this review stale because a parent entered `Dispatched`.
    ///
    /// Stale gates the frontier (what is safe to deep-review), not the loop
    /// itself — a stale review can still transition normally.
    pub fn mark_stale(&mut self) {
        self.stale = true;
    }

    /// Clear the stale flag after a parent reached `Reworked` and restack
    /// succeeded.
    pub fn clear_stale(&mut self) {
        self.stale = false;
    }

    /// `Approved → Merged`. Terminal.
    ///
    /// Inherent to [`Review`] rather than part of [`Gated`]: only reviews merge,
    /// plans never do. Every other transition out of `Merged` is rejected by the
    /// `Gated` methods, so `Merged` is a true sink.
    pub fn mark_merged(&mut self) -> Result<(), Error> {
        if self.gate_state != GateState::Approved {
            return Err(Error::IllegalTransition {
                from: self.gate_state,
                event: "mark_merged",
            });
        }
        self.gate_state = GateState::Merged;
        Ok(())
    }

    /// Apply an agent's completion, using git HEAD — not agent output — as the
    /// source of truth for whether work landed.
    ///
    /// Agent stdout can claim success while committing nothing; the only trusted
    /// signal is whether the branch HEAD actually advanced. When `new_head` is
    /// `Some(h)` and differs from the current [`Review::head_sha`], the rework
    /// landed a commit: adopt the new HEAD, clear the agent handle, and move
    /// `Dispatched → Reworked` (which clears comments). Otherwise the agent made
    /// no progress: clear the agent handle and move `Dispatched → InReview`,
    /// preserving comments for re-dispatch.
    pub fn apply_agent_completion(
        &mut self,
        new_head: Option<String>,
    ) -> Result<AgentOutcome, Error> {
        match new_head {
            Some(h) if h != self.head_sha => {
                self.head_sha = h;
                self.agent = None;
                self.mark_reworked()?;
                Ok(AgentOutcome::Reworked)
            }
            _ => {
                self.agent = None;
                self.mark_agent_failed()?;
                Ok(AgentOutcome::Failed)
            }
        }
    }

    /// Capture a [`DispatchSnapshot`] of the current HEAD and comments,
    /// overwriting any previous snapshot.
    ///
    /// Records what the reviewer asked for at dispatch time for the current cycle
    /// only. It is not a durable comment store (Invariant §0.4): comments stay
    /// ephemeral and are cleared on `Reworked`.
    pub fn snapshot_dispatch(&mut self) {
        self.dispatch_snapshot = Some(DispatchSnapshot {
            reviewed_sha: self.head_sha.clone(),
            comments: self.comments.clone(),
        });
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::model::{
        AgentMode, AgentRun, Anchor, CommentId, CommentOrigin, DiffData, DiffSide, IssueRef,
        PlanDoc, PrRef, ProjectRef, ReviewId, ReviewSource,
    };
    use std::time::SystemTime;

    /// Build a minimal `Review` starting at the given `GateState`.
    fn review_in(state: GateState) -> Review {
        Review {
            id: ReviewId::new("r-1"),
            issue: IssueRef::new("ISSUE-1"),
            pr: PrRef::new("owner/repo#1"),
            title: String::new(),
            body: String::new(),
            branch: "alejandro/test".into(),
            base: "main".into(),
            base_sha: "000".into(),
            source: ReviewSource::Frontier,
            worktree: PathBuf::from("/tmp/wt"),
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
            dispatch_snapshot: None,
        }
    }

    /// Build a minimal `ProjectPlan` starting at the given `GateState`.
    fn plan_in(state: GateState) -> ProjectPlan {
        ProjectPlan {
            project: ProjectRef::new("proj-1"),
            doc: PlanDoc {
                summary: "test plan".into(),
                steps: vec![],
                files: vec![],
                risks: vec![],
                raw: String::new(),
            },
            gate_state: state,
            comments: vec![],
            agent: None,
            plan_path: None,
        }
    }

    /// Add a dummy comment to anything that implements `Gated`.
    fn add_comment(target: &mut impl Gated) {
        target.comments_mut().push(Comment {
            id: CommentId::new("c-1"),
            anchor: Anchor::DiffLine {
                path: PathBuf::from("src/main.rs"),
                range: (1, 5),
                side: DiffSide::New,
            },
            body: "fix this".into(),
            origin: CommentOrigin::Local,
        });
    }

    // ---------------------------------------------------------------
    // Legal transitions
    // ---------------------------------------------------------------

    #[test]
    fn pending_to_in_review() {
        let mut r = review_in(GateState::Pending);
        r.open().unwrap();
        assert_eq!(r.gate_state(), GateState::InReview);
    }

    #[test]
    fn in_review_to_dispatched() {
        let mut r = review_in(GateState::InReview);
        add_comment(&mut r);
        r.request_changes().unwrap();
        assert_eq!(r.gate_state(), GateState::Dispatched);
    }

    #[test]
    fn in_review_to_approved() {
        let mut r = review_in(GateState::InReview);
        r.approve().unwrap();
        assert_eq!(r.gate_state(), GateState::Approved);
    }

    #[test]
    fn dispatched_to_reworked() {
        let mut r = review_in(GateState::InReview);
        add_comment(&mut r);
        r.request_changes().unwrap();
        assert_eq!(r.gate_state(), GateState::Dispatched);

        r.mark_reworked().unwrap();
        assert_eq!(r.gate_state(), GateState::Reworked);
    }

    #[test]
    fn dispatched_to_in_review_agent_failed() {
        let mut r = review_in(GateState::Dispatched);
        r.mark_agent_failed().unwrap();
        assert_eq!(r.gate_state(), GateState::InReview);
    }

    #[test]
    fn reworked_to_in_review() {
        let mut r = review_in(GateState::Reworked);
        r.open().unwrap();
        assert_eq!(r.gate_state(), GateState::InReview);
    }

    // ---------------------------------------------------------------
    // Illegal transitions — open
    // ---------------------------------------------------------------

    #[test]
    fn open_from_in_review_rejected() {
        let mut r = review_in(GateState::InReview);
        assert!(r.open().is_err());
    }

    #[test]
    fn open_from_dispatched_rejected() {
        let mut r = review_in(GateState::Dispatched);
        assert!(r.open().is_err());
    }

    #[test]
    fn open_from_approved_rejected() {
        let mut r = review_in(GateState::Approved);
        assert!(r.open().is_err());
    }

    // ---------------------------------------------------------------
    // Illegal transitions — request_changes
    // ---------------------------------------------------------------

    #[test]
    fn request_changes_from_pending_rejected() {
        let mut r = review_in(GateState::Pending);
        add_comment(&mut r);
        assert!(r.request_changes().is_err());
    }

    #[test]
    fn request_changes_from_dispatched_rejected() {
        let mut r = review_in(GateState::Dispatched);
        add_comment(&mut r);
        assert!(r.request_changes().is_err());
    }

    #[test]
    fn request_changes_from_reworked_rejected() {
        let mut r = review_in(GateState::Reworked);
        add_comment(&mut r);
        assert!(r.request_changes().is_err());
    }

    #[test]
    fn request_changes_from_approved_rejected() {
        let mut r = review_in(GateState::Approved);
        add_comment(&mut r);
        assert!(r.request_changes().is_err());
    }

    // ---------------------------------------------------------------
    // Illegal transitions — approve
    // ---------------------------------------------------------------

    #[test]
    fn approve_from_pending_rejected() {
        let mut r = review_in(GateState::Pending);
        assert!(r.approve().is_err());
    }

    #[test]
    fn approve_from_dispatched_rejected() {
        let mut r = review_in(GateState::Dispatched);
        assert!(r.approve().is_err());
    }

    #[test]
    fn approve_from_reworked_rejected() {
        let mut r = review_in(GateState::Reworked);
        assert!(r.approve().is_err());
    }

    #[test]
    fn approve_from_approved_rejected() {
        let mut r = review_in(GateState::Approved);
        assert!(r.approve().is_err());
    }

    // ---------------------------------------------------------------
    // Illegal transitions — mark_reworked
    // ---------------------------------------------------------------

    #[test]
    fn mark_reworked_from_pending_rejected() {
        let mut r = review_in(GateState::Pending);
        assert!(r.mark_reworked().is_err());
    }

    #[test]
    fn mark_reworked_from_in_review_rejected() {
        let mut r = review_in(GateState::InReview);
        assert!(r.mark_reworked().is_err());
    }

    #[test]
    fn mark_reworked_from_reworked_rejected() {
        let mut r = review_in(GateState::Reworked);
        assert!(r.mark_reworked().is_err());
    }

    #[test]
    fn mark_reworked_from_approved_rejected() {
        let mut r = review_in(GateState::Approved);
        assert!(r.mark_reworked().is_err());
    }

    // ---------------------------------------------------------------
    // Illegal transitions — mark_agent_failed
    // ---------------------------------------------------------------

    #[test]
    fn mark_agent_failed_from_pending_rejected() {
        let mut r = review_in(GateState::Pending);
        assert!(r.mark_agent_failed().is_err());
    }

    #[test]
    fn mark_agent_failed_from_in_review_rejected() {
        let mut r = review_in(GateState::InReview);
        assert!(r.mark_agent_failed().is_err());
    }

    #[test]
    fn mark_agent_failed_from_reworked_rejected() {
        let mut r = review_in(GateState::Reworked);
        assert!(r.mark_agent_failed().is_err());
    }

    #[test]
    fn mark_agent_failed_from_approved_rejected() {
        let mut r = review_in(GateState::Approved);
        assert!(r.mark_agent_failed().is_err());
    }

    // ---------------------------------------------------------------
    // Edge cases
    // ---------------------------------------------------------------

    #[test]
    fn request_changes_no_comments_rejected() {
        let mut r = review_in(GateState::InReview);
        let err = r.request_changes().unwrap_err();
        assert!(
            matches!(err, Error::NoComments),
            "expected NoComments, got {err:?}"
        );
    }

    #[test]
    fn comments_cleared_on_reworked() {
        let mut r = review_in(GateState::InReview);
        add_comment(&mut r);
        assert_eq!(r.comments().len(), 1);

        r.request_changes().unwrap();
        r.mark_reworked().unwrap();
        assert!(r.comments().is_empty());
    }

    #[test]
    fn agent_failed_preserves_comments() {
        let mut r = review_in(GateState::InReview);
        add_comment(&mut r);
        r.request_changes().unwrap();
        assert_eq!(r.comments().len(), 1);

        r.mark_agent_failed().unwrap();
        assert_eq!(
            r.comments().len(),
            1,
            "comments should survive agent failure"
        );
    }

    #[test]
    fn full_cycle() {
        let mut r = review_in(GateState::Pending);

        // Pending → InReview
        r.open().unwrap();
        assert_eq!(r.gate_state(), GateState::InReview);

        // InReview → Dispatched
        add_comment(&mut r);
        r.request_changes().unwrap();
        assert_eq!(r.gate_state(), GateState::Dispatched);

        // Dispatched → Reworked
        r.mark_reworked().unwrap();
        assert_eq!(r.gate_state(), GateState::Reworked);
        assert!(r.comments().is_empty());

        // Reworked → InReview
        r.open().unwrap();
        assert_eq!(r.gate_state(), GateState::InReview);

        // InReview → Approved
        r.approve().unwrap();
        assert_eq!(r.gate_state(), GateState::Approved);
    }

    #[test]
    fn agent_failed_then_redispatch() {
        let mut r = review_in(GateState::InReview);
        add_comment(&mut r);
        r.request_changes().unwrap();

        // Agent fails
        r.mark_agent_failed().unwrap();
        assert_eq!(r.gate_state(), GateState::InReview);

        // Re-dispatch (comments survived)
        r.request_changes().unwrap();
        assert_eq!(r.gate_state(), GateState::Dispatched);
    }

    // ---------------------------------------------------------------
    // Stale flag
    // ---------------------------------------------------------------

    #[test]
    fn mark_stale() {
        let mut r = review_in(GateState::InReview);
        assert!(!r.stale);
        r.mark_stale();
        assert!(r.stale);
    }

    #[test]
    fn clear_stale() {
        let mut r = review_in(GateState::InReview);
        r.mark_stale();
        assert!(r.stale);
        r.clear_stale();
        assert!(!r.stale);
    }

    #[test]
    fn stale_does_not_block_transitions() {
        let mut r = review_in(GateState::InReview);
        r.mark_stale();

        // Stale review can still transition normally
        add_comment(&mut r);
        r.request_changes().unwrap();
        assert_eq!(r.gate_state(), GateState::Dispatched);
        assert!(r.stale, "stale flag is orthogonal to gate state");
    }

    // ---------------------------------------------------------------
    // ProjectPlan uses the same trait
    // ---------------------------------------------------------------

    #[test]
    fn project_plan_full_cycle() {
        let mut p = plan_in(GateState::Pending);

        p.open().unwrap();
        assert_eq!(p.gate_state(), GateState::InReview);

        add_comment(&mut p);
        p.request_changes().unwrap();
        assert_eq!(p.gate_state(), GateState::Dispatched);

        p.mark_reworked().unwrap();
        assert_eq!(p.gate_state(), GateState::Reworked);
        assert!(p.comments().is_empty());

        p.open().unwrap();
        p.approve().unwrap();
        assert_eq!(p.gate_state(), GateState::Approved);
    }

    // The plan gate reuses the exact same 5 states as reviews (Invariant §0.3).
    // These tests pin the plan-gate failure edges called out for Phase 2.

    #[test]
    fn plan_gate_reuses_the_five_review_states() {
        // Every state a Review can be in is a valid state for a ProjectPlan,
        // and the same transition methods drive both — there are no
        // plan-specific gate variants.
        for state in [
            GateState::Pending,
            GateState::InReview,
            GateState::Dispatched,
            GateState::Reworked,
            GateState::Approved,
        ] {
            let plan = plan_in(state);
            assert_eq!(plan.gate_state(), state);
        }
    }

    #[test]
    fn planner_failed_returns_plan_to_in_review() {
        // Initial-generation / rework planner failed: Dispatched -> InReview,
        // comments preserved for re-dispatch.
        let mut p = plan_in(GateState::InReview);
        add_comment(&mut p);
        p.request_changes().unwrap();
        assert_eq!(p.gate_state(), GateState::Dispatched);

        p.mark_agent_failed().unwrap();
        assert_eq!(p.gate_state(), GateState::InReview);
        assert_eq!(
            p.comments().len(),
            1,
            "plan comments survive planner failure for re-dispatch"
        );
    }

    #[test]
    fn plan_rework_clears_comments_on_reworked() {
        let mut p = plan_in(GateState::InReview);
        add_comment(&mut p);
        p.request_changes().unwrap();
        p.mark_reworked().unwrap();
        assert_eq!(p.gate_state(), GateState::Reworked);
        assert!(
            p.comments().is_empty(),
            "plan comments are ephemeral (Invariant 4)"
        );
    }

    #[test]
    fn plan_stays_pending_during_initial_generation() {
        // Initial generation is an artifact-fill: the plan is not opened, so it
        // stays Pending while the planner runs. A completion that (wrongly)
        // tried to mark_reworked from Pending is rejected, preserving Pending.
        let mut p = plan_in(GateState::Pending);
        assert!(
            p.mark_reworked().is_err(),
            "cannot mark_reworked from Pending"
        );
        assert_eq!(p.gate_state(), GateState::Pending);
    }

    #[test]
    fn plan_approve_only_from_in_review() {
        // Approve → fan-out is guarded: it is only legal from InReview, never
        // directly from Pending/Reworked (must open first).
        let mut pending = plan_in(GateState::Pending);
        assert!(pending.approve().is_err());

        let mut reworked = plan_in(GateState::Reworked);
        assert!(reworked.approve().is_err());

        let mut in_review = plan_in(GateState::InReview);
        in_review.approve().unwrap();
        assert_eq!(in_review.gate_state(), GateState::Approved);
    }

    // ---------------------------------------------------------------
    // Merged (terminal) — Review-only
    // ---------------------------------------------------------------

    /// Build a minimal running agent handle for tests that need one attached.
    fn dummy_agent() -> AgentRun {
        AgentRun {
            pid: 1234,
            mode: AgentMode::Fix,
            started_at: SystemTime::UNIX_EPOCH,
            prompt_hash: "hash".into(),
            log_path: PathBuf::from("/tmp/agent.log"),
        }
    }

    #[test]
    fn approved_to_merged() {
        let mut r = review_in(GateState::Approved);
        r.mark_merged().unwrap();
        assert_eq!(r.gate_state(), GateState::Merged);
    }

    #[test]
    fn mark_merged_only_from_approved() {
        for state in [
            GateState::Pending,
            GateState::InReview,
            GateState::Dispatched,
            GateState::Reworked,
            GateState::Merged,
        ] {
            let mut r = review_in(state);
            assert!(
                r.mark_merged().is_err(),
                "mark_merged from {state:?} must be illegal"
            );
        }
    }

    #[test]
    fn merged_is_terminal() {
        // Every other transition out of Merged is rejected — it is a true sink.
        let mut r = review_in(GateState::Merged);
        assert!(r.open().is_err(), "open from Merged");

        let mut r = review_in(GateState::Merged);
        add_comment(&mut r);
        assert!(r.request_changes().is_err(), "request_changes from Merged");

        let mut r = review_in(GateState::Merged);
        assert!(r.approve().is_err(), "approve from Merged");

        let mut r = review_in(GateState::Merged);
        assert!(r.mark_reworked().is_err(), "mark_reworked from Merged");

        let mut r = review_in(GateState::Merged);
        assert!(
            r.mark_agent_failed().is_err(),
            "mark_agent_failed from Merged"
        );

        let mut r = review_in(GateState::Merged);
        assert!(r.mark_merged().is_err(), "mark_merged from Merged");
    }

    // ---------------------------------------------------------------
    // apply_agent_completion — HEAD is authoritative
    // ---------------------------------------------------------------

    #[test]
    fn apply_agent_completion_advanced_head_reworks() {
        let mut r = review_in(GateState::Dispatched);
        add_comment(&mut r);
        r.agent = Some(dummy_agent());

        let outcome = r
            .apply_agent_completion(Some("newsha".into()))
            .expect("advancing HEAD should succeed");

        assert_eq!(outcome, AgentOutcome::Reworked);
        assert_eq!(r.gate_state(), GateState::Reworked);
        assert_eq!(r.head_sha, "newsha", "HEAD updated to the new commit");
        assert!(r.agent.is_none(), "agent handle cleared");
        assert!(r.comments().is_empty(), "comments cleared on rework");
    }

    #[test]
    fn apply_agent_completion_same_head_fails() {
        let mut r = review_in(GateState::Dispatched);
        add_comment(&mut r);
        r.agent = Some(dummy_agent());
        let head_before = r.head_sha.clone();

        let outcome = r
            .apply_agent_completion(Some(head_before.clone()))
            .expect("unchanged HEAD should still return Ok(Failed)");

        assert_eq!(outcome, AgentOutcome::Failed);
        assert_eq!(r.gate_state(), GateState::InReview);
        assert_eq!(r.head_sha, head_before, "HEAD unchanged");
        assert!(r.agent.is_none(), "agent handle cleared");
        assert_eq!(r.comments().len(), 1, "comments preserved on failure");
    }

    #[test]
    fn apply_agent_completion_none_head_fails() {
        let mut r = review_in(GateState::Dispatched);
        add_comment(&mut r);
        r.agent = Some(dummy_agent());
        let head_before = r.head_sha.clone();

        let outcome = r
            .apply_agent_completion(None)
            .expect("missing HEAD should return Ok(Failed)");

        assert_eq!(outcome, AgentOutcome::Failed);
        assert_eq!(r.gate_state(), GateState::InReview);
        assert_eq!(r.head_sha, head_before, "HEAD unchanged");
        assert!(r.agent.is_none(), "agent handle cleared");
        assert_eq!(r.comments().len(), 1, "comments preserved on failure");
    }

    // ---------------------------------------------------------------
    // snapshot_dispatch
    // ---------------------------------------------------------------

    #[test]
    fn snapshot_dispatch_captures_sha_and_comments() {
        let mut r = review_in(GateState::InReview);
        add_comment(&mut r);

        r.snapshot_dispatch();

        let snap = r
            .dispatch_snapshot
            .as_ref()
            .expect("snapshot should be set");
        assert_eq!(snap.reviewed_sha, r.head_sha);
        assert_eq!(snap.comments.len(), 1);
        assert_eq!(snap.comments, r.comments);
    }

    #[test]
    fn snapshot_dispatch_overwrites_previous() {
        let mut r = review_in(GateState::InReview);
        add_comment(&mut r);
        r.snapshot_dispatch();
        assert_eq!(
            r.dispatch_snapshot.as_ref().map(|s| s.comments.len()),
            Some(1)
        );

        // A second cycle with a different HEAD and no comments overwrites it.
        r.head_sha = "later-sha".into();
        r.comments_mut().clear();
        r.snapshot_dispatch();

        let snap = r
            .dispatch_snapshot
            .as_ref()
            .expect("snapshot should still be set");
        assert_eq!(snap.reviewed_sha, "later-sha");
        assert!(
            snap.comments.is_empty(),
            "snapshot reflects the latest dispatch, not the previous one"
        );
    }
}
