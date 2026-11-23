//! `ReteNetwork::execute_agenda_items_for_fact` — fact-scoped agenda drive
//! for embedding hosts: drain only the pending activations whose `wme_list`
//! includes a given fact id, in normal salience/FIFO order, leaving every
//! other pending item on the agenda exactly as it was.
#![cfg(feature = "embedding-host")]

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

fn action(id: &str) -> Action {
    Action {
        action_type: "constraint_violation".to_string(),
        params: vec![format!("{id} fired")],
        ..Default::default()
    }
}

fn rule(id: &str, priority: i32, conditions: Vec<Condition>) -> Rule {
    Rule {
        id: id.to_string(),
        priority,
        conditions,
        actions: vec![action(id)],
    }
}

/// A higher-salience rule on an unrelated fact sits ahead of the target
/// activation in normal pop order; the fact-scoped drain must still reach
/// and fire the target without touching the unrelated one.
#[tokio::test]
async fn selects_target_fact_behind_higher_salience_unrelated_activation() {
    let net = ReteNetwork::new();
    net.add_rule(rule("high", 100, vec![cond("p", &["?x"])]))
        .await
        .unwrap();
    net.add_rule(rule("target", 1, vec![cond("q", &["?x"])]))
        .await
        .unwrap();

    net.assert_fact(fact("f-high", "p", &["a"])).await.unwrap();
    net.assert_fact(fact("f-target", "q", &["b"]))
        .await
        .unwrap();

    let executed = net.execute_agenda_items_for_fact("f-target").unwrap();
    assert_eq!(executed.len(), 1);
    assert_eq!(executed[0].item.rule.id, "target");
    assert_eq!(executed[0].actions[0].params[0], "target fired");

    // The unrelated higher-salience activation is still pending.
    let remaining = net.agenda_snapshot().unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].rule.id, "high");
}

/// Unselected items must come back onto the agenda with their `seq`
/// (insertion order) and refraction state exactly as popped — no
/// retract/reassert trick that would reset their position.
#[tokio::test]
async fn unselected_items_keep_their_original_seq() {
    let net = ReteNetwork::new();
    net.add_rule(rule("a", 1, vec![cond("pa", &["?x"])]))
        .await
        .unwrap();
    net.add_rule(rule("b", 1, vec![cond("pb", &["?x"])]))
        .await
        .unwrap();
    net.add_rule(rule("target", 1, vec![cond("q", &["?x"])]))
        .await
        .unwrap();

    net.assert_fact(fact("f-a", "pa", &["a"])).await.unwrap();
    net.assert_fact(fact("f-b", "pb", &["b"])).await.unwrap();
    net.assert_fact(fact("f-target", "q", &["c"]))
        .await
        .unwrap();

    let before = net.agenda_snapshot().unwrap();
    let seq_before: std::collections::HashMap<String, u64> = before
        .iter()
        .map(|item| (item.rule.id.clone(), item.seq))
        .collect();

    net.execute_agenda_items_for_fact("f-target").unwrap();

    let after = net.agenda_snapshot().unwrap();
    assert_eq!(after.len(), 2);
    for item in &after {
        assert_eq!(
            item.seq, seq_before[&item.rule.id],
            "rule '{}' seq changed by a drain that did not select it",
            item.rule.id
        );
    }
}

/// An activation already consumed by a prior full drain must not be
/// re-fired by a later fact-scoped drain for a different fact: it is
/// simply gone from the agenda, same as any other consumed activation.
#[tokio::test]
async fn already_fired_unrelated_activation_is_not_rediscovered() {
    let net = ReteNetwork::new();
    net.add_rule(rule("other", 1, vec![cond("p", &["?x"])]))
        .await
        .unwrap();
    net.add_rule(rule("target", 1, vec![cond("q", &["?x"])]))
        .await
        .unwrap();

    net.assert_fact(fact("f-other", "p", &["a"])).await.unwrap();
    net.assert_fact(fact("f-target", "q", &["b"]))
        .await
        .unwrap();

    let consequences = net.fire_all_consequences().unwrap();
    assert_eq!(
        consequences.len(),
        2,
        "both activations fire once, up front"
    );

    // Nothing left pending, so a fact-scoped drain for either fact finds
    // nothing to fire — it does not resurrect what already fired.
    assert!(
        net.execute_agenda_items_for_fact("f-other")
            .unwrap()
            .is_empty()
    );
    assert!(
        net.execute_agenda_items_for_fact("f-target")
            .unwrap()
            .is_empty()
    );
}

/// A multi-condition (join) activation is selected by either of its
/// bound fact ids.
#[tokio::test]
async fn selects_a_join_activation_by_either_bound_fact_id() {
    let net = ReteNetwork::new();
    net.add_rule(rule(
        "join",
        1,
        vec![cond("p", &["?x"]), cond("q", &["?x"])],
    ))
    .await
    .unwrap();

    net.assert_fact(fact("f-p", "p", &["v"])).await.unwrap();
    net.assert_fact(fact("f-q", "q", &["v"])).await.unwrap();

    let executed = net.execute_agenda_items_for_fact("f-q").unwrap();
    assert_eq!(executed.len(), 1);
    assert_eq!(executed[0].item.rule.id, "join");
    let bound_ids: Vec<&str> = executed[0]
        .item
        .wme_list
        .iter()
        .map(|w| w.id.as_str())
        .collect();
    assert!(bound_ids.contains(&"f-p"));
    assert!(bound_ids.contains(&"f-q"));
}

/// A guard that evaluates `true` fires normally.
#[tokio::test]
async fn guard_true_fires_the_activation() {
    let net = ReteNetwork::new();
    net.add_rule(rule(
        "guarded",
        1,
        vec![cond("p", &["?x"]), script("facts_contain('p', ['?x'])")],
    ))
    .await
    .unwrap();

    net.assert_fact(fact("f-p", "p", &["v"])).await.unwrap();

    let executed = net.execute_agenda_items_for_fact("f-p").unwrap();
    assert_eq!(executed.len(), 1);
    assert_eq!(executed[0].item.rule.id, "guarded");
}

/// A guard that evaluates `false` drops the activation (nothing executed)
/// and clears its refraction key, exactly as the existing single-step and
/// drain-all paths already do — no new error policy is introduced.
#[tokio::test]
async fn guard_false_drops_the_activation_without_executing_it() {
    let net = ReteNetwork::new();
    net.add_rule(rule(
        "guarded",
        1,
        vec![cond("p", &["?x"]), script("facts_contain('never', ['*'])")],
    ))
    .await
    .unwrap();

    net.assert_fact(fact("f-p", "p", &["v"])).await.unwrap();

    let executed = net.execute_agenda_items_for_fact("f-p").unwrap();
    assert!(executed.is_empty(), "a false guard must not fire");
    assert!(
        net.agenda_snapshot().unwrap().is_empty(),
        "the dropped activation is not left pending either"
    );
}

/// A guard that cannot be evaluated fails closed: the rule's own actions
/// still fire, same as `execute_next_agenda_item`/`execute_all_agenda_items`.
#[tokio::test]
async fn guard_error_fails_closed_and_still_fires() {
    let net = ReteNetwork::new();
    net.add_rule(rule(
        "guarded",
        1,
        vec![cond("p", &["?x"]), script("bogus_fn()")],
    ))
    .await
    .unwrap();

    net.assert_fact(fact("f-p", "p", &["v"])).await.unwrap();

    let executed = net.execute_agenda_items_for_fact("f-p").unwrap();
    assert_eq!(executed.len(), 1, "a guard error fails closed, i.e. fires");
    assert_eq!(executed[0].item.rule.id, "guarded");
}

/// A fact id with no pending activation at all returns an empty result and
/// leaves the agenda untouched.
#[tokio::test]
async fn no_matching_activation_returns_empty_and_leaves_agenda_untouched() {
    let net = ReteNetwork::new();
    net.add_rule(rule("a", 1, vec![cond("p", &["?x"])]))
        .await
        .unwrap();
    net.assert_fact(fact("f-a", "p", &["v"])).await.unwrap();

    let before = net.agenda_snapshot().unwrap();
    let executed = net.execute_agenda_items_for_fact("no-such-fact").unwrap();
    assert!(executed.is_empty());
    let after = net.agenda_snapshot().unwrap();
    assert_eq!(before.len(), after.len());
    assert_eq!(before[0].seq, after[0].seq);
}

/// While a fast `FactBatch` is open, the agenda-insertion work a new
/// assertion would trigger is deferred to the batch's close — this call
/// does not itself run that catch-up scan, so a fact asserted mid-batch is
/// not yet selectable. After the batch closes, it is.
#[tokio::test]
async fn open_fast_batch_defers_visibility_of_a_newly_asserted_facts_match() {
    let net = ReteNetwork::new();
    net.add_rule(rule("target", 1, vec![cond("q", &["?x"])]))
        .await
        .unwrap();

    let batch = net.begin_batch().unwrap();
    net.assert_fact(fact("f-target", "q", &["v"]))
        .await
        .unwrap();

    assert!(
        net.execute_agenda_items_for_fact("f-target")
            .unwrap()
            .is_empty(),
        "mid-batch, the match is not yet on the agenda"
    );

    batch.close().unwrap();

    let executed = net.execute_agenda_items_for_fact("f-target").unwrap();
    assert_eq!(executed.len(), 1, "after close, the match is selectable");
}

#[tokio::test]
async fn multiple_selected_activations_preserve_order_bindings_and_refraction() {
    let net = ReteNetwork::new();
    for (id, priority, predicate) in [
        ("low-first", 1, "p"),
        ("high", 10, "p"),
        ("low-last", 1, "p"),
        ("unrelated", 100, "q"),
    ] {
        let mut r = rule(id, priority, vec![cond(predicate, &["?x"])]);
        r.actions[0].params = vec![id.into(), "?x".into()];
        net.add_rule(r).await.expect("rule");
    }
    net.assert_fact(fact("target", "p", &["bound-value"]))
        .await
        .expect("target");
    net.assert_fact(fact("other", "q", &["other-value"]))
        .await
        .expect("other");
    let before = net.agenda_snapshot().expect("before");
    let expected: Vec<_> = before
        .iter()
        .filter(|item| item.rule.id != "unrelated")
        .map(|item| (item.id.clone(), item.seq))
        .collect();
    let executed = net
        .execute_agenda_items_for_fact("target")
        .expect("execute");
    assert_eq!(
        executed
            .iter()
            .map(|e| e.item.rule.id.as_str())
            .collect::<Vec<_>>(),
        vec!["high", "low-first", "low-last"]
    );
    assert_eq!(
        executed
            .iter()
            .map(|e| (e.item.id.clone(), e.item.seq))
            .collect::<Vec<_>>(),
        expected
    );
    for activation in executed {
        assert_eq!(
            activation.actions[0].params,
            vec![activation.item.rule.id, "bound-value".into()]
        );
    }
    net.update_agenda().await.expect("rescan");
    assert!(
        net.execute_agenda_items_for_fact("target")
            .expect("refracted")
            .is_empty()
    );
    let after = net.agenda_snapshot().expect("unrelated remains");
    assert_eq!(after.len(), 1);
    assert_eq!(
        (after[0].id.clone(), after[0].seq),
        (before[0].id.clone(), before[0].seq)
    );
}
