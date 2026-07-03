//! `CompositeScriptEvaluator` routing: builtin-DSL forms go to the builtin
//! evaluator, everything else to Rhai — so bundled builtin-DSL packs and
//! expressive Rhai guards coexist.

use std::collections::HashMap;

use phronesis::{Fact, ScriptEval};
use phronesis_rhai::CompositeScriptEvaluator;

fn fact(id: &str, predicate: &str, args: &[&str]) -> Fact {
    Fact {
        id: id.to_string(),
        predicate: predicate.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
        timestamp: 0,
    }
}

fn eval(script: &str, facts: &[Fact]) -> Result<bool, String> {
    CompositeScriptEvaluator::new().evaluate(script, facts, &HashMap::new())
}

#[test]
fn builtin_facts_count_form_routes_to_builtin() {
    // The confidence gate's exact shape: `'*'` is a Rhai char literal and
    // `facts_count` is not a Rhai function, so this only works via builtin.
    let facts = vec![
        fact("1", "signal_pass", &["build", "ok"]),
        fact("2", "signal_pass", &["test", "ok"]),
    ];
    assert!(eval("facts_count('signal_pass', ['*','*']) >= 2", &facts).unwrap());
    assert!(!eval("facts_count('signal_pass', ['*','*']) >= 3", &facts).unwrap());
}

#[test]
fn builtin_facts_contain_and_negation_route_to_builtin() {
    let facts = vec![fact("1", "confidence_enabled", &[])];
    assert!(eval("facts_contain('confidence_enabled', [])", &facts).unwrap());
    assert!(!eval("!facts_contain('confidence_enabled', [])", &facts).unwrap());
}

#[test]
fn non_builtin_form_routes_to_rhai() {
    // A numeric comparison over a fact arg — impossible in the builtin DSL,
    // must be handled by Rhai.
    let facts = vec![fact("1", "inventory", &["sword", "9"])];
    assert!(
        eval(
            "facts.some(|f| f.predicate == \"inventory\" && f.args[1].parse_int() >= 5)",
            &facts,
        )
        .unwrap()
    );
    assert!(eval("facts.len() == 1", &facts).unwrap());
}

#[test]
fn rhai_error_still_surfaces_through_composite() {
    let facts = vec![];
    // Non-bool Rhai return is an error (routed to Rhai, not builtin).
    assert!(eval("facts.len()", &facts).is_err());
}
