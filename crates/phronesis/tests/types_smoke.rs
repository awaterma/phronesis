//! Type-rank smoke tests for the episteme surface.
//!
//! These run without any phronesis deps (no rodio, no alsa, no tokio full
//! runtime) so they can validate the abstraction in constrained sandboxes.
//!
//! What we're checking:
//! 1. The JSON shape of `Consequence` is stable and readable — this is
//!    the contract every host (phronesis push, phronesis pull, future sheet
//!    FFI, future conversational-commitments) will rely on.
//! 2. `Provenance` variants serialize with the `source` tag we documented.
//! 3. The `Actor` trait is object-safe and async — we can build an
//!    `Arc<dyn Actor>` in callers.
//! 4. A trivial canned actor compiles and runs end-to-end given a
//!    consequence slice.
//!
//! If any of these break, we broke the contract, not just the tests.

use std::sync::Arc;

use async_trait::async_trait;
use phronesis::{Actor, ActorOutput, Consequence, ConsequenceKind, Provenance};
use serde_json::json;

fn samanale_rule_firing() -> Consequence {
    Consequence {
        kind: ConsequenceKind::Event,
        predicate: "card.played".to_string(),
        payload: json!({ "player": "zoran", "value_delta": -3, "value_remaining": 6 }),
        provenance: Provenance::RuleFiring {
            rule_id: "play.apply_cost".to_string(),
            bound_facts: vec!["fact-42".to_string(), "fact-7".to_string()],
            bindings: Default::default(),
        },
    }
}

fn samanale_lookup() -> Consequence {
    Consequence {
        kind: ConsequenceKind::Snapshot,
        predicate: "lookup_symbol".to_string(),
        payload: json!({
            "name": "magic_missile",
            "rank": 1,
            "cost": "1d4+1 force",
        }),
        provenance: Provenance::Lookup {
            tool: "lookup_symbol".to_string(),
            schema_version: 1,
        },
    }
}

#[test]
fn consequence_serializes_with_expected_shape() {
    let json_value = serde_json::to_value(samanale_rule_firing()).expect("serialize");

    assert_eq!(json_value["kind"], "event");
    assert_eq!(json_value["predicate"], "card.played");
    assert_eq!(json_value["payload"]["value_delta"], -3);
    assert_eq!(json_value["provenance"]["source"], "rule_firing");
    assert_eq!(json_value["provenance"]["rule_id"], "play.apply_cost");
    assert_eq!(
        json_value["provenance"]["bound_facts"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn consequence_roundtrips_through_json() {
    let original = samanale_rule_firing();
    let encoded = serde_json::to_string(&original).expect("encode");
    let decoded: Consequence = serde_json::from_str(&encoded).expect("decode");

    assert_eq!(decoded.kind, original.kind);
    assert_eq!(decoded.predicate, original.predicate);
    assert_eq!(decoded.payload, original.payload);
    match decoded.provenance {
        Provenance::RuleFiring {
            rule_id,
            bound_facts,
            ..
        } => {
            assert_eq!(rule_id, "play.apply_cost");
            assert_eq!(bound_facts, vec!["fact-42", "fact-7"]);
        }
        other => panic!("expected RuleFiring, got {other:?}"),
    }
}

#[test]
fn provenance_tag_is_stable_across_variants() {
    let rf = serde_json::to_value(Provenance::RuleFiring {
        rule_id: "r".into(),
        bound_facts: vec![],
        bindings: Default::default(),
    })
    .unwrap();
    let lu = serde_json::to_value(Provenance::Lookup {
        tool: "t".into(),
        schema_version: 1,
    })
    .unwrap();
    let a = serde_json::to_value(Provenance::Asserted { by: "boot".into() }).unwrap();

    assert_eq!(rf["source"], "rule_firing");
    assert_eq!(lu["source"], "lookup");
    assert_eq!(a["source"], "asserted");
}

#[test]
fn consequence_kind_round_trips() {
    for kind in [
        ConsequenceKind::Event,
        ConsequenceKind::Snapshot,
        ConsequenceKind::Constraint,
        ConsequenceKind::Affordance,
    ] {
        let s = serde_json::to_string(&kind).unwrap();
        let back: ConsequenceKind = serde_json::from_str(&s).unwrap();
        assert_eq!(kind, back);
    }
}

#[test]
fn lookup_consequence_serializes_with_schema_version() {
    let v = serde_json::to_value(samanale_lookup()).unwrap();
    assert_eq!(v["provenance"]["source"], "lookup");
    assert_eq!(v["provenance"]["tool"], "lookup_symbol");
    assert_eq!(v["provenance"]["schema_version"], 1);
}

/// A trivial actor that joins every consequence's predicate into a single
/// string. Proves the trait is implementable and object-safe without any
/// LLM plumbing.
#[derive(Default)]
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

#[tokio::test]
async fn actor_trait_is_implementable_and_object_safe() {
    let actor: Arc<dyn Actor> = Arc::new(PredicateJoiner);
    let consequences = vec![samanale_rule_firing(), samanale_lookup()];
    let out = actor.act(&consequences).await.unwrap();
    match out {
        ActorOutput::Text(s) => {
            assert_eq!(s, "card.played, lookup_symbol");
        }
        other => panic!("expected Text, got {other:?}"),
    }
}
