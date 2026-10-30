//! Tests for the evaluation module: fresh-network snapshot evaluation,
//! selection modes, reserved extension prevalidation, fact-id uniqueness,
//! demotion, and error-vs-denial separation.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Mutex;

use phr::{Fact, RuleId, ScriptEval};
use phronesis_governance::decision::{DecisionDiagnostic, Severity, Verdict};
use phronesis_governance::evaluation::{
    EvaluationError, FactSnapshot, builtin_evaluator, evaluate,
};
use phronesis_governance::policy::{
    CompiledPolicy, DiskAction, DiskCondition, DiskRule, Phase, Selection,
};

/// The `ScriptEval` trait fixes its error type to `String`; the impls below
/// name it through this alias.
type ScriptResult = Result<bool, String>;

const RESERVED: &[&str] = &["signal_probe", "journey_flag", "rule_overridden"];

fn fact(id: &str, predicate: &str, args: &[&str]) -> Fact {
    Fact {
        id: id.to_string(),
        predicate: predicate.to_string(),
        args: args.iter().map(|a| a.to_string()).collect(),
        timestamp: 0,
        source: None,
    }
}

fn sourced_fact(id: &str, predicate: &str, args: &[&str], source: &str) -> Fact {
    Fact {
        source: Some(source.to_string()),
        ..fact(id, predicate, args)
    }
}

fn disk_rule(id: &str, phase: &str, predicate: &str, action_type: &str) -> DiskRule {
    DiskRule {
        id: id.to_string(),
        phase: phase.to_string(),
        priority: 10,
        conditions: vec![DiskCondition {
            predicate: predicate.to_string(),
            args: vec!["set".to_string()],
            script: None,
        }],
        actions: vec![DiskAction {
            action_type: action_type.to_string(),
            params: vec![format!("{action_type} by {id}")],
            data: None,
        }],
        silent: None,
        audit: None,
        doc_excepted: None,
    }
}

fn block_rule(id: &str, phase: &str, predicate: &str) -> DiskRule {
    disk_rule(id, phase, predicate, "constraint_violation")
}

fn audit_block_rule(id: &str, phase: &str, predicate: &str) -> DiskRule {
    DiskRule {
        audit: Some(true),
        ..block_rule(id, phase, predicate)
    }
}

fn warn_rule(id: &str, phase: &str, predicate: &str) -> DiskRule {
    disk_rule(id, phase, predicate, "constraint_warning")
}

fn script_block_rule(id: &str, phase: &str, script: &str) -> DiskRule {
    DiskRule {
        conditions: vec![DiskCondition {
            predicate: "__script__".to_string(),
            args: vec![],
            script: Some(script.to_string()),
        }],
        ..block_rule(id, phase, "unused")
    }
}

fn policy(rules: Vec<DiskRule>) -> CompiledPolicy {
    CompiledPolicy::compile(rules).expect("test policy must compile")
}

fn reserved_set() -> HashSet<String> {
    RESERVED.iter().map(|r| r.to_string()).collect()
}

fn demote(ids: &[&str]) -> BTreeSet<RuleId> {
    ids.iter().map(|id| RuleId::new(*id)).collect()
}

fn snap(host: Vec<Fact>, extensions: Vec<Fact>) -> FactSnapshot {
    FactSnapshot { host, extensions }
}

fn flag_fact() -> Fact {
    fact("f1", "flag", &["set"])
}

/// A test double for `ScriptEval` returning a fixed outcome and recording
/// every script it is asked to evaluate.
#[derive(Debug)]
struct ScriptedEval {
    outcome: ScriptResult,
    seen: Mutex<Vec<String>>,
}

impl ScriptedEval {
    fn allow() -> Self {
        Self {
            outcome: Ok(true),
            seen: Mutex::new(Vec::new()),
        }
    }

    fn deny() -> Self {
        Self {
            outcome: Ok(false),
            seen: Mutex::new(Vec::new()),
        }
    }

    fn broken(error: &str) -> Self {
        Self {
            outcome: Err(error.to_string()),
            seen: Mutex::new(Vec::new()),
        }
    }
}

impl ScriptEval for ScriptedEval {
    fn evaluate(
        &self,
        script: &str,
        _facts: &[Fact],
        _bindings: &HashMap<String, String>,
    ) -> ScriptResult {
        self.seen.lock().unwrap().push(script.to_string());
        self.outcome.clone()
    }
}

#[tokio::test]
async fn empty_selection_allows_without_network() {
    let policy = policy(vec![block_rule("pre-block", "pre", "flag")]);
    let snapshot = snap(vec![flag_fact()], vec![]);

    let decision = evaluate(
        &policy,
        Selection::Hook(Phase::None),
        &snapshot,
        &reserved_set(),
        &demote(&[]),
        builtin_evaluator(),
    )
    .await
    .expect("empty selection evaluates successfully");

    assert_eq!(decision.verdict, Verdict::Allow);
    assert!(decision.consequences.is_empty());
    assert!(decision.reasons.is_empty());
    assert!(decision.diagnostics.is_empty());
}

#[tokio::test]
async fn all_selection_modes_preserve_baseline_filtering() {
    let policy = policy(vec![
        block_rule("pre-block", "pre", "flag"),
        warn_rule("post-warn", "post", "flag"),
        DiskRule {
            audit: Some(true),
            ..warn_rule("audit-phase-audit", "audit", "flag")
        },
        // Parked phase but opted into audit: baseline filter_audit_rules
        // selects exactly audit == Some(true) without testing phase, so
        // this rule appears in audits today (documented discrepancy).
        audit_block_rule("parked-audit", "none", "flag"),
    ]);
    let snapshot = snap(vec![flag_fact()], vec![]);
    let reserved = reserved_set();
    let no_demote = demote(&[]);

    let pre = evaluate(
        &policy,
        Selection::Hook(Phase::Pre),
        &snapshot,
        &reserved,
        &no_demote,
        builtin_evaluator(),
    )
    .await
    .unwrap();
    assert_eq!(pre.verdict, Verdict::Deny);
    assert_eq!(
        pre.block_messages(),
        vec!["constraint_violation by pre-block"]
    );

    let post = evaluate(
        &policy,
        Selection::Hook(Phase::Post),
        &snapshot,
        &reserved,
        &no_demote,
        builtin_evaluator(),
    )
    .await
    .unwrap();
    assert_eq!(post.verdict, Verdict::Warn);
    assert_eq!(
        post.warning_messages(),
        vec!["constraint_warning by post-warn"]
    );

    let hook_audit = evaluate(
        &policy,
        Selection::Hook(Phase::Audit),
        &snapshot,
        &reserved,
        &no_demote,
        builtin_evaluator(),
    )
    .await
    .unwrap();
    assert_eq!(hook_audit.verdict, Verdict::Warn);
    assert_eq!(
        hook_audit.warning_messages(),
        vec!["constraint_warning by audit-phase-audit"]
    );

    let none = evaluate(
        &policy,
        Selection::Hook(Phase::None),
        &snapshot,
        &reserved,
        &no_demote,
        builtin_evaluator(),
    )
    .await
    .unwrap();
    assert_eq!(none.verdict, Verdict::Allow);

    let audit = evaluate(
        &policy,
        Selection::Audit,
        &snapshot,
        &reserved,
        &no_demote,
        builtin_evaluator(),
    )
    .await
    .unwrap();
    // Exactly audit == Some(true) regardless of phase: the parked rule's
    // block participates (source discrepancy preserved, not fixed here).
    assert_eq!(audit.verdict, Verdict::Deny);
    assert_eq!(
        audit.warning_messages(),
        vec!["constraint_warning by audit-phase-audit"]
    );
    assert_eq!(
        audit.block_messages(),
        vec!["constraint_violation by parked-audit"]
    );
}

#[tokio::test]
async fn reserved_extension_predicate_rejected_before_any_assertion() {
    let policy = policy(vec![block_rule("pre-block", "pre", "flag")]);

    // Rejected even though the selection would match a rule and the host
    // fact is fine: the whole snapshot is refused, nothing is asserted.
    let snapshot = snap(
        vec![flag_fact()],
        vec![fact("ext1", "signal_probe", &["x"])],
    );
    let error = evaluate(
        &policy,
        Selection::Hook(Phase::Pre),
        &snapshot,
        &reserved_set(),
        &demote(&[]),
        builtin_evaluator(),
    )
    .await
    .expect_err("reserved extension predicate must be rejected");
    assert_eq!(
        error,
        EvaluationError::ReservedExtensionPredicate {
            fact_id: "ext1".to_string(),
            predicate: "signal_probe".to_string(),
        }
    );

    // Rejected even with an empty selection: reserved input is refused
    // before any consideration of what would run.
    let empty = evaluate(
        &policy,
        Selection::Hook(Phase::None),
        &snapshot,
        &reserved_set(),
        &demote(&[]),
        builtin_evaluator(),
    )
    .await;
    assert!(matches!(
        empty,
        Err(EvaluationError::ReservedExtensionPredicate { .. })
    ));
}

#[tokio::test]
async fn reserved_extension_rejected_even_when_source_claims_host() {
    let policy = policy(vec![block_rule("pre-block", "pre", "flag")]);
    let snapshot = snap(
        vec![],
        vec![sourced_fact("ext1", "journey_flag", &["x"], "host")],
    );
    let error = evaluate(
        &policy,
        Selection::Hook(Phase::Pre),
        &snapshot,
        &reserved_set(),
        &demote(&[]),
        builtin_evaluator(),
    )
    .await
    .expect_err("source attribution is not authorization");
    assert!(matches!(
        error,
        EvaluationError::ReservedExtensionPredicate { .. }
    ));
}

#[tokio::test]
async fn host_facts_may_assert_reserved_predicates() {
    let policy = policy(vec![block_rule("res-block", "pre", "signal_probe")]);
    let snapshot = snap(vec![fact("h1", "signal_probe", &["set"])], vec![]);
    let decision = evaluate(
        &policy,
        Selection::Hook(Phase::Pre),
        &snapshot,
        &reserved_set(),
        &demote(&[]),
        builtin_evaluator(),
    )
    .await
    .expect("host facts own the reserved namespace");
    assert_eq!(decision.verdict, Verdict::Deny);
    assert_eq!(
        decision.block_messages(),
        vec!["constraint_violation by res-block"]
    );
}

#[tokio::test]
async fn duplicate_fact_ids_rejected_across_host_and_extensions() {
    let policy = policy(vec![block_rule("pre-block", "pre", "flag")]);
    let reserved = reserved_set();
    let no_demote = demote(&[]);

    let within_host = snap(vec![flag_fact(), fact("f1", "other", &["set"])], vec![]);
    let error = evaluate(
        &policy,
        Selection::Hook(Phase::Pre),
        &within_host,
        &reserved,
        &no_demote,
        builtin_evaluator(),
    )
    .await
    .unwrap_err();
    assert_eq!(
        error,
        EvaluationError::DuplicateFactId {
            fact_id: "f1".to_string()
        }
    );

    let across = snap(vec![flag_fact()], vec![fact("f1", "ext", &["set"])]);
    let error = evaluate(
        &policy,
        Selection::Hook(Phase::Pre),
        &across,
        &reserved,
        &no_demote,
        builtin_evaluator(),
    )
    .await
    .unwrap_err();
    assert_eq!(
        error,
        EvaluationError::DuplicateFactId {
            fact_id: "f1".to_string()
        }
    );

    let within_extensions = snap(
        vec![],
        vec![fact("e1", "a", &["set"]), fact("e1", "b", &["set"])],
    );
    let error = evaluate(
        &policy,
        Selection::Hook(Phase::Pre),
        &within_extensions,
        &reserved,
        &no_demote,
        builtin_evaluator(),
    )
    .await
    .unwrap_err();
    assert_eq!(
        error,
        EvaluationError::DuplicateFactId {
            fact_id: "e1".to_string()
        }
    );

    // Distinct ids across host and extensions are fine.
    let ok = snap(vec![flag_fact()], vec![fact("e1", "ext", &["set"])]);
    evaluate(
        &policy,
        Selection::Hook(Phase::Pre),
        &ok,
        &reserved,
        &no_demote,
        builtin_evaluator(),
    )
    .await
    .expect("unique ids pass");
}

#[tokio::test]
async fn reserved_predicate_check_precedes_duplicate_id_check() {
    let policy = policy(vec![block_rule("pre-block", "pre", "flag")]);
    let snapshot = snap(
        vec![flag_fact()],
        vec![fact("f1", "signal_probe", &["set"])],
    );
    let error = evaluate(
        &policy,
        Selection::Hook(Phase::Pre),
        &snapshot,
        &reserved_set(),
        &demote(&[]),
        builtin_evaluator(),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        EvaluationError::ReservedExtensionPredicate { .. }
    ));
}

#[tokio::test]
async fn custom_injected_evaluator_controls_script_firing() {
    let policy = policy(vec![script_block_rule(
        "script-block",
        "pre",
        "facts_contain('flag', ['set'])",
    )]);
    let snapshot = snap(vec![flag_fact()], vec![]);
    let reserved = reserved_set();
    let no_demote = demote(&[]);

    let decision = evaluate(
        &policy,
        Selection::Hook(Phase::Pre),
        &snapshot,
        &reserved,
        &no_demote,
        Box::new(ScriptedEval::allow()),
    )
    .await
    .unwrap();
    assert_eq!(decision.verdict, Verdict::Deny);

    let decision = evaluate(
        &policy,
        Selection::Hook(Phase::Pre),
        &snapshot,
        &reserved,
        &no_demote,
        Box::new(ScriptedEval::deny()),
    )
    .await
    .unwrap();
    assert_eq!(decision.verdict, Verdict::Allow);
    assert!(decision.consequences.is_empty());
}

#[tokio::test]
async fn guard_error_fails_closed_with_diagnostic() {
    let policy = policy(vec![script_block_rule("script-block", "pre", "boom")]);
    let snapshot = snap(vec![flag_fact()], vec![]);
    let decision = evaluate(
        &policy,
        Selection::Hook(Phase::Pre),
        &snapshot,
        &reserved_set(),
        &demote(&[]),
        Box::new(ScriptedEval::broken("evaluator exploded")),
    )
    .await
    .expect("a guard failure is a successful evaluation, not an error");

    // Fail closed: the rule fired as matched, so its own block stands.
    assert_eq!(decision.verdict, Verdict::Deny);
    let raw = &decision.consequences[0];
    assert_eq!(
        raw.payload.get("guard_error").and_then(|v| v.as_str()),
        Some("evaluator exploded")
    );
    assert_eq!(
        raw.payload.get("action_type").and_then(|v| v.as_str()),
        Some("constraint_violation")
    );
    assert!(decision.diagnostics.iter().any(|d| matches!(
        d,
        DecisionDiagnostic::GuardError { error, .. } if error == "evaluator exploded"
    )));
}

#[tokio::test]
async fn builtin_evaluator_convenience_runs_builtin_scripts() {
    let policy = policy(vec![script_block_rule(
        "script-block",
        "pre",
        "facts_contain('flag', ['set'])",
    )]);
    let snapshot = snap(vec![flag_fact()], vec![]);
    let decision = evaluate(
        &policy,
        Selection::Hook(Phase::Pre),
        &snapshot,
        &reserved_set(),
        &demote(&[]),
        builtin_evaluator(),
    )
    .await
    .unwrap();
    assert_eq!(decision.verdict, Verdict::Deny);
}

#[tokio::test]
async fn exact_demotion_only_demotes_named_rule_ids() {
    let policy = policy(vec![
        block_rule("alpha", "pre", "flag"),
        // A different rule whose id merely contains the demoted id as a
        // substring must never be demoted by inference.
        block_rule("alpha-in-src", "pre", "flag"),
    ]);
    let snapshot = snap(vec![flag_fact()], vec![]);
    let reserved = reserved_set();

    let decision = evaluate(
        &policy,
        Selection::Hook(Phase::Pre),
        &snapshot,
        &reserved,
        &demote(&["alpha"]),
        builtin_evaluator(),
    )
    .await
    .unwrap();

    assert_eq!(
        decision.verdict,
        Verdict::Deny,
        "the non-demoted block still blocks"
    );
    let demoted_reason = decision
        .reasons
        .iter()
        .find(|r| r.rule_id.as_ref().is_some_and(|id| id.as_str() == "alpha"))
        .expect("alpha fired");
    assert_eq!(demoted_reason.severity, Severity::Warning);
    assert!(demoted_reason.demoted);
    let kept_reason = decision
        .reasons
        .iter()
        .find(|r| {
            r.rule_id
                .as_ref()
                .is_some_and(|id| id.as_str() == "alpha-in-src")
        })
        .expect("alpha-in-src fired");
    assert_eq!(kept_reason.severity, Severity::Block);
    assert!(!kept_reason.demoted);

    assert!(decision.diagnostics.iter().any(|d| matches!(
        d,
        DecisionDiagnostic::Demoted { rule_id, .. } if rule_id.as_str() == "alpha"
    )));
    assert!(!decision.diagnostics.iter().any(|d| matches!(
        d,
        DecisionDiagnostic::Demoted { rule_id, .. } if rule_id.as_str() == "alpha-in-src"
    )));

    // Raw consequences are never mutated by demotion.
    for consequence in &decision.consequences {
        assert_eq!(
            consequence
                .payload
                .get("action_type")
                .and_then(|v| v.as_str()),
            Some("constraint_violation")
        );
    }
    assert_eq!(
        decision.block_messages(),
        vec!["constraint_violation by alpha-in-src"]
    );
    assert_eq!(
        decision.warning_messages(),
        vec!["constraint_violation by alpha"]
    );

    let undemoted = evaluate(
        &policy,
        Selection::Hook(Phase::Pre),
        &snapshot,
        &reserved,
        &demote(&[]),
        builtin_evaluator(),
    )
    .await
    .unwrap();
    assert!(
        undemoted
            .reasons
            .iter()
            .all(|r| r.severity == Severity::Block && !r.demoted)
    );
}

#[tokio::test]
async fn repeated_evaluations_are_isolated() {
    let policy = policy(vec![
        block_rule("flag-block", "pre", "flag"),
        block_rule("extra-block", "pre", "extra"),
    ]);
    let reserved = reserved_set();
    let no_demote = demote(&[]);
    let snapshot = snap(vec![flag_fact()], vec![]);

    let first = evaluate(
        &policy,
        Selection::Hook(Phase::Pre),
        &snapshot,
        &reserved,
        &no_demote,
        builtin_evaluator(),
    )
    .await
    .unwrap();
    let second = evaluate(
        &policy,
        Selection::Hook(Phase::Pre),
        &snapshot,
        &reserved,
        &no_demote,
        builtin_evaluator(),
    )
    .await
    .unwrap();
    assert_eq!(first.verdict, second.verdict);
    assert_eq!(
        serde_json::to_value(&first.consequences).unwrap(),
        serde_json::to_value(&second.consequences).unwrap()
    );

    // State does not leak between calls: a fact only present in a later
    // snapshot fires only the rule that matches it, and the earlier
    // decision's record is untouched.
    let grown = snap(vec![flag_fact(), fact("f2", "extra", &["set"])], vec![]);
    let third = evaluate(
        &policy,
        Selection::Hook(Phase::Pre),
        &grown,
        &reserved,
        &no_demote,
        builtin_evaluator(),
    )
    .await
    .unwrap();
    assert_eq!(third.block_messages().len(), 2);
    assert_eq!(first.block_messages().len(), 1);
}

#[tokio::test]
async fn deny_is_a_successful_decision_not_an_operational_error() {
    let policy = policy(vec![block_rule("pre-block", "pre", "flag")]);
    let snapshot = snap(vec![flag_fact()], vec![]);
    let decision = evaluate(
        &policy,
        Selection::Hook(Phase::Pre),
        &snapshot,
        &reserved_set(),
        &demote(&[]),
        builtin_evaluator(),
    )
    .await
    .expect("policy Deny is Ok");
    assert_eq!(decision.verdict, Verdict::Deny);

    let reserved_snapshot = snap(vec![], vec![fact("e1", "rule_overridden", &["set"])]);
    let error = evaluate(
        &policy,
        Selection::Hook(Phase::Pre),
        &reserved_snapshot,
        &reserved_set(),
        &demote(&[]),
        builtin_evaluator(),
    )
    .await
    .expect_err("reserved extension input is the typed operational error");
    assert!(matches!(
        error,
        EvaluationError::ReservedExtensionPredicate { .. }
    ));
}

#[tokio::test]
async fn raw_provenance_bindings_and_fact_sources_are_retained() {
    let policy = policy(vec![DiskRule {
        conditions: vec![DiskCondition {
            predicate: "flag".to_string(),
            args: vec!["?v".to_string()],
            script: None,
        }],
        ..block_rule("bind-block", "pre", "flag")
    }]);
    let snapshot = snap(
        vec![sourced_fact("h1", "flag", &["set"], "host-label")],
        vec![],
    );
    let decision = evaluate(
        &policy,
        Selection::Hook(Phase::Pre),
        &snapshot,
        &reserved_set(),
        &demote(&[]),
        builtin_evaluator(),
    )
    .await
    .unwrap();
    assert_eq!(decision.consequences.len(), 1);
    let provenance = serde_json::to_value(&decision.consequences[0].provenance).unwrap();
    assert_eq!(
        provenance,
        serde_json::json!({
            "source": "rule_firing",
            "rule_id": "bind-block",
            "bound_facts": ["h1"],
            "bindings": { "?v": "set" },
            "fact_sources": { "h1": "host-label" },
        })
    );
    let reason = &decision.reasons[0];
    assert_eq!(reason.index, 0);
    assert_eq!(
        reason.rule_id.as_ref().map(|id| id.as_str()),
        Some("bind-block")
    );
    assert_eq!(reason.action_type, "constraint_violation");
    assert_eq!(reason.message, "constraint_violation by bind-block");
    assert_eq!(reason.severity, Severity::Block);
    assert!(!reason.demoted);
}
