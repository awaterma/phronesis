//! Hermetic end-to-end tests for the RETE machinery.
//!
//! These tests prove the RETE engine runs end-to-end using only
//! phronesis's public surface, with no dependency on any host
//! application:
//!
//!   ReteNetwork::new → add_rule → assert_fact → update_agenda →
//!   execute_all_agenda_items → Vec<Action>
//!
//! If any of these break, something in the core engine has regressed
//! — not a host-side integration layer.

use phronesis::{Action, Condition, Fact, ReteNetwork, Rule};

fn greet_rule() -> Rule {
    // When we see fact `greet(?who)`, fire action `wave(?who)`.
    Rule {
        id: "greet_rule".to_string(),
        priority: 10,
        conditions: vec![Condition {
            predicate: "greet".to_string(),
            args: vec!["?who".to_string()],
            script: None,
        }],
        actions: vec![Action {
            action_type: "wave".to_string(),
            params: vec!["?who".to_string()],
        }],
    }
}

fn greet_fact(who: &str, id: &str) -> Fact {
    Fact {
        id: id.to_string(),
        predicate: "greet".to_string(),
        args: vec![who.to_string()],
        timestamp: 0,
    }
}

#[tokio::test]
async fn single_fact_fires_single_rule() {
    let network = ReteNetwork::new();

    network
        .add_rule(greet_rule())
        .await
        .expect("add_rule should succeed");

    network
        .assert_fact(greet_fact("alice", "fact-1"))
        .await
        .expect("assert_fact should succeed");

    network
        .update_agenda()
        .await
        .expect("update_agenda should succeed");

    let actions = network
        .execute_all_agenda_items()
        .expect("execute_all_agenda_items should succeed");

    assert_eq!(actions.len(), 1, "exactly one action should fire");
    assert_eq!(actions[0].action_type, "wave");
    // Variable substitution happened: ?who → "alice".
    assert_eq!(actions[0].params, vec!["alice".to_string()]);
}

#[tokio::test]
async fn multiple_facts_produce_one_action_each() {
    let network = ReteNetwork::new();
    network.add_rule(greet_rule()).await.unwrap();

    for (i, name) in ["alice", "bob", "carol"].iter().enumerate() {
        network
            .assert_fact(greet_fact(name, &format!("fact-{i}")))
            .await
            .unwrap();
    }

    network.update_agenda().await.unwrap();
    let actions = network.execute_all_agenda_items().unwrap();

    assert_eq!(actions.len(), 3);
    let mut names: Vec<_> = actions.iter().map(|a| a.params[0].clone()).collect();
    names.sort();
    assert_eq!(names, vec!["alice", "bob", "carol"]);
}

#[tokio::test]
async fn non_matching_fact_produces_no_action() {
    let network = ReteNetwork::new();
    network.add_rule(greet_rule()).await.unwrap();

    // Predicate mismatch — should not activate greet_rule.
    let fact = Fact {
        id: "fact-unrelated".to_string(),
        predicate: "sneeze".to_string(),
        args: vec!["alice".to_string()],
        timestamp: 0,
    };
    network.assert_fact(fact).await.unwrap();

    network.update_agenda().await.unwrap();
    let actions = network.execute_all_agenda_items().unwrap();

    assert!(
        actions.is_empty(),
        "a non-matching fact must not fire the rule"
    );
}

#[cfg(feature = "embedding-host")]
#[tokio::test]
async fn rule_count_reflects_added_rules() {
    let network = ReteNetwork::new();
    assert_eq!(network.get_rules_count().await.unwrap(), 0);

    network.add_rule(greet_rule()).await.unwrap();
    assert_eq!(network.get_rules_count().await.unwrap(), 1);
}

#[tokio::test]
async fn asserted_fact_is_retrievable_via_wmes() {
    let network = ReteNetwork::new();
    network
        .assert_fact(greet_fact("dave", "fact-d"))
        .await
        .unwrap();

    let wmes = network
        .get_all_wmes()
        .await
        .expect("get_all_wmes should succeed");

    assert_eq!(wmes.len(), 1);
    assert_eq!(wmes[0].fact.predicate, "greet");
    assert_eq!(wmes[0].fact.args, vec!["dave".to_string()]);
}

#[tokio::test]
async fn retracting_a_fact_removes_it_from_wmes() {
    let network = ReteNetwork::new();
    network
        .assert_fact(greet_fact("erin", "fact-e"))
        .await
        .unwrap();

    network
        .retract_fact("fact-e")
        .await
        .expect("retract_fact should succeed");

    let wmes = network.get_all_wmes().await.unwrap();
    assert!(
        wmes.is_empty(),
        "retracted fact should no longer appear in WMEs"
    );
}

#[cfg(test)]
mod token_merge_tests {
    use phronesis::{Bindings, Fact, Token, WorkingMemoryElement};

    #[test]
    fn token_merge_same_variable_different_values() {
        let mut bindings1 = Bindings::new();
        bindings1.add_binding("?x", "a").unwrap();

        let mut bindings2 = Bindings::new();
        bindings2.add_binding("?x", "b").unwrap();

        let result = bindings1.merge(&bindings2);
        assert!(result.is_err());
    }

    #[test]
    fn token_merge_compatible_bindings() {
        let mut bindings1 = Bindings::new();
        bindings1.add_binding("?x", "a").unwrap();

        let mut bindings2 = Bindings::new();
        bindings2.add_binding("?y", "b").unwrap();

        let result = bindings1.merge(&bindings2);
        assert!(result.is_ok());
        let merged = result.unwrap();
        assert_eq!(merged.get_binding("?x"), Some(&"a".to_string()));
        assert_eq!(merged.get_binding("?y"), Some(&"b".to_string()));
    }

    #[test]
    fn token_extend_chain_preserves_parent() {
        let token2 = Token::new_with_bindings(vec![], {
            let mut b = Bindings::new();
            b.add_binding("?x", "a").unwrap();
            b
        });

        let token3 = token2
            .extend_with_binding(
                WorkingMemoryElement::new(Fact {
                    id: "1".to_string(),
                    predicate: "test".to_string(),
                    args: vec![],
                    timestamp: 0,
                }),
                &Bindings::new(),
            )
            .unwrap();

        assert!(token3.parent.is_some());
        assert_eq!(
            token3.parent.as_ref().unwrap().bindings.get_binding("?x"),
            Some(&"a".to_string())
        );
    }
}

#[cfg(test)]
mod token_conditions_match_tests {
    use phronesis::{Bindings, Condition, Fact};

    #[test]
    fn condition_match_all_args() {
        let condition = Condition {
            predicate: "greet".to_string(),
            args: vec!["?who".to_string()],
            script: None,
        };
        let fact = Fact {
            id: "1".to_string(),
            predicate: "greet".to_string(),
            args: vec!["alice".to_string()],
            timestamp: 0,
        };

        let bindings = Bindings::new();
        let result = bindings.can_bind(&condition, &fact);
        assert!(result.is_ok());
    }

    #[test]
    fn condition_match_multiple_args() {
        let condition = Condition {
            predicate: "greet".to_string(),
            args: vec!["?who".to_string(), "?target".to_string()],
            script: None,
        };
        let fact = Fact {
            id: "1".to_string(),
            predicate: "greet".to_string(),
            args: vec!["alice".to_string(), "bob".to_string()],
            timestamp: 0,
        };

        let bindings = Bindings::new();
        let result = bindings.can_bind(&condition, &fact);
        assert!(result.is_ok());
        let bindings = result.unwrap();
        assert!(bindings.get_binding("?who").is_some());
        assert!(bindings.get_binding("?target").is_some());
    }

    #[test]
    fn condition_match_arg_count_mismatch() {
        let condition = Condition {
            predicate: "greet".to_string(),
            args: vec!["?who".to_string()],
            script: None,
        };
        let fact = Fact {
            id: "1".to_string(),
            predicate: "greet".to_string(),
            args: vec!["alice".to_string(), "bob".to_string()],
            timestamp: 0,
        };

        let bindings = Bindings::new();
        let result = bindings.can_bind(&condition, &fact);
        assert!(result.is_err());
    }
}
