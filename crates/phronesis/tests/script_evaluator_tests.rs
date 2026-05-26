//! Unit tests for ScriptEvaluator

use std::collections::HashMap;
use phronesis::{ScriptEvaluator, Fact};

#[test]
fn facts_contain_basic_match() {
    let evaluator = ScriptEvaluator::new();
    let facts = vec![
        Fact {
            id: "1".to_string(),
            predicate: "greet".to_string(),
            args: vec!["alice".to_string()],
            timestamp: 0,
        },
        Fact {
            id: "2".to_string(),
            predicate: "farewell".to_string(),
            args: vec!["bob".to_string()],
            timestamp: 0,
        },
    ];
    let bindings = HashMap::new();

    let result = evaluator.evaluate(
        "facts_contain('greet', ['alice'])",
        &facts,
        &bindings,
    );
    assert!(result.is_ok());
    assert!(result.unwrap());
}

#[test]
fn facts_contain_no_match() {
    let evaluator = ScriptEvaluator::new();
    let facts = vec![
        Fact {
            id: "1".to_string(),
            predicate: "greet".to_string(),
            args: vec!["alice".to_string()],
            timestamp: 0,
        },
    ];
    let bindings = HashMap::new();

    let result = evaluator.evaluate(
        "facts_contain('farewell', ['bob'])",
        &facts,
        &bindings,
    );
    assert!(result.is_ok());
    assert!(!result.unwrap());
}

#[test]
fn facts_contain_wildcard_match() {
    let evaluator = ScriptEvaluator::new();
    let facts = vec![
        Fact {
            id: "1".to_string(),
            predicate: "greet".to_string(),
            args: vec!["alice".to_string()],
            timestamp: 0,
        },
    ];
    let bindings = HashMap::new();

    let result = evaluator.evaluate(
        "facts_contain('greet', ['*'])",
        &facts,
        &bindings,
    );
    assert!(result.is_ok());
    assert!(result.unwrap());
}

#[test]
fn facts_contain_negation() {
    let evaluator = ScriptEvaluator::new();
    let facts = vec![
        Fact {
            id: "1".to_string(),
            predicate: "greet".to_string(),
            args: vec!["alice".to_string()],
            timestamp: 0,
        },
    ];
    let bindings = HashMap::new();

    let result = evaluator.evaluate(
        "!facts_contain('farewell', ['bob'])",
        &facts,
        &bindings,
    );
    assert!(result.is_ok());
    assert!(result.unwrap());
}

#[test]
fn facts_count_basic() {
    let evaluator = ScriptEvaluator::new();
    let facts = vec![
        Fact {
            id: "1".to_string(),
            predicate: "greet".to_string(),
            args: vec!["alice".to_string()],
            timestamp: 0,
        },
        Fact {
            id: "2".to_string(),
            predicate: "greet".to_string(),
            args: vec!["bob".to_string()],
            timestamp: 0,
        },
        Fact {
            id: "3".to_string(),
            predicate: "greet".to_string(),
            args: vec!["carol".to_string()],
            timestamp: 0,
        },
    ];
    let bindings = HashMap::new();

    let result = evaluator.evaluate(
        "facts_count('greet', ['*']) >= 3",
        &facts,
        &bindings,
    );
    assert!(result.is_ok());
    assert!(result.unwrap());
}

#[test]
fn facts_count_with_wildcard() {
    let evaluator = ScriptEvaluator::new();
    let facts = vec![
        Fact {
            id: "1".to_string(),
            predicate: "greet".to_string(),
            args: vec!["alice".to_string()],
            timestamp: 0,
        },
        Fact {
            id: "2".to_string(),
            predicate: "farewell".to_string(),
            args: vec!["alice".to_string()],
            timestamp: 0,
        },
    ];
    let bindings = HashMap::new();

    let result = evaluator.evaluate(
        "facts_count('greet', ['*']) >= 2",
        &facts,
        &bindings,
    );
    assert!(result.is_ok());
    assert!(!result.unwrap());
}

#[test]
fn facts_count_comparison_operators() {
    let evaluator = ScriptEvaluator::new();
    let facts = vec![
        Fact {
            id: "1".to_string(),
            predicate: "greet".to_string(),
            args: vec!["alice".to_string()],
            timestamp: 0,
        },
        Fact {
            id: "2".to_string(),
            predicate: "greet".to_string(),
            args: vec!["bob".to_string()],
            timestamp: 0,
        },
    ];
    let bindings = HashMap::new();

    // Test >
    assert!(evaluator.evaluate("facts_count('greet', ['*']) > 1", &facts, &bindings).unwrap());
    // Test ==
    assert!(evaluator.evaluate("facts_count('greet', ['*']) == 2", &facts, &bindings).unwrap());
    // Test <
    assert!(evaluator.evaluate("facts_count('greet', ['*']) < 5", &facts, &bindings).unwrap());
}

#[test]
fn variable_substitution() {
    let evaluator = ScriptEvaluator::new();
    let facts = vec![
        Fact {
            id: "1".to_string(),
            predicate: "greet".to_string(),
            args: vec!["alice".to_string()],
            timestamp: 0,
        },
    ];
    let mut bindings = HashMap::new();
    bindings.insert("?name".to_string(), "alice".to_string());

    let result = evaluator.evaluate(
        "facts_contain('greet', ['?name'])",
        &facts,
        &bindings,
    );
    assert!(result.is_ok());
    assert!(result.unwrap());
}

#[test]
fn malformed_expression() {
    let evaluator = ScriptEvaluator::new();
    let facts = vec![];
    let bindings = HashMap::new();

    let result = evaluator.evaluate(
        "unknown_function('test')",
        &facts,
        &bindings,
    );
    assert!(result.is_err());
}

#[test]
fn malformed_facts_contain() {
    let evaluator = ScriptEvaluator::new();
    let facts = vec![];
    let bindings = HashMap::new();

    let result = evaluator.evaluate(
        "facts_contain('test')",
        &facts,
        &bindings,
    );
    assert!(result.is_err());
}

#[test]
fn malformed_facts_count() {
    let evaluator = ScriptEvaluator::new();
    let facts = vec![];
    let bindings = HashMap::new();

    let result = evaluator.evaluate(
        "facts_count('test')",
        &facts,
        &bindings,
    );
    assert!(result.is_err());
}
