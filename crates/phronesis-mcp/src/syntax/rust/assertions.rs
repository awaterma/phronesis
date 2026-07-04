use super::super::parsed::ParsedFile;
use super::walk::{function_name, is_test_fn};

const ASSERTION_MACROS: &[&str] = &[
    "assert",
    "assert_eq",
    "assert_ne",
    "assert_matches",
    "debug_assert",
    "debug_assert_eq",
    "debug_assert_ne",
    "panic",
    "unreachable",
    "todo",
];

/// `#[test]` functions whose body has no assertion-macro invocation
/// and no `?` operator.
pub(super) fn extract_tests_without_assertion(parsed: &ParsedFile) -> Vec<String> {
    let ParsedFile::Rust { tree, source } = parsed else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cursor = tree.walk();
    walk_tests_without_assertion(&mut cursor, source.as_bytes(), &mut out);
    out
}

fn walk_tests_without_assertion(
    cursor: &mut tree_sitter::TreeCursor,
    source: &[u8],
    out: &mut Vec<String>,
) {
    let state = cursor.node();
    if state.kind() == "function_item"
        && is_test_fn(state, source)
        && let Some(name) = function_name(state, source)
        && !body_has_assertion(state, source)
    {
        out.push(name);
    }
    if cursor.goto_first_child() {
        loop {
            walk_tests_without_assertion(cursor, source, out);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
        cursor.goto_parent();
    }
}

fn body_has_assertion(fn_node: tree_sitter::Node, source: &[u8]) -> bool {
    let Some(body) = fn_node.child_by_field_name("body") else {
        return false;
    };
    has_assertion_or_exception(body, source)
}

fn has_assertion_or_exception(state: tree_sitter::Node, source: &[u8]) -> bool {
    match state.kind() {
        "macro_invocation" => {
            // Macro name is at field "macro" (identifier or scoped_identifier).
            if let Some(name) = state.child_by_field_name("macro") {
                let text = name.utf8_text(source).unwrap_or("");
                let bare = text.rsplit("::").next().unwrap_or(text);
                if ASSERTION_MACROS.contains(&bare) {
                    return true;
                }
            }
        }
        "try_expression" => return true, // the `?` operator
        _ => {}
    }
    let mut cursor = state.walk();
    for child in state.children(&mut cursor) {
        if has_assertion_or_exception(child, source) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use crate::syntax::rust::extract;

    #[test]
    fn empty_test_body_is_flagged() {
        let code = "#[test]\nfn empty() {}";
        let facts = extract(code);
        assert_eq!(facts.tests_without_assertion, vec!["empty".to_string()]);
    }

    #[test]
    fn test_with_assert_eq_is_not_flagged() {
        let code = "#[test]\nfn good() { assert_eq!(1, 1); }";
        let facts = extract(code);
        assert!(facts.tests_without_assertion.is_empty());
    }

    #[test]
    fn test_with_exception_operator_is_not_flagged() {
        let code = "#[test]\nfn good() -> Result<(), String> { let _ = parse(\"\")?; Ok(()) }";
        let facts = extract(code);
        assert!(
            facts.tests_without_assertion.is_empty(),
            "? operator on Result counts as assertion"
        );
    }

    #[test]
    fn test_with_non_assert_macro_is_flagged() {
        // println! is not an assertion macro.
        let code = "#[test]\nfn weak() { println!(\"hello\"); }";
        let facts = extract(code);
        assert_eq!(facts.tests_without_assertion, vec!["weak".to_string()]);
    }

    #[test]
    fn empty_test_with_doc_comment_is_still_flagged() {
        // Regression: is_test_fn used to bail on line_comments preceding #[test],
        // so documented-but-empty tests slipped past warn-empty-test.
        let code = "/// Tests the foo widget.\n#[test]\nfn documented_empty() {}";
        let facts = extract(code);
        assert_eq!(
            facts.tests_without_assertion,
            vec!["documented_empty".to_string()]
        );
    }
}
