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
        // must FAIL, not half-work. Every probe returns a STRING when the
        // capability is present, so a rejection can only come from the
        // capability being absent — never from `NotAString`.
        for forbidden in [
            "emit_fact(\"x\", []); `emitted`",
            "let p = \"/tmp/pwned\"; open_file(p); `opened`",
            "eval(\"`x` + `y`\")",
            "import \"std\" as s; `imported`",
        ] {
            match phronesis_rhai::render(forbidden, &input) {
                Err(RenderError::Eval { .. }) => {}
                other => panic!(
                    "forbidden capability must be absent from render scope: {forbidden} -> {other:?}"
                ),
            }
        }
        assert!(
            phronesis_rhai::render_scope_freeze_holds(),
            "the callable scope-freeze check must hold for the render engine"
        );
        // And the worked template still renders verus code.
        let body = phronesis_rhai::render(verus_template(), &input).unwrap();
        assert!(
            body.contains("verus!"),
            "the worked template renders verus code"
        );
    }

    /// Scope freeze (SPEC-C render contract): the property record and the
    /// dependency facts are read-only. Every attempt to mutate them — field
    /// assignment, index assignment, a mutating method, reassignment — is a
    /// render failure, never a silently-accepted edit. Each probe returns a
    /// string when the mutation goes through, so an `Eval` error can only
    /// mean the mutation was refused.
    #[test]
    fn render_refuses_mutation_of_the_property_record_and_inputs() {
        let mut fact = Map::new();
        fact.insert("predicate".into(), "property_depends_on".into());
        fact.insert("args".into(), rhai::Dynamic::from(rhai::Array::new()));
        let input = RenderInput::frozen(property_map("p.id", "safe_divide"), vec![fact]);
        for probe in [
            r#"property.id = "forged"; property.id"#,
            r#"property["id"] = "forged"; property.id"#,
            r#"property.remove("id"); `removed`"#,
            r#"property.clear(); `cleared`"#,
            r#"property += #{ status: "verified" }; `merged`"#,
            r#"property = #{}; `reassigned`"#,
            r#"facts.push(#{ predicate: "forged" }); `pushed`"#,
            r#"facts.clear(); `cleared`"#,
            r#"facts[0].predicate = "forged"; `edited`"#,
            r#"facts = []; `reassigned`"#,
        ] {
            match phronesis_rhai::render(probe, &input) {
                Err(RenderError::Eval { .. }) => {}
                other => panic!("render inputs must be read-only: {probe} -> {other:?}"),
            }
        }
        // And the host's input is untouched by the attempts.
        assert_eq!(
            input.property.get("id").map(|v| v.to_string()),
            Some("p.id".to_string())
        );
        assert_eq!(input.dependency_facts.len(), 1);
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

    /// SPEC-C: "`eval` and dynamic script evaluation are disabled". A
    /// string-returning eval — the only shape that would otherwise render —
    /// must be rejected, and so must every aliasing path to it.
    #[test]
    fn render_rejects_eval_and_its_aliases() {
        let input = RenderInput::frozen(property_map("p", "s"), vec![]);
        for probe in [
            "eval(\"`x` + `y`\")",
            "let e = eval; e(\"`x`\")",
            "Fn(\"eval\").call(\"`x`\")",
            "call(Fn(\"eval\"), \"`x`\")",
            "let f = Fn(\"ev\" + \"al\"); f.call(\"`x`\")",
            "\"`x`\".eval()",
            "let code = \"`x`\"; eval(code)",
        ] {
            match phronesis_rhai::render(probe, &input) {
                Err(RenderError::Eval { .. }) => {}
                other => panic!("eval must be unreachable in render: {probe} -> {other:?}"),
            }
        }
    }

    /// The body cap is 64 KiB (`MAX_RENDER_BYTES`); the proven 10-VC harness
    /// is ~7 KiB. A ~8 KiB, 240-line body must render.
    #[test]
    fn render_accepts_a_harness_sized_body() {
        let input = RenderInput::frozen(property_map("p", "safe_divide"), vec![]);
        let template = r#"
            let subject = property.get("subject");
            let body = "";
            for i in 0..240 {
                body += `    assert(${subject}_vc_${i}(x));   // line\n`;
            }
            body
        "#;
        let body = phronesis_rhai::render(template, &input).expect("8 KiB body renders");
        assert!(body.len() > 8 * 1024, "body is {} bytes", body.len());
        assert!(body.len() < phronesis_rhai::MAX_RENDER_BYTES);
    }

    /// Exactly at the cap renders; one byte over is `TooLarge` — the
    /// intended error, not a generic engine failure.
    #[test]
    fn render_enforces_the_64_kib_cap_with_too_large() {
        let input = RenderInput::frozen(property_map("p", "s"), vec![]);
        let cap = phronesis_rhai::MAX_RENDER_BYTES;
        assert_eq!(cap, 64 * 1024);

        let at_cap = format!("let s = \"\"; s.pad({cap}, \"a\"); s");
        let body = phronesis_rhai::render(&at_cap, &input).expect("body at the cap renders");
        assert_eq!(body.len(), cap);

        let over = format!("let s = \"\"; s.pad({}, \"a\"); s", cap + 1);
        match phronesis_rhai::render(&over, &input) {
            Err(RenderError::TooLarge { limit }) => assert_eq!(limit, cap),
            other => panic!("expected TooLarge, got {other:?}"),
        }
    }
}

/// The render engine's larger string budget must not leak into guards: a
/// `__script__` guard still cannot build a string over 4 KiB.
#[test]
fn guard_string_limit_is_unchanged_by_render_budget() {
    let b = HashMap::new();
    assert!(eval(r#"let s = ""; s.pad(4096, "a"); s.len() == 4096"#, &[], &b).unwrap());
    assert!(eval(r#"let s = ""; s.pad(4097, "a"); true"#, &[], &b).is_err());
}

#[test]
fn guard_engine_rejects_eval() {
    // Guards run in the D7 sandbox: `eval` is disabled in every engine, so a
    // string-returning `eval` is a parse error (a blocked guard), not `true`.
    let b = HashMap::new();
    assert!(eval(r#"eval("\"x\"") == "x""#, &[], &b).is_err());
    assert!(eval(r#"let code = "true"; eval(code)"#, &[], &b).is_err());
}

/// A realistic hook fact base: hundreds of small facts plus a multi-KB
/// `new_content` payload. Rhai sizes a value recursively, so the injected
/// `facts` array as a whole used to count against the 4 KiB per-string
/// limit — any guard touching `facts` errored, and a guard error fails
/// closed, so it blocked.
fn realistic_facts() -> Vec<Fact> {
    let mut facts: Vec<Fact> = (0..300)
        .map(|i| {
            fact(
                &format!("f{i}"),
                "file_path_matches",
                &[&format!(
                    "component-{i:04}-padding-padding-padding-padding-pad"
                )],
            )
        })
        .collect();
    let content = "fn main() { println!(\"needle\"); }\n".repeat(600); // ~20 KB
    facts.push(fact("new_content", "new_content", &[&content]));
    facts
}

#[test]
fn guard_reads_a_realistic_fact_base_without_tripping_limits() {
    let facts = realistic_facts();
    let b = HashMap::new();
    assert_eq!(eval("facts.len() > 100000", &facts, &b), Ok(false));
    assert_eq!(
        eval(
            r#"facts.filter(|f| f.predicate == "file_path_matches").len() == 300"#,
            &facts,
            &b
        ),
        Ok(true)
    );
    assert_eq!(
        eval(
            r#"facts.some(|f| f.predicate == "new_content" && f.args[0].contains("needle"))"#,
            &facts,
            &b
        ),
        Ok(true)
    );
}

#[test]
fn guard_runaway_still_errors_over_a_large_fact_base() {
    // The budget grows with the injected data only; a script that builds
    // its own unbounded data still hits a limit and fails closed.
    let facts = realistic_facts();
    let b = HashMap::new();
    assert!(eval(r#"let s = "ab"; loop { s += s; }"#, &facts, &b).is_err());
    assert!(eval("let a = []; loop { a.push(1); }", &facts, &b).is_err());
}
