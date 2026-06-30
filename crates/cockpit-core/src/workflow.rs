//! Workflow automation: dispatch side effects on gate state transitions.
//!
//! When the gate state machine transitions (e.g. `InReview -> Dispatched`),
//! callers can evaluate a set of [`Rule`]s against the [`TransitionEvent`]
//! to determine which [`Action`]s should fire. The actions themselves are
//! data — execution is the caller's responsibility, keeping this module
//! pure and testable.

use crate::model::GateState;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Event
// ---------------------------------------------------------------------------

/// A state transition event emitted after a [`Gated`](crate::gate::Gated)
/// transition succeeds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransitionEvent {
    /// The object that transitioned (review ID or plan ID as a string).
    pub object_id: String,
    /// Previous gate state.
    pub from: GateState,
    /// New gate state.
    pub to: GateState,
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

/// An action to perform when a transition matches a rule.
///
/// Actions are data-only; the caller is responsible for executing them
/// (e.g. calling the Linear API, sending a notification, or POSTing to
/// a webhook). This keeps the workflow module free of side effects.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Action {
    /// Update a Linear issue status.
    UpdateLinearStatus {
        /// The target status string (e.g. `"In Review"`, `"Done"`).
        status: String,
    },
    /// Send a desktop notification.
    Notify {
        /// Notification title.
        title: String,
        /// Notification body text.
        body: String,
    },
    /// POST to a webhook URL.
    Webhook {
        /// The URL to POST the transition event to.
        url: String,
    },
}

// ---------------------------------------------------------------------------
// Rules
// ---------------------------------------------------------------------------

/// A rule mapping a transition pattern to a set of actions.
///
/// Both `from_state` and `to_state` are optional wildcards: `None` matches
/// any state, `Some(s)` matches only that specific state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    /// Match transitions originating from this state (`None` = any).
    pub from_state: Option<GateState>,
    /// Match transitions arriving at this state (`None` = any).
    pub to_state: Option<GateState>,
    /// Actions to execute when the rule matches.
    pub actions: Vec<Action>,
}

// ---------------------------------------------------------------------------
// Evaluation
// ---------------------------------------------------------------------------

/// Evaluate rules against a transition event and return matching actions.
///
/// A rule matches if both its `from_state` (when set) equals `event.from`
/// and its `to_state` (when set) equals `event.to`. `None` acts as a
/// wildcard, matching any state.
pub fn evaluate_rules<'a>(event: &TransitionEvent, rules: &'a [Rule]) -> Vec<&'a Action> {
    rules
        .iter()
        .filter(|rule| {
            rule.from_state.as_ref().is_none_or(|s| *s == event.from)
                && rule.to_state.as_ref().is_none_or(|s| *s == event.to)
        })
        .flat_map(|rule| &rule.actions)
        .collect()
}

// ---------------------------------------------------------------------------
// Transition event constructor (for use in gate.rs callers)
// ---------------------------------------------------------------------------

/// Construct a [`TransitionEvent`] from its parts.
///
/// Convenience helper so callers don't need to import the struct and build
/// it manually.
pub fn transition_event(object_id: &str, from: GateState, to: GateState) -> TransitionEvent {
    TransitionEvent {
        object_id: object_id.to_owned(),
        from,
        to,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn notify_action(title: &str, body: &str) -> Action {
        Action::Notify {
            title: title.into(),
            body: body.into(),
        }
    }

    fn linear_action(status: &str) -> Action {
        Action::UpdateLinearStatus {
            status: status.into(),
        }
    }

    fn webhook_action(url: &str) -> Action {
        Action::Webhook { url: url.into() }
    }

    // -- evaluate_rules ---------------------------------------------------

    #[test]
    fn matches_exact_transition() {
        let rules = vec![Rule {
            from_state: Some(GateState::InReview),
            to_state: Some(GateState::Dispatched),
            actions: vec![notify_action("Dispatched", "Review dispatched to agent")],
        }];

        let event = transition_event("r-1", GateState::InReview, GateState::Dispatched);
        let actions = evaluate_rules(&event, &rules);

        assert_eq!(actions.len(), 1);
        assert_eq!(
            *actions[0],
            notify_action("Dispatched", "Review dispatched to agent")
        );
    }

    #[test]
    fn wildcard_from_matches_any_source() {
        let rules = vec![Rule {
            from_state: None,
            to_state: Some(GateState::Approved),
            actions: vec![linear_action("Done")],
        }];

        // Should match regardless of the source state.
        let event = transition_event("r-1", GateState::InReview, GateState::Approved);
        let actions = evaluate_rules(&event, &rules);
        assert_eq!(actions.len(), 1);
        assert_eq!(*actions[0], linear_action("Done"));
    }

    #[test]
    fn wildcard_to_matches_any_target() {
        let rules = vec![Rule {
            from_state: Some(GateState::Dispatched),
            to_state: None,
            actions: vec![webhook_action("https://example.com/hook")],
        }];

        let event = transition_event("r-1", GateState::Dispatched, GateState::Reworked);
        let actions = evaluate_rules(&event, &rules);
        assert_eq!(actions.len(), 1);
        assert_eq!(*actions[0], webhook_action("https://example.com/hook"));
    }

    #[test]
    fn fully_wildcard_matches_everything() {
        let rules = vec![Rule {
            from_state: None,
            to_state: None,
            actions: vec![webhook_action("https://example.com/all")],
        }];

        let event = transition_event("r-1", GateState::Pending, GateState::InReview);
        let actions = evaluate_rules(&event, &rules);
        assert_eq!(actions.len(), 1);
    }

    #[test]
    fn no_match_returns_empty() {
        let rules = vec![Rule {
            from_state: Some(GateState::InReview),
            to_state: Some(GateState::Approved),
            actions: vec![notify_action("Approved", "Nice")],
        }];

        let event = transition_event("r-1", GateState::Dispatched, GateState::Reworked);
        let actions = evaluate_rules(&event, &rules);
        assert!(actions.is_empty());
    }

    #[test]
    fn multiple_rules_can_match() {
        let rules = vec![
            Rule {
                from_state: None,
                to_state: Some(GateState::Approved),
                actions: vec![linear_action("Done")],
            },
            Rule {
                from_state: Some(GateState::InReview),
                to_state: Some(GateState::Approved),
                actions: vec![notify_action("Approved", "Review approved")],
            },
        ];

        let event = transition_event("r-1", GateState::InReview, GateState::Approved);
        let actions = evaluate_rules(&event, &rules);
        assert_eq!(actions.len(), 2);
    }

    #[test]
    fn multiple_actions_per_rule() {
        let rules = vec![Rule {
            from_state: None,
            to_state: Some(GateState::Approved),
            actions: vec![
                linear_action("Done"),
                notify_action("Merged", "PR merged"),
                webhook_action("https://example.com/merged"),
            ],
        }];

        let event = transition_event("r-1", GateState::InReview, GateState::Approved);
        let actions = evaluate_rules(&event, &rules);
        assert_eq!(actions.len(), 3);
    }

    #[test]
    fn empty_rules_returns_empty() {
        let event = transition_event("r-1", GateState::Pending, GateState::InReview);
        let actions = evaluate_rules(&event, &[]);
        assert!(actions.is_empty());
    }

    #[test]
    fn rule_with_no_actions_contributes_nothing() {
        let rules = vec![Rule {
            from_state: None,
            to_state: None,
            actions: vec![],
        }];

        let event = transition_event("r-1", GateState::Pending, GateState::InReview);
        let actions = evaluate_rules(&event, &rules);
        assert!(actions.is_empty());
    }

    // -- transition_event -------------------------------------------------

    #[test]
    fn transition_event_construction() {
        let event = transition_event("plan-42", GateState::Pending, GateState::InReview);
        assert_eq!(event.object_id, "plan-42");
        assert_eq!(event.from, GateState::Pending);
        assert_eq!(event.to, GateState::InReview);
    }

    // -- serialization round-trip -----------------------------------------

    #[test]
    fn action_round_trip() {
        let action = Action::UpdateLinearStatus {
            status: "In Progress".into(),
        };
        let json = serde_json::to_string(&action).expect("serialize");
        let back: Action = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(action, back);
    }

    #[test]
    fn rule_round_trip() {
        let rule = Rule {
            from_state: Some(GateState::InReview),
            to_state: Some(GateState::Dispatched),
            actions: vec![
                notify_action("Review sent", "Dispatched to agent"),
                webhook_action("https://example.com/hook"),
            ],
        };

        let json = serde_json::to_string(&rule).expect("serialize");
        let back: Rule = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.from_state, rule.from_state);
        assert_eq!(back.to_state, rule.to_state);
        assert_eq!(back.actions, rule.actions);
    }
}
