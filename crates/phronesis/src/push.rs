//! Push-mode adapter helpers.
//!
//! [`pull`](crate::pull) covers the actor-queries-a-lookup path. This
//! module covers the reverse: rule fires → the emitted [`Action`]s get
//! wrapped as [`Consequence`]s an [`Actor`](crate::actor::Actor)
//! consumes.
//!
//! # The gap this closes
//!
//! [`ReteNetwork::execute_all_agenda_items`](crate::network::ReteNetwork::execute_all_agenda_items) returns a `Vec<Action>` —
//! the instructions produced by the rule firings. Actions are what
//! the host *does*. [`Consequence`]s are what the host *says* about
//! what just happened. Both typically move together after a rule
//! fires; the host executes the Action (side effects) and hands the
//! Consequence to an Actor (narration or decision).
//!
//! This module does not own side-effect execution — that's
//! deliberately left to the host. It owns only the typed-wire
//! translation.
//!
//! # Pattern
//!
//! ```
//! use phronesis::{Action, Consequence, ConsequenceKind};
//! use phronesis::push::rule_firing_to_consequences;
//!
//! // Imagine the network just fired "greet_rule" with ?who=alice,
//! // producing one Action.
//! let rule_id = "greet_rule";
//! let bound_facts = vec!["fact-42".into()];
//! let actions = vec![Action {
//!     action_type: "wave".to_string(),
//!     params: vec!["alice".to_string()],
//! }];
//!
//! let consequences: Vec<Consequence> =
//!     rule_firing_to_consequences(rule_id, &bound_facts, ConsequenceKind::Event, actions);
//!
//! assert_eq!(consequences.len(), 1);
//! assert_eq!(consequences[0].predicate, "wave");
//! ```

use crate::consequence::{Consequence, ConsequenceKind, Provenance};
use crate::engine_types::Action;

/// Convert a set of [`Action`]s from a single rule firing into
/// [`Consequence`]s ready for an [`Actor`](crate::actor::Actor).
///
/// Each `Action` becomes one `Consequence` with the same predicate
/// (the action_type), a JSON payload containing the action's params,
/// and a [`Provenance::RuleFiring`] pointing back at the rule that
/// fired plus the facts that satisfied its conditions.
///
/// `kind` is passed in because the host knows what the action *means*
/// — `Event` is the common case ("something happened, describe it"),
/// but `Affordance` fits rules that fire to offer choices, and
/// `Constraint` fits rules that fire to restrict actor output.
pub fn rule_firing_to_consequences(
    rule_id: &str,
    bound_facts: &[String],
    kind: ConsequenceKind,
    actions: Vec<Action>,
) -> Vec<Consequence> {
    actions
        .into_iter()
        .map(|a| action_to_consequence(rule_id, bound_facts, kind, a))
        .collect()
}

fn action_to_consequence(
    rule_id: &str,
    bound_facts: &[String],
    kind: ConsequenceKind,
    action: Action,
) -> Consequence {
    Consequence {
        kind,
        predicate: action.action_type.clone(),
        payload: serde_json::json!({
            "action_type": action.action_type,
            "params": action.params,
        }),
        provenance: Provenance::RuleFiring {
            rule_id: rule_id.into(),
            bound_facts: bound_facts.to_vec(),
            bindings: Default::default(),
        },
    }
}
