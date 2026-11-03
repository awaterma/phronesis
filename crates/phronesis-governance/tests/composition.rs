//! Tests for pure rule-layer composition: ordering, override recording,
//! opaque source IDs, empty inputs, override-fact shape, and the
//! pack-before-project shadowing pattern the MCP resolver relies on.

use std::collections::HashMap;

use phronesis_governance::composition::{
    ComposedPolicy, LoadedLayer, OVERRIDE_PREDICATE, RuleOrigin, RuleOverride, compose,
    override_facts,
};
use phronesis_governance::policy::{DiskAction, DiskCondition, DiskRule};

fn rule(id: &str, priority: i32) -> DiskRule {
    DiskRule {
        id: id.to_string(),
        phase: "pre".to_string(),
        priority,
        conditions: vec![DiskCondition {
            predicate: "file_path_matches".to_string(),
            args: vec!["src/".to_string()],
            script: None,
        }],
        actions: vec![DiskAction {
            action_type: "block".to_string(),
            params: vec!["test".to_string()],
            data: None,
        }],
        silent: None,
        audit: None,
        doc_excepted: None,
    }
}

fn layer(name: &str, source_id: &str, decision: Option<&str>, rules: Vec<DiskRule>) -> LoadedLayer {
    LoadedLayer {
        origin: RuleOrigin {
            layer: name.to_string(),
            source_id: source_id.to_string(),
            decision: decision.map(str::to_string),
        },
        rules,
    }
}

fn origin_of(layer: &LoadedLayer) -> RuleOrigin {
    layer.origin.clone()
}

#[test]
fn later_layer_wins_at_original_slot() {
    let base = layer("base", ".phronesis/rules.json", None, vec![rule("same", 1)]);
    let user = layer(
        "user",
        "personal/rules.json",
        Some("ADR-1"),
        vec![rule("same", 9)],
    );

    let composed = compose(&[base, user]);

    assert_eq!(composed.rules.len(), 1);
    assert_eq!(composed.rules[0].id, "same");
    assert_eq!(composed.rules[0].priority, 9, "winner's rule content");
    let user_origin = RuleOrigin {
        layer: "user".to_string(),
        source_id: "personal/rules.json".to_string(),
        decision: Some("ADR-1".to_string()),
    };
    assert_eq!(composed.origins.get("same"), Some(&user_origin));
    assert_eq!(composed.overrides.len(), 1);
    assert_eq!(composed.overrides[0].rule_id, "same");
    assert_eq!(
        composed.overrides[0].overridden.layer, "base",
        "overridden origin is the previously winning layer"
    );
    assert_eq!(composed.overrides[0].winner, user_origin);
}

#[test]
fn distinct_rules_keep_first_seen_order() {
    let first = layer("one", "a.json", None, vec![rule("a", 1), rule("b", 2)]);
    let second = layer("two", "b.json", None, vec![rule("c", 3), rule("a", 10)]);

    let composed = compose(&[first, second]);

    let ids: Vec<&str> = composed.rules.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["a", "b", "c"],
        "a keeps its original slot even though layer two re-declares it first"
    );
    assert_eq!(composed.rules[0].priority, 10, "replacement content");
    assert_eq!(composed.rules[1].priority, 2);
    assert_eq!(composed.rules[2].priority, 3);
    assert_eq!(composed.overrides.len(), 1);
    assert_eq!(composed.overrides[0].rule_id, "a");
}

#[test]
fn every_successive_override_is_recorded() {
    let one = layer("one", "1.json", None, vec![rule("dup", 1)]);
    let two = layer("two", "2.json", None, vec![rule("dup", 2)]);
    let three = layer("three", "3.json", None, vec![rule("dup", 3)]);

    let composed = compose(&[one, two, three]);

    assert_eq!(composed.overrides.len(), 2, "both replacements recorded");
    assert_eq!(composed.overrides[0].overridden.layer, "one");
    assert_eq!(composed.overrides[0].winner.layer, "two");
    assert_eq!(composed.overrides[1].overridden.layer, "two");
    assert_eq!(composed.overrides[1].winner.layer, "three");
    assert_eq!(composed.rules.len(), 1);
    assert_eq!(composed.rules[0].priority, 3, "last winner's content");
    let expected = RuleOrigin {
        layer: "three".to_string(),
        source_id: "3.json".to_string(),
        decision: None,
    };
    assert_eq!(composed.origins.get("dup"), Some(&expected));
}

#[test]
fn identical_cross_layer_rules_still_override() {
    let shared = || rule("twin", 5);
    let one = layer("one", "1.json", None, vec![shared()]);
    let two = layer("two", "2.json", None, vec![shared()]);

    let composed = compose(&[one, two]);

    assert_eq!(
        composed.overrides.len(),
        1,
        "content equality is never consulted"
    );
    assert_eq!(composed.rules.len(), 1);
    assert_eq!(composed.rules[0], rule("twin", 5));
}

#[test]
fn opaque_source_ids_flow_through_untouched() {
    let pack = layer(
        "rust-pack",
        "pack:rust@0.40.0|sha256:abcdef",
        None,
        vec![rule("no-unwrap-in-src", 10)],
    );
    let project = layer(
        "project",
        "mem://workspace/team/policy#section=rules v2",
        Some("ADR-shadowing"),
        vec![rule("no-unwrap-in-src", 20)],
    );

    let composed = compose(&[pack, project]);

    assert_eq!(composed.overrides.len(), 1);
    let override_item = &composed.overrides[0];
    assert_eq!(
        override_item.overridden.source_id,
        "pack:rust@0.40.0|sha256:abcdef"
    );
    assert_eq!(
        override_item.winner.source_id,
        "mem://workspace/team/policy#section=rules v2"
    );

    let facts = override_facts(&composed.overrides);
    assert_eq!(facts[0].args[2], "pack:rust@0.40.0|sha256:abcdef");
    assert_eq!(
        facts[0].args[4],
        "mem://workspace/team/policy#section=rules v2"
    );
}

#[test]
fn empty_inputs_compose_to_empty() {
    let empty: ComposedPolicy = compose(&[]);
    assert!(empty.rules.is_empty());
    assert!(empty.origins.is_empty());
    assert!(empty.overrides.is_empty());

    let empty_layers = vec![layer("silent", "nothing.json", None, vec![])];
    let composed = compose(&empty_layers);
    assert!(composed.rules.is_empty());
    assert!(composed.origins.is_empty());
    assert!(composed.overrides.is_empty());

    assert!(override_facts(&[]).is_empty());
}

#[test]
fn override_facts_preserve_shape_with_source_ids() {
    let overrides = vec![RuleOverride {
        rule_id: "same".to_string(),
        overridden: RuleOrigin {
            layer: "project".to_string(),
            source_id: ".phronesis/rules.json".to_string(),
            decision: None,
        },
        winner: RuleOrigin {
            layer: "user".to_string(),
            source_id: "personal/rules.json".to_string(),
            decision: Some("ADR-user-policy".to_string()),
        },
    }];

    let facts = override_facts(&overrides);

    assert_eq!(facts.len(), 1);
    let fact = &facts[0];
    assert_eq!(fact.id, "__rule_override__:0:same");
    assert_eq!(fact.predicate, OVERRIDE_PREDICATE);
    assert_eq!(
        fact.args,
        vec![
            "same".to_string(),
            "project".to_string(),
            ".phronesis/rules.json".to_string(),
            "user".to_string(),
            "personal/rules.json".to_string(),
            "ADR-user-policy".to_string(),
        ]
    );
    assert_eq!(fact.source.as_deref(), Some("rule_layers"));
}

#[test]
fn missing_winner_decision_renders_empty_arg() {
    let overrides = vec![RuleOverride {
        rule_id: "r".to_string(),
        overridden: RuleOrigin {
            layer: "a".to_string(),
            source_id: "a.json".to_string(),
            decision: None,
        },
        winner: RuleOrigin {
            layer: "b".to_string(),
            source_id: "b.json".to_string(),
            decision: None,
        },
    }];

    let facts = override_facts(&overrides);
    assert_eq!(
        facts[0].args[5], "",
        "absent decision renders as empty string"
    );
}

#[test]
fn override_facts_index_by_input_order() {
    let overrides = vec![
        RuleOverride {
            rule_id: "alpha".to_string(),
            overridden: RuleOrigin {
                layer: "a".to_string(),
                source_id: "a.json".to_string(),
                decision: None,
            },
            winner: RuleOrigin {
                layer: "b".to_string(),
                source_id: "b.json".to_string(),
                decision: None,
            },
        },
        RuleOverride {
            rule_id: "beta".to_string(),
            overridden: RuleOrigin {
                layer: "c".to_string(),
                source_id: "c.json".to_string(),
                decision: None,
            },
            winner: RuleOrigin {
                layer: "d".to_string(),
                source_id: "d.json".to_string(),
                decision: None,
            },
        },
    ];

    let facts = override_facts(&overrides);
    assert_eq!(facts[0].id, "__rule_override__:0:alpha");
    assert_eq!(facts[1].id, "__rule_override__:1:beta");
    assert_eq!(facts[1].args[0], "beta");
}

#[test]
fn project_shadowing_matches_pack_then_project_resolution() {
    // Mirrors the MCP resolver's later_layer_overrides_and_emits_provenance
    // test: pack layers compose first, the project layer shadows them, and
    // the provenance fact names both layers and the project's decision.
    let pack = layer("pack", "packs/rust/rules.json", None, vec![rule("same", 1)]);
    let project = layer(
        "project",
        ".phronesis/rules.json",
        None,
        vec![rule("same", 1)],
    );
    let user = layer(
        "user",
        "personal/rules.json",
        Some("ADR-user-policy"),
        vec![rule("same", 9)],
    );

    let composed = compose(&[pack, project, user]);

    assert_eq!(composed.rules.len(), 1);
    assert_eq!(composed.rules[0].priority, 9);
    assert_eq!(composed.overrides.len(), 2);
    assert_eq!(composed.overrides[0].overridden.layer, "pack");
    assert_eq!(composed.overrides[0].winner.layer, "project");
    assert_eq!(composed.overrides[1].overridden.layer, "project");
    assert_eq!(composed.overrides[1].winner.layer, "user");

    let facts = override_facts(&composed.overrides);
    assert_eq!(facts[1].predicate, "rule_overridden");
    assert_eq!(facts[1].args[0], "same");
    assert_eq!(facts[1].args[1], "project");
    assert_eq!(facts[1].args[3], "user");
    assert_eq!(facts[1].args[5], "ADR-user-policy");
}

#[test]
fn origins_map_covers_all_winning_rules() {
    let first = layer("one", "1.json", None, vec![rule("a", 1), rule("b", 2)]);
    let second = layer("two", "2.json", None, vec![rule("c", 3)]);

    let composed = compose(&[first, second]);

    let expected: HashMap<String, RuleOrigin> = [
        (
            "a".to_string(),
            RuleOrigin {
                layer: "one".to_string(),
                source_id: "1.json".to_string(),
                decision: None,
            },
        ),
        (
            "b".to_string(),
            RuleOrigin {
                layer: "one".to_string(),
                source_id: "1.json".to_string(),
                decision: None,
            },
        ),
        (
            "c".to_string(),
            RuleOrigin {
                layer: "two".to_string(),
                source_id: "2.json".to_string(),
                decision: None,
            },
        ),
    ]
    .into_iter()
    .collect();
    assert_eq!(composed.origins, expected);
    assert!(composed.overrides.is_empty());
}

#[test]
fn composed_policy_round_trips_through_origins_only() {
    // Sanity: the composed rules are the winner contents, not copies of the
    // origins map; origin lookup by rule ID is total over the rules.
    let first = layer("one", "1.json", None, vec![rule("keep", 4)]);
    let second = layer("two", "2.json", None, vec![rule("keep", 8)]);
    let second_origin = origin_of(&second);

    let composed = compose(&[first, second]);

    assert_eq!(composed.rules.iter().map(|r| r.priority).sum::<i32>(), 8);
    assert_eq!(composed.origins.len(), 1);
    assert_eq!(composed.origins["keep"].layer, "two");
    assert_eq!(second_origin.layer, "two");
}
