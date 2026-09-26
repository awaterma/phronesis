//! `__script__` guard semantics: guards are judged against the final
//! working memory at fire time, and a guard that cannot be evaluated fails
//! closed.
//!
//! Historical bugs:
//! - A guard was evaluated when the triggering WME arrived and the
//!   activation latched (refraction), so a guard reading facts asserted
//!   *later* was judged too early and the verdict depended on assertion
//!   order.
//! - A guard whose evaluation errored was dropped with a `warn!` the hook
//!   never printed ("treating as blocked"), so a block rule silently
//!   allowed.

use phronesis::{Action, Condition, Consequence, Fact, ReteNetwork, Rule};

fn fact(id: &str, predicate: &str, args: &[&str]) -> Fact {
    Fact {
        id: id.to_string(),
        predicate: predicate.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
        timestamp: 0,
        source: None,
    }
}

fn cond(predicate: &str, args: &[&str]) -> Condition {
    Condition {
        predicate: predicate.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
        script: None,
    }
}

fn script(expr: &str) -> Condition {
    Condition {
        predicate: "__script__".to_string(),
        args: vec![],
        script: Some(expr.to_string()),
    }
}

fn block(msg: &str) -> Action {
    Action {
        action_type: "constraint_violation".to_string(),
        params: vec![msg.to_string()],
        ..Default::default()
    }
}

fn rule(id: &str, conditions: Vec<Condition>) -> Rule {
    Rule {
        id: id.to_string(),
        priority: 0,
        conditions,
        actions: vec![block(&format!("{id} fired"))],
    }
}

/// Every permutation of `items` (small inputs only).
fn permutations<T: Clone>(items: &[T]) -> Vec<Vec<T>> {
    if items.len() <= 1 {
        return vec![items.to_vec()];
    }
    let mut out = Vec::new();
    for i in 0..items.len() {
        let mut rest = items.to_vec();
        let head = rest.remove(i);
        for mut tail in permutations(&rest) {
            tail.insert(0, head.clone());
            out.push(tail);
        }
    }
    out
}

/// Rules first, facts in `order`, then the hook's drive sequence
/// (`update_agenda` + `fire_all_consequences`). Returns the fired rule ids,
/// sorted.
async fn fired_rules(rules: &[Rule], order: &[Fact]) -> Vec<String> {
    let net = ReteNetwork::new();
    for r in rules {
        net.add_rule(r.clone()).await.unwrap();
    }
    for f in order {
        net.assert_fact(f.clone()).await.unwrap();
    }
    net.update_agenda().await.unwrap();
    let mut ids: Vec<String> = net
        .fire_all_consequences()
        .unwrap()
        .iter()
        .map(|c| c.predicate.clone())
        .collect();
    ids.sort();
    ids
}

async fn assert_order_independent(rules: &[Rule], facts: &[Fact], expected: &[&str]) {
    for order in permutations(facts) {
        let got = fired_rules(rules, &order).await;
        let names: Vec<&str> = order.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(
            got, expected,
            "verdict must not depend on assertion order; order {names:?}"
        );
    }
}

fn signal_pass_rule() -> Rule {
    rule(
        "commit-needs-two-passes",
        vec![
            cond("bash_command_matches", &["git commit"]),
            script("facts_count('signal_pass', ['*']) <= 1"),
        ],
    )
}

#[tokio::test]
async fn single_condition_guard_sees_facts_asserted_after_the_trigger() {
    let facts = [
        fact("cmd", "bash_command_matches", &["git commit"]),
        fact("p1", "signal_pass", &["a"]),
        fact("p2", "signal_pass", &["b"]),
    ];
    assert_order_independent(&[signal_pass_rule()], &facts, &[]).await;
    // Control: with a single pass the guard holds in every order.
    assert_order_independent(
        &[signal_pass_rule()],
        &facts[..2],
        &["commit-needs-two-passes"],
    )
    .await;
}

#[tokio::test]
async fn multi_condition_guard_is_order_independent() {
    let r = rule(
        "joined-guard",
        vec![
            cond("a", &["?x"]),
            cond("b", &["?x"]),
            script("!facts_contain('c', ['?x'])"),
        ],
    );
    let facts = [
        fact("a1", "a", &["1"]),
        fact("b1", "b", &["1"]),
        fact("c1", "c", &["1"]),
    ];
    assert_order_independent(std::slice::from_ref(&r), &facts, &[]).await;
    assert_order_independent(&[r], &facts[..2], &["joined-guard"]).await;
}

#[tokio::test]
async fn pure_script_rule_is_order_independent() {
    let r = rule(
        "pure",
        vec![
            script("facts_count('foo', ['*']) >= 1"),
            script("facts_count('bar', ['*']) == 0"),
        ],
    );
    let facts = [fact("f1", "foo", &["a"]), fact("b1", "bar", &["b"])];
    assert_order_independent(std::slice::from_ref(&r), &facts, &[]).await;
    assert_order_independent(&[r], &facts[..1], &["pure"]).await;
}

#[tokio::test]
async fn guard_that_becomes_true_later_fires_on_a_later_cycle() {
    // Serve mode: fire, assert more, fire again. A guard that was false at
    // the first fire must still be judged again, without an explicit
    // `update_agenda`, once the fact it waits for arrives.
    let net = ReteNetwork::new();
    net.add_rule(rule(
        "waits-for-b",
        vec![cond("a", &["?x"]), script("facts_contain('b', ['?x'])")],
    ))
    .await
    .unwrap();
    net.assert_fact(fact("a1", "a", &["1"])).await.unwrap();
    assert!(net.fire_all_consequences().unwrap().is_empty());
    assert!(
        net.agenda_snapshot().unwrap().is_empty(),
        "a guard that is false right now is not a pending activation"
    );

    net.assert_fact(fact("b1", "b", &["1"])).await.unwrap();
    assert_eq!(net.agenda_snapshot().unwrap().len(), 1);
    let fired = net.fire_all_consequences().unwrap();
    assert_eq!(fired.len(), 1, "guard is now true: {fired:?}");
    // Refraction still holds: the activation is consumed.
    assert!(net.fire_all_consequences().unwrap().is_empty());
}

#[tokio::test]
async fn retracting_the_trigger_drops_a_deferred_activation() {
    let net = ReteNetwork::new();
    net.add_rule(rule(
        "waits-for-b",
        vec![cond("a", &["?x"]), script("facts_contain('b', ['*'])")],
    ))
    .await
    .unwrap();
    net.assert_fact(fact("a1", "a", &["1"])).await.unwrap();
    assert!(net.fire_all_consequences().unwrap().is_empty());
    net.retract_fact("a1").await.unwrap();
    net.assert_fact(fact("b1", "b", &["1"])).await.unwrap();
    assert!(
        net.fire_all_consequences().unwrap().is_empty(),
        "an activation whose trigger was retracted must never fire"
    );
}

fn guard_error(c: &Consequence) -> Option<&str> {
    c.payload.get("guard_error").and_then(|v| v.as_str())
}

#[tokio::test]
async fn guard_evaluation_error_fails_closed_with_annotation() {
    // `bogus_fn()` is not in the builtin DSL: evaluation errors.
    let net = ReteNetwork::new();
    net.add_rule(rule(
        "broken-guard",
        vec![cond("a", &["?x"]), script("bogus_fn()")],
    ))
    .await
    .unwrap();
    net.assert_fact(fact("a1", "a", &["1"])).await.unwrap();
    net.update_agenda().await.unwrap();

    let fired = net.fire_all_consequences().unwrap();
    assert_eq!(
        fired.len(),
        1,
        "a broken guard must not silently pass or drop"
    );
    let c = &fired[0];
    assert_eq!(c.predicate, "broken-guard");
    assert_eq!(c.payload["action_type"], "constraint_violation");
    let error = guard_error(c).expect("guard_error annotation");
    assert!(error.contains("bogus_fn"), "{error}");
    let message = c.payload["message"].as_str().unwrap();
    assert!(message.contains("broken-guard fired"), "{message}");
    assert!(message.contains("broken-guard"), "{message}");
    assert!(message.contains("bogus_fn"), "{message}");
}

#[tokio::test]
async fn pure_script_guard_error_fails_closed() {
    let net = ReteNetwork::new();
    net.add_rule(rule("broken-pure", vec![script("not a guard")]))
        .await
        .unwrap();
    net.update_agenda().await.unwrap();
    let fired = net.fire_all_consequences().unwrap();
    assert_eq!(fired.len(), 1);
    assert!(guard_error(&fired[0]).is_some());
}

#[tokio::test]
async fn passing_guard_carries_no_error_annotation() {
    let net = ReteNetwork::new();
    net.add_rule(signal_pass_rule()).await.unwrap();
    net.assert_fact(fact("cmd", "bash_command_matches", &["git commit"]))
        .await
        .unwrap();
    let fired = net.fire_all_consequences().unwrap();
    assert_eq!(fired.len(), 1);
    assert!(guard_error(&fired[0]).is_none());
    assert_eq!(fired[0].payload["message"], "commit-needs-two-passes fired");
}

#[tokio::test]
async fn legacy_action_path_applies_the_same_guard_semantics() {
    let net = ReteNetwork::new();
    net.add_rule(signal_pass_rule()).await.unwrap();
    net.assert_fact(fact("cmd", "bash_command_matches", &["git commit"]))
        .await
        .unwrap();
    net.assert_fact(fact("p1", "signal_pass", &["a"]))
        .await
        .unwrap();
    net.assert_fact(fact("p2", "signal_pass", &["b"]))
        .await
        .unwrap();
    net.update_agenda().await.unwrap();
    assert!(net.execute_all_agenda_items().unwrap().is_empty());
}
