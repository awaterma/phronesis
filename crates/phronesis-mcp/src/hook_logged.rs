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
/// up to the top level gives `jq` users a flat, predictable shape.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct LoggedConsequence {
    pub rule_id: phr::RuleId,
    pub action_type: String,
    pub message: String,
    #[serde(default)]
    pub bindings: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<String>,
}

impl LoggedConsequence {
    /// Project a `Consequence` whose provenance is `RuleFiring` into a
    /// flat log entry. Returns `None` for other provenance variants
    /// (Lookup, RuleDrivenLookup, Asserted) which the hook doesn't
    /// currently emit.
    pub(crate) fn from_consequence(c: &Consequence) -> Option<Self> {
        let (rule_id, bindings, decisions) = match &c.provenance {
            Provenance::RuleFiring {
                rule_id,
                bindings,
                decisions,
                ..
            } => (rule_id.clone(), bindings.clone(), decisions.clone()),
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
            decisions,
        })
    }
}

/// Demote violations raised by `rule_ids` to warnings, in place.
///
/// Used when a rule's evidence base is known to be untrustworthy — today,
/// when the structural graph has drifted from the working tree. The rule
/// itself is unchanged and still says "block"; the harness declines to act
/// on that verdict because it cannot vouch for the input. Keeping this
/// decision here rather than in `rules.json` means rule authors describe the
/// code, not the reliability of the machinery reading it.
///
/// Returns the number of consequences demoted.
pub(crate) fn demote_violations_from(
    items: &mut [LoggedConsequence],
    rule_ids: &std::collections::BTreeSet<phr::RuleId>,
) -> usize {
    let mut demoted = 0;
    for c in items.iter_mut() {
        if c.action_type == "constraint_violation" && rule_ids.contains(&c.rule_id) {
            c.action_type = "constraint_warning".to_string();
            demoted += 1;
        }
    }
    demoted
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

#[cfg(test)]
mod demote_tests {
    use super::*;
    use std::collections::BTreeSet;

    fn consequence(rule_id: &str, action_type: &str) -> LoggedConsequence {
        LoggedConsequence {
            rule_id: rule_id.into(),
            action_type: action_type.to_string(),
            message: format!("from {rule_id}"),
            bindings: HashMap::new(),
            decisions: Vec::new(),
        }
    }

    fn ids(names: &[&str]) -> BTreeSet<phr::RuleId> {
        names.iter().map(|n| phr::RuleId::from(*n)).collect()
    }

    #[test]
    fn a_named_rules_violation_becomes_a_warning() {
        let mut items = vec![consequence("graph-rule", "constraint_violation")];
        assert_eq!(demote_violations_from(&mut items, &ids(&["graph-rule"])), 1);
        assert_eq!(items[0].action_type, "constraint_warning");
    }

    #[test]
    fn an_unnamed_rules_violation_still_blocks() {
        let mut items = vec![consequence("other-rule", "constraint_violation")];
        assert_eq!(demote_violations_from(&mut items, &ids(&["graph-rule"])), 0);
        assert_eq!(items[0].action_type, "constraint_violation");
    }

    #[test]
    fn an_existing_warning_is_left_alone() {
        let mut items = vec![consequence("graph-rule", "constraint_warning")];
        assert_eq!(demote_violations_from(&mut items, &ids(&["graph-rule"])), 0);
        assert_eq!(items[0].action_type, "constraint_warning");
    }

    #[test]
    fn the_message_survives_demotion() {
        let mut items = vec![consequence("graph-rule", "constraint_violation")];
        demote_violations_from(&mut items, &ids(&["graph-rule"]));
        assert_eq!(items[0].message, "from graph-rule");
    }

    #[test]
    fn demotion_makes_the_split_report_it_as_a_warning() {
        let mut items = vec![consequence("graph-rule", "constraint_violation")];
        demote_violations_from(&mut items, &ids(&["graph-rule"]));
        let (violations, warnings) = split_messages_by_action_type(&items);
        assert!(
            violations.is_empty(),
            "a stale-evidence rule must not block"
        );
        assert_eq!(warnings.len(), 1);
    }
}
