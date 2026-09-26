//! Profile a long-lived session in which guard-false activations accumulate.
//!
//! Each round asserts one trigger fact whose `__script__` guard is false,
//! fires, and snapshots the agenda — the shape of an MCP `serve` session
//! driving `assert_fact` / `fire_rules` / `get_agenda`. Guards are judged
//! at fire time and a guard-false activation stays latched, so this measures
//! what re-judging the deferred set costs as it grows.
//!
//! Run: `cargo run --release --example profile_guard_deferral -p phronesis`

use std::time::Instant;

use phronesis::{Action, Condition, Fact, ReteNetwork, Rule};

fn rule() -> Rule {
    Rule {
        id: "waits-for-b".to_string(),
        priority: 0,
        conditions: vec![
            Condition {
                predicate: "a".to_string(),
                args: vec!["?x".to_string()],
                script: None,
            },
            Condition {
                predicate: "__script__".to_string(),
                args: vec![],
                script: Some("facts_contain('b', ['?x'])".to_string()),
            },
        ],
        actions: vec![Action {
            action_type: "constraint_warning".to_string(),
            params: vec!["?x".to_string()],
            ..Default::default()
        }],
    }
}

fn main() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime");
    for rounds in [250usize, 1000, 2000, 4000] {
        let net = ReteNetwork::new();
        runtime.block_on(net.add_rule(rule())).expect("rule");
        let start = Instant::now();
        for i in 0..rounds {
            let fact = Fact {
                id: format!("a{i}"),
                predicate: "a".to_string(),
                args: vec![i.to_string()],
                timestamp: 0,
                source: None,
            };
            runtime.block_on(net.assert_fact(fact)).expect("assert");
            net.fire_all_consequences().expect("fire");
            net.agenda_snapshot().expect("snapshot");
        }
        println!(
            "rounds={rounds:5}, total={:?}, per-round={:?}",
            start.elapsed(),
            start.elapsed() / rounds as u32
        );
    }
}
