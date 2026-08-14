//! Layer 1 — unit tests for `RhaiScriptEvaluator` against the
//! `ScriptEval` trait, in isolation from the RETE network.

use std::collections::HashMap;

use phronesis::{Fact, ScriptEval};
use phronesis_rhai::RhaiScriptEvaluator;

fn fact(id: &str, predicate: &str, args: &[&str]) -> Fact {
    Fact {
        id: id.to_string(),
        predicate: predicate.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
        timestamp: 0,
        source: None,
    }
}

fn eval(script: &str, facts: &[Fact], bindings: &HashMap<String, String>) -> Result<bool, String> {
    RhaiScriptEvaluator::new().evaluate(script, facts, bindings)
}

#[test]
fn simple_boolean_literals() {
    let facts = vec![];
    let b = HashMap::new();
    assert!(eval("true", &facts, &b).unwrap());
    assert!(!eval("false", &facts, &b).unwrap());
    assert!(eval("1 > 0", &facts, &b).unwrap());
    assert!(!eval("2 < 1", &facts, &b).unwrap());
}

#[test]
fn fact_count_and_iteration() {
    let facts = vec![fact("1", "greet", &["alice"]), fact("2", "greet", &["bob"])];
    let b = HashMap::new();

    assert!(eval("facts.len() == 2", &facts, &b).unwrap());
    assert!(eval("facts.len() > 1", &facts, &b).unwrap());

    // Inspect predicate and args on individual facts.
    assert!(eval("facts[0].predicate == \"greet\"", &facts, &b).unwrap());
    assert!(eval("facts[0].args[0] == \"alice\"", &facts, &b).unwrap());
    assert!(eval("facts[1].args.contains(\"bob\")", &facts, &b).unwrap());
}

#[test]
fn numeric_comparison_over_args() {
    // The core motivation: a numeric comparison on a fact argument, which
    // the builtin DSL cannot express.
    let facts = vec![fact("1", "inventory", &["sword", "5"])];
    let b = HashMap::new();

    assert!(eval("facts[0].args[1].parse_int() >= 3", &facts, &b).unwrap());
    assert!(!eval("facts[0].args[1].parse_int() > 10", &facts, &b).unwrap());
}

#[test]
fn binding_access() {
    let facts = vec![];
    let mut b = HashMap::new();
    b.insert("?player".to_string(), "alice".to_string());

    assert!(eval("bindings[\"?player\"] == \"alice\"", &facts, &b).unwrap());
    assert!(!eval("bindings[\"?player\"] == \"bob\"", &facts, &b).unwrap());
    assert!(eval("bindings.contains(\"?player\")", &facts, &b).unwrap());
    assert!(!eval("bindings.contains(\"?missing\")", &facts, &b).unwrap());
}

#[test]
fn compound_boolean_logic() {
    let facts = vec![
        fact("1", "a", &["x"]),
        fact("2", "b", &["y"]),
        fact("3", "c", &["z"]),
    ];
    let mut b = HashMap::new();
    b.insert("?name".to_string(), "n".to_string());

    assert!(
        eval(
            "facts.len() > 2 && bindings.contains(\"?name\")",
            &facts,
            &b
        )
        .unwrap()
    );
    assert!(
        eval(
            "facts.len() > 5 || bindings.contains(\"?name\")",
            &facts,
            &b
        )
        .unwrap()
    );
    assert!(
        !eval(
            "facts.len() > 5 && bindings.contains(\"?name\")",
            &facts,
            &b
        )
        .unwrap()
    );
}

#[test]
fn any_predicate_present() {
    // Express "at least one `auth` fact exists" via a filter/some idiom.
    let facts = vec![fact("1", "read", &["a"]), fact("2", "auth", &["login"])];
    let b = HashMap::new();
    assert!(eval("facts.some(|f| f.predicate == \"auth\")", &facts, &b).unwrap());
    assert!(!eval("facts.some(|f| f.predicate == \"delete\")", &facts, &b).unwrap());
}

#[test]
fn non_bool_return_is_error() {
    let facts = vec![];
    let b = HashMap::new();
    assert!(eval("42", &facts, &b).is_err());
    assert!(eval("\"a string\"", &facts, &b).is_err());
    assert!(eval("facts.len()", &facts, &b).is_err());
}

#[test]
fn syntax_error_is_error() {
    let facts = vec![];
    let b = HashMap::new();
    assert!(eval("this is not )( valid", &facts, &b).is_err());
    assert!(eval("", &facts, &b).is_err());
}

#[test]
fn sandbox_rejects_runaway_operations() {
    // A loop that blows the operation budget must return Err, not hang.
    let facts = vec![];
    let b = HashMap::new();
    let script = "let x = 0; loop { x += 1; }";
    let result = eval(script, &facts, &b);
    assert!(
        result.is_err(),
        "runaway loop should hit the operations cap"
    );
}

#[test]
fn evaluator_is_reusable_across_calls() {
    // A fresh scope per call means no state leaks between evaluations.
    let evaluator = RhaiScriptEvaluator::new();
    let b = HashMap::new();
    let facts_a = vec![fact("1", "x", &["a"])];
    let facts_b = vec![fact("1", "y", &["b"]), fact("2", "y", &["c"])];

    assert!(
        evaluator
            .evaluate("facts.len() == 1", &facts_a, &b)
            .unwrap()
    );
    assert!(
        evaluator
            .evaluate("facts.len() == 2", &facts_b, &b)
            .unwrap()
    );
    assert!(
        evaluator
            .evaluate("facts.len() == 1", &facts_a, &b)
            .unwrap()
    );
}
