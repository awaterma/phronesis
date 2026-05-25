//! Pull-mode adapter tests.
//!
//! Two jobs:
//! 1. Prove `Consequence::from_lookup` / `from_rule_firing` produce the
//!    JSON shapes the contract promises.
//! 2. Prove the adapter fits the shape of a real phronesis spec-046 tool
//!    response (schemaVersion, kind, available, found, camelCase) without
//!    depending on phronesis. If this test ever needs to change, the
//!    contract between episteme and phronesis's `src/tools/*` has moved.

use phronesis::{
    dyn_lookup_as_consequence, lookup_as_consequence, Consequence, ConsequenceKind, DynLookup,
    Lookup, Provenance,
};
use serde::{Deserialize, Serialize};

/// Mirrors the exact wire shape of `phronesis_simple::tools::LookupSpellResponse`
/// (see src/tools/lookup_symbol.rs). We don't import from phronesis — we
/// mirror the shape. If phronesis's tool drifts, this test needs updating,
/// which is the right signal.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FakeSpellResponse {
    schema_version: u8,
    kind: &'static str,
    available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    found: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

struct FakeSpellTool;

impl Lookup for FakeSpellTool {
    type Request = String;
    type Response = FakeSpellResponse;

    fn name(&self) -> &'static str {
        "card"
    }

    fn schema_version(&self) -> u8 {
        1
    }

    fn invoke(&self, req: Self::Request) -> anyhow::Result<Self::Response> {
        Ok(FakeSpellResponse {
            schema_version: 1,
            kind: "card",
            available: false,
            found: None,
            name: Some(req),
            error: Some("not wired yet".into()),
        })
    }
}

#[test]
fn from_lookup_uses_snapshot_kind_and_lookup_provenance() {
    let payload = serde_json::json!({"answer": 42});
    let c = Consequence::from_lookup("card", 1, "card", &payload).unwrap();

    assert_eq!(c.kind, ConsequenceKind::Snapshot);
    assert_eq!(c.predicate, "card");
    match c.provenance {
        Provenance::Lookup {
            tool,
            schema_version,
        } => {
            assert_eq!(tool, "card");
            assert_eq!(schema_version, 1);
        }
        other => panic!("expected Lookup provenance, got {other:?}"),
    }
}

#[test]
fn from_rule_firing_preserves_bound_facts() {
    let payload = serde_json::json!({"value_delta": -3});
    let c = Consequence::from_rule_firing(
        "play.apply_cost",
        "card.played",
        vec!["f1".into(), "f2".into()],
        ConsequenceKind::Event,
        &payload,
    )
    .unwrap();

    assert_eq!(c.kind, ConsequenceKind::Event);
    assert_eq!(c.predicate, "card.played");
    match c.provenance {
        Provenance::RuleFiring {
            rule_id,
            bound_facts,
            ..
        } => {
            assert_eq!(rule_id, "play.apply_cost");
            assert_eq!(bound_facts, vec!["f1", "f2"]);
        }
        other => panic!("expected RuleFiring, got {other:?}"),
    }
}

#[test]
fn lookup_trait_drives_consequence_end_to_end() {
    let tool = FakeSpellTool;
    let c = lookup_as_consequence(&tool, "fireball".into()).unwrap();

    assert_eq!(c.kind, ConsequenceKind::Snapshot);
    assert_eq!(c.predicate, "card");

    // Payload preserves the exact camelCase contract spec 046 guarantees.
    let v = serde_json::to_value(&c).unwrap();
    assert_eq!(v["payload"]["schemaVersion"], 1);
    assert_eq!(v["payload"]["kind"], "card");
    assert_eq!(v["payload"]["available"], false);
    assert_eq!(v["payload"]["name"], "fireball");
    assert_eq!(v["payload"]["error"], "not wired yet");
    // `found` was None, should be omitted.
    assert!(v["payload"].get("found").is_none());
}

#[test]
fn typed_lookup_is_automatically_a_dyn_lookup() {
    // TypeScript analogy: the typed Lookup is a compile-time convenience;
    // at runtime the engine only needs the dynamic shape. The blanket
    // impl lets hosts treat every typed tool uniformly alongside any
    // Rhai/RPC/WASM tools that only speak Value -> Value.
    let tool: Box<dyn DynLookup> = Box::new(FakeSpellTool);
    let req = serde_json::Value::String("fireball".into());
    let resp = tool.invoke_dyn(req).unwrap();

    assert_eq!(resp["kind"], "card");
    assert_eq!(resp["name"], "fireball");
    assert_eq!(resp["available"], false);
    assert_eq!(tool.name(), "card");
    assert_eq!(tool.schema_version(), 1);
}

#[test]
fn dyn_lookup_as_consequence_round_trips_untyped() {
    let tool: Box<dyn DynLookup> = Box::new(FakeSpellTool);
    let c =
        dyn_lookup_as_consequence(tool.as_ref(), serde_json::Value::String("cure".into())).unwrap();

    assert_eq!(c.kind, ConsequenceKind::Snapshot);
    assert_eq!(c.predicate, "card");
    match &c.provenance {
        Provenance::Lookup {
            tool: t,
            schema_version,
        } => {
            assert_eq!(t, "card");
            assert_eq!(*schema_version, 1);
        }
        other => panic!("expected Lookup, got {other:?}"),
    }
    // The payload is preserved unchanged — schema-agnostic core.
    assert_eq!(c.payload["name"], "cure");
    assert_eq!(c.payload["available"], false);
}

/// A purely dynamic tool — the thing a Rhai or WASM host would produce.
/// It doesn't implement `Lookup`; it implements `DynLookup` directly.
/// This proves the engine can host tools whose shapes aren't known to
/// the Rust compiler.
struct DynamicEchoTool;

impl DynLookup for DynamicEchoTool {
    fn name(&self) -> &'static str {
        "echo"
    }

    fn schema_version(&self) -> u8 {
        1
    }

    fn invoke_dyn(&self, req: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        Ok(serde_json::json!({
            "schemaVersion": 1,
            "kind": "echo",
            "available": true,
            "found": true,
            "echoed": req,
        }))
    }
}

#[test]
fn purely_dynamic_tool_produces_a_consequence() {
    let tool = DynamicEchoTool;
    let c = dyn_lookup_as_consequence(&tool, serde_json::json!({"msg": "hi"})).unwrap();

    assert_eq!(c.predicate, "echo");
    assert_eq!(c.payload["echoed"]["msg"], "hi");
    assert_eq!(c.payload["available"], true);
}

#[test]
fn unavailable_lookup_still_produces_a_consequence() {
    // Philosophical test: "tool not wired yet" is a real consequence
    // the actor should reason from, not an error to suppress.
    let tool = FakeSpellTool;
    let c = lookup_as_consequence(&tool, "anything".into()).unwrap();
    let v = serde_json::to_value(&c).unwrap();
    assert_eq!(v["payload"]["available"], false);
    assert_eq!(v["kind"], "snapshot");
    assert_eq!(v["provenance"]["source"], "lookup");
}
