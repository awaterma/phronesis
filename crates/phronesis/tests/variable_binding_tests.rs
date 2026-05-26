//! Unit tests for variable_binding module

use phronesis::{Bindings, Condition, Fact, Token};
use phronesis::wme::WorkingMemoryElement;

#[test]
fn bindings_add_valid() {
    let mut bindings = Bindings::new();
    let result = bindings.add_binding("?x", "hello");
    assert!(result.is_ok());
    assert_eq!(bindings.get_binding("?x"), Some(&"hello".to_string()));
}

#[test]
fn bindings_add_invalid_prefix() {
    let mut bindings = Bindings::new();
    let result = bindings.add_binding("x", "hello");
    assert!(result.is_err());
}

#[test]
fn bindings_add_duplicate_same_value() {
    let mut bindings = Bindings::new();
    bindings.add_binding("?x", "hello").unwrap();
    let result = bindings.add_binding("?x", "hello");
    assert!(result.is_ok());
}

#[test]
fn bindings_add_duplicate_different_value() {
    let mut bindings = Bindings::new();
    bindings.add_binding("?x", "hello").unwrap();
    let result = bindings.add_binding("?x", "game");
    assert!(result.is_err());
}

#[test]
fn bindings_get_nonexistent() {
    let bindings = Bindings::new();
    assert_eq!(bindings.get_binding("?x"), None);
}

#[test]
fn bindings_contains_var() {
    let mut bindings = Bindings::new();
    assert!(!bindings.contains_var("?x"));
    bindings.add_binding("?x", "hello").unwrap();
    assert!(bindings.contains_var("?x"));
    assert!(!bindings.contains_var("?y"));
}

#[test]
fn bindings_merge_compatible() {
    let mut bindings1 = Bindings::new();
    bindings1.add_binding("?x", "hello").unwrap();

    let mut bindings2 = Bindings::new();
    bindings2.add_binding("?y", "game").unwrap();

    let result = bindings1.merge(&bindings2);
    assert!(result.is_ok());
    let merged = result.unwrap();
    assert_eq!(merged.get_binding("?x"), Some(&"hello".to_string()));
    assert_eq!(merged.get_binding("?y"), Some(&"game".to_string()));
}

#[test]
fn bindings_merge_conflict() {
    let mut bindings1 = Bindings::new();
    bindings1.add_binding("?x", "hello").unwrap();

    let mut bindings2 = Bindings::new();
    bindings2.add_binding("?x", "game").unwrap();

    let result = bindings1.merge(&bindings2);
    assert!(result.is_err());
}

#[test]
fn can_bind_success() {
    let bindings = Bindings::new();
    let condition = Condition {
        predicate: "greet".to_string(),
        args: vec!["?who".to_string()],
        script: None,
    };
    let fact = Fact {
        id: "1".to_string(),
        predicate: "greet".to_string(),
        args: vec!["alice".to_string()],
        timestamp: 0,
    };

    let result = bindings.can_bind(&condition, &fact);
    assert!(result.is_ok());
    let new_bindings = result.unwrap();
    assert_eq!(new_bindings.get_binding("?who"), Some(&"alice".to_string()));
}

#[test]
fn can_bind_predicate_mismatch() {
    let bindings = Bindings::new();
    let condition = Condition {
        predicate: "greet".to_string(),
        args: vec!["?who".to_string()],
        script: None,
    };
    let fact = Fact {
        id: "1".to_string(),
        predicate: "farewell".to_string(),
        args: vec!["alice".to_string()],
        timestamp: 0,
    };

    let result = bindings.can_bind(&condition, &fact);
    assert!(result.is_err());
}

#[test]
fn can_bind_variable_already_bound() {
    let mut bindings = Bindings::new();
    bindings.add_binding("?who", "alice").unwrap();

    let condition = Condition {
        predicate: "greet".to_string(),
        args: vec!["?who".to_string()],
        script: None,
    };
    let fact = Fact {
        id: "1".to_string(),
        predicate: "greet".to_string(),
        args: vec!["bob".to_string()],
        timestamp: 0,
    };

    let result = bindings.can_bind(&condition, &fact);
    assert!(result.is_err());
}

#[test]
fn can_bind_constant_mismatch() {
    let bindings = Bindings::new();
    let condition = Condition {
        predicate: "greet".to_string(),
        args: vec!["alice".to_string()],
        script: None,
    };
    let fact = Fact {
        id: "1".to_string(),
        predicate: "greet".to_string(),
        args: vec!["bob".to_string()],
        timestamp: 0,
    };

    let result = bindings.can_bind(&condition, &fact);
    assert!(result.is_err());
}

#[test]
fn token_new() {
    let token = Token::new();
    assert!(token.wmes.is_empty());
    assert!(token.bindings.bindings.is_empty());
    assert!(token.parent.is_none());
}

#[test]
fn token_new_with_wme() {
    let wme = WorkingMemoryElement::new(
        Fact {
            id: "1".to_string(),
            predicate: "greet".to_string(),
            args: vec!["alice".to_string()],
            timestamp: 0,
        },
    );
    let token = Token::new_with_wme(wme);
    assert_eq!(token.wmes.len(), 1);
}

#[test]
fn token_new_with_bindings() {
    let wmes = vec![
        WorkingMemoryElement::new(
            Fact {
                id: "1".to_string(),
                predicate: "greet".to_string(),
                args: vec!["alice".to_string()],
                timestamp: 0,
            },
        ),
    ];
    let mut bindings = Bindings::new();
    bindings.add_binding("?who", "alice").unwrap();

    let token = Token::new_with_bindings(wmes, bindings);
    assert_eq!(token.wmes.len(), 1);
    assert_eq!(token.bindings.get_binding("?who"), Some(&"alice".to_string()));
}

#[test]
fn token_extend_with_binding() {
    let token = Token::new_with_bindings(
        vec![
            WorkingMemoryElement::new(
                Fact {
                    id: "1".to_string(),
                    predicate: "greet".to_string(),
                    args: vec!["alice".to_string()],
                    timestamp: 0,
                },
            ),
        ],
        {
            let mut b = Bindings::new();
            b.add_binding("?who", "alice").unwrap();
            b
        },
    );

    let new_wme = WorkingMemoryElement::new(
        Fact {
            id: "2".to_string(),
            predicate: "farewell".to_string(),
            args: vec!["bob".to_string()],
            timestamp: 0,
        },
    );

    let mut additional_bindings = Bindings::new();
    additional_bindings.add_binding("?target", "bob").unwrap();

    let result = token.extend_with_binding(new_wme, &additional_bindings);
    assert!(result.is_ok());
    let extended = result.unwrap();
    assert_eq!(extended.wmes.len(), 2);
    assert_eq!(extended.bindings.get_binding("?who"), Some(&"alice".to_string()));
    assert_eq!(extended.bindings.get_binding("?target"), Some(&"bob".to_string()));
    assert!(extended.parent.is_some());
}
