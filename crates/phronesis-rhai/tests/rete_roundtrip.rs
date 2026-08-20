//! Layer 2 — full RETE round-trip: a `ReteNetwork` wired with the Rhai
//! evaluator, firing rules whose `when` includes a `__script__` clause.

use phronesis::{Action, Condition, Fact, Provenance, ReteNetwork, Rule};
use phronesis_rhai::RhaiScriptEvaluator;

fn fact(id: &str, predicate: &str, args: &[&str]) -> Fact {
    Fact {
        id: id.to_string(),
        predicate: predicate.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
        timestamp: 0,
        source: None,
    }
}

fn script(expr: &str) -> Condition {
    Condition {
        predicate: "__script__".to_string(),
        args: vec![],
        script: Some(expr.to_string()),
    }
}

fn warn(msg: &str) -> Action {
    Action {
        action_type: "constraint_warning".to_string(),
        params: vec![msg.to_string()],
        ..Default::default()
    }
}

fn network() -> ReteNetwork {
    ReteNetwork::with_script_evaluator(Box::new(RhaiScriptEvaluator::new()))
}

/// A numeric-comparison guard the builtin DSL cannot express: fire only
/// when some `inventory` fact has a quantity (arg 1) of at least 5.
fn low_stock_rule() -> Rule {
    Rule {
        id: "rhai-quantity-guard".to_string(),
        priority: 0,
        conditions: vec![script(
            "facts.some(|f| f.predicate == \"inventory\" && f.args[1].parse_int() >= 5)",
        )],
        actions: vec![warn("inventory quantity >= 5")],
    }
}

#[tokio::test]
async fn script_true_fires_rule() {
    let network = network();
    network
        .assert_fact(fact("f1", "inventory", &["sword", "9"]))
        .await
        .unwrap();
    network.add_rule(low_stock_rule()).await.unwrap();
    network.update_agenda().await.unwrap();

    let consequences = network.fire_all_consequences().unwrap();
    assert_eq!(
        consequences.len(),
        1,
        "rule should fire when the Rhai guard returns true"
    );
    match &consequences[0].provenance {
        Provenance::RuleFiring { rule_id, .. } => {
            assert_eq!(rule_id.as_str(), "rhai-quantity-guard");
        }
        other => panic!("expected RuleFiring provenance, got {other:?}"),
    }
}

#[tokio::test]
async fn script_false_does_not_fire_rule() {
    let network = network();
    // Quantity 2 < 5 → guard is false.
    network
        .assert_fact(fact("f1", "inventory", &["shield", "2"]))
        .await
        .unwrap();
    network.add_rule(low_stock_rule()).await.unwrap();
    network.update_agenda().await.unwrap();

    let consequences = network.fire_all_consequences().unwrap();
    assert!(
        consequences.is_empty(),
        "rule must not fire when the Rhai guard returns false; got {consequences:?}"
    );
}

#[tokio::test]
async fn script_error_blocks_rule_safe_default() {
    let network = network();
    network
        .assert_fact(fact("f1", "inventory", &["potion", "3"]))
        .await
        .unwrap();
    // A script that returns a non-bool is an error → treated as blocked.
    let rule = Rule {
        id: "rhai-bad-return".to_string(),
        priority: 0,
        conditions: vec![script("facts.len()")],
        actions: vec![warn("should never fire")],
    };
    network.add_rule(rule).await.unwrap();
    network.update_agenda().await.unwrap();

    let consequences = network.fire_all_consequences().unwrap();
    assert!(
        consequences.is_empty(),
        "a script error must block the rule, not fire it; got {consequences:?}"
    );
}

#[tokio::test]
async fn script_combined_with_leaf_condition() {
    // A normal predicate condition anchors the rule; the script refines it.
    // A leaf condition indexes into the alpha network, so the rule must be
    // added before the facts it should match are asserted.
    let network = network();
    let rule = Rule {
        id: "commit-needs-signals".to_string(),
        priority: 0,
        conditions: vec![
            Condition {
                predicate: "commit".to_string(),
                args: vec!["?branch".to_string()],
                script: None,
            },
            script(
                "facts.some(|f| f.predicate == \"signals_passed\" && f.args[0].parse_int() >= 3)",
            ),
        ],
        actions: vec![warn("commit with >= 3 passing signals")],
    };
    network.add_rule(rule).await.unwrap();
    network
        .assert_fact(fact("f1", "commit", &["main"]))
        .await
        .unwrap();
    network
        .assert_fact(fact("f2", "signals_passed", &["3"]))
        .await
        .unwrap();
    network.update_agenda().await.unwrap();

    let consequences = network.fire_all_consequences().unwrap();
    assert_eq!(
        consequences.len(),
        1,
        "leaf condition + passing script guard should fire exactly once"
    );
}
