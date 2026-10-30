//! End-to-end consumer walkthrough of the governance core:
//! strict policy parse, compiled-policy evaluation with explicit
//! facts/reservations/evaluator, allow+deny+warn verdicts, provenance,
//! and per-call isolation. Run with:
//!   cargo run -p phronesis-governance --example consumer_flow

use std::collections::{BTreeSet, HashSet};

use phr::{Fact, Provenance};
use phronesis_governance::decision::Verdict;
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

fn fact(id: &str, predicate: &str, arg: &str) -> Fact {
    Fact {
        id: id.to_string(),
        predicate: predicate.to_string(),
        args: vec![arg.to_string()],
        timestamp: 0,
        source: Some("consumer-flow".to_string()),
    }
}

fn unwrap_fact() -> Fact {
    fact("fact-unwrap", "new_content_contains", ".unwrap(")
}

fn todo_fact() -> Fact {
    fact("fact-todo", "new_content_contains", "TODO")
}

fn rust_file_fact() -> Fact {
    fact("fact-rust", "file_extension_is", "rs")
}

fn reserved_set() -> HashSet<String> {
    ["new_content_contains", "file_extension_is"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

fn snapshot(host: Vec<Fact>) -> FactSnapshot {
    FactSnapshot {
        host,
        extensions: vec![],
    }
}

fn report(label: &str, decision: &phronesis_governance::decision::Decision) {
    println!("--- {label}: verdict = {:?}", decision.verdict);
    for message in decision.block_messages() {
        println!("    block: {message}");
    }
    for message in decision.warning_messages() {
        println!("    warn:  {message}");
    }
    for (reason, consequence) in decision.iter_reasons() {
        println!(
            "    reason[{}] action_type={} severity={:?} demoted={}",
            reason.index, reason.action_type, reason.severity, reason.demoted
        );
        if let Some(rule_id) = &reason.rule_id {
            println!("    provenance rule_id = {rule_id}");
        }
        if let Provenance::RuleFiring { bound_facts, .. } = &consequence.provenance {
            println!("    provenance bound_facts = {bound_facts:?}");
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let parsed = parse(POLICY, "example-policy").expect("example policy must parse");
    let policy = CompiledPolicy::compile(parsed.file.rules).expect("example policy must compile");
    println!(
        "compiled policy: {} rules, {} diagnostics",
        policy.rules().len(),
        policy.diagnostics().len()
    );
    let reserved = reserved_set();
    let no_demote = BTreeSet::new();

    println!("\n[deny] pre-hook sees unwrap() in new content");
    let deny = evaluate(
        &policy,
        Selection::Hook(Phase::Pre),
        &snapshot(vec![unwrap_fact(), rust_file_fact()]),
        &reserved,
        &no_demote,
        builtin_evaluator(),
    )
    .await
    .expect("deny scenario must evaluate");
    report("deny scenario", &deny);
    assert_eq!(deny.verdict, Verdict::Deny);

    println!("\n[allow] pre-hook sees only a rust file change");
    let allow = evaluate(
        &policy,
        Selection::Hook(Phase::Pre),
        &snapshot(vec![rust_file_fact()]),
        &reserved,
        &no_demote,
        builtin_evaluator(),
    )
    .await
    .expect("allow scenario must evaluate");
    report("allow scenario", &allow);
    assert_eq!(allow.verdict, Verdict::Allow);

    println!("\n[warn] post-hook sees a leftover TODO");
    let warn = evaluate(
        &policy,
        Selection::Hook(Phase::Post),
        &snapshot(vec![todo_fact(), rust_file_fact()]),
        &reserved,
        &no_demote,
        builtin_evaluator(),
    )
    .await
    .expect("warn scenario must evaluate");
    report("warn scenario", &warn);
    assert_eq!(warn.verdict, Verdict::Warn);

    println!("\n[demote] caller demotes no-unwrap for this decision only");
    let mut demote = BTreeSet::new();
    demote.insert(phr::RuleId::new("no-unwrap".to_string()));
    let demoted = evaluate(
        &policy,
        Selection::Hook(Phase::Pre),
        &snapshot(vec![unwrap_fact()]),
        &reserved,
        &demote,
        builtin_evaluator(),
    )
    .await
    .expect("demote scenario must evaluate");
    report("demote scenario", &demoted);
    assert_eq!(demoted.verdict, Verdict::Warn);
    assert!(
        demoted
            .warning_messages()
            .iter()
            .any(|m| m.contains("unwrap"))
    );

    println!("\n[isolation] re-running the allow scenario after the deny scenario");
    let again = evaluate(
        &policy,
        Selection::Hook(Phase::Pre),
        &snapshot(vec![rust_file_fact()]),
        &reserved,
        &no_demote,
        builtin_evaluator(),
    )
    .await
    .expect("isolation scenario must evaluate");
    report("isolated re-run", &again);
    assert_eq!(again.verdict, Verdict::Allow);

    println!("\n[isolation] extension fact may not assert a reserved predicate");
    let rejected = evaluate(
        &policy,
        Selection::Hook(Phase::Pre),
        &FactSnapshot {
            host: vec![],
            extensions: vec![fact("ext-1", "new_content_contains", ".unwrap(")],
        },
        &reserved,
        &no_demote,
        builtin_evaluator(),
    )
    .await;
    match &rejected {
        Err(EvaluationError::ReservedExtensionPredicate { fact_id, predicate }) => {
            println!("    rejected snapshot: fact {fact_id} asserts reserved {predicate}");
        }
        other => panic!("expected ReservedExtensionPredicate, got {other:?}"),
    }
    let after_rejection = evaluate(
        &policy,
        Selection::Hook(Phase::Pre),
        &snapshot(vec![rust_file_fact()]),
        &reserved,
        &no_demote,
        builtin_evaluator(),
    )
    .await
    .expect("evaluation after rejected snapshot must still work");
    report("after rejected snapshot", &after_rejection);
    assert_eq!(after_rejection.verdict, Verdict::Allow);

    println!("\n[isolation] duplicate fact ids are rejected before anything asserts");
    let duplicate = evaluate(
        &policy,
        Selection::Hook(Phase::Pre),
        &snapshot(vec![rust_file_fact(), rust_file_fact()]),
        &reserved,
        &no_demote,
        builtin_evaluator(),
    )
    .await;
    match &duplicate {
        Err(EvaluationError::DuplicateFactId { fact_id }) => {
            println!("    rejected snapshot: duplicate fact id {fact_id}");
        }
        other => panic!("expected DuplicateFactId, got {other:?}"),
    }

    println!("\nall consumer-flow checks passed");
}
