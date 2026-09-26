//! Retraction under active rules — the undertested heart of the
//! determinism claim. A retracted fact must not fire pending agenda
//! items, must not clobber refraction state of unrelated facts whose
//! ids share a prefix, and must allow a clean re-assert/re-fire cycle.

use phronesis::engine_types::{Action, Condition, Fact, Rule};
use phronesis::network::ReteNetwork;

fn rule(id: &str, priority: i32, conditions: &[(&str, &[&str])]) -> Rule {
    Rule {
        id: id.into(),
        priority,
        conditions: conditions
            .iter()
            .map(|(pred, args)| Condition {
                predicate: (*pred).into(),
                args: args.iter().map(|s| s.to_string()).collect(),
                script: None,
            })
            .collect(),
        actions: vec![Action {
            action_type: "log".into(),
            params: vec![format!("{id} fired with ?x")],
            ..Default::default()
        }],
    }
}

fn fact(id: &str, pred: &str, args: &[&str]) -> Fact {
    Fact {
        id: id.into(),
        predicate: pred.into(),
        args: args.iter().map(|s| s.to_string()).collect(),
        timestamp: 0,
        source: None,
    }
}

#[tokio::test]
async fn retracted_fact_does_not_fire_pending_rule() {
    let net = ReteNetwork::new();
    net.add_rule(rule("r1", 1, &[("p", &["?x"])]))
        .await
        .unwrap();
    net.assert_fact(fact("f1", "p", &["v1"])).await.unwrap();
    net.update_agenda().await.unwrap();

    // The activation is pending on the agenda. Retract before firing.
    net.retract_fact("f1").await.unwrap();

    let consequences = net.fire_all_consequences().unwrap();
    assert!(
        consequences.is_empty(),
        "a retracted fact must not fire: {consequences:?}"
    );
}

#[tokio::test]
async fn retraction_only_purges_items_referencing_the_fact() {
    let net = ReteNetwork::new();
    net.add_rule(rule("r1", 1, &[("p", &["?x"])]))
        .await
        .unwrap();
    net.assert_fact(fact("f1", "p", &["v1"])).await.unwrap();
    net.assert_fact(fact("f2", "p", &["v2"])).await.unwrap();
    net.update_agenda().await.unwrap();

    net.retract_fact("f1").await.unwrap();

    let consequences = net.fire_all_consequences().unwrap();
    assert_eq!(consequences.len(), 1, "only f2's activation survives");
    assert_eq!(consequences[0].payload["message"], "r1 fired with v2");
}

#[tokio::test]
async fn refraction_keys_survive_prefix_sibling_retraction() {
    // Retraction compares exact WME ids. A substring purge on
    // retract("f1") would also clobber the key for "f10", letting f10
    // re-fire on the next full agenda rebuild.
    let net = ReteNetwork::new();
    net.add_rule(rule("r1", 1, &[("p", &["?x"])]))
        .await
        .unwrap();
    net.assert_fact(fact("f1", "p", &["v1"])).await.unwrap();
    net.assert_fact(fact("f10", "p", &["v10"])).await.unwrap();
    net.update_agenda().await.unwrap();
    let first = net.fire_all_consequences().unwrap();
    assert_eq!(first.len(), 2, "both fire once");

    net.retract_fact("f1").await.unwrap();
    net.update_agenda().await.unwrap();
    let after = net.fire_all_consequences().unwrap();
    assert!(
        after.is_empty(),
        "f10 already fired; retracting f1 must not reopen it: {after:?}"
    );
}

#[tokio::test]
async fn reassert_after_retract_fires_again() {
    let net = ReteNetwork::new();
    net.add_rule(rule("r1", 1, &[("p", &["?x"])]))
        .await
        .unwrap();
    net.assert_fact(fact("f1", "p", &["v1"])).await.unwrap();
    net.update_agenda().await.unwrap();
    assert_eq!(net.fire_all_consequences().unwrap().len(), 1);

    net.retract_fact("f1").await.unwrap();
    net.assert_fact(fact("f1", "p", &["v1b"])).await.unwrap();
    net.update_agenda().await.unwrap();
    let again = net.fire_all_consequences().unwrap();
    assert_eq!(again.len(), 1, "fresh assertion fires fresh");
    assert_eq!(again[0].payload["message"], "r1 fired with v1b");
}

#[tokio::test]
async fn multi_condition_pending_activation_dies_with_either_wme() {
    let net = ReteNetwork::new();
    net.add_rule(rule("join", 1, &[("a", &["?x"]), ("b", &["?x"])]))
        .await
        .unwrap();
    net.assert_fact(fact("fa", "a", &["k"])).await.unwrap();
    net.assert_fact(fact("fb", "b", &["k"])).await.unwrap();
    net.update_agenda().await.unwrap();

    // Pending two-WME activation; retract the right-hand fact.
    net.retract_fact("fb").await.unwrap();

    let consequences = net.fire_all_consequences().unwrap();
    assert!(
        consequences.is_empty(),
        "join activation must die with fb: {consequences:?}"
    );
}

// Refraction keys must be injective over (rule id, ordered WME ids). Rule
// and fact ids are free-form strings — live ids already carry `:` and `,`
// (`coverage:...`, `provider:x:0`, `__rule_override__:i:id`) — so any
// delimiter-joined string key can collide or mis-parse.

#[tokio::test]
async fn reassert_after_retract_fires_again_with_colon_in_rule_id() {
    let net = ReteNetwork::new();
    net.add_rule(rule("ns:r", 1, &[("p", &["?x"])]))
        .await
        .unwrap();
    net.assert_fact(fact("f1", "p", &["v1"])).await.unwrap();
    net.update_agenda().await.unwrap();
    assert_eq!(net.fire_all_consequences().unwrap().len(), 1);

    net.retract_fact("f1").await.unwrap();
    net.assert_fact(fact("f1", "p", &["v1b"])).await.unwrap();
    net.update_agenda().await.unwrap();
    let again = net.fire_all_consequences().unwrap();
    assert_eq!(again.len(), 1, "retraction must clear the `ns:r` key");
    assert_eq!(again[0].payload["message"], "ns:r fired with v1b");
}

#[tokio::test]
async fn refraction_keys_do_not_collide_across_colon_in_rule_and_fact_ids() {
    // A string key "rule:wmes" maps (a, [b:c]) and (a:b, [c]) to "a:b:c".
    let net = ReteNetwork::new();
    net.add_rule(rule("a", 1, &[("p", &["?x"])])).await.unwrap();
    net.add_rule(rule("a:b", 1, &[("p", &["?x"])]))
        .await
        .unwrap();
    net.assert_fact(fact("b:c", "p", &["v1"])).await.unwrap();
    net.assert_fact(fact("c", "p", &["v2"])).await.unwrap();
    net.update_agenda().await.unwrap();

    let consequences = net.fire_all_consequences().unwrap();
    assert_eq!(
        consequences.len(),
        4,
        "two rules x two facts are four distinct activations: {consequences:?}"
    );
}

#[tokio::test]
async fn refraction_keys_do_not_collide_across_comma_in_fact_ids() {
    // A comma-joined WME list maps [a,b | c] and [a | b,c] to "a,b,c".
    let net = ReteNetwork::new();
    net.add_rule(rule("join", 1, &[("l", &["?x"]), ("r", &["?y"])]))
        .await
        .unwrap();
    net.assert_fact(fact("a,b", "l", &["1"])).await.unwrap();
    net.assert_fact(fact("a", "l", &["2"])).await.unwrap();
    net.assert_fact(fact("c", "r", &["3"])).await.unwrap();
    net.assert_fact(fact("b,c", "r", &["4"])).await.unwrap();
    net.update_agenda().await.unwrap();

    let consequences = net.fire_all_consequences().unwrap();
    assert_eq!(
        consequences.len(),
        4,
        "two left x two right facts are four distinct joins: {consequences:?}"
    );
}
