//! Task 2 of the language-pack plan: the sandboxed adapter evaluator.
//! Narrow to this crate's public contract — JSON in, JSON array out — since
//! `phronesis-rhai` does not depend on `phronesis-mcp`'s record schema.

use phronesis_rhai::{
    AdapterError, evaluate_adapter, evaluate_adapter_with_output_limit, validate_adapter,
};
use serde_json::json;

#[test]
fn valid_script_returns_array_from_injected_fields() {
    let input = json!({
        "context": {"file_rel": "a.go"},
        "matches": [{"query": "decls", "captures": [{"name": "def", "text": "Foo"}]}]
    });
    let script = r#"
        let out = [];
        for m in matches {
            for c in m.captures {
                out.push(#{ kind: "declaration", name: c.text, file: context.file_rel });
            }
        }
        out
    "#;
    let result = evaluate_adapter(script, &input).unwrap();
    assert_eq!(
        result,
        json!([{"kind": "declaration", "name": "Foo", "file": "a.go"}])
    );
}

#[test]
fn validate_adapter_accepts_well_formed_script_and_rejects_syntax_error() {
    validate_adapter("[]").unwrap();
    let err = validate_adapter("let x = ").unwrap_err();
    assert!(matches!(err, AdapterError::Compile(_)), "{err:?}");
}

#[test]
fn expression_depth_is_bounded_and_independent_of_build_profile() {
    let accepted = format!("{}0{}", "(".repeat(40), ")".repeat(40));
    validate_adapter(&accepted).expect("40 levels fit the explicit parse budget");
    let rejected = format!("{}0{}", "(".repeat(200), ")".repeat(200));
    assert!(matches!(
        validate_adapter(&rejected),
        Err(AdapterError::Compile(_))
    ));
}

#[test]
fn unicode_lowercase_helper_uses_letter_category() {
    let script = "[is_unicode_lowercase('é'), is_unicode_lowercase('ƛ'), is_unicode_lowercase('ª'), is_unicode_lowercase('A')]";
    assert_eq!(
        evaluate_adapter(script, &json!({})).unwrap(),
        json!([true, true, false, false])
    );
}

#[test]
fn non_array_final_value_is_rejected() {
    let input = json!({});
    let err = evaluate_adapter("42", &input).unwrap_err();
    assert!(matches!(err, AdapterError::NotAnArray(_)), "{err:?}");

    let err = evaluate_adapter("#{a: 1}", &input).unwrap_err();
    assert!(matches!(err, AdapterError::NotAnArray(_)), "{err:?}");
}

#[test]
fn non_object_input_is_rejected() {
    let err = evaluate_adapter("[]", &json!([1, 2, 3])).unwrap_err();
    assert!(matches!(err, AdapterError::InputNotAnObject(_)), "{err:?}");
}

#[test]
fn malformed_script_is_an_eval_error_not_a_panic() {
    let err = evaluate_adapter("this is not rhai {{{", &json!({})).unwrap_err();
    assert!(matches!(err, AdapterError::Eval(_)), "{err:?}");

    let err = evaluate_adapter("1 / 0", &json!({})).unwrap_err();
    assert!(matches!(err, AdapterError::Eval(_)), "{err:?}");
}

#[test]
fn eval_is_disabled_in_every_spelling() {
    let input = json!({});
    for script in [
        r#"eval("1 + 1"); []"#,
        r#"let code = "1 + 1"; eval(code); []"#,
        r#"Fn("eval").call("1 + 1"); []"#,
        r#""1 + 1".eval(); []"#,
    ] {
        let err = evaluate_adapter(script, &input).unwrap_err();
        assert!(
            matches!(err, AdapterError::Eval(_) | AdapterError::Compile(_)),
            "{script:?} -> {err:?}"
        );
    }
}

#[test]
fn filesystem_and_process_functions_are_not_registered() {
    let input = json!({});
    for script in [
        r#"open_file("/etc/passwd"); []"#,
        r#"import "std" as s; []"#,
    ] {
        let err = evaluate_adapter(script, &input).unwrap_err();
        assert!(
            matches!(err, AdapterError::Eval(_) | AdapterError::Compile(_)),
            "{script:?} should be rejected, got {err:?}"
        );
    }
}

#[test]
fn print_is_an_unregistered_no_op_not_a_host_leak() {
    // `print`/`debug` are Rhai intrinsics that call a host callback; the raw
    // engine registers none, so they silently no-op rather than erroring —
    // harmless, since nothing in this sandbox reads stdout.
    let result = evaluate_adapter(r#"print("x"); []"#, &json!({})).unwrap();
    assert_eq!(result, json!([]));
}

#[test]
fn infinite_loop_hits_the_operation_budget_rather_than_hanging() {
    let err = evaluate_adapter("loop { }", &json!({})).unwrap_err();
    assert!(matches!(err, AdapterError::Eval(_)), "{err:?}");
}

#[test]
fn injected_fields_cannot_be_mutated() {
    // `context` is a scope *constant*; assigning to it is a compile/eval
    // error rather than silently succeeding and leaving the host's input
    // unaffected.
    let input = json!({"context": {"a": 1}});
    let err = evaluate_adapter("context.a = 2; []", &input).unwrap_err();
    assert!(matches!(err, AdapterError::Eval(_)), "{err:?}");
}

#[test]
fn large_injected_array_does_not_exhaust_the_operation_budget() {
    let items: Vec<_> = (0..4000).map(|i| json!({"n": i})).collect();
    let input = json!({ "matches": items });
    let script = r#"
        let out = [];
        for m in matches {
            out.push(#{ kind: "fact", name: "n", args: [m.n.to_string()] });
        }
        out
    "#;
    let result = evaluate_adapter(script, &input).unwrap();
    assert_eq!(result.as_array().unwrap().len(), 4000);
}

#[test]
fn round_trips_every_json_shape() {
    let input = json!({
        "v": {
            "n": null,
            "b": true,
            "i": 7,
            "f": 1.5,
            "s": "hi",
            "arr": [1, "two", false],
            "obj": {"k": "v"}
        }
    });
    let result = evaluate_adapter("[v]", &input).unwrap();
    assert_eq!(result, json!([input["v"]]));
}

#[test]
fn host_output_limit_is_checked_before_json_conversion() {
    let err = evaluate_adapter_with_output_limit("[1, 2, 3]", &json!({}), 2).unwrap_err();
    assert!(matches!(err, AdapterError::OutputLimit(2)), "{err:?}");
}
