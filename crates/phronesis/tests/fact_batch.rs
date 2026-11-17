use phronesis::engine_types::{Action, Condition, Fact, Rule};
use phronesis::network::ReteNetwork;

fn rule(id: &str, priority: i32, clauses: &[(&str, &[&str])]) -> Rule {
    Rule {
        id: id.into(),
        priority,
        conditions: clauses
            .iter()
            .map(|(p, a)| Condition {
                predicate: (*p).into(),
                args: a.iter().map(|s| s.to_string()).collect(),
                script: None,
            })
            .collect(),
        actions: vec![Action {
            action_type: "log".into(),
            params: vec![id.into()],
            ..Default::default()
        }],
    }
}

fn fact(id: &str, predicate: &str, args: &[&str]) -> Fact {
    Fact {
        id: id.into(),
        predicate: predicate.into(),
        args: args.iter().map(|s| s.to_string()).collect(),
        timestamp: 0,
        source: None,
    }
}

#[tokio::test]
async fn single_and_join_matches_are_visible_only_after_outer_close() {
    let n = ReteNetwork::new();
    n.add_rule(rule("single", 1, &[("p", &["?x"])]))
        .await
        .unwrap();
    n.add_rule(rule("join", 9, &[("p", &["?x"]), ("q", &["?x"])]))
        .await
        .unwrap();
    let passes_before = n.agenda_pass_count();
    let outer = n.begin_batch().unwrap();
    n.assert_fact(fact("p1", "p", &["v"])).await.unwrap();
    let inner = n.begin_batch().unwrap();
    n.assert_fact(fact("q1", "q", &["v"])).await.unwrap();
    assert_eq!(n.agenda_pass_count(), passes_before);
    assert!(n.agenda_snapshot().unwrap().is_empty());
    assert!(n.fire_all_consequences().unwrap().is_empty());
    inner.close().unwrap();
    assert_eq!(n.batch_depth().unwrap(), 1);
    assert!(n.agenda_snapshot().unwrap().is_empty());
    outer.close().unwrap();
    assert_eq!(n.agenda_pass_count(), passes_before + 1);
    let fired = n.fire_all_consequences().unwrap();
    assert_eq!(fired.len(), 2);
    assert_eq!(fired[0].payload["message"], "join");
    assert_eq!(fired[1].payload["message"], "single");
}

#[tokio::test]
async fn retract_then_reassert_inside_batch_purges_stale_activation_and_preserves_refraction() {
    let n = ReteNetwork::new();
    n.add_rule(rule("r", 1, &[("p", &["?x"])])).await.unwrap();
    let passes_before = n.agenda_pass_count();
    let b = n.begin_batch().unwrap();
    n.assert_fact(fact("f", "p", &["old"])).await.unwrap();
    assert_eq!(n.batch_depth().unwrap(), 1);
    n.retract_fact("f").await.unwrap();
    assert_eq!(n.agenda_pass_count(), passes_before);
    n.assert_fact(fact("f", "p", &["new"])).await.unwrap();
    b.close().unwrap();
    assert_eq!(n.agenda_pass_count(), passes_before + 1);
    assert_eq!(n.fire_all_consequences().unwrap().len(), 1);
    n.update_agenda().await.unwrap();
    assert!(n.fire_all_consequences().unwrap().is_empty());
}

#[tokio::test]
async fn direct_update_and_fire_during_batch_do_not_expose_new_matches() {
    let n = ReteNetwork::new();
    n.add_rule(rule("r", 1, &[("p", &["?x"])])).await.unwrap();
    let b = n.begin_batch().unwrap();
    let passes_before = n.agenda_pass_count();
    n.assert_fact(fact("f", "p", &["v"])).await.unwrap();
    n.update_agenda().await.unwrap();
    assert_eq!(n.agenda_pass_count(), passes_before);
    assert!(n.agenda_snapshot().unwrap().is_empty());
    assert!(n.fire_all_consequences().unwrap().is_empty());
    drop(b);
    assert_eq!(n.agenda_pass_count(), passes_before + 1);
    assert_eq!(n.fire_all_consequences().unwrap().len(), 1);
}

#[tokio::test]
async fn dropping_inner_guard_keeps_outer_open_and_flushes_when_outer_drops() {
    let n = ReteNetwork::new();
    n.add_rule(rule("r", 1, &[("p", &["?x"])])).await.unwrap();
    let outer = n.begin_batch().unwrap();
    {
        let _inner = n.begin_batch().unwrap();
        n.assert_fact(fact("f", "p", &["v"])).await.unwrap();
    }
    assert_eq!(n.batch_depth().unwrap(), 1);
    assert!(n.agenda_snapshot().unwrap().is_empty());
    drop(outer);
    assert_eq!(n.batch_depth().unwrap(), 0);
    assert_eq!(n.fire_all_consequences().unwrap().len(), 1);
}

#[tokio::test]
async fn failed_operation_does_not_rollback_prior_batch_work() {
    let n = ReteNetwork::new();
    n.add_rule(rule("r", 1, &[("p", &["?x"])])).await.unwrap();
    let b = n.begin_batch().unwrap();
    n.assert_fact(fact("f", "p", &["v"])).await.unwrap();
    assert!(n.retract_fact("missing").await.is_err());
    assert_eq!(n.batch_depth().unwrap(), 1);
    b.close().unwrap();
    assert_eq!(n.fire_all_consequences().unwrap().len(), 1);
}

#[tokio::test]
async fn negative_and_multiple_bindings_are_rebuilt_on_close() {
    let n = ReteNetwork::new();
    n.add_rule(rule("join", 1, &[("left", &["?x"]), ("right", &["?y"])]))
        .await
        .unwrap();
    let b = n.begin_batch().unwrap();
    n.assert_fact(fact("l1", "left", &["a"])).await.unwrap();
    n.assert_fact(fact("l2", "left", &["b"])).await.unwrap();
    n.assert_fact(fact("r1", "right", &["1"])).await.unwrap();
    n.assert_fact(fact("r2", "right", &["2"])).await.unwrap();
    b.close().unwrap();
    assert_eq!(n.fire_all_consequences().unwrap().len(), 4);
}

#[tokio::test]
async fn join_with_mismatched_bindings_does_not_fire_but_matching_values_do() {
    let n = ReteNetwork::new();
    n.add_rule(rule("join", 1, &[("p", &["?x"]), ("q", &["?x"])]))
        .await
        .unwrap();

    // Mismatched values on the shared ?x variable: unification must reject this.
    let b = n.begin_batch().unwrap();
    n.assert_fact(fact("p1", "p", &["v"])).await.unwrap();
    n.assert_fact(fact("q1", "q", &["w"])).await.unwrap();
    b.close().unwrap();
    assert!(n.agenda_snapshot().unwrap().is_empty());
    assert!(n.fire_all_consequences().unwrap().is_empty());

    // Complement: two `p` facts, one matching value for `q` — exactly one join fires.
    let b2 = n.begin_batch().unwrap();
    n.assert_fact(fact("p2", "p", &["a"])).await.unwrap();
    n.assert_fact(fact("p3", "p", &["b"])).await.unwrap();
    n.assert_fact(fact("q2", "q", &["b"])).await.unwrap();
    b2.close().unwrap();
    let fired = n.fire_all_consequences().unwrap();
    assert_eq!(fired.len(), 1);
    assert_eq!(fired[0].payload["message"], "join");
}

#[tokio::test]
async fn previously_fired_activation_remains_latched_after_close() {
    let n = ReteNetwork::new();
    n.add_rule(rule("r", 1, &[("p", &["?x"])])).await.unwrap();
    n.assert_fact(fact("f", "p", &["v"])).await.unwrap();
    assert_eq!(n.fire_all_consequences().unwrap().len(), 1);
    let b = n.begin_batch().unwrap();
    n.assert_fact(fact("other", "other", &["v"])).await.unwrap();
    b.close().unwrap();
    assert!(n.fire_all_consequences().unwrap().is_empty());
}

#[tokio::test]
async fn retracting_preexisting_pending_activation_removes_it() {
    let n = ReteNetwork::new();
    n.add_rule(rule("r", 1, &[("p", &["?x"])])).await.unwrap();
    n.assert_fact(fact("f", "p", &["v"])).await.unwrap();
    assert_eq!(n.agenda_snapshot().unwrap().len(), 1);
    let b = n.begin_batch().unwrap();
    n.retract_fact("f").await.unwrap();
    b.close().unwrap();
    assert!(n.agenda_snapshot().unwrap().is_empty());
    assert!(n.fire_all_consequences().unwrap().is_empty());
}

#[tokio::test]
async fn multi_join_retract_reassert_keeps_only_current_activation() {
    let n = ReteNetwork::new();
    n.add_rule(rule(
        "chain",
        1,
        &[("a", &["?x"]), ("b", &["?x", "?y"]), ("c", &["?y"])],
    ))
    .await
    .unwrap();
    let b = n.begin_batch().unwrap();
    n.assert_fact(fact("a", "a", &["x"])).await.unwrap();
    n.assert_fact(fact("b", "b", &["x", "old"])).await.unwrap();
    n.assert_fact(fact("cold", "c", &["old"])).await.unwrap();
    n.retract_fact("b").await.unwrap();
    n.assert_fact(fact("b", "b", &["x", "new"])).await.unwrap();
    n.assert_fact(fact("cnew", "c", &["new"])).await.unwrap();
    b.close().unwrap();
    assert_eq!(n.fire_all_consequences().unwrap().len(), 1);
}

#[tokio::test]
async fn out_of_order_drop_only_flushes_after_last_guard() {
    let n = ReteNetwork::new();
    n.add_rule(rule("r", 1, &[("p", &["?x"])])).await.unwrap();
    let first = n.begin_batch().unwrap();
    let second = n.begin_batch().unwrap();
    n.assert_fact(fact("f", "p", &["v"])).await.unwrap();
    drop(first);
    assert_eq!(n.batch_depth().unwrap(), 1);
    assert!(n.agenda_snapshot().unwrap().is_empty());
    drop(second);
    assert_eq!(n.batch_depth().unwrap(), 0);
    assert_eq!(n.fire_all_consequences().unwrap().len(), 1);
}

#[test]
fn unwind_closes_batch_and_flushes() {
    let n = ReteNetwork::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime
        .block_on(n.add_rule(rule("r", 1, &[("p", &["?x"])])))
        .unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _batch = n.begin_batch().unwrap();
        runtime
            .block_on(n.assert_fact(fact("f", "p", &["v"])))
            .unwrap();
        panic!("unwind batch scope");
    }));
    assert!(result.is_err());
    assert_eq!(n.batch_depth().unwrap(), 0);
    assert_eq!(n.fire_all_consequences().unwrap().len(), 1);
}

#[test]
fn concurrent_guards_share_batch_depth_and_close_without_underflow() {
    use std::sync::{Arc, Barrier};
    use std::thread;

    let n = Arc::new(ReteNetwork::new());
    let both_open = Arc::new(Barrier::new(3));
    let both_checked = Arc::new(Barrier::new(3));
    thread::scope(|scope| {
        let mut handles = Vec::new();
        for _ in 0..2 {
            let n = Arc::clone(&n);
            let both_open = Arc::clone(&both_open);
            let both_checked = Arc::clone(&both_checked);
            handles.push(scope.spawn(move || {
                let batch = n.begin_batch().expect("begin shared batch");
                both_open.wait();
                let observed_depth = n.batch_depth().expect("read shared depth");
                both_checked.wait();
                batch.close().expect("close shared batch");
                observed_depth
            }));
        }
        both_open.wait();
        let observed_depth = n.batch_depth().unwrap();
        both_checked.wait();
        assert_eq!(observed_depth, 2);
        for handle in handles {
            assert_eq!(handle.join().expect("guard thread"), 2);
        }
    });
    assert_eq!(n.batch_depth().unwrap(), 0);
}
