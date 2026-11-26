use super::*;

#[tokio::test]
async fn selected_execution_error_keeps_not_yet_selected_items_pending() -> Result<(), ReteError> {
    let net = ReteNetwork::new();
    for (id, priority, predicate) in [
        ("earlier", 20, "p"),
        ("missing", 10, "p"),
        ("later", 1, "p"),
        ("other", 100, "q"),
    ] {
        net.add_rule(Rule {
            id: id.into(),
            priority,
            conditions: vec![Condition {
                predicate: predicate.into(),
                args: vec!["?x".into()],
                script: None,
            }],
            actions: vec![],
        })
        .await?;
    }
    for (id, predicate) in [("target", "p"), ("unrelated", "q")] {
        net.assert_fact(Fact {
            id: id.into(),
            predicate: predicate.into(),
            args: vec!["value".into()],
            timestamp: 0,
            source: None,
        })
        .await?;
    }
    let expected: Vec<_> = net
        .agenda_snapshot()?
        .into_iter()
        .filter(|item| item.rule.id != "missing" && item.rule.id != "earlier")
        .map(|item| (item.id, item.seq))
        .collect();
    // Internal corruption exercises the existing error path without adding a
    // public fault-injection API or changing production mutation semantics.
    net.production_network
        .lock()
        .expect("production lock")
        .states
        .retain(|state| state.rule.id != "missing");
    assert!(matches!(net.execute_agenda_items_for_fact("target"),
        Err(ReteError::ProductionStateNotFound(id)) if id == "missing"));
    let remaining: Vec<_> = net
        .agenda_snapshot()?
        .into_iter()
        .map(|item| (item.id, item.seq))
        .collect();
    assert_eq!(remaining, expected);
    let later = net.execute_agenda_items_for_fact("target")?;
    assert_eq!(later.len(), 1);
    assert_eq!(later[0].item.rule.id, "later");
    assert_eq!(net.agenda_snapshot()?[0].rule.id, "other");
    Ok(())
}

#[tokio::test]
async fn collect_only_drain_reuses_one_guard_snapshot_across_matches() -> Result<(), ReteError> {
    let net = ReteNetwork::new();
    for id in ["first", "second"] {
        net.add_rule(Rule {
            id: id.into(),
            priority: 1,
            conditions: vec![
                Condition {
                    predicate: "p".into(),
                    args: vec!["?x".into()],
                    script: None,
                },
                Condition {
                    predicate: "__script__".into(),
                    args: vec![],
                    script: Some("facts_contain('ready', [])".into()),
                },
            ],
            actions: vec![],
        })
        .await?;
    }
    net.assert_fact(Fact {
        id: "ready".into(),
        predicate: "ready".into(),
        args: vec![],
        timestamp: 0,
        source: None,
    })
    .await?;
    net.assert_fact(Fact {
        id: "target".into(),
        predicate: "p".into(),
        args: vec!["value".into()],
        timestamp: 0,
        source: None,
    })
    .await?;

    let before = net.guard_snapshot_count();
    let executed = net.execute_agenda_items_for_fact("target")?;
    assert_eq!(executed.len(), 2, "both rules match the target fact");
    assert_eq!(
        net.guard_snapshot_count() - before,
        1,
        "a no-op collect-only drain must not rebuild the guard snapshot \
         per activation — nothing it does can change working memory"
    );
    Ok(())
}

#[tokio::test]
async fn host_callback_drain_refreshes_guard_snapshot_per_activation() -> Result<(), ReteError> {
    let net = ReteNetwork::new();
    for id in ["first", "second"] {
        net.add_rule(Rule {
            id: id.into(),
            priority: 1,
            conditions: vec![
                Condition {
                    predicate: "p".into(),
                    args: vec!["?x".into()],
                    script: None,
                },
                Condition {
                    predicate: "__script__".into(),
                    args: vec![],
                    script: Some("facts_contain('ready', [])".into()),
                },
            ],
            actions: vec![],
        })
        .await?;
    }
    net.assert_fact(Fact {
        id: "ready".into(),
        predicate: "ready".into(),
        args: vec![],
        timestamp: 0,
        source: None,
    })
    .await?;
    net.assert_fact(Fact {
        id: "target".into(),
        predicate: "p".into(),
        args: vec!["value".into()],
        timestamp: 0,
        source: None,
    })
    .await?;

    let before = net.guard_snapshot_count();
    let executed = net.execute_agenda_items_for_fact_with("target", |_| Ok(()))?;
    assert_eq!(executed.len(), 2, "both rules match the target fact");
    assert_eq!(
        net.guard_snapshot_count() - before,
        2,
        "a host callback may mutate working memory, so each activation \
         must judge guards against a fresh snapshot"
    );
    Ok(())
}

#[tokio::test]
async fn partially_failed_assertion_invalidates_ordered_scan() -> Result<(), ReteError> {
    let net = ReteNetwork::new();
    for (id, args) in [
        ("left-pair", ["?x", "?x", "?y"]),
        ("right-pair", ["?x", "?y", "?y"]),
    ] {
        net.add_rule(Rule {
            id: id.into(),
            priority: 1,
            conditions: vec![Condition {
                predicate: "p".into(),
                args: args.map(str::to_string).to_vec(),
                script: None,
            }],
            actions: vec![],
        })
        .await?;
    }
    for id in ["t1", "t2"] {
        net.assert_fact(Fact {
            id: id.into(),
            predicate: "temp".into(),
            args: vec![],
            timestamp: 0,
            source: None,
        })
        .await?;
    }
    let batch = net.begin_ordered_batch()?;
    net.retract_fact("t1").await?;
    // Alpha state traversal is hash ordered. Choose input that passes the first
    // real state and fails the second so the partial update is deterministic.
    let first = net
        .alpha_network
        .lock()
        .expect("alpha lock")
        .states
        .values()
        .next()
        .expect("two alpha states")
        .condition
        .clone();
    let args = if first.args[0] == first.args[1] {
        ["a", "a", "b"]
    } else {
        ["a", "b", "b"]
    };
    assert!(matches!(
        net.assert_fact(Fact {
            id: "partial".into(),
            predicate: "p".into(),
            args: args.map(str::to_string).to_vec(),
            timestamp: 0,
            source: None,
        })
        .await,
        Err(ReteError::BindingConflict { .. })
    ));
    assert!(
        net.agenda_snapshot()?.is_empty(),
        "assertion failed before agenda insertion"
    );
    net.retract_fact("t2").await?;
    assert_eq!(
        net.agenda_snapshot()?.len(),
        1,
        "ordinary retraction rediscovers the match left in alpha memory"
    );
    assert_eq!(net.agenda_snapshot()?[0].wme_list[0].id, "partial");
    batch.close()?;
    Ok(())
}
