//! Rule replacement semantics for multi-condition beta terminals.
//!
//! Removing a rule clears its production state, agenda items, and refraction
//! keys. It must also retire the beta p-state marker the rule's terminal
//! carried: a rule re-added under the same id would otherwise inherit the
//! stale association the next time a fact flows through the old terminal —
//! the activation lookup finds the *current* rule by id and fires it with the
//! removed generation's pattern and salience. These tests pin that removal
//! retires the marker (agenda empty immediately, no consequences, no
//! update_agenda/rescan anywhere), while shared join memories, surviving
//! rules, and re-added generations keep working.
//!
//! Facts use one value per generation so cross-instance token joins (same
//! `?x` bound to different values) stay in conflict and each expected firing
//! is exactly one.

use phronesis::consequence::Provenance;
use phronesis::engine_types::{Action, Condition, Fact, Rule};
use phronesis::network::ReteNetwork;

fn var_condition(predicate: &str) -> Condition {
    Condition {
        predicate: predicate.into(),
        args: vec!["?x".into()],
        script: None,
    }
}

fn rule(id: &str, priority: i32, predicates: &[&str]) -> Rule {
    Rule {
        id: id.into(),
        priority,
        conditions: predicates.iter().map(|p| var_condition(p)).collect(),
        actions: vec![Action {
            action_type: "log".into(),
            params: vec![format!("{id} fired with ?x")],
            ..Default::default()
        }],
    }
}

fn fact(id: &str, predicate: &str, value: &str) -> Fact {
    Fact {
        id: id.into(),
        predicate: predicate.into(),
        args: vec![value.into()],
        timestamp: 0,
        source: None,
    }
}

fn count_for_rule(agenda: &[phronesis::agenda::AgendaItem], rule_id: &str) -> usize {
    agenda.iter().filter(|item| item.rule.id == rule_id).count()
}

fn fired_rule_ids(consequences: &[phronesis::consequence::Consequence]) -> Vec<&str> {
    consequences
        .iter()
        .filter_map(|c| match &c.provenance {
            Provenance::RuleFiring { rule_id, .. } => Some(rule_id.as_str()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn readded_rule_with_changed_predicates_ignores_old_pattern_facts() {
    let net = ReteNetwork::new();
    net.add_rule(rule("r", 5, &["a", "b"])).await.unwrap();
    net.remove_rule("r").unwrap();
    net.add_rule(rule("r", 9, &["x", "y"])).await.unwrap();

    assert!(
        net.agenda_snapshot().unwrap().is_empty(),
        "agenda must be empty immediately after re-add, before any assert"
    );

    // Facts for the removed generation's pattern only.
    net.assert_fact(fact("a1", "a", "1")).await.unwrap();
    net.assert_fact(fact("b1", "b", "1")).await.unwrap();

    let agenda = net.agenda_snapshot().unwrap();
    assert_eq!(
        count_for_rule(&agenda, "r"),
        0,
        "a stale terminal association must not activate the replacement rule on the old pattern"
    );
    assert!(
        net.fire_all_consequences().unwrap().is_empty(),
        "old-pattern facts must produce no consequences for the replacement rule"
    );
}

#[tokio::test]
async fn removal_retires_prefix_asserted_before_it() {
    let net = ReteNetwork::new();
    net.add_rule(rule("r", 5, &["a", "b"])).await.unwrap();
    net.assert_fact(fact("a1", "a", "1")).await.unwrap();
    net.remove_rule("r").unwrap();
    net.add_rule(rule("r", 9, &["x", "y"])).await.unwrap();
    net.assert_fact(fact("b1", "b", "1")).await.unwrap();

    let agenda = net.agenda_snapshot().unwrap();
    assert_eq!(
        count_for_rule(&agenda, "r"),
        0,
        "a prefix token held by the removed generation's terminal must not complete the replacement rule"
    );
    assert!(net.fire_all_consequences().unwrap().is_empty());
}

#[tokio::test]
async fn replacement_fires_once_with_new_salience_on_new_predicates() {
    let net = ReteNetwork::new();
    net.add_rule(rule("r", 5, &["a", "b"])).await.unwrap();
    net.remove_rule("r").unwrap();
    net.add_rule(rule("r", 9, &["x", "y"])).await.unwrap();

    net.assert_fact(fact("x1", "x", "1")).await.unwrap();
    net.assert_fact(fact("y1", "y", "1")).await.unwrap();

    let agenda = net.agenda_snapshot().unwrap();
    assert_eq!(
        count_for_rule(&agenda, "r"),
        1,
        "replacement must activate once on its own pattern"
    );
    assert_eq!(
        agenda[0].salience, 9,
        "the replacement must carry the salience it was re-added with"
    );

    let consequences = net.fire_all_consequences().unwrap();
    assert_eq!(consequences.len(), 1);
    assert_eq!(fired_rule_ids(&consequences), vec!["r"]);

    // After firing, the old pattern still never reaches the replacement.
    net.assert_fact(fact("a1", "a", "1")).await.unwrap();
    net.assert_fact(fact("b1", "b", "1")).await.unwrap();
    assert!(net.agenda_snapshot().unwrap().is_empty());
    assert!(net.fire_all_consequences().unwrap().is_empty());
}

#[tokio::test]
async fn removing_one_rule_keeps_shared_pattern_survivor_firing() {
    let net = ReteNetwork::new();
    net.add_rule(rule("r", 5, &["a", "b"])).await.unwrap();
    net.add_rule(rule("s", 3, &["a", "b"])).await.unwrap();

    net.assert_fact(fact("a1", "a", "1")).await.unwrap();
    net.assert_fact(fact("b1", "b", "1")).await.unwrap();
    let consequences = net.fire_all_consequences().unwrap();
    assert_eq!(
        fired_rule_ids(&consequences).len(),
        2,
        "both rules sharing the pattern fire on the first match: {consequences:?}"
    );

    net.remove_rule("r").unwrap();
    net.assert_fact(fact("a2", "a", "2")).await.unwrap();
    net.assert_fact(fact("b2", "b", "2")).await.unwrap();

    let agenda = net.agenda_snapshot().unwrap();
    assert_eq!(
        count_for_rule(&agenda, "r"),
        0,
        "removed rule must not reactivate"
    );
    assert_eq!(
        count_for_rule(&agenda, "s"),
        1,
        "survivor must keep firing on new matches"
    );
    let consequences = net.fire_all_consequences().unwrap();
    assert_eq!(fired_rule_ids(&consequences), vec!["s"]);
}

#[tokio::test]
async fn removing_rule_drops_pending_multicondition_activation() {
    let net = ReteNetwork::new();
    net.add_rule(rule("r", 5, &["a", "b"])).await.unwrap();
    net.assert_fact(fact("a1", "a", "1")).await.unwrap();
    net.assert_fact(fact("b1", "b", "1")).await.unwrap();
    assert_eq!(count_for_rule(&net.agenda_snapshot().unwrap(), "r"), 1);

    net.remove_rule("r").unwrap();
    assert!(
        net.agenda_snapshot().unwrap().is_empty(),
        "removal must drop the rule's pending activation"
    );
    assert!(net.fire_all_consequences().unwrap().is_empty());
}

#[tokio::test]
async fn repeated_remove_and_readd_fires_each_generation_once() {
    let net = ReteNetwork::new();

    net.add_rule(rule("r", 5, &["a", "b"])).await.unwrap();
    net.assert_fact(fact("a1", "a", "1")).await.unwrap();
    net.assert_fact(fact("b1", "b", "1")).await.unwrap();
    assert_eq!(net.fire_all_consequences().unwrap().len(), 1);
    net.remove_rule("r").unwrap();

    net.add_rule(rule("r", 7, &["x", "y"])).await.unwrap();
    net.assert_fact(fact("a2", "a", "2")).await.unwrap();
    net.assert_fact(fact("b2", "b", "2")).await.unwrap();
    assert!(
        net.agenda_snapshot().unwrap().is_empty(),
        "generation 2 must not fire on generation 1's pattern"
    );
    net.assert_fact(fact("x1", "x", "2")).await.unwrap();
    net.assert_fact(fact("y1", "y", "2")).await.unwrap();
    let agenda = net.agenda_snapshot().unwrap();
    assert_eq!(count_for_rule(&agenda, "r"), 1);
    assert_eq!(agenda[0].salience, 7);
    assert_eq!(net.fire_all_consequences().unwrap().len(), 1);
    net.remove_rule("r").unwrap();

    net.add_rule(rule("r", 3, &["m", "n"])).await.unwrap();
    net.assert_fact(fact("a3", "a", "3")).await.unwrap();
    net.assert_fact(fact("b3", "b", "3")).await.unwrap();
    net.assert_fact(fact("x2", "x", "3")).await.unwrap();
    net.assert_fact(fact("y2", "y", "3")).await.unwrap();
    assert!(
        net.agenda_snapshot().unwrap().is_empty(),
        "generation 3 must not fire on earlier generations' patterns"
    );
    net.assert_fact(fact("m1", "m", "3")).await.unwrap();
    net.assert_fact(fact("n1", "n", "3")).await.unwrap();
    let agenda = net.agenda_snapshot().unwrap();
    assert_eq!(count_for_rule(&agenda, "r"), 1);
    assert_eq!(agenda[0].salience, 3);
    assert_eq!(net.fire_all_consequences().unwrap().len(), 1);
}
