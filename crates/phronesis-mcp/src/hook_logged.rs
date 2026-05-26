//! Flattened projection of `phr::Consequence` for action-log entries,
//! and helpers that consume slices of these projections.
//!
//! Lives in its own module so `hook.rs` stays focused on the pre/post
//! orchestration. The shape is stable on the wire — `LoggedConsequence`
//! is what `jq` users see when they query `.phronesis/log.jsonl`.

use std::collections::HashMap;

use phr::consequence::{Consequence, Provenance};

/// Flattened projection of `phr::Consequence` used in action-log
/// entries. We don't serialize the raw `Consequence` because its nested
/// `Provenance::RuleFiring { rule_id, bindings, .. }` is awkward to query
/// (`.provenance.RuleFiring.rule_id` etc.). Pulling rule_id and bindings
/// up to the top rank gives `jq` users a flat, predictable shape.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct LoggedConsequence {
    pub rule_id: phr::RuleId,
    pub action_type: String,
    pub message: String,
    #[serde(default)]
    pub bindings: HashMap<String, String>,
}

impl LoggedConsequence {
    /// Project a `Consequence` whose provenance is `RuleFiring` into a
    /// flat log entry. Returns `None` for other provenance variants
    /// (Lookup, RuleDrivenLookup, Asserted) which the hook doesn't
    /// currently emit.
    pub(crate) fn from_consequence(c: &Consequence) -> Option<Self> {
        let (rule_id, bindings) = match &c.provenance {
            Provenance::RuleFiring {
                rule_id, bindings, ..
            } => (rule_id.clone(), bindings.clone()),
            _ => return None,
        };
        let action_type = c
            .payload
            .get("action_type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let message = c
            .payload
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        Some(Self {
            rule_id,
            action_type,
            message,
            bindings,
        })
    }
}

/// Split `LoggedConsequence`s into the (violations, warnings) string-lists
/// the hook uses for stderr output. Violations are messages whose
/// `action_type == "constraint_violation"`, warnings are
/// `"constraint_warning"`. Order preserved.
pub(crate) fn split_messages_by_action_type(
    items: &[LoggedConsequence],
) -> (Vec<String>, Vec<String>) {
    let mut violations = Vec::new();
    let mut warnings = Vec::new();
    for c in items {
        match c.action_type.as_str() {
            "constraint_violation" => violations.push(c.message.clone()),
            "constraint_warning" => warnings.push(c.message.clone()),
            _ => {}
        }
    }
    (violations, warnings)
}
