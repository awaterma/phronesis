//! Errors must be discriminable by callers — the whole point of the
//! typed-error migration. Each test pattern-matches a variant rather
//! than string-matching, pinning the API contract.

use phronesis::engine_types::Fact;
use phronesis::error::ReteError;
use phronesis::network::ReteNetwork;
use phronesis::variable_binding::Bindings;

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
async fn duplicate_assert_yields_duplicate_fact_id() {
    let net = ReteNetwork::new();
    net.assert_fact(fact("f1", "p", &["a"])).await.unwrap();
    let err = net
        .assert_fact(fact("f1", "p", &["b"]))
        .await
        .expect_err("conflicting duplicate must error");
    assert_eq!(err, ReteError::DuplicateFactId("f1".into()));
}

#[tokio::test]
async fn retract_unknown_yields_fact_not_found() {
    let net = ReteNetwork::new();
    let err = net.retract_fact("ghost").await.expect_err("must error");
    assert!(matches!(err, ReteError::FactNotFound(id) if id == "ghost"));
}

#[tokio::test]
async fn remove_unknown_rule_yields_rule_not_found() {
    let net = ReteNetwork::new();
    let err = net.remove_rule("ghost-rule").expect_err("must error");
    assert!(matches!(err, ReteError::RuleNotFound(id) if id == "ghost-rule"));
}

#[test]
fn binding_conflict_carries_both_values() {
    let mut bindings = Bindings::new();
    bindings.add_binding("?x", "one").unwrap();
    let err = bindings
        .add_binding("?x", "two")
        .expect_err("conflicting rebind must error");
    assert_eq!(
        err,
        ReteError::BindingConflict {
            variable: "?x".into(),
            existing: "one".into(),
            attempted: "two".into(),
        }
    );
}

#[test]
fn non_variable_binding_yields_invalid_variable() {
    let mut bindings = Bindings::new();
    let err = bindings
        .add_binding("x", "one")
        .expect_err("non-? name must error");
    assert!(matches!(err, ReteError::InvalidVariable(name) if name == "x"));
}

#[test]
fn errors_render_legacy_messages_and_convert_to_string() {
    let err = ReteError::FactNotFound("f9".into());
    assert_eq!(err.to_string(), "WME with ID f9 not found");
    // Transition shim for hosts still carrying Result<_, String>.
    let as_string: String = err.into();
    assert_eq!(as_string, "WME with ID f9 not found");
}

// `execute_next_agenda_item` — and thus the only way to reach EmptyAgenda —
// is behind the `embedding-host` feature; `execute_all_agenda_items` drains
// the agenda and never surfaces the variant.
#[cfg(feature = "embedding-host")]
#[tokio::test]
async fn empty_agenda_is_a_matchable_variant() {
    let net = ReteNetwork::new();
    let err = net
        .execute_next_agenda_item()
        .expect_err("empty agenda must error");
    assert_eq!(err, ReteError::EmptyAgenda);
}
