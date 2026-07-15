use std::sync::LazyLock;

use streaming_iterator::StreamingIterator;
use tree_sitter::{Query, QueryCursor, QueryMatch};

use super::super::parsed::ParsedFile;
use super::walk::walk_function_items;

static PUB_FN_QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(
        &tree_sitter_rust::LANGUAGE.into(),
        r#"
        (function_item
          (visibility_modifier) @vis
          name: (identifier) @name)
        "#,
    )
    .expect("PUB_FN_QUERY compiles")
});

static FN_QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(
        &tree_sitter_rust::LANGUAGE.into(),
        r#"
        (function_item
          name: (identifier) @name
          return_type: (generic_type
            type: (type_identifier) @return_outer
            type_arguments: (type_arguments) @return_args))
        "#,
    )
    .expect("FN_QUERY compiles")
});

/// (fn_name, param_name, type_text). `type_text` is whitespace-normalized so
/// rules can match against a stable form: runs of whitespace collapse to one
/// space, and whitespace adjacent to `<`, `>`, `,`, `:`, or `&` is stripped.
/// Examples: `& String` → `&String`, `Vec< u8 >` → `Vec<u8>`,
/// `dyn Trait + Send` → `dyn Trait + Send`, `&'a str` → `&'a str`.
pub(super) fn extract_function_param_types(parsed: &ParsedFile) -> Vec<(String, String, String)> {
    let ParsedFile::Rust { tree, source } = parsed else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut walker = tree.walk();
    walk_function_items(&mut walker, source.as_bytes(), &mut |fn_node, name| {
        let Some(params) = fn_node.child_by_field_name("parameters") else {
            return;
        };
        let mut param_walker = params.walk();
        for child in params.children(&mut param_walker) {
            if child.kind() != "parameter" {
                continue;
            }
            let Some(pat) = child.child_by_field_name("pattern") else {
                continue;
            };
            let Some(ty) = child.child_by_field_name("type") else {
                continue;
            };
            let pat_text = pat.utf8_text(source.as_bytes()).unwrap_or("");
            let ty_text = ty.utf8_text(source.as_bytes()).unwrap_or("");
            let collapsed: String = ty_text.split_whitespace().collect::<Vec<_>>().join(" ");
            let normalized = collapsed
                .replace(" <", "<")
                .replace("< ", "<")
                .replace(" >", ">")
                .replace("> ", ">")
                .replace(" ,", ",")
                .replace(", ", ",")
                .replace(" :", ":")
                .replace(": ", ":")
                .replace("& ", "&");
            out.push((name.to_string(), pat_text.to_string(), normalized));
        }
    });
    out
}

/// Returns functions whose individual business-parameter count meets the
/// threshold. Counting each syntax node independently avoids combining
/// identically named methods from separate trait implementations.
pub(super) fn extract_function_param_counts_high(
    parsed: &ParsedFile,
    threshold: usize,
) -> Vec<(String, usize)> {
    let ParsedFile::Rust { tree, source } = parsed else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut walker = tree.walk();
    walk_function_items(&mut walker, source.as_bytes(), &mut |fn_node, name| {
        let Some(params) = fn_node.child_by_field_name("parameters") else {
            return;
        };
        let mut param_walker = params.walk();
        let count = params
            .children(&mut param_walker)
            .filter(|child| child.kind() == "parameter")
            .count();
        if count >= threshold {
            out.push((name.to_string(), count));
        }
    });
    out
}

/// `async fn` — walks function_item states and checks whether their
/// `function_modifiers` child contains the `async` keyword.
pub(super) fn extract_async_functions(parsed: &ParsedFile) -> Vec<String> {
    let ParsedFile::Rust { tree, source } = parsed else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut walker = tree.walk();
    walk_function_items(&mut walker, source.as_bytes(), &mut |fn_node, name| {
        let mut child_walker = fn_node.walk();
        for child in fn_node.children(&mut child_walker) {
            if child.kind() == "function_modifiers" {
                let text = child.utf8_text(source.as_bytes()).unwrap_or("");
                if text.split_whitespace().any(|w| w == "async") {
                    out.push(name.to_string());
                    return;
                }
            }
        }
    });
    out
}

/// `pub fn` only — `pub(crate)`, `pub(super)`, etc. are deliberately excluded
/// because they don't expose API outside the crate boundary.
pub(super) fn extract_public_functions(parsed: &ParsedFile) -> Vec<String> {
    let ParsedFile::Rust { tree, source } = parsed else {
        return Vec::new();
    };
    let mut cursor = QueryCursor::new();
    cursor
        .matches(&PUB_FN_QUERY, tree.root_node(), source.as_bytes())
        .filter_map_deref(|m| pub_fn_name(m, source.as_bytes()))
        .collect()
}

/// From one `PUB_FN_QUERY` match: the function name iff its
/// visibility_modifier is exactly `pub`.
fn pub_fn_name(m: &QueryMatch, source: &[u8]) -> Option<String> {
    let vis = capture_node(m, &PUB_FN_QUERY, "vis")?;
    if vis.utf8_text(source).unwrap_or("").trim() != "pub" {
        return None;
    }
    let name = capture_node(m, &PUB_FN_QUERY, "name")?;
    Some(name.utf8_text(source).unwrap_or("").to_string())
}

/// Functions whose return type is `Result<_, String>` (error state is String).
pub(super) fn extract_result_string_returns(parsed: &ParsedFile) -> Vec<String> {
    let ParsedFile::Rust { tree, source } = parsed else {
        return Vec::new();
    };
    let mut cursor = QueryCursor::new();
    cursor
        .matches(&FN_QUERY, tree.root_node(), source.as_bytes())
        .filter_map_deref(|m| result_string_offender(m, source.as_bytes()))
        .collect()
}

/// From one `FN_QUERY` match: the function name iff the return type is
/// `Result<_, String>` — outer type is `Result`, two or more type
/// arguments, and the last argument is the bare `String` identifier.
fn result_string_offender(m: &QueryMatch, source: &[u8]) -> Option<String> {
    let outer = capture_node(m, &FN_QUERY, "return_outer")?;
    if outer.utf8_text(source).unwrap_or("") != "Result" {
        return None;
    }
    let args = capture_node(m, &FN_QUERY, "return_args")?;
    let count = args.named_child_count();
    if count < 2 {
        return None;
    }
    let last = args.named_child(count - 1)?;
    if last.kind() != "type_identifier" || last.utf8_text(source).unwrap_or("") != "String" {
        return None;
    }
    let name = capture_node(m, &FN_QUERY, "name")?;
    Some(name.utf8_text(source).unwrap_or("").to_string())
}

/// The node captured under `name` in `m`, if that capture is present.
fn capture_node<'t>(
    m: &QueryMatch<'_, 't>,
    query: &Query,
    name: &str,
) -> Option<tree_sitter::Node<'t>> {
    m.captures
        .iter()
        .find(|cap| query.capture_names()[cap.index as usize] == name)
        .map(|cap| cap.node)
}

#[cfg(test)]
mod tests {
    use crate::syntax::SyntaxFacts;
    use crate::syntax::rust::extract;

    #[test]
    fn detects_result_string_return() {
        let code = "fn foo() -> Result<u32, String> { Ok(1) }";
        let facts = extract(code);
        assert_eq!(facts.functions_returning_result_string, vec!["foo"]);
    }

    #[test]
    fn ignores_result_with_error_type() {
        let code = "fn ok() -> Result<u32, MyError> { Ok(1) }";
        let facts = extract(code);
        assert!(facts.functions_returning_result_string.is_empty());
    }

    #[test]
    fn ignores_non_result_returns() {
        let code = "fn x() -> u32 { 1 } fn y() -> String { String::new() }";
        let facts = extract(code);
        assert!(facts.functions_returning_result_string.is_empty());
    }

    #[test]
    fn detects_multiple_offenders() {
        let code = "\
fn one() -> Result<(), String> { Ok(()) }
fn two() -> Result<u32, String> { Ok(0) }
fn safe() -> Result<u32, MyError> { Ok(0) }
";
        let facts = extract(code);
        assert_eq!(facts.functions_returning_result_string, vec!["one", "two"]);
    }

    #[test]
    fn empty_input_yields_no_facts() {
        assert_eq!(extract(""), SyntaxFacts::default());
    }

    #[test]
    fn invalid_rust_does_not_panic() {
        let result = std::panic::catch_unwind(|| extract("fn !!! broken values here"));
        assert!(result.is_ok(), "invalid Rust input must not panic");
    }

    #[test]
    fn does_not_flag_single_arg_result_string() {
        let code = "fn get_name() -> Result<String> { Ok(String::new()) }";
        let facts = extract(code);
        assert!(facts.functions_returning_result_string.is_empty());
    }

    #[test]
    fn does_not_flag_scoped_result_with_single_arg() {
        let code = "fn get_name() -> anyhow::Result<String> { Ok(String::new()) }";
        let facts = extract(code);
        assert!(facts.functions_returning_result_string.is_empty());
    }

    #[test]
    fn flags_result_string_string_pair() {
        let code = "fn weird() -> Result<String, String> { Ok(String::new()) }";
        let facts = extract(code);
        assert_eq!(facts.functions_returning_result_string, vec!["weird"]);
    }

    #[test]
    fn does_not_flag_result_with_complex_success_and_typed_error() {
        let code = "fn make() -> Result<Vec<u8>, MyError> { Ok(vec![]) }";
        let facts = extract(code);
        assert!(facts.functions_returning_result_string.is_empty());
    }

    #[test]
    fn does_not_flag_result_with_string_in_success_via_generic() {
        let code = "fn list() -> Result<Vec<String>, MyError> { Ok(vec![]) }";
        let facts = extract(code);
        assert!(facts.functions_returning_result_string.is_empty());
    }

    #[test]
    fn still_flags_standard_result_unit_string() {
        let code = "fn fail() -> Result<(), String> { Err(\"oops\".into()) }";
        let facts = extract(code);
        assert_eq!(facts.functions_returning_result_string, vec!["fail"]);
    }

    #[test]
    fn detects_pub_function() {
        let code = "pub fn foo() {}";
        let facts = extract(code);
        assert_eq!(facts.public_functions, vec!["foo"]);
    }

    #[test]
    fn ignores_private_function() {
        let code = "fn foo() {}";
        let facts = extract(code);
        assert!(facts.public_functions.is_empty());
    }

    #[test]
    fn ignores_pub_crate_function() {
        let code = "pub(crate) fn foo() {}";
        let facts = extract(code);
        assert!(
            facts.public_functions.is_empty(),
            "pub(crate) must not count as fully public: {:?}",
            facts.public_functions
        );
    }

    #[test]
    fn detects_pub_impl_method() {
        let code = "impl S { pub fn bar(&self) {} }";
        let facts = extract(code);
        assert_eq!(facts.public_functions, vec!["bar"]);
    }

    #[test]
    fn detects_async_function() {
        let code = "async fn foo() {}";
        let facts = extract(code);
        assert_eq!(facts.async_functions, vec!["foo"]);
    }

    #[test]
    fn detects_pub_async_function() {
        let code = "pub async fn foo() {}";
        let facts = extract(code);
        assert_eq!(facts.async_functions, vec!["foo"]);
    }

    #[test]
    fn ignores_sync_function() {
        let code = "fn foo() {}";
        let facts = extract(code);
        assert!(facts.async_functions.is_empty());
    }

    #[test]
    fn detects_async_impl_method() {
        let code = "impl S { async fn foo(&self) {} }";
        let facts = extract(code);
        assert_eq!(facts.async_functions, vec!["foo"]);
    }

    #[test]
    fn extracts_single_param_type() {
        let code = "fn foo(name: &String) {}";
        let facts = extract(code);
        assert_eq!(
            facts.function_param_types,
            vec![("foo".to_string(), "name".to_string(), "&String".to_string())]
        );
    }

    #[test]
    fn extracts_multiple_param_types() {
        let code = "fn foo(a: u32, b: Vec<u8>) {}";
        let facts = extract(code);
        assert_eq!(
            facts.function_param_types,
            vec![
                ("foo".to_string(), "a".to_string(), "u32".to_string()),
                ("foo".to_string(), "b".to_string(), "Vec<u8>".to_string()),
            ]
        );
    }

    #[test]
    fn normalizes_whitespace_in_param_type() {
        let code = "fn foo(name: & String) {}";
        let facts = extract(code);
        assert_eq!(
            facts.function_param_types,
            vec![("foo".to_string(), "name".to_string(), "&String".to_string())],
            "internal whitespace should be normalized so rules can match consistently"
        );
    }

    #[test]
    fn preserves_dyn_trait_internal_spacing() {
        let code = "fn foo(x: &dyn Trait) {}";
        let facts = extract(code);
        assert_eq!(
            facts.function_param_types,
            vec![("foo".to_string(), "x".to_string(), "&dyn Trait".to_string())],
            "dyn Trait should keep its internal space; only spaces adjacent to & / < / > / , / : are stripped"
        );
    }

    #[test]
    fn preserves_lifetime_spacing() {
        let code = "fn foo<'a>(s: &'a str) {}";
        let facts = extract(code);
        assert_eq!(
            facts.function_param_types,
            vec![("foo".to_string(), "s".to_string(), "&'a str".to_string())],
            "lifetime-bound references should preserve the space before the type name"
        );
    }

    #[test]
    fn function_param_counts_high_fires_at_threshold() {
        let code = "fn small(a: u32, b: u32) {} fn big(a: u32, b: u32, c: u32, d: u32, e: u32) {} fn xl(a: u32, b: u32, c: u32, d: u32, e: u32, f: u32) {}";
        let facts = extract(code);
        assert_eq!(
            facts.function_param_counts_high,
            vec![("big".to_string(), 5), ("xl".to_string(), 6)],
            "threshold is 5: small (2) skipped, big (5) and xl (6) flagged"
        );
    }

    #[test]
    fn function_param_counts_high_ignores_self() {
        // `&self` does not count toward the parameter total (matches the
        // intent of 'methods with N business params').
        let code = "impl S { fn method(&self, a: u32, b: u32, c: u32, d: u32) {} }";
        let facts = extract(code);
        assert!(
            facts.function_param_counts_high.is_empty(),
            "4 business params + &self should NOT fire the threshold-5 rule"
        );
    }

    #[test]
    fn function_param_counts_do_not_combine_same_named_methods() {
        let code = r#"
            trait A { fn invoke_dyn(&self, a: u32); }
            trait B { fn invoke_dyn(&self, a: u32, b: u32); }
            impl A for S { fn invoke_dyn(&self, a: u32) {} }
            impl B for S { fn invoke_dyn(&self, a: u32, b: u32) {} }
        "#;
        let facts = extract(code);
        assert!(
            facts.function_param_counts_high.is_empty(),
            "separate methods with the same name must not have their parameters combined"
        );
    }

    #[test]
    fn vec_ref_params_flagged_for_shared_and_mut_refs() {
        let code = "fn a(xs: &Vec<u8>) {} fn b(ys: &mut Vec<String>) {} fn c(zs: Vec<i32>) {} fn d(ws: &[u8]) {}";
        let facts = extract(code);
        assert_eq!(
            facts.vec_ref_params,
            vec![
                ("a".to_string(), "xs".to_string()),
                ("b".to_string(), "ys".to_string()),
            ],
            "only &Vec<T> and &mut Vec<T> should be flagged; owned Vec<T> and &[T] should be left alone"
        );
    }

    #[test]
    fn skips_self_parameter() {
        let code = "impl S { fn foo(&self, x: u32) {} }";
        let facts = extract(code);
        assert_eq!(
            facts.function_param_types,
            vec![("foo".to_string(), "x".to_string(), "u32".to_string())]
        );
    }

    #[test]
    fn no_param_types_for_zero_arg_fn() {
        let code = "fn foo() {}";
        let facts = extract(code);
        assert!(facts.function_param_types.is_empty());
    }
}
