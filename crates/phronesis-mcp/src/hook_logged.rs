//! Flattened projection of `phr::Consequence` for action-log entries,
//! and helpers that consume slices of these projections.
//!
//! Lives in its own module so `hook.rs` stays focused on the pre/post
//! orchestration. The shape is stable on the wire — `LoggedConsequence`
//! is what `jq` users see when they query `.phronesis/log.jsonl`.

use std::collections::HashMap;

use phr::consequence::{Consequence, Provenance};

use phronesis_governance::decision::{self, ACTION_WARN};

/// Flattened projection of `phr::Consequence` used in action-log
/// entries. We don't serialize the raw `Consequence` because its nested
/// `Provenance::RuleFiring { rule_id, bindings, .. }` is awkward to query
/// (`.provenance.RuleFiring.rule_id` etc.). Pulling rule_id and bindings
/// up to the top level gives `jq` users a flat, predictable shape.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct LoggedConsequence {
    pub rule_id: phr::RuleId,
    pub action_type: String,
    pub message: String,
    #[serde(default)]
    pub bindings: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<String>,
    /// Set when the rule fired because its `__script__` guard failed to
    /// evaluate (fail closed); the evaluator's error text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guard_error: Option<String>,
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
        let guard_error = c
            .payload
            .get("guard_error")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        Some(Self {
            rule_id,
            action_type,
            message,
            bindings,
            decisions,
            guard_error,
        })
    }
}

/// Classify `consequences` through the shared governance decision layer
/// (`phronesis_governance::decision::classify`) and project the outcome
/// into `(logged, violations, warnings)`.
///
/// This is the single classification path for every hook surface —
/// pre-check, post-check, and the Codex adapter all funnel through here;
/// audit derives its levels from the same `decision::action_severity`.
///
/// Only `Provenance::RuleFiring` consequences reach the classifier: that
/// is the hook's wire contract (non-firing provenance is omitted from the
/// log and has never carried decision weight), so a block-shaped payload
/// on Lookup/Asserted provenance stays non-blocking exactly as before.
/// Demotion takes the caller's exact rule-id set — graph freshness
/// detection stays a host concern; demoted blocks keep their raw payload
/// but log and print as `constraint_warning`, byte-identical to the
/// previous in-place rewrite.
///
/// Returns the logged wire entries plus the violation and warning message
/// lists, in original consequence order.
pub(crate) fn classify_logged(
    consequences: &[Consequence],
    demote_rule_ids: &std::collections::BTreeSet<phr::RuleId>,
) -> (Vec<LoggedConsequence>, Vec<String>, Vec<String>) {
    let firing: Vec<Consequence> = consequences
        .iter()
        .filter(|c| matches!(c.provenance, Provenance::RuleFiring { .. }))
        .cloned()
        .collect();
    let decision = decision::classify(&firing, demote_rule_ids);
    let logged: Vec<LoggedConsequence> = decision
        .reasons
        .iter()
        .zip(decision.consequences.iter())
        .filter_map(|(reason, consequence)| {
            let mut logged = LoggedConsequence::from_consequence(consequence)?;
            if reason.demoted {
                logged.action_type = ACTION_WARN.to_string();
            }
            Some(logged)
        })
        .collect();
    let violations = decision
        .block_messages()
        .into_iter()
        .map(str::to_string)
        .collect();
    let warnings = decision
        .warning_messages()
        .into_iter()
        .map(str::to_string)
        .collect();
    (logged, violations, warnings)
}

#[cfg(test)]
mod demote_tests {
    use super::*;
    use std::collections::BTreeSet;

    /// The legacy projection, kept verbatim as the parity reference: what
    /// `LoggedConsequence::from_consequence` + in-place
    /// `demote_violations_from` + `split_messages_by_action_type` produced
    /// before classification moved to the shared governance layer.
    mod legacy {
        use super::*;

        pub(crate) fn project(consequences: &[Consequence]) -> Vec<LoggedConsequence> {
            consequences
                .iter()
                .filter_map(LoggedConsequence::from_consequence)
                .collect()
        }

        pub(crate) fn demote(items: &mut [LoggedConsequence], rule_ids: &BTreeSet<phr::RuleId>) {
            for c in items.iter_mut() {
                if c.action_type == "constraint_violation" && rule_ids.contains(&c.rule_id) {
                    c.action_type = "constraint_warning".to_string();
                }
            }
        }

        pub(crate) fn split(items: &[LoggedConsequence]) -> (Vec<String>, Vec<String>) {
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
    }

    fn firing(rule_id: &str, action_type: &str) -> Consequence {
        Consequence {
            kind: phr::consequence::ConsequenceKind::Constraint,
            predicate: "check".into(),
            payload: serde_json::json!({
                "action_type": action_type,
                "message": format!("from {rule_id} ({action_type})"),
            }),
            provenance: phr::consequence::Provenance::RuleFiring {
                rule_id: rule_id.into(),
                bound_facts: Vec::new(),
                bindings: HashMap::new(),
                fact_sources: Default::default(),
                decisions: Vec::new(),
            },
        }
    }

    fn lookup(action_type: &str) -> Consequence {
        Consequence {
            kind: phr::consequence::ConsequenceKind::Constraint,
            predicate: "check".into(),
            payload: serde_json::json!({
                "action_type": action_type,
                "message": "from lookup",
            }),
            provenance: phr::consequence::Provenance::Lookup {
                tool: "test-tool".into(),
                schema_version: 1,
            },
        }
    }

    fn guard_fired(rule_id: &str, action_type: &str, error: &str) -> Consequence {
        let mut c = firing(rule_id, action_type);
        c.payload["guard_error"] = serde_json::json!(error);
        c
    }

    fn ids(names: &[&str]) -> BTreeSet<phr::RuleId> {
        names.iter().map(|n| phr::RuleId::from(*n)).collect()
    }

    fn assert_parity(
        consequences: &[Consequence],
        demote: &BTreeSet<phr::RuleId>,
    ) -> (Vec<String>, Vec<String>) {
        let (logged, violations, warnings) = classify_logged(consequences, demote);
        let mut legacy_logged = legacy::project(consequences);
        legacy::demote(&mut legacy_logged, demote);
        let (legacy_v, legacy_w) = legacy::split(&legacy_logged);
        assert_eq!(
            logged, legacy_logged,
            "logged wire output must be identical"
        );
        assert_eq!(violations, legacy_v, "violation messages must be identical");
        assert_eq!(warnings, legacy_w, "warning messages must be identical");
        (violations, warnings)
    }

    #[test]
    fn mixed_verdicts_match_the_legacy_projection() {
        let cs = [
            firing("block-rule", "constraint_violation"),
            firing("warn-rule", "constraint_warning"),
            firing("log-rule", "log"),
            firing("capsule-rule", "emit_capsule"),
            firing("odd-rule", "unknown_action"),
        ];
        let (violations, warnings) = assert_parity(&cs, &BTreeSet::new());
        assert_eq!(violations, vec!["from block-rule (constraint_violation)"]);
        assert_eq!(warnings, vec!["from warn-rule (constraint_warning)"]);
    }

    #[test]
    fn stale_demotion_matches_the_legacy_in_place_rewrite() {
        let cs = [
            firing("graph-rule", "constraint_violation"),
            firing("graph-rule", "constraint_warning"),
            firing("solid-rule", "constraint_violation"),
        ];
        let (violations, warnings) = assert_parity(&cs, &ids(&["graph-rule"]));
        assert_eq!(violations, vec!["from solid-rule (constraint_violation)"]);
        assert_eq!(
            warnings,
            vec![
                "from graph-rule (constraint_violation)",
                "from graph-rule (constraint_warning)",
            ]
        );
    }

    #[test]
    fn demotion_is_exact_id_only_and_matches_legacy() {
        let cs = [firing("base#or2", "constraint_violation")];
        let (violations, warnings) = assert_parity(&cs, &ids(&["base"]));
        assert_eq!(violations.len(), 1, "base-id sets must not demote #orN ids");
        assert!(warnings.is_empty());
    }

    #[test]
    fn non_rule_firing_provenance_is_omitted_and_never_blocks() {
        let cs = [
            lookup("constraint_violation"),
            firing("real-rule", "constraint_warning"),
        ];
        let (logged, violations, warnings) = classify_logged(&cs, &BTreeSet::new());
        assert_parity(&cs, &BTreeSet::new());
        assert_eq!(
            logged.len(),
            1,
            "non-RuleFiring provenance stays off the wire"
        );
        assert_eq!(
            violations.len(),
            0,
            "Lookup payloads carry no decision weight"
        );
        assert_eq!(warnings, vec!["from real-rule (constraint_warning)"]);
    }

    #[test]
    fn guard_failure_keeps_the_error_and_the_action_severity() {
        let cs = [
            guard_fired("guarded-block", "constraint_violation", "boom"),
            guard_fired("guarded-log", "log", "hush"),
        ];
        let (logged, violations, warnings) = classify_logged(&cs, &BTreeSet::new());
        assert_parity(&cs, &BTreeSet::new());
        assert_eq!(
            violations,
            vec!["from guarded-block (constraint_violation)"]
        );
        assert!(warnings.is_empty());
        assert_eq!(logged[0].guard_error.as_deref(), Some("boom"));
        assert_eq!(logged[1].guard_error.as_deref(), Some("hush"));
        assert_eq!(
            logged[1].action_type, "log",
            "guard error is diagnostic only"
        );
    }

    #[test]
    fn silent_rules_classify_like_any_other_rule() {
        let cs = [firing("quiet-rule", "constraint_violation")];
        let (violations, _warnings) = assert_parity(&cs, &BTreeSet::new());
        assert_eq!(violations.len(), 1, "classification is silent-agnostic");
    }

    #[test]
    fn an_unnamed_rules_violation_still_blocks() {
        let cs = [firing("other-rule", "constraint_violation")];
        let (violations, warnings) = assert_parity(&cs, &ids(&["graph-rule"]));
        assert_eq!(violations.len(), 1);
        assert!(warnings.is_empty());
    }

    #[test]
    fn an_existing_warning_is_left_alone() {
        let cs = [firing("graph-rule", "constraint_warning")];
        let (violations, warnings) = assert_parity(&cs, &ids(&["graph-rule"]));
        assert!(violations.is_empty());
        assert_eq!(warnings.len(), 1);
    }
}
