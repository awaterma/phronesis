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
