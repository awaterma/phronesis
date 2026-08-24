/// Shared tree-walking utilities used by every extractor submodule.
/// Kept in a private `walk` module so siblings access them via `super::walk::*`.
use tree_sitter::{Node, TreeCursor};

/// Recursively visits every `function_item` in the tree. Trait method
/// declarations without bodies (`function_signature_item`) are intentionally
/// skipped — predicates here describe implementations, not signatures.
/// Calls `f(fn_node, name)` for each match. Lets predicates share traversal logic.
pub fn walk_function_items<F: FnMut(Node, &str)>(
    walker: &mut TreeCursor,
    source: &[u8],
    f: &mut F,
) {
    let state = walker.node();
    if state.kind() == "function_item" {
        let name = state
            .child_by_field_name("name")
            .and_then(|n| n.utf8_text(source).ok())
            .unwrap_or("");
        if !name.is_empty() {
            f(state, name);
        }
    }
    if walker.goto_first_child() {
        loop {
            walk_function_items(walker, source, f);
            if !walker.goto_next_sibling() {
                break;
            }
        }
        walker.goto_parent();
    }
}

/// Recursively visits every `struct_item` in the tree, calling
/// `f(struct_state, name)` for each. Analogous to `walk_function_items`.
pub fn walk_struct_items<F: FnMut(Node, &str)>(walker: &mut TreeCursor, source: &[u8], f: &mut F) {
    let state = walker.node();
    if state.kind() == "struct_item" {
        let name = state
            .child_by_field_name("name")
            .and_then(|n| n.utf8_text(source).ok())
            .unwrap_or("");
        if !name.is_empty() {
            f(state, name);
        }
    }
    if walker.goto_first_child() {
        loop {
            walk_struct_items(walker, source, f);
            if !walker.goto_next_sibling() {
                break;
            }
        }
        walker.goto_parent();
    }
}

/// Returns the function name from a `function_item` node.
pub fn function_name(state: Node, source: &[u8]) -> Option<String> {
    state
        .child_by_field_name("name")
        .and_then(|n| n.utf8_text(source).ok())
        .map(|s| s.to_string())
}

/// True if any preceding-sibling attribute is `#[test]` or `#[tokio::test...]`.
pub fn is_test_fn(state: Node, source: &[u8]) -> bool {
    let mut prev = state.prev_sibling();
    while let Some(sib) = prev {
        match sib.kind() {
            "attribute_item" => {
                let text = sib.utf8_text(source).unwrap_or("");
                if text.contains("#[test]") || text.contains("#[tokio::test") {
                    return true;
                }
                prev = sib.prev_sibling();
            }
            "line_comment" | "block_comment" => {
                // Doc or regular comments don't break the attribute chain;
                // a documented #[test] should still be recognized as a test.
                prev = sib.prev_sibling();
            }
            _ => return false,
        }
    }
    false
}

/// True when `node` sits in test code: inside a `#[test]`/`#[tokio::test]`
/// function, or anywhere under a `#[cfg(test)]` module.
///
/// Dogfooding these rules on Phronesis produced 34 of 39 blocking-call hits in
/// test bodies. Blocking I/O in a test is not a latency defect, and the noise
/// buries the real findings — four production `async fn`s on the hook path.
/// The ownership extractor excludes test bodies for the same reason (D14).
pub fn in_test_code(node: Node, source: &[u8]) -> bool {
    let mut current = Some(node);
    while let Some(candidate) = current {
        match candidate.kind() {
            "function_item" if is_test_fn(candidate, source) => return true,
            "mod_item" if has_cfg_test_attribute(candidate, source) => return true,
            _ => {}
        }
        current = candidate.parent();
    }
    false
}

/// True if a preceding-sibling attribute on `node` is `#[cfg(test)]`.
fn has_cfg_test_attribute(node: Node, source: &[u8]) -> bool {
    let mut prev = node.prev_sibling();
    while let Some(sibling) = prev {
        match sibling.kind() {
            "attribute_item" => {
                let text = sibling.utf8_text(source).unwrap_or("");
                if text.replace(char::is_whitespace, "").contains("cfg(test)") {
                    return true;
                }
                prev = sibling.prev_sibling();
            }
            "line_comment" | "block_comment" => prev = sibling.prev_sibling(),
            _ => return false,
        }
    }
    false
}

/// True if `state` is a `function_item` whose visibility_modifier is exactly
/// `pub` (not `pub(crate)`, `pub(super)`, etc.). Mirrors `extract_public_functions`.
pub fn is_pub_fn_node(state: Node, source: &[u8]) -> bool {
    let mut c = state.walk();
    for child in state.children(&mut c) {
        if child.kind() == "visibility_modifier" {
            let text = child.utf8_text(source).unwrap_or("").trim();
            return text == "pub";
        }
    }
    false
}
