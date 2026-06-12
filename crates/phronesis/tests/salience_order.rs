//! Conflict-resolution ordering: higher salience fires first, and ties
//! fire in agenda-insertion (FIFO) order — pinned so firing order is
//! deterministic by construction, not by BinaryHeap accident. Hosts that
//! replay-test recorded sessions depend on this being stable.

use phronesis::engine_types::{Action, Condition, Fact, Rule};
use phronesis::network::ReteNetwork;

fn rule(id: &str, priority: i32, predicate: &str) -> Rule {
    Rule {
        id: id.into(),
        priority,
        conditions: vec![Condition {
            predicate: predicate.into(),
            args: vec!["?x".into()],
            script: None,
        }],
        actions: vec![Action {
            action_type: "log".into(),
            params: vec![id.to_string()],
        }],
    }
}

fn fact(id: &str, pred: &str) -> Fact {
    Fact {
        id: id.into(),
        predicate: pred.into(),
        args: vec!["v".into()],
        timestamp: 0,
    }
}

fn fired_order(consequences: &[phronesis::Consequence]) -> Vec<String> {
    consequences
        .iter()
        .map(|c| {
            c.payload["message"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn higher_salience_fires_first() {
    let net = ReteNetwork::new();
    net.add_rule(rule("low", 1, "pa")).await.unwrap();
    net.add_rule(rule("high", 10, "pb")).await.unwrap();
    net.add_rule(rule("mid", 5, "pc")).await.unwrap();
    net.assert_fact(fact("fa", "pa")).await.unwrap();
    net.assert_fact(fact("fb", "pb")).await.unwrap();
    net.assert_fact(fact("fc", "pc")).await.unwrap();
    net.update_agenda().await.unwrap();

    let order = fired_order(&net.fire_all_consequences().unwrap());
    assert_eq!(order, vec!["high", "mid", "low"]);
}

#[tokio::test]
async fn same_salience_fires_in_insertion_order() {
    // 8 same-salience activations: enough heap depth that BinaryHeap's
    // arbitrary tie behavior diverges from FIFO unless the agenda pins it.
    for round in 0..5 {
        let net = ReteNetwork::new();
        let expected: Vec<String> = (0..8).map(|i| format!("rule-{i}")).collect();
        for i in 0..8 {
            net.add_rule(rule(&format!("rule-{i}"), 5, &format!("p{i}")))
                .await
                .unwrap();
        }
        // Assertion order drives agenda insertion order.
        for i in 0..8 {
            net.assert_fact(fact(&format!("f{i}"), &format!("p{i}")))
                .await
                .unwrap();
        }
        net.update_agenda().await.unwrap();

        let order = fired_order(&net.fire_all_consequences().unwrap());
        assert_eq!(
            order, expected,
            "round {round}: same-salience activations must fire FIFO"
        );
    }
}

#[tokio::test]
async fn salience_beats_insertion_order() {
    let net = ReteNetwork::new();
    net.add_rule(rule("late-but-high", 9, "pb")).await.unwrap();
    net.add_rule(rule("early-but-low", 2, "pa")).await.unwrap();
    net.assert_fact(fact("fa", "pa")).await.unwrap(); // inserted first
    net.assert_fact(fact("fb", "pb")).await.unwrap(); // inserted second, higher salience
    net.update_agenda().await.unwrap();

    let order = fired_order(&net.fire_all_consequences().unwrap());
    assert_eq!(order, vec!["late-but-high", "early-but-low"]);
}

#[tokio::test]
async fn retraction_preserves_relative_order_of_survivors() {
    for round in 0..10 {
        let net = ReteNetwork::new();
        for (rule_id, pred) in [("first", "p1"), ("second", "p2"), ("third", "p3")] {
            net.add_rule(rule(rule_id, 5, pred)).await.unwrap();
        }
        net.assert_fact(fact("f1", "p1")).await.unwrap();
        net.assert_fact(fact("f2", "p2")).await.unwrap();
        net.assert_fact(fact("f3", "p3")).await.unwrap();
        net.update_agenda().await.unwrap();

        // Drop the middle pending activation; survivors keep FIFO order.
        net.retract_fact("f2").await.unwrap();

        let order = fired_order(&net.fire_all_consequences().unwrap());
        assert_eq!(
            order,
            vec!["first", "third"],
            "round {round}: retraction must not reshuffle survivors"
        );
    }
}
