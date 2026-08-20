//! Pure-`__script__` rules — engine behavior when a rule's `when` is
//! entirely script clauses with no leaf condition to anchor an alpha state.
//!
//! Historical bug: `add_rule` skipped script conditions when building the
//! alpha network, then derived `terminal_state_id` from the remaining
//! count. A pure-script rule got `terminal_state_id = None`, no p-state
//! marking, and was disconnected from the agenda. `update_agenda` now
//! walks the production network and treats `real_count == 0` rules as
//! "every cycle is a candidate" — script clauses are evaluated against
//! the current fact base with empty bindings, and an empty-WME activation
//! is added to the agenda when every clause passes.

use phronesis::{Action, Condition, Fact, ReteNetwork, Rule};

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

fn warn_action(msg: &str) -> Action {
    Action {
        action_type: "constraint_warning".to_string(),
        params: vec![msg.to_string()],
        ..Default::default()
    }
}

fn auth_without_tests_rule() -> Rule {
    Rule {
        id: "pure-script-fires".to_string(),
        priority: 0,
        conditions: vec![
            script("facts_count('foo', ['*']) >= 1"),
            script("facts_count('bar', ['*']) == 0"),
        ],
        actions: vec![warn_action("foo present, bar absent")],
    }
}

#[tokio::test]
async fn pure_script_rule_fires_when_all_script_conditions_pass() {
    let network = ReteNetwork::new();
    network
        .assert_fact(fact("f1", "foo", &["a"]))
        .await
        .unwrap();

    network.add_rule(auth_without_tests_rule()).await.unwrap();
    network.update_agenda().await.unwrap();

    let consequences = network.fire_all_consequences().unwrap();
    assert_eq!(
        consequences.len(),
        1,
        "pure-script rule should fire once when both scripts pass"
    );
    match &consequences[0].provenance {
        phronesis::Provenance::RuleFiring { rule_id, .. } => {
            assert_eq!(rule_id.as_str(), "pure-script-fires");
        }
        _ => panic!("expected RuleFiring provenance"),
    }
}

#[tokio::test]
async fn pure_script_rule_does_not_fire_when_a_script_fails() {
    let network = ReteNetwork::new();
    network
        .assert_fact(fact("f1", "foo", &["a"]))
        .await
        .unwrap();
    // Asserting a `bar` fact makes the second script (`== 0`) false.
    network
        .assert_fact(fact("f2", "bar", &["b"]))
        .await
        .unwrap();

    network.add_rule(auth_without_tests_rule()).await.unwrap();
    network.update_agenda().await.unwrap();

    let consequences = network.fire_all_consequences().unwrap();
    assert!(
        consequences.is_empty(),
        "pure-script rule must be blocked when any script clause fails; got: {:?}",
        consequences
    );
}

#[tokio::test]
async fn pure_script_rule_does_not_double_fire() {
    // Two update_agenda passes should produce one consequence total —
    // the fired-activations dedupe must apply to the empty-WME case too.
    let network = ReteNetwork::new();
    network
        .assert_fact(fact("f1", "foo", &["a"]))
        .await
        .unwrap();
    network.add_rule(auth_without_tests_rule()).await.unwrap();

    network.update_agenda().await.unwrap();
    network.update_agenda().await.unwrap();

    let consequences = network.fire_all_consequences().unwrap();
    assert_eq!(
        consequences.len(),
        1,
        "pure-script rule should be deduped across repeated update_agenda calls"
    );
}

#[tokio::test]
async fn rule_with_no_conditions_does_not_fire() {
    // Defensive: a rule with zero conditions of any kind is degenerate.
    // The pure-script branch in update_agenda skips it rather than firing
    // an always-true activation.
    let network = ReteNetwork::new();
    let rule = Rule {
        id: "no-conds".to_string(),
        priority: 0,
        conditions: vec![],
        actions: vec![warn_action("never")],
    };
    network.add_rule(rule).await.unwrap();
    network.update_agenda().await.unwrap();

    let consequences = network.fire_all_consequences().unwrap();
    assert!(
        consequences.is_empty(),
        "rule with zero conditions must not fire"
    );
}

#[tokio::test]
async fn pure_script_rule_blocked_by_first_clause_fails() {
    // The first script clause fails; the rule must not fire and dedupe
    // must not lock out a later cycle where it could succeed.
    let network = ReteNetwork::new();
    // No `foo` fact, so `facts_count('foo') >= 1` is false.
    network.add_rule(auth_without_tests_rule()).await.unwrap();
    network.update_agenda().await.unwrap();
    let consequences = network.fire_all_consequences().unwrap();
    assert!(consequences.is_empty());

    // Add the foo fact and rerun; the rule should now fire.
    network
        .assert_fact(fact("f1", "foo", &["a"]))
        .await
        .unwrap();
    network.update_agenda().await.unwrap();
    let consequences = network.fire_all_consequences().unwrap();
    assert_eq!(
        consequences.len(),
        1,
        "rule should fire on a later cycle once its scripts pass"
    );
}

#[tokio::test]
async fn single_script_pure_rule_fires() {
    // The SPEC's build-staleness shape: one __script__ clause, no leaf.
    let network = ReteNetwork::new();
    network
        .assert_fact(fact("f1", "journey_since_ge", &["build", "8"]))
        .await
        .unwrap();
    let rule = Rule {
        id: "build-staleness".to_string(),
        priority: 0,
        conditions: vec![script(
            "facts_count('journey_since_ge', ['build','8']) >= 1",
        )],
        actions: vec![warn_action("8+ tool calls since the last build")],
    };
    network.add_rule(rule).await.unwrap();
    network.update_agenda().await.unwrap();

    let consequences = network.fire_all_consequences().unwrap();
    assert_eq!(
        consequences.len(),
        1,
        "single-clause pure-script rule should reach the agenda"
    );
}
