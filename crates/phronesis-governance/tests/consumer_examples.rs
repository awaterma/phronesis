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

// Syntax policies use structural facts; TODO is intentionally textual.
const POLICY: &str = r#"{
    "rules": [
        {
            "id": "no-unwrap",
            "phase": "pre",
            "when": [{"rust_governed_invocation": ["?file", "?fn", "unwrap"]}],
            "then": {"block": "unwrap() is not allowed in governed Rust code"}
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

// The host supplies this AST-derived fact shape; the governance core does
// not parse Rust. A real extractor omits comments, strings, and test code.
fn invocation_fact(id: &str, invocation: &str) -> Fact {
    Fact {
        id: id.to_string(),
        predicate: "rust_governed_invocation".to_string(),
        args: vec![
            "src/lib.rs".to_string(),
            "process".to_string(),
            invocation.to_string(),
        ],
        timestamp: 0,
        source: Some("consumer-flow".to_string()),
    }
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
    [
        "rust_governed_invocation",
        "new_content_contains",
        "file_extension_is",
    ]
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
            invocation_fact("fact-unwrap", "unwrap"),
            fact("fact-rust", "file_extension_is", "rs"),
        ],
    )
    .await;
    assert_eq!(decision.verdict, Verdict::Deny);
    assert_eq!(
        decision.block_messages(),
        vec!["unwrap() is not allowed in governed Rust code"]
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
async fn unwrap_rule_requires_a_structural_unwrap_fact() {
    let policy = compiled();
    // A lexical hit could describe a comment or string. It is not evidence
    // of a Rust call, and a different structural invocation is not unwrap.
    for host in [
        vec![fact("text-only", "new_content_contains", ".unwrap(")],
        vec![invocation_fact("other-call", "expect_empty")],
    ] {
        let decision = evaluate_pre(&policy, host).await;
        assert_eq!(decision.verdict, Verdict::Allow);
        assert!(decision.reasons.is_empty());
    }
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
        &snapshot(vec![invocation_fact("fact-unwrap", "unwrap")]),
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
    let deny = evaluate_pre(&policy, vec![invocation_fact("fact-unwrap", "unwrap")]).await;
    assert_eq!(deny.verdict, Verdict::Deny);

    let again = evaluate_pre(&policy, vec![fact("fact-rust", "file_extension_is", "rs")]).await;
    assert_eq!(again.verdict, Verdict::Allow);

    let rejected = evaluate(
        &policy,
        Selection::Hook(Phase::Pre),
        &FactSnapshot {
            host: vec![],
            extensions: vec![invocation_fact("ext-1", "unwrap")],
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
            predicate: "rust_governed_invocation".to_string(),
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

// The shipped expense-approval fixtures: a non-software domain (expense
// approval, not code rules) evaluated through the same public surface the
// operator flow uses. The fixtures live beside the acceptance script so the
// packaged-consumer docs and these tests cannot drift apart.
fn fixture_path(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/governance-consumer/fixtures")
        .join(name)
}

fn read_fixture(name: &str) -> String {
    std::fs::read_to_string(fixture_path(name))
        .unwrap_or_else(|e| panic!("fixture {name} unreadable: {e}"))
}

fn expense_policy() -> CompiledPolicy {
    let parsed = parse(&read_fixture("expense-rules.json"), "expense-rules.json").unwrap();
    CompiledPolicy::compile(parsed.file.rules).unwrap()
}

#[derive(serde::Deserialize)]
struct FixtureEvent {
    id: String,
    predicate: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    timestamp: u64,
    #[serde(default)]
    source: Option<String>,
}

impl FixtureEvent {
    fn into_fact(self) -> Fact {
        Fact {
            id: self.id,
            predicate: self.predicate,
            args: self.args,
            timestamp: self.timestamp,
            source: self.source,
        }
    }
}

// Mirrors the operator flow's events input: reservations are an explicit
// host-configuration list, never derived from the host facts present.
#[derive(serde::Deserialize)]
struct FixtureEvents {
    reserved_predicates: Vec<String>,
    #[serde(default)]
    host: Vec<FixtureEvent>,
    #[serde(default)]
    extensions: Vec<FixtureEvent>,
}

fn fixture_events(name: &str) -> FixtureEvents {
    serde_json::from_str(&read_fixture(name))
        .unwrap_or_else(|e| panic!("fixture {name} malformed: {e}"))
}

async fn evaluate_expense(
    policy: &CompiledPolicy,
    selection: Selection,
    events: FixtureEvents,
) -> Result<Decision, EvaluationError> {
    let reserved: HashSet<String> = events.reserved_predicates.into_iter().collect();
    let snapshot = FactSnapshot {
        host: events.host.into_iter().map(|f| f.into_fact()).collect(),
        extensions: events
            .extensions
            .into_iter()
            .map(|f| f.into_fact())
            .collect(),
    };
    evaluate(
        policy,
        selection,
        &snapshot,
        &reserved,
        &no_demote(),
        builtin_evaluator(),
    )
    .await
}

#[tokio::test]
async fn expense_fixtures_approve_warn_deny_and_reject_spoof() {
    let policy = expense_policy();

    let approve = evaluate_expense(
        &policy,
        Selection::Hook(Phase::Pre),
        fixture_events("expense-events-approve.json"),
    )
    .await
    .unwrap();
    assert_eq!(approve.verdict, Verdict::Allow);
    assert!(approve.reasons.is_empty());

    let warn = evaluate_expense(
        &policy,
        Selection::Hook(Phase::Pre),
        fixture_events("expense-events-warn.json"),
    )
    .await
    .unwrap();
    assert_eq!(warn.verdict, Verdict::Warn);
    assert!(
        warn.warning_messages()
            .iter()
            .any(|m| m.contains("receipt")),
        "expected a receipt warning, got {:?}",
        warn.warning_messages()
    );

    let deny = evaluate_expense(
        &policy,
        Selection::Hook(Phase::Pre),
        fixture_events("expense-events-deny.json"),
    )
    .await
    .unwrap();
    assert_eq!(deny.verdict, Verdict::Deny);
    assert!(
        deny.block_messages()
            .iter()
            .any(|m| m.contains("approval limit")),
        "expected a limit block, got {:?}",
        deny.block_messages()
    );
    assert_eq!(
        deny.reasons[0].rule_id.as_ref().unwrap().as_str(),
        "expense-over-limit"
    );

    // The spoofed extension asserts `rule_overridden`, which the events
    // configuration reserves but which NO host fact currently carries, and
    // it claims source "host": reservations are explicit configuration, not
    // derived from present host facts, and source is never authorization.
    let spoof = fixture_events("expense-events-spoof.json");
    assert!(
        spoof
            .reserved_predicates
            .iter()
            .any(|p| p == "rule_overridden"),
        "spoof fixture must reserve rule_overridden by configuration"
    );
    assert!(
        spoof.host.iter().all(|f| f.predicate != "rule_overridden"),
        "spoof fixture's discriminator is the predicate's absence from host facts"
    );
    let rejected = evaluate_expense(&policy, Selection::Hook(Phase::Pre), spoof)
        .await
        .unwrap_err();
    assert_eq!(
        rejected,
        EvaluationError::ReservedExtensionPredicate {
            fact_id: "ext-spoof-1045".to_string(),
            predicate: "rule_overridden".to_string(),
        }
    );
}

#[tokio::test]
async fn audit_phase_selection_targets_audit_flag_not_hook_phase() {
    let policy = expense_policy();

    // Selection::Audit picks the audit:true rule regardless of its disk
    // phase ("post"), matching the MCP audit engine's filter_audit_rules.
    let audited = evaluate_expense(
        &policy,
        Selection::Audit,
        fixture_events("expense-events-approve.json"),
    )
    .await
    .unwrap();
    assert_eq!(audited.verdict, Verdict::Allow);
    assert_eq!(audited.reasons.len(), 1);
    assert_eq!(
        audited.reasons[0].rule_id.as_ref().unwrap().as_str(),
        "expense-audit-trail"
    );
    assert!(audited.reasons[0].severity.is_block() || audited.reasons[0].action_type == "log");

    // Selection::Hook(Phase::Audit) would select only rules whose disk
    // phase is "audit" — none exist here, so nothing fires. This pins the
    // operator flow's `--phase audit` to Selection::Audit, not Hook(Audit).
    let hook_audit = evaluate_expense(
        &policy,
        Selection::Hook(Phase::Audit),
        fixture_events("expense-events-approve.json"),
    )
    .await
    .unwrap();
    assert_eq!(hook_audit.verdict, Verdict::Allow);
    assert!(hook_audit.reasons.is_empty());

    // At hook time the audit flag is ignored: the rule's disk phase ("post")
    // matches Selection::Hook(Phase::Post) and fires as informational.
    let hook_post = evaluate_expense(
        &policy,
        Selection::Hook(Phase::Post),
        fixture_events("expense-events-approve.json"),
    )
    .await
    .unwrap();
    assert_eq!(hook_post.verdict, Verdict::Allow);
    assert_eq!(hook_post.reasons.len(), 1);
    assert_eq!(
        hook_post.reasons[0].rule_id.as_ref().unwrap().as_str(),
        "expense-audit-trail"
    );
}
