//! Performance smoke test — runs with `cargo test`, not `cargo bench`.
//!
//! Asserts the RETE hot path (build network + assert facts + fire
//! consequences) completes within a generous time budget. Not a
//! benchmark — no statistical analysis, no warm-up. Purpose is to
//! catch catastrophic regressions (10x slowdowns, accidental O(n^2))
//! on every build, cheaply.
//!
//! The criterion benchmarks in `benches/rete_hot_path.rs` provide
//! proper statistical measurement; this test provides the always-on
//! guardrail.

use phronesis::{Action, Condition, Fact, ReteNetwork, Rule};
use std::time::Instant;

fn build_rules() -> Vec<Rule> {
    let mut rules = Vec::new();
    for i in 0..10 {
        rules.push(Rule {
            id: format!("single-{i}"),
            priority: 1,
            conditions: vec![Condition {
                predicate: format!("pred_a_{}", i % 5),
                args: vec!["?x".to_string()],
                script: None,
            }],
            actions: vec![Action {
                action_type: "constraint_violation".to_string(),
                params: vec![format!("hit {i}")],
            }],
        });
    }
    for i in 0..10 {
        rules.push(Rule {
            id: format!("two-{i}"),
            priority: 1,
            conditions: vec![
                Condition {
                    predicate: format!("pred_b_{}", i % 5),
                    args: vec!["?file".to_string()],
                    script: None,
                },
                Condition {
                    predicate: format!("pred_c_{}", i % 5),
                    args: vec!["?file".to_string(), "?detail".to_string()],
                    script: None,
                },
            ],
            actions: vec![Action {
                action_type: "constraint_violation".to_string(),
                params: vec![format!("hit two {i}")],
            }],
        });
    }
    rules
}

fn build_facts(n: usize) -> Vec<Fact> {
    let preds = ["pred_a_", "pred_b_", "pred_c_"];
    (0..n)
        .map(|i| {
            let pred = format!("{}{}", preds[i % preds.len()], i % 5);
            let args = if pred.starts_with("pred_a") || pred.starts_with("pred_b") {
                vec![format!("crates/src/mod_{}.rs", i % 7)]
            } else {
                vec![
                    format!("crates/src/mod_{}.rs", i % 7),
                    format!("detail-{i}"),
                ]
            };
            Fact {
                id: format!("fact-{i}"),
                predicate: pred,
                args,
                timestamp: 0,
            }
        })
        .collect()
}

/// The hook hot path: build network, register 20 rules, assert 100
/// facts, fire consequences. Must complete in under 10ms. The real
/// criterion bench measures ~500µs; 10ms is a 20x margin that avoids
/// flaky failures on slow CI runners while still catching O(n^2)
/// regressions.
#[tokio::test]
async fn hot_path_completes_within_budget() {
    let rules = build_rules();
    let facts = build_facts(100);

    let start = Instant::now();

    let net = ReteNetwork::new();
    for r in &rules {
        net.add_rule(r.clone()).await.expect("add_rule");
    }
    for f in &facts {
        net.assert_fact(f.clone()).await.expect("assert_fact");
    }
    let consequences = net.fire_all_consequences().expect("fire");
    drop(consequences);

    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() < 10,
        "hot path took {}ms — expected <10ms (criterion baseline ~0.5ms). \
         Possible performance regression.",
        elapsed.as_millis()
    );
}

/// Single fact assertion into a populated network. Must complete in
/// under 2ms (criterion baseline ~100µs at 200 preloaded facts).
#[tokio::test]
async fn single_assert_within_budget() {
    let rules = build_rules();
    let preload = build_facts(200);
    let probe = Fact {
        id: "probe".to_string(),
        predicate: "pred_b_0".to_string(),
        args: vec!["crates/probe/src/lib.rs".to_string()],
        timestamp: 0,
    };

    let net = ReteNetwork::new();
    for r in &rules {
        net.add_rule(r.clone()).await.expect("add_rule");
    }
    for f in &preload {
        net.assert_fact(f.clone()).await.expect("assert_fact");
    }

    let start = Instant::now();
    net.assert_fact(probe).await.expect("assert_fact");
    let elapsed = start.elapsed();

    assert!(
        elapsed.as_millis() < 2,
        "single assert took {}ms — expected <2ms (criterion baseline ~0.1ms). \
         Possible performance regression.",
        elapsed.as_millis()
    );
}
