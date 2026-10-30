use std::collections::{BTreeSet, HashSet};

use phr::{Fact, Provenance};
use phronesis_governance::decision::Verdict;
use phronesis_governance::evaluation::{FactSnapshot, builtin_evaluator, evaluate};
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
        }
    ]
}"#;

fn fact(id: &str, predicate: &str, arg: &str) -> Fact {
    Fact {
        id: id.to_string(),
        predicate: predicate.to_string(),
        args: vec![arg.to_string()],
        timestamp: 0,
        source: Some("packaged-consumer".to_string()),
    }
}

fn snapshot(host: Vec<Fact>) -> FactSnapshot {
    FactSnapshot {
        host,
        extensions: vec![],
    }
}

fn reserved() -> HashSet<String> {
    ["new_content_contains", "file_extension_is"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let parsed = parse(POLICY, "packaged-consumer").expect("policy must parse");
    let policy = CompiledPolicy::compile(parsed.file.rules).expect("policy must compile");
    let reserved = reserved();
    let no_demote = BTreeSet::new();

    let deny = evaluate(
        &policy,
        Selection::Hook(Phase::Pre),
        &snapshot(vec![fact("f1", "new_content_contains", ".unwrap(")]),
        &reserved,
        &no_demote,
        builtin_evaluator(),
    )
    .await
    .expect("deny scenario must evaluate");
    println!("deny verdict: {:?}", deny.verdict);
    println!("deny block: {}", deny.block_messages().join("; "));
    assert_eq!(deny.verdict, Verdict::Deny);
    for (reason, consequence) in deny.iter_reasons() {
        println!(
            "deny reason rule={:?} severity={:?}",
            reason.rule_id, reason.severity
        );
        if let Provenance::RuleFiring { bound_facts, .. } = &consequence.provenance {
            println!("deny provenance bound={bound_facts:?}");
        }
    }

    let allow = evaluate(
        &policy,
        Selection::Hook(Phase::Pre),
        &snapshot(vec![fact("f2", "file_extension_is", "rs")]),
        &reserved,
        &no_demote,
        builtin_evaluator(),
    )
    .await
    .expect("allow scenario must evaluate");
    println!("allow verdict: {:?}", allow.verdict);
    assert_eq!(allow.verdict, Verdict::Allow);

    let warn = evaluate(
        &policy,
        Selection::Hook(Phase::Post),
        &snapshot(vec![fact("f3", "new_content_contains", "TODO")]),
        &reserved,
        &no_demote,
        builtin_evaluator(),
    )
    .await
    .expect("warn scenario must evaluate");
    println!("warn verdict: {:?}", warn.verdict);
    println!("warn warning: {}", warn.warning_messages().join("; "));
    assert_eq!(warn.verdict, Verdict::Warn);

    let demoted = {
        let mut demote = BTreeSet::new();
        demote.insert(phr::RuleId::new("no-unwrap".to_string()));
        evaluate(
            &policy,
            Selection::Hook(Phase::Pre),
            &snapshot(vec![fact("f1", "new_content_contains", ".unwrap(")]),
            &reserved,
            &demote,
            builtin_evaluator(),
        )
        .await
        .expect("demote scenario must evaluate")
    };
    println!("demoted verdict: {:?}", demoted.verdict);
    assert_eq!(demoted.verdict, Verdict::Warn);

    let again = evaluate(
        &policy,
        Selection::Hook(Phase::Pre),
        &snapshot(vec![fact("f2", "file_extension_is", "rs")]),
        &reserved,
        &no_demote,
        builtin_evaluator(),
    )
    .await
    .expect("isolation scenario must evaluate");
    println!("isolated verdict: {:?}", again.verdict);
    assert_eq!(again.verdict, Verdict::Allow);

    println!("governance-consumer: all packaged-crate checks passed");
}