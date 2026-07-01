//! The shared review loop — the `Gated` trait and its state-transition logic.
//!
//! Both [`crate::model::ProjectPlan`] and [`crate::model::Review`] implement
//! [`Gated`]. The pure state transitions (`open`, `request_changes`, `approve`,
//! `mark_reworked`, `mark_agent_failed`) are default methods that enforce
//! `SPEC.md` §7's transition table. Implementors supply only the accessor
//! plumbing and the effectful `dispatch`/`reconcile`.

use crate::model::{AgentRun, Comment, GateState, ProjectPlan, Review};
use crate::workflow::TransitionEvent;

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

/// Record a transition for workflow automation.
///
/// Constructs a [`TransitionEvent`] from the object ID and the before/after
/// states. Callers use this after a successful transition to feed into
/// [`crate::workflow::evaluate_rules`].
pub fn transition_event(object_id: &str, from: GateState, to: GateState) -> TransitionEvent {
    crate::workflow::transition_event(object_id, from, to)
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
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::model::{
        Anchor, CommentId, CommentOrigin, DiffData, IssueRef, PlanDoc, PrRef, ProjectRef, ReviewId,
        ReviewSource,
    };

    /// Build a minimal `Review` starting at the given `GateState`.
    fn review_in(state: GateState) -> Review {
        Review {
            id: ReviewId::new("r-1"),
            issue: IssueRef::new("ISSUE-1"),
            pr: PrRef::new("owner/repo#1"),
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
}
