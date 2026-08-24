use super::super::parsed::ParsedFile;
use super::walk::{function_name, in_test_code};

/// `(enclosing_function, construct)` for panic/debug-like Rust invocations
/// governed by the starter pack. Module-scope expressions use `<module>`.
pub(super) fn extract_governed_invocations(parsed: &ParsedFile) -> Vec<(String, String)> {
    let ParsedFile::Rust { tree, source } = parsed else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cursor = tree.walk();
    walk(&mut cursor, source.as_bytes(), None, &mut out);
    out
}

fn walk(
    cursor: &mut tree_sitter::TreeCursor,
    source: &[u8],
    enclosing_fn: Option<String>,
    out: &mut Vec<(String, String)>,
) {
    let node = cursor.node();
    let enclosing_fn = if node.kind() == "function_item" {
        function_name(node, source).or(enclosing_fn)
    } else {
        enclosing_fn
    };

    let construct = match node.kind() {
        "macro_invocation" => macro_construct(node, source),
        "call_expression" => {
            method_construct(node, source).or_else(|| free_call_construct(node, source))
        }
        _ => None,
    };
    if let Some(construct) = construct
        && !in_test_code(node, source)
    {
        out.push((
            enclosing_fn
                .clone()
                .unwrap_or_else(|| "<module>".to_string()),
            construct.to_string(),
        ));
    }

    if cursor.goto_first_child() {
        loop {
            walk(cursor, source, enclosing_fn.clone(), out);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
        cursor.goto_parent();
    }
}

pub(super) fn macro_construct<'a>(node: tree_sitter::Node, source: &'a [u8]) -> Option<&'a str> {
    let name = node
        .child_by_field_name("macro")?
        .utf8_text(source)
        .ok()?
        .rsplit("::")
        .next()?;
    match name {
        "todo" | "panic" | "unimplemented" | "dbg" => Some(name),
        _ => None,
    }
}

pub(super) fn method_construct(node: tree_sitter::Node, source: &[u8]) -> Option<&'static str> {
    let function = node.child_by_field_name("function")?;
    let field_expression = match function.kind() {
        "field_expression" => function,
        "generic_function" => {
            let mut cursor = function.walk();
            function
                .children(&mut cursor)
                .find(|child| child.kind() == "field_expression")?
        }
        _ => return None,
    };
    let method = field_expression
        .child_by_field_name("field")?
        .utf8_text(source)
        .ok()?;
    let arguments = node.child_by_field_name("arguments")?;
    let mut cursor = arguments.walk();
    let values: Vec<_> = arguments
        .children(&mut cursor)
        .filter(|child| !matches!(child.kind(), "(" | ")" | ","))
        .collect();

    match method {
        "unwrap" if values.is_empty() => Some("unwrap"),
        "expect"
            if values.len() == 1
                && values[0].kind() == "string_literal"
                && values[0].utf8_text(source).is_ok_and(|text| text == "\"\"") =>
        {
            Some("expect_empty")
        }
        _ => None,
    }
}

fn free_call_construct(node: tree_sitter::Node, source: &[u8]) -> Option<&'static str> {
    let function = node.child_by_field_name("function")?;
    let text = function
        .utf8_text(source)
        .ok()?
        .replace(char::is_whitespace, "");
    matches!(text.as_str(), "env::set_var" | "std::env::set_var").then_some("env_set_var")
}

#[cfg(test)]
mod tests {
    use crate::syntax::rust::extract;

    #[test]
    fn finds_governed_methods_and_macros_across_formatting_variants() {
        let facts = extract(
            r#"
fn demo(value: Option<u8>) {
    value.unwrap ( );
    value.expect("");
    todo!("later");
    std::panic!("boom");
    unimplemented! { "later" };
    dbg!(value);
    std::env::set_var("KEY", "value");
}
"#,
        );
        assert_eq!(
            facts.rust_governed_invocations,
            vec![
                ("demo".to_string(), "unwrap".to_string()),
                ("demo".to_string(), "expect_empty".to_string()),
                ("demo".to_string(), "todo".to_string()),
                ("demo".to_string(), "panic".to_string()),
                ("demo".to_string(), "unimplemented".to_string()),
                ("demo".to_string(), "dbg".to_string()),
                ("demo".to_string(), "env_set_var".to_string()),
            ]
        );
    }

    #[test]
    fn ignores_comments_strings_nonempty_expect_and_similar_names() {
        let facts = extract(
            r#"
fn safe(value: Option<u8>) {
    // value.unwrap(); panic!("no");
    let _ = ".unwrap() todo!()";
    value.expect("documented invariant");
    value.unwrap_or(0);
    debugging!(value);
}
"#,
        );
        assert!(facts.rust_governed_invocations.is_empty());
    }

    #[test]
    fn malformed_partial_source_does_not_panic() {
        let _ = extract("fn partial() { Some(1).unwrap(");
    }

    #[test]
    fn ignores_governed_invocations_inside_test_code() {
        let facts = extract(
            r#"
fn prod(value: Option<u8>) {
    value.unwrap();
}

#[test]
fn t() {
    let x: Option<u8> = None;
    x.unwrap();
}

#[cfg(test)]
mod tests {
    fn helper() {
        let x: Option<u8> = None;
        x.unwrap();
        panic!("boom");
    }
}
"#,
        );
        assert_eq!(
            facts.rust_governed_invocations,
            vec![("prod".to_string(), "unwrap".to_string())],
            "only the production unwrap should be reported, not the #[test] fn or #[cfg(test)] mod"
        );
    }
}
