//! Characterization tests for the shipped governance consumer examples:
//! the same parse → compile → evaluate flow with explicit facts,
//! reservations, and evaluator, asserting allow, deny, warn, provenance,
//! demotion, and per-call isolation so the examples cannot silently rot.

use std::collections::{BTreeSet, HashSet};

use phr::{Fact, Provenance, RuleId};
use phronesis_governance::decision::{Decision, Verdict};
use phronesis_governance::evaluation::{
    EvaluationError, FactSnapshot, builtin_evaluator, evaluate,
};
use phronesis_governance::{CompiledPolicy, Phase, Selection, parse};

const POLICY: &str = r#"{
    "rules": [
        {
            "id": "no-unwrap",
            "phase": "pre",
            "when": [{"new_content_contains": ".unwrap("}],
            "then": {"block": "unwrap() is not allowed in production sources"}
        },
        {
            "id": "flag-todo",
            "phase": "post",
            "when": [{"new_content_contains": "TODO"}],
            "then": {"warn": "a TODO marker was left behind"}
        },
        {
            "id": "note-rust",
            "phase": "post",
            "when": [{"file_extension_is": "rs"}],
            "then": {"log": "a rust file changed"}
        }
    ]
}"#;

fn compiled() -> CompiledPolicy {
    let parsed = parse(POLICY, "consumer-examples-test").unwrap();
    CompiledPolicy::compile(parsed.file.rules).unwrap()
}

fn fact(id: &str, predicate: &str, arg: &str) -> Fact {
    Fact {
        id: id.to_string(),
        predicate: predicate.to_string(),
        args: vec![arg.to_string()],
        timestamp: 0,
        source: Some("consumer-examples-test".to_string()),
    }
}

fn snapshot(host: Vec<Fact>) -> FactSnapshot {
    FactSnapshot {
        host,
        extensions: vec![],
    }
}

fn reserved_set() -> HashSet<String> {
    ["new_content_contains", "file_extension_is"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

fn no_demote() -> BTreeSet<RuleId> {
    BTreeSet::new()
}

async fn evaluate_pre(policy: &CompiledPolicy, host: Vec<Fact>) -> Decision {
    evaluate(
        policy,
        Selection::Hook(Phase::Pre),
        &snapshot(host),
        &reserved_set(),
        &no_demote(),
        builtin_evaluator(),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn deny_scenario_blocks_with_provenance() {
    let policy = compiled();
    let decision = evaluate_pre(
        &policy,
        vec![
            fact("fact-unwrap", "new_content_contains", ".unwrap("),
            fact("fact-rust", "file_extension_is", "rs"),
        ],
    )
    .await;
    assert_eq!(decision.verdict, Verdict::Deny);
    assert_eq!(
        decision.block_messages(),
        vec!["unwrap() is not allowed in production sources"]
    );
    let (reason, consequence) = decision.iter_reasons().next().unwrap();
    assert_eq!(reason.index, 0);
    assert_eq!(reason.rule_id.as_ref().unwrap().as_str(), "no-unwrap");
    assert!(reason.severity.is_block());
    match &consequence.provenance {
        Provenance::RuleFiring { bound_facts, .. } => {
            assert_eq!(bound_facts, &vec!["fact-unwrap".to_string()]);
        }
        other => panic!("expected RuleFiring provenance, got {other:?}"),
    }
}

#[tokio::test]
async fn allow_scenario_has_no_reasons() {
    let policy = compiled();
    let decision = evaluate_pre(&policy, vec![fact("fact-rust", "file_extension_is", "rs")]).await;
    assert_eq!(decision.verdict, Verdict::Allow);
    assert!(decision.block_messages().is_empty());
    assert!(decision.warning_messages().is_empty());
    assert!(decision.reasons.is_empty());
}

#[tokio::test]
async fn warn_scenario_classifies_warning_and_info() {
    let policy = compiled();
    let decision = evaluate(
        &policy,
        Selection::Hook(Phase::Post),
        &snapshot(vec![
            fact("fact-todo", "new_content_contains", "TODO"),
            fact("fact-rust", "file_extension_is", "rs"),
        ]),
        &reserved_set(),
        &no_demote(),
        builtin_evaluator(),
    )
    .await
    .unwrap();
    assert_eq!(decision.verdict, Verdict::Warn);
    assert_eq!(
        decision.warning_messages(),
        vec!["a TODO marker was left behind"]
    );
    assert!(decision.block_messages().is_empty());
    let log_reason = decision
        .reasons
        .iter()
        .find(|r| r.action_type == "log")
        .expect("log consequence keeps a reason");
    assert!(!log_reason.severity.is_warning());
    assert_eq!(log_reason.rule_id.as_ref().unwrap().as_str(), "note-rust");
}

#[tokio::test]
async fn demote_scenario_downgrades_exact_rule_only() {
    let policy = compiled();
    let mut demote = BTreeSet::new();
    demote.insert(RuleId::new("no-unwrap".to_string()));
    let decision = evaluate(
        &policy,
        Selection::Hook(Phase::Pre),
        &snapshot(vec![fact(
            "fact-unwrap",
            "new_content_contains",
            ".unwrap(",
        )]),
        &reserved_set(),
        &demote,
        builtin_evaluator(),
    )
    .await
    .unwrap();
    assert_eq!(decision.verdict, Verdict::Warn);
    assert!(
        decision
            .warning_messages()
            .iter()
            .any(|m| m.contains("unwrap"))
    );
    assert!(decision.diagnostics.iter().any(|d| {
        d.rule_id()
            .map(|id| id.as_str() == "no-unwrap")
            .unwrap_or(false)
    }));
}

#[tokio::test]
async fn evaluations_are_isolated_and_rejections_leak_nothing() {
    let policy = compiled();
    let deny = evaluate_pre(
        &policy,
        vec![fact("fact-unwrap", "new_content_contains", ".unwrap(")],
    )
    .await;
    assert_eq!(deny.verdict, Verdict::Deny);

    let again = evaluate_pre(&policy, vec![fact("fact-rust", "file_extension_is", "rs")]).await;
    assert_eq!(again.verdict, Verdict::Allow);

    let rejected = evaluate(
        &policy,
        Selection::Hook(Phase::Pre),
        &FactSnapshot {
            host: vec![],
            extensions: vec![fact("ext-1", "new_content_contains", ".unwrap(")],
        },
        &reserved_set(),
        &no_demote(),
        builtin_evaluator(),
    )
    .await;
    assert_eq!(
        rejected.unwrap_err(),
        EvaluationError::ReservedExtensionPredicate {
            fact_id: "ext-1".to_string(),
            predicate: "new_content_contains".to_string(),
        }
    );

    let after = evaluate_pre(&policy, vec![fact("fact-rust", "file_extension_is", "rs")]).await;
    assert_eq!(after.verdict, Verdict::Allow);

    let duplicate = evaluate(
        &policy,
        Selection::Hook(Phase::Pre),
        &snapshot(vec![
            fact("fact-rust", "file_extension_is", "rs"),
            fact("fact-rust", "file_extension_is", "rs"),
        ]),
        &reserved_set(),
        &no_demote(),
        builtin_evaluator(),
    )
    .await;
    assert_eq!(
        duplicate.unwrap_err(),
        EvaluationError::DuplicateFactId {
            fact_id: "fact-rust".to_string(),
        }
    );
}
