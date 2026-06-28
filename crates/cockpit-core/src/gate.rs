//! The shared review loop — the `Gated` trait and its state-transition logic.
//!
//! Both [`crate::model::ProjectPlan`] and [`crate::model::Review`] implement
//! [`Gated`]. The pure state transitions (`open`, `request_changes`, `approve`,
//! `mark_reworked`, `mark_agent_failed`) are default methods that enforce
//! `SPEC.md` §7's transition table. Implementors supply only the accessor
//! plumbing and the effectful `dispatch`/`reconcile`.

use crate::model::{AgentRun, Comment, GateState, ProjectPlan, Review};

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
    };

    /// Build a minimal `Review` starting at the given `GateState`.
    fn review_in(state: GateState) -> Review {
        Review {
            id: ReviewId::new("r-1"),
            issue: IssueRef::new("ISSUE-1"),
            pr: PrRef::new("owner/repo#1"),
            branch: "alejandro/test".into(),
            base: "main".into(),
            worktree: PathBuf::from("/tmp/wt"),
            gate_state: state,
            diff: DiffData { raw: String::new() },
            head_sha: "abc123".into(),
            comments: vec![],
            parents: vec![],
            children: vec![],
            stale: false,
            agent: None,
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
}
