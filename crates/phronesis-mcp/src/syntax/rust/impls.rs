use super::super::parsed::ParsedFile;
use super::invocations::{macro_construct, method_construct};
use super::walk::{function_name, in_test_code};

/// `(implementing_type, trait_name)` for trait impl blocks.
pub(super) fn extract_trait_impls(parsed: &ParsedFile) -> Vec<(String, String)> {
    let ParsedFile::Rust { tree, source } = parsed else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cursor = tree.walk();
    walk(&mut cursor, source.as_bytes(), &mut out);
    out
}

fn walk(cursor: &mut tree_sitter::TreeCursor, source: &[u8], out: &mut Vec<(String, String)>) {
    let node = cursor.node();
    if node.kind() == "impl_item"
        && let Some(trait_node) = node.child_by_field_name("trait")
        && let Some(type_node) = node.child_by_field_name("type")
        && let (Ok(trait_text), Ok(type_text)) =
            (trait_node.utf8_text(source), type_node.utf8_text(source))
        && !in_test_code(node, source)
    {
        let trait_name = trait_text.rsplit("::").next().unwrap_or(trait_text);
        out.push((type_text.to_string(), trait_name.to_string()));
    }
    if cursor.goto_first_child() {
        loop {
            walk(cursor, source, out);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
        cursor.goto_parent();
    }
}

/// `(implementing_type, construct)` for panicking constructs — unwrap,
/// empty-message expect, panic!, todo!, unimplemented! — inside the body of a
/// `Drop::drop` implementation. A panic that occurs during unwind inside
/// `Drop::drop` aborts the whole process (`std::process::abort`) rather than
/// failing gracefully, so these constructs are a distinct hazard there.
/// Test code (`#[test]` fns, `#[cfg(test)]` mods) is excluded.
pub(super) fn extract_panic_in_drop(parsed: &ParsedFile) -> Vec<(String, String)> {
    let ParsedFile::Rust { tree, source } = parsed else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cursor = tree.walk();
    walk_drop_impls(&mut cursor, source.as_bytes(), &mut out);
    out
}

fn walk_drop_impls(
    cursor: &mut tree_sitter::TreeCursor,
    source: &[u8],
    out: &mut Vec<(String, String)>,
) {
    let node = cursor.node();
    if node.kind() == "impl_item"
        && let Some(trait_node) = node.child_by_field_name("trait")
        && let Some(type_node) = node.child_by_field_name("type")
        && let (Ok(trait_text), Ok(type_text)) =
            (trait_node.utf8_text(source), type_node.utf8_text(source))
        && is_drop_trait(trait_text)
        && !in_test_code(node, source)
        && let Some(body) = node.child_by_field_name("body")
    {
        let mut body_cursor = body.walk();
        for child in body.children(&mut body_cursor) {
            if child.kind() == "function_item"
                && function_name(child, source).as_deref() == Some("drop")
            {
                let mut fn_cursor = child.walk();
                collect_panicking_constructs(&mut fn_cursor, source, type_text, out);
            }
        }
    }
    if cursor.goto_first_child() {
        loop {
            walk_drop_impls(cursor, source, out);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
        cursor.goto_parent();
    }
}

/// True when a trait text (whitespace stripped) is exactly `Drop` or a path
/// ending in `::Drop` (e.g. `std::ops::Drop`).
fn is_drop_trait(trait_text: &str) -> bool {
    let normalized: String = trait_text.chars().filter(|c| !c.is_whitespace()).collect();
    normalized == "Drop" || normalized.ends_with("::Drop")
}

fn collect_panicking_constructs(
    cursor: &mut tree_sitter::TreeCursor,
    source: &[u8],
    type_name: &str,
    out: &mut Vec<(String, String)>,
) {
    let node = cursor.node();
    let construct = match node.kind() {
        // `dbg` is classified by `macro_construct` but is not a
        // panic-in-unwind hazard, so it is filtered here.
        "macro_invocation" => macro_construct(node, source)
            .filter(|name| matches!(*name, "todo" | "panic" | "unimplemented")),
        "call_expression" => method_construct(node, source),
        _ => None,
    };
    if let Some(construct) = construct
        && !in_test_code(node, source)
    {
        out.push((type_name.to_string(), construct.to_string()));
    }
    if cursor.goto_first_child() {
        loop {
            collect_panicking_constructs(cursor, source, type_name, out);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
        cursor.goto_parent();
    }
}

#[cfg(test)]
mod tests {
    use crate::syntax::rust::extract;

    #[test]
    fn extracts_qualified_and_plain_trait_impls() {
        let facts = extract(
            "impl std::ops::Deref for Wrapper<u8> { type Target = u8; fn deref(&self) -> &u8 { &0 } } impl Display for S {} impl S {}",
        );
        assert_eq!(
            facts.rust_trait_impls,
            vec![
                ("Wrapper<u8>".to_string(), "Deref".to_string()),
                ("S".to_string(), "Display".to_string()),
            ]
        );
    }

    #[test]
    fn ignores_trait_impls_inside_test_code() {
        let facts = extract(
            r#"
struct Prod;
impl Deref for Prod { type Target = u8; fn deref(&self) -> &u8 { &0 } }

#[cfg(test)]
mod tests {
    struct MockWrapper;
    impl Deref for MockWrapper { type Target = u8; fn deref(&self) -> &u8 { &0 } }
}
"#,
        );
        assert_eq!(
            facts.rust_trait_impls,
            vec![("Prod".to_string(), "Deref".to_string())],
            "only the production impl should be reported, not the #[cfg(test)] mod one"
        );
    }

    #[test]
    fn ignores_impl_text_in_comments_and_strings() {
        let facts =
            extract("// impl Deref for Fake {}\nconst NOTE: &str = \"impl Deref for Fake\";\n");
        assert!(facts.rust_trait_impls.is_empty());
    }

    #[test]
    fn detects_each_panicking_construct_inside_drop_impl() {
        let facts = extract(
            r#"
struct A; impl Drop for A { fn drop(&mut self) { self.handle.take().unwrap(); } }
struct B; impl Drop for B { fn drop(&mut self) { self.flush().expect(""); } }
struct C; impl std::ops::Drop for C { fn drop(&mut self) { panic!("boom"); } }
struct D; impl Drop for D { fn drop(&mut self) { todo!(); } }
struct E; impl Drop for E { fn drop(&mut self) { unimplemented!(); } }
"#,
        );
        assert_eq!(
            facts.rust_panic_in_drop,
            vec![
                ("A".to_string(), "unwrap".to_string()),
                ("B".to_string(), "expect_empty".to_string()),
                ("C".to_string(), "panic".to_string()),
                ("D".to_string(), "todo".to_string()),
                ("E".to_string(), "unimplemented".to_string()),
            ]
        );
    }

    #[test]
    fn ignores_panicking_constructs_outside_drop_impls() {
        let facts = extract(
            r#"
struct S;
impl Display for S { fn fmt(&self) -> u8 { self.value.unwrap(); panic!("x") } }
impl S { fn drop_like(&mut self) { todo!() } }
fn free() { unimplemented!(); Some(1).expect(""); }
"#,
        );
        assert!(
            facts.rust_panic_in_drop.is_empty(),
            "non-Drop impls and free functions must not fire: {:?}",
            facts.rust_panic_in_drop
        );
    }

    #[test]
    fn ignores_non_panic_constructs_inside_drop_impl() {
        let facts = extract(
            r#"
struct S;
impl Drop for S {
    fn drop(&mut self) {
        dbg!(&self.value);
        std::env::set_var("KEY", "value");
        self.value.expect("documented invariant");
    }
}
"#,
        );
        assert!(
            facts.rust_panic_in_drop.is_empty(),
            "dbg/env_set_var/non-empty expect are not panic-in-unwind hazards: {:?}",
            facts.rust_panic_in_drop
        );
    }

    #[test]
    fn ignores_panicking_drop_impls_inside_test_code() {
        let facts = extract(
            r#"
struct Prod;
impl Drop for Prod { fn drop(&mut self) { self.value.take().unwrap(); } }

#[cfg(test)]
mod tests {
    struct MockGuard;
    impl Drop for MockGuard { fn drop(&mut self) { panic!("test-only"); } }
}
"#,
        );
        assert_eq!(
            facts.rust_panic_in_drop,
            vec![("Prod".to_string(), "unwrap".to_string())],
            "only the production Drop impl should be reported, not the #[cfg(test)] mod one"
        );
    }

    #[test]
    fn malformed_partial_drop_source_does_not_panic() {
        let _ = extract("impl Drop for Broken { fn drop(&mut self) { panic!(");
        let _ = extract("impl Drop for");
    }
}
