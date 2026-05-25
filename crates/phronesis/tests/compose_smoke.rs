//! Comanaosition tests: a rule fires, its action invokes a registered
//! tool, and the result lands as a Consequence with provenance
//! recording both the rule and the tool.
//!
//! This is the "push + pull meeting at one point" surface — rules
//! whose actions are tool invocations. See `phronesis/src/compose.rs`
//! for the thesis.

use phronesis::{
    invoke_rule_driven_lookups, try_invoke_rule_driven_lookups, Action, ConsequenceKind, DynLookup,
    LookupRegistry, Provenance,
};

/// A toy pull-mode tool. Given a opponent id as input, returns a
/// value-block payload. Deterministic, sync, no network. Exactly the
/// shape a phronesis spec-046 tool would take after wrapping.
struct LookupCard;

impl DynLookup for LookupCard {
    fn name(&self) -> &'static str {
        "lookup_opponent"
    }

    fn schema_version(&self) -> u8 {
        1
    }

    fn invoke_dyn(&self, req: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        // req is a Value::Array of Value::String per the compose
        // convention; first element is the opponent id.
        let id = req
            .as_array()
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let (value, ac) = match id {
            "goblin" => (7, 15),
            "orc" => (15, 13),
            _ => (1, 10),
        };

        Ok(serde_json::json!({
            "schemaVersion": 1,
            "kind": "opponent",
            "available": true,
            "found": id == "goblin" || id == "orc",
            "id": id,
            "value": value,
            "ac": ac,
        }))
    }
}

fn registry_with_opponent() -> LookupRegistry {
    let mut r = LookupRegistry::new();
    r.register(LookupCard);
    r
}

#[test]
fn rule_firing_tool_action_produces_rule_driven_lookup_consequence() {
    // A rule that fires when an opponent is spotted and whose action is
    // to look the opponent up.
    let actions = vec![Action {
        action_type: "lookup_opponent".to_string(),
        params: vec!["goblin".to_string()],
    }];

    let (consequences, remaining) = invoke_rule_driven_lookups(
        "opponent_appeared_rule",
        &["fact-opponent-spotted".to_string()],
        actions,
        &registry_with_opponent(),
    );

    // Tool resolved → one consequence, no remaining actions.
    assert_eq!(consequences.len(), 1);
    assert!(remaining.is_empty());

    let c = &consequences[0];
    assert_eq!(c.kind, ConsequenceKind::Snapshot);
    assert_eq!(c.predicate, "lookup_opponent");
    assert_eq!(c.payload["value"], 7);
    assert_eq!(c.payload["ac"], 15);

    // The load-bearing bit: provenance carries BOTH layers.
    match &c.provenance {
        Provenance::RuleDrivenLookup {
            rule_id,
            bound_facts,
            tool,
            schema_version,
            ..
        } => {
            assert_eq!(rule_id, "opponent_appeared_rule");
            assert_eq!(bound_facts, &vec!["fact-opponent-spotted".to_string()]);
            assert_eq!(tool, "lookup_opponent");
            assert_eq!(*schema_version, 1);
        }
        other => panic!("expected RuleDrivenLookup, got {other:?}"),
    }
}

#[test]
fn unregistered_action_passes_through() {
    // An action whose type isn't a registered tool should be returned
    // in `remaining` so the host handles it normally (via push adapter
    // or direct execution).
    let actions = vec![Action {
        action_type: "apply_cost".to_string(),
        params: vec!["player".into(), "5".into()],
    }];

    let (consequences, remaining) =
        invoke_rule_driven_lookups("cost_rule", &[], actions, &registry_with_opponent());

    assert!(consequences.is_empty());
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].action_type, "apply_cost");
}

#[test]
fn mixed_actions_split_correctly() {
    // A rule whose firing produces both a tool invocation and a
    // regular action (e.g. "log that the opponent appeared AND fetch
    // its values").
    let actions = vec![
        Action {
            action_type: "log_event".to_string(),
            params: vec!["opponent_spotted".into()],
        },
        Action {
            action_type: "lookup_opponent".to_string(),
            params: vec!["orc".into()],
        },
        Action {
            action_type: "play_sound".to_string(),
            params: vec!["growl.wav".into()],
        },
    ];

    let (consequences, remaining) = invoke_rule_driven_lookups(
        "opponent_appeared_rule",
        &["fact-1".into()],
        actions,
        &registry_with_opponent(),
    );

    assert_eq!(consequences.len(), 1, "one tool invocation");
    assert_eq!(remaining.len(), 2, "two non-tool actions pass through");
    assert_eq!(consequences[0].payload["id"], "orc");
    assert_eq!(consequences[0].payload["value"], 15);

    let remaining_types: Vec<&str> = remaining.iter().map(|a| a.action_type.as_str()).collect();
    assert_eq!(remaining_types, vec!["log_event", "play_sound"]);
}

#[test]
fn tool_returning_unavailable_still_produces_consequence() {
    // The pull-mode semantics hold: `available: false` is a real
    // consequence, not an error. The tool returned OK, so the
    // consequence is emitted.
    struct UnavailableTool;
    impl DynLookup for UnavailableTool {
        fn name(&self) -> &'static str {
            "stub_tool"
        }
        fn schema_version(&self) -> u8 {
            1
        }
        fn invoke_dyn(&self, _: serde_json::Value) -> anyhow::Result<serde_json::Value> {
            Ok(serde_json::json!({"available": false, "error": "not wired"}))
        }
    }

    let mut registry = LookupRegistry::new();
    registry.register(UnavailableTool);

    let actions = vec![Action {
        action_type: "stub_tool".to_string(),
        params: vec![],
    }];

    let (consequences, remaining) = invoke_rule_driven_lookups("any_rule", &[], actions, &registry);

    assert_eq!(consequences.len(), 1);
    assert!(remaining.is_empty());
    assert_eq!(consequences[0].payload["available"], false);
}

#[test]
fn tool_erroring_falls_back_to_remaining() {
    // If the tool's invoke_dyn returns Err, the action should fall
    // through to the remaining set — not silently drop. Errors on
    // registered tools are real errors; the host deserves to see the
    // action and decide what to do.
    struct ErroringTool;
    impl DynLookup for ErroringTool {
        fn name(&self) -> &'static str {
            "bad_tool"
        }
        fn schema_version(&self) -> u8 {
            1
        }
        fn invoke_dyn(&self, _: serde_json::Value) -> anyhow::Result<serde_json::Value> {
            anyhow::bail!("upstream data source died")
        }
    }

    let mut registry = LookupRegistry::new();
    registry.register(ErroringTool);

    let actions = vec![Action {
        action_type: "bad_tool".to_string(),
        params: vec!["anything".into()],
    }];

    let (consequences, remaining) = invoke_rule_driven_lookups("any_rule", &[], actions, &registry);

    assert!(consequences.is_empty());
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].action_type, "bad_tool");
}

#[test]
fn registry_supports_replacement_for_test_mocks() {
    // re-registering under the same name should swap the tool — lets
    // tests substitute a mock.
    let mut registry = LookupRegistry::new();
    registry.register(LookupCard);
    assert!(registry.contains("lookup_opponent"));

    // Replace with a tool that returns a different shape.
    struct MockCard;
    impl DynLookup for MockCard {
        fn name(&self) -> &'static str {
            "lookup_opponent"
        }
        fn schema_version(&self) -> u8 {
            99
        }
        fn invoke_dyn(&self, _: serde_json::Value) -> anyhow::Result<serde_json::Value> {
            Ok(serde_json::json!({"mocked": true}))
        }
    }
    registry.register(MockCard);

    let (consequences, _) = invoke_rule_driven_lookups(
        "r",
        &[],
        vec![Action {
            action_type: "lookup_opponent".into(),
            params: vec![],
        }],
        &registry,
    );

    assert_eq!(consequences[0].payload["mocked"], true);
    match &consequences[0].provenance {
        Provenance::RuleDrivenLookup { schema_version, .. } => {
            assert_eq!(*schema_version, 99);
        }
        _ => panic!("expected RuleDrivenLookup"),
    }
}

// -------------------------------------------------------------------
// `try_invoke_rule_driven_lookups` — fail-fast variant.
// -------------------------------------------------------------------

#[test]
fn try_invoke_succeeds_when_all_tools_succeed() {
    let actions = vec![Action {
        action_type: "lookup_opponent".to_string(),
        params: vec!["goblin".to_string()],
    }];

    let (consequences, remaining) = try_invoke_rule_driven_lookups(
        "opponent_appeared_rule",
        &["fact-1".to_string()],
        actions,
        &registry_with_opponent(),
    )
    .expect("all tools succeed");

    assert_eq!(consequences.len(), 1);
    assert!(remaining.is_empty());
    assert_eq!(consequences[0].payload["value"], 7);
}

#[test]
fn try_invoke_returns_error_when_tool_fails() {
    struct ErroringTool;
    impl DynLookup for ErroringTool {
        fn name(&self) -> &'static str {
            "bad_tool"
        }
        fn schema_version(&self) -> u8 {
            7
        }
        fn invoke_dyn(&self, _: serde_json::Value) -> anyhow::Result<serde_json::Value> {
            anyhow::bail!("upstream data source died")
        }
    }

    let mut registry = LookupRegistry::new();
    registry.register(ErroringTool);

    let action = Action {
        action_type: "bad_tool".to_string(),
        params: vec!["x".into()],
    };
    let err = try_invoke_rule_driven_lookups(
        "any_rule",
        &["fact-a".to_string()],
        vec![action.clone()],
        &registry,
    )
    .expect_err("strict variant must surface tool errors");

    assert_eq!(err.rule_id, "any_rule");
    assert_eq!(err.tool, "bad_tool");
    assert_eq!(err.schema_version, 7);
    assert_eq!(err.action, action);
    assert!(err.source.to_string().contains("upstream data source died"));
    assert!(err.to_string().contains("bad_tool"));
}

#[test]
fn try_invoke_passes_through_unregistered_actions() {
    // Unregistered action types are not errors, just like in the
    // lenient variant — they pass through to `remaining`.
    let actions = vec![
        Action {
            action_type: "log_event".to_string(),
            params: vec!["opponent_spotted".into()],
        },
        Action {
            action_type: "lookup_opponent".to_string(),
            params: vec!["goblin".into()],
        },
    ];

    let (consequences, remaining) =
        try_invoke_rule_driven_lookups("r", &[], actions, &registry_with_opponent())
            .expect("no tool errored");

    assert_eq!(consequences.len(), 1);
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].action_type, "log_event");
}

#[test]
fn try_invoke_aborts_on_first_failure() {
    // When mixed tools include one that errors, the loop aborts at
    // that point. Two contracts the strict variant must hold:
    //
    //   (a) The function returns `Err` — pre-failure consequences are
    //       NOT smuggled out as a partial `Ok`. Hosts on the strict
    //       path are explicitly asking for "no fallback"; partial output
    //       would be a quiet fallback.
    //   (b) Post-failure actions are NOT invoked. The loop aborts at
    //       the first error, regardless of what comes after.
    //
    // We instrument both tools with invocation counters so we can pin
    // (b) directly rather than inferring it from the error field.
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct OkTool {
        calls: Arc<AtomicUsize>,
    }
    impl DynLookup for OkTool {
        fn name(&self) -> &'static str {
            "ok_tool"
        }
        fn schema_version(&self) -> u8 {
            1
        }
        fn invoke_dyn(&self, _: serde_json::Value) -> anyhow::Result<serde_json::Value> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(serde_json::json!({"ok": true}))
        }
    }
    struct ErrTool {
        calls: Arc<AtomicUsize>,
    }
    impl DynLookup for ErrTool {
        fn name(&self) -> &'static str {
            "err_tool"
        }
        fn schema_version(&self) -> u8 {
            1
        }
        fn invoke_dyn(&self, _: serde_json::Value) -> anyhow::Result<serde_json::Value> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("boom")
        }
    }

    let ok_calls = Arc::new(AtomicUsize::new(0));
    let err_calls = Arc::new(AtomicUsize::new(0));

    let mut registry = LookupRegistry::new();
    registry.register(OkTool {
        calls: Arc::clone(&ok_calls),
    });
    registry.register(ErrTool {
        calls: Arc::clone(&err_calls),
    });

    let actions = vec![
        Action {
            action_type: "ok_tool".to_string(),
            params: vec![],
        },
        Action {
            action_type: "err_tool".to_string(),
            params: vec![],
        },
        Action {
            action_type: "ok_tool".to_string(),
            params: vec![],
        },
    ];

    let result = try_invoke_rule_driven_lookups("r", &[], actions, &registry);

    // Contract (a): no partial Ok. We explicitly pattern-match to keep
    // the panic message useful if this regresses to leaking pre-failure
    // consequences out as `Ok((vec![consequence_from_ok_tool], _))`.
    let err = match result {
        Err(e) => e,
        Ok((consequences, remaining)) => panic!(
            "expected Err — got Ok with {} consequence(s) and {} remaining action(s); \
             pre-failure consequences must be discarded under the strict variant",
            consequences.len(),
            remaining.len()
        ),
    };
    assert_eq!(err.tool, "err_tool");

    // Contract (b): the third action (ok_tool again) was never invoked.
    assert_eq!(
        ok_calls.load(Ordering::SeqCst),
        1,
        "ok_tool should run exactly once — the post-failure action must be skipped"
    );
    assert_eq!(err_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn provenance_serializes_with_expected_source_tag() {
    // Wire contract: RuleDrivenLookup has source = "rule_driven_lookup".
    // Evaluation harnesses and sheet's swift-bridge will filter by this.
    let prov = Provenance::RuleDrivenLookup {
        rule_id: "r".into(),
        bound_facts: vec!["f1".into()],
        bindings: Default::default(),
        tool: "t".into(),
        schema_version: 1,
    };
    let v = serde_json::to_value(&prov).unwrap();
    assert_eq!(v["source"], "rule_driven_lookup");
    assert_eq!(v["rule_id"], "r");
    assert_eq!(v["bound_facts"][0], "f1");
    assert_eq!(v["tool"], "t");
    assert_eq!(v["schema_version"], 1);
}
