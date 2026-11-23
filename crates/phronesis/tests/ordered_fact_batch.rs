//! `ReteNetwork::begin_ordered_batch` — the opt-in batch mode that trades
//! away the fast batch's coalescing for an unbatched-identical firing
//! trace. Each test runs the same `assert_fact`/`retract_fact` sequence
//! twice — once with no batch, once wrapped in an ordered batch (or mixed
//! with a fast batch) — and compares the resulting `(rule_id,
//! bound_fact_ids)` trace instead of asserting against the implementation.
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

/// `(rule_id, bound_fact_ids)` trace in firing order, extracted from
/// `Consequence::provenance` so the test observes what a host actually
/// sees rather than internal agenda state.
fn trace(consequences: &[phronesis::Consequence]) -> Vec<(String, Vec<String>)> {
    consequences
        .iter()
        .map(|c| match &c.provenance {
            phronesis::Provenance::RuleFiring {
                rule_id,
                bound_facts,
                ..
            } => (rule_id.to_string(), bound_facts.clone()),
            other => panic!("expected RuleFiring provenance, got {other:?}"),
        })
        .collect()
}

async fn add_rules(net: &ReteNetwork, rules: &[Rule]) {
    for r in rules {
        net.add_rule(r.clone()).await.unwrap();
    }
}

fn equal_salience_rules() -> Vec<Rule> {
    vec![
        rule("r1", 5, vec![cond("p", &["?x"])]),
        rule("r2", 5, vec![cond("p", &["?x"])]),
        rule("r3", 5, vec![cond("p", &["?x"])]),
    ]
}

fn join_rules() -> Vec<Rule> {
    vec![
        rule("single", 5, vec![cond("p", &["?x"])]),
        rule("join", 5, vec![cond("p", &["?x"]), cond("q", &["?x"])]),
    ]
}

/// Equal-salience ties: an ordered batch must reproduce the exact unbatched
/// insertion-order (FIFO) trace, which a fast batch's final full scan is not
/// guaranteed to.
#[tokio::test]
async fn ordered_batch_matches_unbatched_trace_with_equal_salience_ties() {
    let unbatched = ReteNetwork::new();
    add_rules(&unbatched, &equal_salience_rules()).await;
    unbatched
        .assert_fact(fact("f1", "p", &["a"]))
        .await
        .unwrap();
    unbatched
        .assert_fact(fact("f2", "p", &["b"]))
        .await
        .unwrap();
    let expected = trace(&unbatched.fire_all_consequences().unwrap());

    let ordered = ReteNetwork::new();
    add_rules(&ordered, &equal_salience_rules()).await;
    let batch = ordered.begin_ordered_batch().unwrap();
    ordered.assert_fact(fact("f1", "p", &["a"])).await.unwrap();
    ordered.assert_fact(fact("f2", "p", &["b"])).await.unwrap();
    batch.close().unwrap();
    let actual = trace(&ordered.fire_all_consequences().unwrap());

    assert_eq!(actual, expected);
}

/// Join activations: an ordered batch must not reorder a multi-condition
/// match relative to the single-condition matches around it.
#[tokio::test]
async fn ordered_batch_matches_unbatched_trace_with_a_join() {
    let unbatched = ReteNetwork::new();
    add_rules(&unbatched, &join_rules()).await;
    unbatched
        .assert_fact(fact("f-p1", "p", &["a"]))
        .await
        .unwrap();
    unbatched
        .assert_fact(fact("f-p2", "p", &["b"]))
        .await
        .unwrap();
    unbatched
        .assert_fact(fact("f-q2", "q", &["b"]))
        .await
        .unwrap();
    let expected = trace(&unbatched.fire_all_consequences().unwrap());

    let ordered = ReteNetwork::new();
    add_rules(&ordered, &join_rules()).await;
    let batch = ordered.begin_ordered_batch().unwrap();
    ordered
        .assert_fact(fact("f-p1", "p", &["a"]))
        .await
        .unwrap();
    ordered
        .assert_fact(fact("f-p2", "p", &["b"]))
        .await
        .unwrap();
    ordered
        .assert_fact(fact("f-q2", "q", &["b"]))
        .await
        .unwrap();
    batch.close().unwrap();
    let actual = trace(&ordered.fire_all_consequences().unwrap());

    assert_eq!(actual, expected);
}

/// Retraction inside an ordered batch must leave the same survivors, in the
/// same order, as the equivalent unbatched assert/retract sequence.
#[tokio::test]
async fn ordered_batch_matches_unbatched_trace_across_a_retraction() {
    let unbatched = ReteNetwork::new();
    add_rules(&unbatched, &equal_salience_rules()).await;
    unbatched
        .assert_fact(fact("f1", "p", &["a"]))
        .await
        .unwrap();
    unbatched
        .assert_fact(fact("f2", "p", &["b"]))
        .await
        .unwrap();
    unbatched.retract_fact("f1").await.unwrap();
    unbatched
        .assert_fact(fact("f3", "p", &["c"]))
        .await
        .unwrap();
    let expected = trace(&unbatched.fire_all_consequences().unwrap());

    let ordered = ReteNetwork::new();
    add_rules(&ordered, &equal_salience_rules()).await;
    let batch = ordered.begin_ordered_batch().unwrap();
    ordered.assert_fact(fact("f1", "p", &["a"])).await.unwrap();
    ordered.assert_fact(fact("f2", "p", &["b"])).await.unwrap();
    ordered.retract_fact("f1").await.unwrap();
    ordered.assert_fact(fact("f3", "p", &["c"])).await.unwrap();
    batch.close().unwrap();
    let actual = trace(&ordered.fire_all_consequences().unwrap());

    assert_eq!(actual, expected);
    // Sanity: the retracted fact's activation for f1 must not appear.
    for (_, bound) in &actual {
        assert!(!bound.contains(&"f1".to_string()));
    }
}

/// Opening a fast guard while an ordered session is open (or vice versa)
/// is rejected outright: mixed-mode nesting is not supported, and the
/// rejection must not mutate `batch_depth()` or close the open session.
#[tokio::test]
async fn opening_the_other_mode_mid_session_is_rejected_without_mutating_state() {
    let net = ReteNetwork::new();
    add_rules(&net, &equal_salience_rules()).await;

    let outer_ordered = net.begin_ordered_batch().unwrap();
    assert_eq!(net.batch_depth().unwrap(), 1);
    assert!(
        net.begin_batch().is_err(),
        "fast inside an open ordered session must be rejected"
    );
    assert_eq!(
        net.batch_depth().unwrap(),
        1,
        "a rejected open must not change depth"
    );
    outer_ordered.close().unwrap();
    assert_eq!(net.batch_depth().unwrap(), 0);

    let outer_fast = net.begin_batch().unwrap();
    assert!(
        net.begin_ordered_batch().is_err(),
        "ordered inside an open fast session must be rejected"
    );
    assert_eq!(net.batch_depth().unwrap(), 1);
    outer_fast.close().unwrap();
}

/// Same-mode nesting is a plain depth counter once a session's mode is
/// fixed, so guards may close in any order — not just reverse-open — same
/// as the original fast-only guard.
#[tokio::test]
async fn same_mode_guards_close_in_arbitrary_order() {
    let net = ReteNetwork::new();
    add_rules(&net, &equal_salience_rules()).await;

    let first = net.begin_ordered_batch().unwrap();
    let second = net.begin_ordered_batch().unwrap();
    let third = net.begin_ordered_batch().unwrap();
    assert_eq!(net.batch_depth().unwrap(), 3);

    first.close().unwrap();
    assert_eq!(net.batch_depth().unwrap(), 2);
    third.close().unwrap();
    assert_eq!(net.batch_depth().unwrap(), 1);
    second.close().unwrap();
    assert_eq!(net.batch_depth().unwrap(), 0);
}

/// An ordered batch's outermost close must not run the fast-mode catch-up
/// scan at all: nothing was ever deferred (every `assert_fact` already ran
/// its normal incremental update), so a scan here could only rediscover
/// and re-fire an activation a drain inside the batch found guard-false
/// and un-latched — which is exactly what this test pins.
#[tokio::test]
async fn ordered_batch_close_runs_no_scan_and_does_not_resurrect_a_guard_false_drop() {
    let net = ReteNetwork::new();
    net.add_rule(rule(
        "guarded",
        1,
        vec![
            cond("p", &["?x"]),
            Condition {
                predicate: "__script__".to_string(),
                args: vec![],
                script: Some("facts_contain('never', ['*'])".to_string()),
            },
        ],
    ))
    .await
    .unwrap();

    let passes_before = net.agenda_pass_count();
    let batch = net.begin_ordered_batch().unwrap();
    net.assert_fact(fact("f1", "p", &["v"])).await.unwrap();
    // Drain now, inside the batch: the false guard drops and un-latches
    // the activation.
    assert!(net.fire_all_consequences().unwrap().is_empty());
    batch.close().unwrap();

    assert_eq!(
        net.agenda_pass_count(),
        passes_before,
        "an ordered-mode close must not run the catch-up scan at all"
    );
    assert!(
        net.agenda_snapshot().unwrap().is_empty(),
        "closing must not resurrect the guard-false activation the in-batch drain dropped"
    );
}

/// Legacy fast-only nesting keeps its documented behavior: no scan runs
/// until the outermost guard closes (`agenda_pass_count` proves it), and
/// the single scan at outermost close is the only one that runs.
#[tokio::test]
async fn legacy_fast_only_nesting_behavior_is_retained() {
    let net = ReteNetwork::new();
    add_rules(&net, &equal_salience_rules()).await;
    let passes_before = net.agenda_pass_count();

    let outer = net.begin_batch().unwrap();
    net.assert_fact(fact("f1", "p", &["a"])).await.unwrap();
    let inner = net.begin_batch().unwrap();
    net.assert_fact(fact("f2", "p", &["b"])).await.unwrap();
    assert_eq!(net.agenda_pass_count(), passes_before);
    assert!(net.agenda_snapshot().unwrap().is_empty());

    inner.close().unwrap();
    assert_eq!(
        net.agenda_pass_count(),
        passes_before,
        "closing the inner fast guard must not trigger a scan while the outer is still open"
    );
    assert!(net.agenda_snapshot().unwrap().is_empty());

    outer.close().unwrap();
    assert_eq!(net.agenda_pass_count(), passes_before + 1);
    assert_eq!(net.fire_all_consequences().unwrap().len(), 6);
}
