use phronesis_governance::capsule::{parse_duration, validate_emit_capsule_payload};
use phronesis_governance::{CompiledPolicy, parse};
use serde_json::json;

#[test]
fn malformed_unicode_duration_returns_error_instead_of_panicking() {
    for duration in ["é", "5é", "🕐", "1🕐"] {
        assert!(parse_duration(duration).is_none());
        assert!(
            validate_emit_capsule_payload(
                &json!({"id":"x","body":"b","lifecycle":"session","expires_after":duration})
            )
            .is_err()
        );
    }
}

#[test]
fn programmatic_policy_cannot_bypass_nonempty_rule_identity() {
    let mut rules = parse(
        r#"{"rules":[{"id":"x","when":[{"p":true}],"then":{"block":"m"}}]}"#,
        "memory",
    )
    .unwrap()
    .file
    .rules;
    rules[0].id.clear();
    assert!(CompiledPolicy::compile(rules).is_err());
}

#[test]
fn override_fact_time_is_explicit_and_default_is_deterministic() {
    use phronesis_governance::composition::{
        LoadedLayer, RuleOrigin, compose, override_facts, override_facts_at,
    };
    let rules = parse(
        r#"{"rules":[{"id":"x","when":[{"p":true}],"then":{"block":"m"}}]}"#,
        "memory",
    )
    .unwrap()
    .file
    .rules;
    let layers = ["first", "second"].map(|name| LoadedLayer {
        origin: RuleOrigin {
            layer: name.into(),
            source_id: format!("opaque:{name}"),
            decision: None,
        },
        rules: rules.clone(),
    });
    let result = compose(&layers);
    assert_eq!(override_facts_at(&result.overrides, 777)[0].timestamp, 777);
    assert_eq!(override_facts(&result.overrides)[0].timestamp, 0);
}
