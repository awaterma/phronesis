use super::super::parsed::ParsedFile;
use super::walk::{function_name, is_pub_fn_node, is_test_fn};

/// `pub fn` declarations whose preceding region contains no doc comment.
/// Exempts:
/// - functions inside `impl Trait for Type` blocks (trait provides docs)
/// - functions with `#[test]` attribute
pub(super) fn extract_pub_fns_without_doc_comment(parsed: &ParsedFile) -> Vec<String> {
    let ParsedFile::Rust { tree, source } = parsed else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cursor = tree.walk();
    walk_pub_fns_without_doc_comment(&mut cursor, source.as_bytes(), &mut out);
    out
}

fn walk_pub_fns_without_doc_comment(
    cursor: &mut tree_sitter::TreeCursor,
    source: &[u8],
    out: &mut Vec<String>,
) {
    let state = cursor.node();
    if state.kind() == "function_item"
        && is_pub_fn_node(state, source)
        && !is_test_fn(state, source)
        && !is_inside_trait_impl(state)
        && !has_preceding_doc_comment(state, source)
        && let Some(name) = function_name(state, source)
    {
        out.push(name);
    }
    if cursor.goto_first_child() {
        loop {
            walk_pub_fns_without_doc_comment(cursor, source, out);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
        cursor.goto_parent();
    }
}

/// Walk backward through preceding siblings. Returns true if a doc comment
/// (`///` or `/**`) or `#[doc = "..."]` attribute is found. Regular `//`
/// comments and non-doc attributes are walked past; any other state kind
/// terminates the walk with `false`.
fn has_preceding_doc_comment(state: tree_sitter::Node, source: &[u8]) -> bool {
    let mut prev = state.prev_sibling();
    while let Some(sib) = prev {
        let text = sib.utf8_text(source).unwrap_or("");
        match sib.kind() {
            "line_comment" => {
                if text.trim_start().starts_with("///") {
                    return true;
                }
                prev = sib.prev_sibling();
            }
            "block_comment" => {
                if text.trim_start().starts_with("/**") {
                    return true;
                }
                prev = sib.prev_sibling();
            }
            "attribute_item" => {
                if text.trim_start().starts_with("#[doc") {
                    return true;
                }
                prev = sib.prev_sibling();
            }
            _ => return false,
        }
    }
    false
}

/// True iff `state` is inside an `impl Trait for Type` block. A plain
/// inherent `impl Type { ... }` does NOT count (its methods are the type's
/// own public surface and warrant docs).
fn is_inside_trait_impl(state: tree_sitter::Node) -> bool {
    let mut p = state.parent();
    while let Some(parent) = p {
        if parent.kind() == "impl_item" {
            return parent.child_by_field_name("trait").is_some();
        }
        p = parent.parent();
    }
    false
}

#[cfg(test)]
mod tests {
    use crate::syntax::rust::extract;

    #[test]
    fn pub_fn_with_doc_comment_is_not_flagged() {
        let code = "/// Frobnicate the foo.\npub fn frobnicate() {}";
        let facts = extract(code);
        assert!(facts.pub_fns_without_doc_comment.is_empty());
    }

    #[test]
    fn pub_fn_without_any_comment_is_flagged() {
        let code = "pub fn naked() {}";
        let facts = extract(code);
        assert_eq!(facts.pub_fns_without_doc_comment, vec!["naked".to_string()]);
    }

    #[test]
    fn pub_fn_with_doc_comment_then_attribute_is_not_flagged() {
        let code = "/// Frobnicate.\n#[inline]\npub fn frobnicate() {}";
        let facts = extract(code);
        assert!(
            facts.pub_fns_without_doc_comment.is_empty(),
            "attribute between doc and fn must NOT block; got {:?}",
            facts.pub_fns_without_doc_comment
        );
    }

    #[test]
    fn pub_fn_inside_trait_impl_is_exempt() {
        let code = "impl SomeTrait for Foo { pub fn method() {} }";
        let facts = extract(code);
        assert!(
            facts.pub_fns_without_doc_comment.is_empty(),
            "trait-impl methods inherit docs from the trait"
        );
    }

    #[test]
    fn pub_fn_with_test_attribute_is_exempt() {
        let code = "#[test]\npub fn test_something() {}";
        let facts = extract(code);
        assert!(
            facts.pub_fns_without_doc_comment.is_empty(),
            "tests don't need public docs"
        );
    }
}
