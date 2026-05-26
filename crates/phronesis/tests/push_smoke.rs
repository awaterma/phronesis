//! End-to-end tests for the push-mode path inside phronesis.
//!
//! Validates the full chain without any host-application dependency:
//!
//!   assert_fact  →  rule fires  →  Vec<Action>
//!                →  rule_firing_to_consequences  →  Vec<Consequence>
//!                →  Actor::act                   →  ActorOutput
//!
//! Before this test file, the two halves of the crate — the RETE
//! engine and the Consequence/Actor surface — had no single test
//! exercising them together. Proving the chain inside the phronesis
//! crate itself (rather than from a downstream host's integration
//! tests) keeps the sandbox loop tight and avoids dragging in
//! host-specific dependencies.

use async_trait::async_trait;
use phronesis::{
    Action, Actor, ActorOutput, Condition, Consequence, ConsequenceKind, Fact, Provenance,
    ReteNetwork, Rule, rule_firing_to_consequences,
};

/// An Actor that just echoes predicates joined with `, ` — the
/// simplest possible end-to-end verifier.
struct PredicateJoiner;

#[async_trait]
impl Actor for PredicateJoiner {
    async fn act(&self, consequences: &[Consequence]) -> anyhow::Result<ActorOutput> {
        let joined = consequences
            .iter()
            .map(|c| c.predicate.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        Ok(ActorOutput::Text(joined))
    }
}

fn wave_rule() -> Rule {
    Rule {
        id: "wave_rule".to_string(),
        priority: 10,
        conditions: vec![Condition {
            predicate: "greet".to_string(),
            args: vec!["?who".to_string()],
            script: None,
        }],
        actions: vec![Action {
            action_type: "wave_at".to_string(),
            params: vec!["?who".to_string()],
        }],
    }
}

#[tokio::test]
async fn full_push_loop_produces_actor_text() {
    let network = ReteNetwork::new();
    network.add_rule(wave_rule()).await.unwrap();

    let fact = Fact {
        id: "fact-alice".to_string(),
        predicate: "greet".to_string(),
        args: vec!["alice".to_string()],
        timestamp: 0,
    };
    network.assert_fact(fact).await.unwrap();
    network.update_agenda().await.unwrap();
    let actions = network.execute_all_agenda_items().unwrap();

    // Hand-wired provenance — in a real integration the host would
    // thread rule_id + bound_facts through from the agenda item.
    // Until ReteNetwork exposes an execute-with-provenance method,
    // the caller is responsible for providing them.
    let consequences = rule_firing_to_consequences(
        "wave_rule",
        &["fact-alice".to_string()],
        ConsequenceKind::Event,
        actions,
    );

    assert_eq!(consequences.len(), 1);
    assert_eq!(consequences[0].predicate, "wave_at");

    let actor = PredicateJoiner;
    match actor.act(&consequences).await.unwrap() {
        ActorOutput::Text(s) => assert_eq!(s, "wave_at"),
        other => panic!("expected Text, got {other:?}"),
    }
}

#[tokio::test]
async fn push_consequences_carry_provenance_with_bound_facts() {
    let actions = vec![Action {
        action_type: "wave_at".to_string(),
        params: vec!["alice".to_string()],
    }];

    let consequences = rule_firing_to_consequences(
        "wave_rule",
        &["fact-alice".to_string(), "fact-context".to_string()],
        ConsequenceKind::Event,
        actions,
    );

    assert_eq!(consequences.len(), 1);
    match &consequences[0].provenance {
        Provenance::RuleFiring {
            rule_id,
            bound_facts,
            ..
        } => {
            assert_eq!(rule_id, "wave_rule");
            assert_eq!(
                bound_facts,
                &vec!["fact-alice".to_string(), "fact-context".to_string()]
            );
        }
        other => panic!("expected RuleFiring, got {other:?}"),
    }
}

#[tokio::test]
async fn payload_preserves_action_type_and_params() {
    let actions = vec![Action {
        action_type: "play_card".to_string(),
        params: vec!["ace_spades".to_string(), "face_up".to_string()],
    }];

    let consequences =
        rule_firing_to_consequences("play_rule", &[], ConsequenceKind::Event, actions);
    let payload = &consequences[0].payload;

    assert_eq!(payload["action_type"], "play_card");
    assert_eq!(payload["params"][0], "ace_spades");
    assert_eq!(payload["params"][1], "face_up");
}

#[tokio::test]
async fn push_with_no_actions_produces_no_consequences() {
    let consequences =
        rule_firing_to_consequences("a_rule", &["f1".into()], ConsequenceKind::Event, vec![]);
    assert!(consequences.is_empty());
}

#[tokio::test]
async fn push_honors_caller_supplied_kind() {
    let actions = vec![Action {
        action_type: "offer_choice".to_string(),
        params: vec!["draw".into(), "discard".into()],
    }];

    // A rule that fires to *offer* choices to the actor should emit
    // Affordances, not Events. The helper honors the caller's choice.
    let c = rule_firing_to_consequences("choice_rule", &[], ConsequenceKind::Affordance, actions);
    assert_eq!(c[0].kind, ConsequenceKind::Affordance);
}
