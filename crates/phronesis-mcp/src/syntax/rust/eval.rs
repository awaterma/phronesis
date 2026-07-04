use super::super::parsed::ParsedFile;
use super::walk::function_name;

/// Functions that call `engine.eval(...)` or `engine.eval::<T>(...)` with
/// a string literal as the first argument.
pub(super) fn extract_engine_eval_string_literals(parsed: &ParsedFile) -> Vec<String> {
    let ParsedFile::Rust { tree, source } = parsed else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cursor = tree.walk();
    walk_engine_eval_string_literals(&mut cursor, source.as_bytes(), &mut out, None);
    out
}

fn walk_engine_eval_string_literals(
    cursor: &mut tree_sitter::TreeCursor,
    source: &[u8],
    out: &mut Vec<String>,
    enclosing_fn: Option<String>,
) {
    let state = cursor.node();
    let new_enclosing = if state.kind() == "function_item" {
        function_name(state, source).or(enclosing_fn.clone())
    } else {
        enclosing_fn.clone()
    };

    if state.kind() == "call_expression"
        && let Some(name) = new_enclosing.as_deref()
        && call_is_eval_with_string_literal(state, source)
        && !out.iter().any(|n| n == name)
    {
        out.push(name.to_string());
    }

    if cursor.goto_first_child() {
        loop {
            walk_engine_eval_string_literals(cursor, source, out, new_enclosing.clone());
            if !cursor.goto_next_sibling() {
                break;
            }
        }
        cursor.goto_parent();
    }
}

/// True iff `state` is a call_expression of shape `<expr>.eval(<string_literal>, ...)`
/// or `<expr>.eval::<T>(<string_literal>, ...)`.
fn call_is_eval_with_string_literal(state: tree_sitter::Node, source: &[u8]) -> bool {
    let Some(function) = state.child_by_field_name("function") else {
        return false;
    };

    // Unwrap turbofish: generic_function -> field_expression
    let field_expr = match function.kind() {
        "field_expression" => function,
        "generic_function" => {
            let mut c = function.walk();
            let inner = function
                .children(&mut c)
                .find(|n| n.kind() == "field_expression");
            match inner {
                Some(n) => n,
                None => return false,
            }
        }
        _ => return false,
    };

    let Some(field) = field_expr.child_by_field_name("field") else {
        return false;
    };
    if field.utf8_text(source).unwrap_or("") != "eval" {
        return false;
    }

    let Some(args) = state.child_by_field_name("arguments") else {
        return false;
    };
    let mut c = args.walk();
    for child in args.children(&mut c) {
        if matches!(child.kind(), "(" | ")" | ",") {
            continue;
        }
        return child.kind() == "string_literal";
    }
    false
}

#[cfg(test)]
mod tests {
    use crate::syntax::rust::extract;

    #[test]
    fn engine_eval_string_literal_is_flagged() {
        let code = r#"
fn host() {
    let engine = rhai::Engine::new();
    let result: i64 = engine.eval("40 + 2").unwrap();
}
"#;
        let facts = extract(code);
        assert_eq!(facts.engine_eval_string_literals, vec!["host".to_string()]);
    }

    #[test]
    fn engine_eval_variable_is_not_flagged() {
        let code = r#"
fn host() {
    let engine = rhai::Engine::new();
    let script = load_script();
    let result: i64 = engine.eval(&script).unwrap();
}
"#;
        let facts = extract(code);
        assert!(
            facts.engine_eval_string_literals.is_empty(),
            "non-literal arg must not flag"
        );
    }

    #[test]
    fn engine_eval_turbofish_string_literal_is_flagged() {
        let code = r#"
fn host() {
    let engine = rhai::Engine::new();
    let _: i64 = engine.eval::<i64>("40 + 2").unwrap();
}
"#;
        let facts = extract(code);
        assert_eq!(facts.engine_eval_string_literals, vec!["host".to_string()]);
    }

    #[test]
    fn engine_eval_include_str_is_not_flagged() {
        let code = r#"
fn host() {
    let engine = rhai::Engine::new();
    let _: i64 = engine.eval(include_str!("script.rhai")).unwrap();
}
"#;
        let facts = extract(code);
        assert!(
            facts.engine_eval_string_literals.is_empty(),
            "include_str! macro arg is not a string literal"
        );
    }
}
