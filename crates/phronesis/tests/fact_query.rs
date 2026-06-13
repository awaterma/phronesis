//! Public fact-query API over working memory (v0.11).
//!
//! These queries exist so embedding hosts never need to reach into
//! `ReteNetwork::wme_manager` directly. Shapes mirror a real consumer's
//! call-site inventory: snapshot-all, predicate filter, positional-arg
//! filters, id collection for batch retraction, by-id get.

use phronesis::engine_types::Fact;
use phronesis::network::ReteNetwork;

fn fact(id: &str, pred: &str, args: &[&str]) -> Fact {
    Fact {
        id: id.into(),
        predicate: pred.into(),
        args: args.iter().map(|s| s.to_string()).collect(),
        timestamp: 0,
    }
}

async fn seeded() -> ReteNetwork {
    let net = ReteNetwork::new();
    for f in [
        fact("e1", "equipped", &["alice", "head", "helm"]),
        fact("e2", "equipped", &["alice", "hand", "sword"]),
        fact("e3", "equipped", &["bob", "head", "cap"]),
        fact("g1", "gold", &["alice", "30"]),
    ] {
        net.assert_fact(f).await.expect("seed assert");
    }
    net
}

#[tokio::test]
async fn snapshot_returns_all_facts_sorted_by_id() {
    let net = seeded().await;
    let all = net.facts_snapshot().expect("snapshot");
    assert_eq!(all.len(), 4);
    let ids: Vec<&str> = all.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids, vec!["e1", "e2", "e3", "g1"], "deterministic order");
}

#[tokio::test]
async fn matching_predicate_returns_only_that_predicate() {
    let net = seeded().await;
    let equipped = net.facts_matching_predicate("equipped").expect("query");
    assert_eq!(equipped.len(), 3);
    assert!(equipped.iter().all(|f| f.predicate == "equipped"));

    let none = net.facts_matching_predicate("no_such_pred").expect("query");
    assert!(none.is_empty());
}

#[tokio::test]
async fn matching_with_positional_filters() {
    let net = seeded().await;

    // predicate + args[0] (the common "scoped to an entity id" shape)
    let alice = net
        .facts_matching("equipped", &[(0, "alice")])
        .expect("query");
    assert_eq!(alice.len(), 2);

    // predicate + args[0] + args[1] (compound key)
    let head = net
        .facts_matching("equipped", &[(0, "alice"), (1, "head")])
        .expect("query");
    assert_eq!(head.len(), 1);
    assert_eq!(head[0].args[2], "helm");

    // sparse filter: skip args[1], constrain args[2]
    let sword = net
        .facts_matching("equipped", &[(0, "alice"), (2, "sword")])
        .expect("query");
    assert_eq!(sword.len(), 1);
    assert_eq!(sword[0].id, "e2");

    // out-of-range arg index never matches
    let oob = net.facts_matching("gold", &[(5, "x")]).expect("query");
    assert!(oob.is_empty());

    // empty filter list behaves like facts_matching_predicate
    let all_equipped = net.facts_matching("equipped", &[]).expect("query");
    assert_eq!(all_equipped.len(), 3);
}

#[tokio::test]
async fn get_fact_by_id_and_count() {
    let net = seeded().await;
    let e1 = net.get_fact_by_id("e1").expect("query");
    assert_eq!(e1.expect("e1 exists").args, vec!["alice", "head", "helm"]);
    assert!(net.get_fact_by_id("nope").expect("query").is_none());
    assert_eq!(net.fact_count().expect("count"), 4);
}

#[tokio::test]
async fn fact_ids_matching_supports_batch_retract() {
    let net = seeded().await;
    let ids = net
        .fact_ids_matching("equipped", &[(0, "alice")])
        .expect("query");
    assert_eq!(ids.len(), 2);
    for id in ids {
        net.retract_fact(&id).await.expect("retract");
    }
    assert_eq!(net.fact_count().expect("count"), 2);
    assert!(
        net.facts_matching("equipped", &[(0, "alice")])
            .expect("query")
            .is_empty()
    );
}

#[tokio::test]
async fn duplicate_fact_id_is_rejected() {
    let net = seeded().await;
    let err = net
        .assert_fact(fact("e1", "equipped", &["mallory", "head", "crown"]))
        .await
        .expect_err("duplicate id must be rejected");
    assert!(err.to_string().contains("e1"), "error names the id: {err}");

    // Original fact is unchanged.
    let e1 = net.get_fact_by_id("e1").expect("query").expect("e1 exists");
    assert_eq!(e1.args, vec!["alice", "head", "helm"]);
    assert_eq!(net.fact_count().expect("count"), 4);
}

#[tokio::test]
async fn duplicate_assert_does_not_corrupt_predicate_index() {
    let net = seeded().await;
    let _ = net
        .assert_fact(fact("e1", "equipped", &["mallory", "head", "crown"]))
        .await;
    // Before the fix, the rejected duplicate still pushed its id into the
    // predicate index, so e1 came back twice here.
    let equipped = net.facts_matching_predicate("equipped").expect("query");
    assert_eq!(equipped.len(), 3);
    let e1_hits = equipped.iter().filter(|f| f.id == "e1").count();
    assert_eq!(e1_hits, 1, "e1 must appear exactly once");
}

#[tokio::test]
async fn identical_reassert_is_idempotent_noop() {
    let net = seeded().await;
    net.assert_fact(fact("e1", "equipped", &["alice", "head", "helm"]))
        .await
        .expect("identical re-assert is a no-op, not an error");
    assert_eq!(net.fact_count().expect("count"), 4);
    let equipped = net.facts_matching_predicate("equipped").expect("query");
    assert_eq!(
        equipped.iter().filter(|f| f.id == "e1").count(),
        1,
        "no duplicate index entry from the no-op"
    );
}

#[tokio::test]
async fn reassert_after_retract_succeeds() {
    let net = seeded().await;
    net.retract_fact("e1").await.expect("retract");
    net.assert_fact(fact("e1", "equipped", &["alice", "head", "circlet"]))
        .await
        .expect("re-assert after retract is legal");
    let e1 = net.get_fact_by_id("e1").expect("query").expect("e1 exists");
    assert_eq!(e1.args[2], "circlet");
}
