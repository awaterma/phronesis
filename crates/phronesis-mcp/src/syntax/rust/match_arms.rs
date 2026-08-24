use super::super::parsed::ParsedFile;
use super::walk::{function_name, in_test_code};

/// `(enclosing_function, shape)` for starter-rule match-arm shapes.
pub(super) fn extract_governed_match_arms(parsed: &ParsedFile) -> Vec<(String, String)> {
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
    if node.kind() == "match_arm"
        && let Some(shape) = classify_arm(node, source)
        && !in_test_code(node, source)
    {
        out.push((
            enclosing_fn
                .clone()
                .unwrap_or_else(|| "<module>".to_string()),
            shape.to_string(),
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

fn classify_arm(node: tree_sitter::Node, source: &[u8]) -> Option<&'static str> {
    let pattern = node.child_by_field_name("pattern")?;
    let value = node.child_by_field_name("value")?;
    let pattern_text: String = pattern
        .utf8_text(source)
        .ok()?
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect();

    if value.kind() == "block" && value.named_child_count() == 0 {
        return match pattern_text.as_str() {
            "None" => Some("none_empty"),
            "Err(_)" => Some("err_empty"),
            _ => None,
        };
    }

    if value.kind() == "return_expression"
        && let Some(expression) = value.named_child(0)
        && expression.kind() == "call_expression"
        && let Some(function) = expression.child_by_field_name("function")
        && function.utf8_text(source).is_ok_and(|text| text == "Err")
    {
        return Some("return_err");
    }
    None
}

#[cfg(test)]
mod tests {
    use crate::syntax::rust::extract;

    #[test]
    fn classifies_empty_and_return_err_arms_across_formatting() {
        let facts = extract(
            r#"
fn demo(a: Option<u8>, b: Result<u8, E>) -> Result<(), E> {
    match a { None => { }, Some(_) => {} }
    match b { Err ( _ ) => { }, Ok(_) => {} }
    match b { Err(error) => return Err(error), Ok(_) => {} }
    Ok(())
}
"#,
        );
        assert_eq!(
            facts.rust_governed_match_arms,
            vec![
                ("demo".to_string(), "none_empty".to_string()),
                ("demo".to_string(), "err_empty".to_string()),
                ("demo".to_string(), "return_err".to_string()),
            ]
        );
    }

    #[test]
    fn ignores_comments_strings_nonempty_arms_and_non_err_returns() {
        let facts = extract(
            r#"
fn safe(value: Result<u8, E>) -> Result<(), E> {
    // None => {}; Err(_) => {}; => return Err(
    let _ = "None => {} Err(_) => {}";
    match value { Err(_) => { log(); }, Ok(_) => return Ok(()) }
}
"#,
        );
        assert!(facts.rust_governed_match_arms.is_empty());
    }

    #[test]
    fn ignores_governed_match_arms_inside_test_code() {
        let facts = extract(
            r#"
fn prod(value: Result<u8, E>) {
    match value { Err(_) => {}, Ok(_) => {} }
}

#[test]
fn t(value: Result<u8, E>) {
    match value { Err(_) => {}, Ok(_) => {} }
}

#[cfg(test)]
mod tests {
    fn helper(a: Option<u8>, b: Result<u8, E>) -> Result<(), E> {
        match a { None => {}, Some(_) => {} }
        match b { Err(error) => return Err(error), Ok(_) => {} }
        Ok(())
    }
}
"#,
        );
        assert_eq!(
            facts.rust_governed_match_arms,
            vec![("prod".to_string(), "err_empty".to_string())],
            "only the production match arm should be reported, not the #[test] fn or #[cfg(test)] mod"
        );
    }
}
