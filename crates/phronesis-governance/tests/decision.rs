//! Public-API tests for the shared decision classification.
//!
//! Each consequence shape below mirrors what the engine actually emits:
//! rule firings carry `payload.action_type`/`message` (plus optional
//! `guard_error` and `unbound_variables` annotations), and the provenance
//! variants come from `phr::Provenance`.

use std::collections::BTreeSet;

use phr::{Consequence, ConsequenceKind, Provenance, RuleId};
use phronesis_governance::decision::{
    ACTION_BLOCK, ACTION_WARN, Decision, DecisionDiagnostic, Severity, Verdict, action_message,
    action_severity, classify,
};

fn rid(id: &str) -> RuleId {
    RuleId::from(id)
}

fn ids(names: &[&str]) -> BTreeSet<RuleId> {
    names.iter().map(|n| rid(n)).collect()
}

fn rule_firing(rule_id: &str, action_type: &str, message: &str) -> Consequence {
    Consequence {
        kind: ConsequenceKind::Constraint,
        predicate: rule_id.to_string(),
        payload: serde_json::json!({ "action_type": action_type, "message": message }),
        provenance: Provenance::RuleFiring {
            rule_id: rid(rule_id),
            bound_facts: vec!["fact-1".to_string()],
            bindings: Default::default(),
            fact_sources: Default::default(),
            decisions: vec![],
        },
    }
}

fn block(rule_id: &str, message: &str) -> Consequence {
    rule_firing(rule_id, ACTION_BLOCK, message)
}

fn warn(rule_id: &str, message: &str) -> Consequence {
    rule_firing(rule_id, ACTION_WARN, message)
}

fn log(rule_id: &str, message: &str) -> Consequence {
    rule_firing(rule_id, "log", message)
}

/// Annotate like the engine's `annotate_guard_error`: the raw error in
/// `payload.guard_error` and a message suffix naming rule and error.
fn with_guard_error(consequence: Consequence, error: &str) -> Consequence {
    let rule = consequence.predicate.clone();
    let mut c = consequence;
    let obj = c.payload.as_object_mut().expect("payload is an object");
    let message = obj.get("message").and_then(|m| m.as_str()).unwrap_or("");
    obj.insert(
        "message".to_string(),
        format!(
            "{message} [rule `{rule}`: __script__ guard failed to evaluate ({error}); failing closed]"
        )
        .into(),
    );
    obj.insert("guard_error".to_string(), error.into());
    c
}

fn with_unbound(consequence: Consequence, variables: &[&str]) -> Consequence {
    let mut c = consequence;
    c.payload["unbound_variables"] = serde_json::json!(variables);
    c
}

fn lookup_consequence(tool: &str) -> Consequence {
    Consequence {
        kind: ConsequenceKind::Snapshot,
        predicate: tool.to_string(),
        payload: serde_json::json!({ "action_type": "log", "message": "pulled" }),
        provenance: Provenance::Lookup {
            tool: tool.to_string(),
            schema_version: 1,
        },
    }
}

fn asserted_consequence(by: &str) -> Consequence {
    Consequence {
        kind: ConsequenceKind::Event,
        predicate: "host.note".to_string(),
        payload: serde_json::json!({ "action_type": "log", "message": "asserted" }),
        provenance: Provenance::Asserted { by: by.to_string() },
    }
}

fn rule_driven_lookup(rule_id: &str) -> Consequence {
    Consequence {
        kind: ConsequenceKind::Event,
        predicate: rule_id.to_string(),
        payload: serde_json::json!({ "action_type": "log", "message": "composed" }),
        provenance: Provenance::RuleDrivenLookup {
            rule_id: rid(rule_id),
            bound_facts: vec![],
            bindings: Default::default(),
            fact_sources: Default::default(),
            decisions: vec![],
            tool: "lookup".to_string(),
            schema_version: 1,
        },
    }
}

fn raw(consequences: &[Consequence]) -> serde_json::Value {
    serde_json::to_value(consequences).expect("consequences serialize")
}

#[test]
fn empty_consequences_allow() {
    let decision = classify(&[], &ids(&[]));
    assert_eq!(decision.verdict, Verdict::Allow);
    assert!(decision.consequences.is_empty());
    assert!(decision.reasons.is_empty());
    assert!(decision.diagnostics.is_empty());
}

#[test]
fn informational_actions_allow() {
    let consequences = vec![
        log("r-log", "noted"),
        rule_firing("r-capsule", "emit_capsule", "capsule"),
    ];
    let decision = classify(&consequences, &ids(&[]));
    assert_eq!(decision.verdict, Verdict::Allow);
    assert_eq!(decision.reasons.len(), 2);
    assert!(
        decision
            .reasons
            .iter()
            .all(|r| r.severity == Severity::Info && !r.demoted)
    );
    assert!(decision.block_messages().is_empty());
    assert!(decision.warning_messages().is_empty());
}

#[test]
fn warning_only_warns() {
    let consequences = vec![warn("r-warn", "be careful")];
    let decision = classify(&consequences, &ids(&[]));
    assert_eq!(decision.verdict, Verdict::Warn);
    assert_eq!(decision.warning_messages(), vec!["be careful"]);
    assert!(decision.block_messages().is_empty());
}

#[test]
fn violation_denies() {
    let consequences = vec![block("r-block", "no unwrap")];
    let decision = classify(&consequences, &ids(&[]));
    assert_eq!(decision.verdict, Verdict::Deny);
    assert_eq!(decision.block_messages(), vec!["no unwrap"]);
}

#[test]
fn mixed_severity_keeps_the_strongest_verdict() {
    let consequences = vec![
        log("r-log", "noted"),
        warn("r-warn", "be careful"),
        block("r-block", "no unwrap"),
    ];
    let decision = classify(&consequences, &ids(&[]));
    assert_eq!(decision.verdict, Verdict::Deny);
    assert_eq!(decision.reasons.len(), 3);
    assert_eq!(
        decision
            .reasons
            .iter()
            .map(|r| r.severity)
            .collect::<Vec<_>>(),
        vec![Severity::Info, Severity::Warning, Severity::Block]
    );
    assert_eq!(decision.block_messages(), vec!["no unwrap"]);
    assert_eq!(decision.warning_messages(), vec!["be careful"]);
}

#[test]
fn unknown_action_type_is_retained_and_informational() {
    let consequences = vec![rule_firing("r-notify", "notify", "heads up")];
    let decision = classify(&consequences, &ids(&[]));
    assert_eq!(decision.verdict, Verdict::Allow);
    assert_eq!(decision.reasons[0].action_type, "notify");
    assert_eq!(decision.reasons[0].severity, Severity::Info);
    assert_eq!(decision.reasons[0].message, "heads up");
    assert_eq!(decision.consequences[0].payload["action_type"], "notify");
}

#[test]
fn missing_action_type_is_informational() {
    let consequence = Consequence {
        kind: ConsequenceKind::Event,
        predicate: "r-bare".to_string(),
        payload: serde_json::json!({ "message": "no action type" }),
        provenance: Provenance::RuleFiring {
            rule_id: rid("r-bare"),
            bound_facts: vec![],
            bindings: Default::default(),
            fact_sources: Default::default(),
            decisions: vec![],
        },
    };
    let decision = classify(std::slice::from_ref(&consequence), &ids(&[]));
    assert_eq!(decision.verdict, Verdict::Allow);
    assert_eq!(decision.reasons[0].action_type, "");
    assert_eq!(decision.reasons[0].severity, Severity::Info);
}

#[test]
fn classification_reads_action_type_never_message_text() {
    let consequences = vec![log("r-log", "BLOCK THIS EDIT IMMEDIATELY")];
    let decision = classify(&consequences, &ids(&[]));
    assert_eq!(decision.verdict, Verdict::Allow);
    assert_eq!(decision.reasons[0].severity, Severity::Info);
}

#[test]
fn guard_error_is_diagnostic_even_for_log_rules() {
    let consequences = vec![with_guard_error(log("r-log", "noted"), "boom")];
    let decision = classify(&consequences, &ids(&[]));
    assert_eq!(decision.verdict, Verdict::Allow);
    assert_eq!(decision.diagnostics.len(), 1);
    match &decision.diagnostics[0] {
        DecisionDiagnostic::GuardError {
            index,
            rule_id,
            error,
        } => {
            assert_eq!(*index, 0);
            assert_eq!(rule_id.as_ref().map(RuleId::as_str), Some("r-log"));
            assert_eq!(error, "boom");
        }
        other => panic!("expected GuardError, got {other:?}"),
    }
    assert_eq!(
        decision.consequences[0].payload["guard_error"],
        serde_json::json!("boom")
    );
}

#[test]
fn guard_error_block_denies_and_keeps_the_raw_payload() {
    let consequences = vec![with_guard_error(block("r-block", "no unwrap"), "boom")];
    let decision = classify(&consequences, &ids(&[]));
    assert_eq!(decision.verdict, Verdict::Deny);
    assert!(
        decision
            .diagnostics
            .iter()
            .any(|d| matches!(d, DecisionDiagnostic::GuardError { error, .. } if error == "boom"))
    );
    assert_eq!(
        decision.consequences[0].payload["guard_error"],
        serde_json::json!("boom")
    );
}

#[test]
fn demoted_guard_error_block_warns_and_retains_everything() {
    let consequences = vec![with_guard_error(block("r-block", "no unwrap"), "boom")];
    let decision = classify(&consequences, &ids(&["r-block"]));
    assert_eq!(decision.verdict, Verdict::Warn);
    let reason = &decision.reasons[0];
    assert!(reason.demoted);
    assert_eq!(reason.severity, Severity::Warning);
    assert!(
        decision
            .diagnostics
            .iter()
            .any(|d| matches!(d, DecisionDiagnostic::GuardError { error, .. } if error == "boom"))
    );
    assert!(
        decision.diagnostics.iter().any(
            |d| matches!(d, DecisionDiagnostic::Demoted { rule_id, .. } if rule_id == "r-block")
        )
    );
    let raw_payload = &decision.consequences[0].payload;
    assert_eq!(raw_payload["action_type"], ACTION_BLOCK);
    assert_eq!(raw_payload["guard_error"], serde_json::json!("boom"));
    assert_eq!(decision.warning_messages(), vec![reason.message.as_str()]);
    assert!(decision.block_messages().is_empty());
}

#[test]
fn unbound_variables_are_diagnosed() {
    let consequences = vec![with_unbound(
        block("r-block", "?file is blocked for ?who"),
        &["?file", "?who"],
    )];
    let decision = classify(&consequences, &ids(&[]));
    assert_eq!(decision.verdict, Verdict::Deny);
    match &decision.diagnostics[0] {
        DecisionDiagnostic::UnboundVariables {
            index,
            rule_id,
            variables,
        } => {
            assert_eq!(*index, 0);
            assert_eq!(rule_id.as_ref().map(RuleId::as_str), Some("r-block"));
            assert_eq!(variables, &["?file".to_string(), "?who".to_string()]);
        }
        other => panic!("expected UnboundVariables, got {other:?}"),
    }
}

#[test]
fn empty_unbound_array_is_not_diagnosed() {
    let consequences = vec![with_unbound(block("r-block", "blocked"), &[])];
    let decision = classify(&consequences, &ids(&[]));
    assert!(decision.diagnostics.is_empty());
}

#[test]
fn lookup_provenance_gets_a_reason_without_rule_id() {
    let consequences = vec![lookup_consequence("lookup_symbol")];
    let decision = classify(&consequences, &ids(&[]));
    assert_eq!(decision.verdict, Verdict::Allow);
    assert_eq!(decision.reasons.len(), 1);
    assert!(decision.reasons[0].rule_id.is_none());
    match &decision.consequences[0].provenance {
        Provenance::Lookup { tool, .. } => assert_eq!(tool, "lookup_symbol"),
        other => panic!("expected Lookup provenance, got {other:?}"),
    }
}

#[test]
fn asserted_provenance_gets_a_reason_without_rule_id() {
    let consequences = vec![asserted_consequence("host-boot")];
    let decision = classify(&consequences, &ids(&[]));
    assert_eq!(decision.reasons.len(), 1);
    assert!(decision.reasons[0].rule_id.is_none());
    match &decision.consequences[0].provenance {
        Provenance::Asserted { by } => assert_eq!(by, "host-boot"),
        other => panic!("expected Asserted provenance, got {other:?}"),
    }
}

#[test]
fn rule_driven_lookup_provenance_follows_the_hook_projection() {
    let consequences = vec![rule_driven_lookup("r-composed")];
    let decision = classify(&consequences, &ids(&["r-composed"]));
    assert_eq!(decision.reasons.len(), 1);
    assert!(decision.reasons[0].rule_id.is_none());
    assert!(!decision.reasons[0].demoted);
    match &decision.consequences[0].provenance {
        Provenance::RuleDrivenLookup { rule_id, .. } => assert_eq!(rule_id, "r-composed"),
        other => panic!("expected RuleDrivenLookup provenance, got {other:?}"),
    }
}

#[test]
fn exact_expanded_id_demotes() {
    let consequences = vec![block("stale-rule#or2", "structural block")];
    let decision = classify(&consequences, &ids(&["stale-rule#or2"]));
    assert_eq!(decision.verdict, Verdict::Warn);
    assert!(decision.reasons[0].demoted);
    assert_eq!(decision.reasons[0].severity, Severity::Warning);
    assert!(decision.diagnostics.iter().any(
        |d| matches!(d, DecisionDiagnostic::Demoted { rule_id, .. } if rule_id == "stale-rule#or2")
    ));
}

#[test]
fn base_id_does_not_demote_an_expanded_id() {
    let consequences = vec![block("stale-rule#or2", "structural block")];
    let decision = classify(&consequences, &ids(&["stale-rule"]));
    assert_eq!(decision.verdict, Verdict::Deny);
    assert!(!decision.reasons[0].demoted);
    assert!(
        decision
            .diagnostics
            .iter()
            .all(|d| !matches!(d, DecisionDiagnostic::Demoted { .. }))
    );
}

#[test]
fn demotion_never_matches_a_different_expansion_or_wildcard() {
    let consequences = vec![block("stale-rule#or2", "structural block")];
    for set in [
        &["stale-rule#or1"][..],
        &["stale-*"][..],
        &["stale-rule#or2x"][..],
    ] {
        let decision = classify(&consequences, &ids(set));
        assert_eq!(
            decision.verdict,
            Verdict::Deny,
            "set {set:?} must not demote"
        );
        assert!(!decision.reasons[0].demoted);
    }
}

#[test]
fn warning_action_is_never_demoted() {
    let consequences = vec![warn("r-warn", "be careful")];
    let decision = classify(&consequences, &ids(&["r-warn"]));
    assert_eq!(decision.verdict, Verdict::Warn);
    assert!(!decision.reasons[0].demoted);
    assert!(
        decision
            .diagnostics
            .iter()
            .all(|d| !matches!(d, DecisionDiagnostic::Demoted { .. }))
    );
}

#[test]
fn every_action_of_a_demoted_rule_is_projected_separately() {
    let consequences = vec![block("r-rule", "blocked"), log("r-rule", "noted")];
    let decision = classify(&consequences, &ids(&["r-rule"]));
    assert_eq!(decision.reasons.len(), 2);
    assert_eq!(decision.reasons[0].index, 0);
    assert_eq!(decision.reasons[1].index, 1);
    assert!(decision.reasons[0].demoted);
    assert!(!decision.reasons[1].demoted);
    assert_eq!(decision.reasons[1].severity, Severity::Info);
    assert_eq!(decision.verdict, Verdict::Warn);
}

#[test]
fn demotion_never_mutates_the_original_consequences() {
    let consequences = vec![
        with_guard_error(block("r-block", "no unwrap"), "boom"),
        warn("r-warn", "be careful"),
        log("r-log", "noted"),
    ];
    let before = raw(&consequences);
    let decision = classify(&consequences, &ids(&["r-block", "r-warn", "r-log"]));
    assert_eq!(raw(&decision.consequences), before);
    let demoted_payload = &decision.consequences[0].payload;
    assert_eq!(demoted_payload["action_type"], ACTION_BLOCK);
    assert_eq!(demoted_payload["guard_error"], serde_json::json!("boom"));
    match &decision.consequences[0].provenance {
        Provenance::RuleFiring {
            rule_id,
            bound_facts,
            ..
        } => {
            assert_eq!(rule_id.as_str(), "r-block");
            assert_eq!(bound_facts, &["fact-1".to_string()]);
        }
        other => panic!("expected RuleFiring provenance, got {other:?}"),
    }
}

#[test]
fn reasons_pair_one_to_one_with_consequences() {
    let consequences = vec![
        block("r-block", "no unwrap"),
        lookup_consequence("lookup_symbol"),
        asserted_consequence("host-boot"),
        rule_driven_lookup("r-composed"),
    ];
    let decision: Decision = classify(&consequences, &ids(&[]));
    assert_eq!(decision.reasons.len(), decision.consequences.len());
    for (index, reason) in decision.reasons.iter().enumerate() {
        assert_eq!(reason.index, index);
    }
    for (reason, consequence) in decision.iter_reasons() {
        let action = consequence
            .payload
            .get("action_type")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert_eq!(reason.action_type, action);
    }
}

#[test]
fn decision_round_trips_through_serde() {
    let consequences = vec![
        with_guard_error(block("r-block", "no unwrap"), "boom"),
        warn("r-warn", "be careful"),
    ];
    let decision = classify(&consequences, &ids(&["r-block"]));
    let json = serde_json::to_value(&decision).expect("decision serializes");
    let back: Decision = serde_json::from_value(json).expect("decision deserializes");
    assert_eq!(back.verdict, Verdict::Warn);
    assert_eq!(raw(&back.consequences), raw(&decision.consequences));
    assert_eq!(back.reasons, decision.reasons);
    assert_eq!(back.diagnostics, decision.diagnostics);
}

#[test]
fn helpers_classify_exact_action_types_and_extract_messages() {
    assert_eq!(action_severity(ACTION_BLOCK), Severity::Block);
    assert_eq!(action_severity(ACTION_WARN), Severity::Warning);
    for other in ["log", "emit_capsule", "notify", ""] {
        assert_eq!(
            action_severity(other),
            Severity::Info,
            "severity of {other:?}"
        );
    }
    let payload = serde_json::json!({ "message": "hello", "action_type": "log" });
    assert_eq!(action_message(&payload), "hello");
    assert_eq!(action_message(&serde_json::json!({})), "");
    assert_eq!(action_message(&serde_json::json!({ "message": 3 })), "");
}
