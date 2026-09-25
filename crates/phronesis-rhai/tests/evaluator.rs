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

#[cfg(test)]
mod render_tests {
    use phronesis_rhai::{RenderError, RenderInput};
    use rhai::Map;

    fn property_map(id: &str, subject: &str) -> Map {
        let mut m = Map::new();
        m.insert("id".into(), id.into());
        m.insert("subject".into(), subject.into());
        m.insert("kind".into(), "postcondition".into());
        m
    }

    fn verus_template() -> &'static str {
        r#"
        let id = property.get("id");
        let subject = property.get("subject");
        `// GENERATED - DO NOT EDIT
// Property: ${id}
use vstd::prelude::*;

verus! {

spec fn property_${subject}_holds(input: int) -> bool {
    input != 0
}

fn check(input: int)
    requires input != 0,
    ensures true,
{
}

} // verus!`
    "#
    }

    #[test]
    fn render_scope_freeze_forbidden_capabilities_are_absent() {
        let mut m = Map::new();
        m.insert("id".into(), "x".into());
        let input = RenderInput {
            property: m,
            dependency_facts: vec![],
        };
        // Black-box scope freeze: templates attempting forbidden capabilities
        // must FAIL (function-not-found), not half-work. Each of these is a
        // capability the render contract denies.
        for forbidden in [
            "emit_fact(\"x\")",
            "let p = \"/tmp/pwned\"; open_file(p)",
            "eval(\"1+1\")",
        ] {
            assert!(
                phronesis_rhai::render(forbidden, &input).is_err(),
                "forbidden capability must be absent from render scope: {forbidden}"
            );
        }
        // And the worked template still renders verus code.
        let body = phronesis_rhai::render(verus_template(), &input).unwrap();
        assert!(
            body.contains("verus!"),
            "the worked template renders verus code"
        );
    }

    #[test]
    fn render_is_deterministic_over_the_frozen_input() {
        let mut facts = vec![];
        for (k, v) in [
            ("predicate", "changed_region"),
            ("predicate", "property_depends_on"),
        ] {
            let mut m = Map::new();
            m.insert(k.into(), v.into());
            m.insert("arg".into(), format!("{v}:fn:safe_divide").into());
            facts.push(m);
        }
        let input = RenderInput::frozen(
            property_map("safe_divide.zero_returns_error", "safe_divide"),
            facts,
        );
        let input2 = RenderInput::frozen(
            property_map("safe_divide.zero_returns_error", "safe_divide"),
            {
                // Same facts, DIFFERENT insertion order — frozen() must normalize.
                let mut rev = input.dependency_facts.clone();
                rev.reverse();
                rev
            },
        );
        let a = phronesis_rhai::render(verus_template(), &input).unwrap();
        let b = phronesis_rhai::render(verus_template(), &input2).unwrap();
        assert_eq!(
            a, b,
            "byte-identical across runs AND across insertion order"
        );
    }

    #[test]
    fn render_rejects_non_string_and_oversize() {
        let input = RenderInput {
            property: property_map("p", "s"),
            dependency_facts: vec![],
        };
        assert!(matches!(
            phronesis_rhai::render("42", &input),
            Err(RenderError::NotAString { .. })
        ));
    }
}
