use super::super::parsed::ParsedFile;
use super::walk::walk_struct_items;

/// One `(struct, trait)` entry per `#[derive(...)]` argument across the file.
/// Recognizes `derive` attributes attached directly to a `struct_item` as a
/// preceding sibling `attribute_item` under the same parent.
///
/// AST shape (verified against tree-sitter-rust 0.23):
///   `attribute_item -> attribute -> identifier("derive") + arguments: token_tree`
///   `token_tree` contains `identifier` children for each derived trait.
///
/// Grammar caveat: `attribute_item.attribute` and `attribute.path` are regular
/// children, not fields, in tree-sitter-rust 0.23. Only `attribute.arguments`
/// is exposed as a field.
pub(super) fn extract_struct_derives(parsed: &ParsedFile) -> Vec<(String, String)> {
    let ParsedFile::Rust { tree, source } = parsed else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut walker = tree.walk();
    walk_struct_items(&mut walker, source.as_bytes(), &mut |struct_state, name| {
        // Look for sibling attribute_item states immediately preceding the struct.
        // Attributes are children of the struct's parent state (source_file / mod_item),
        // appearing as siblings before the struct_item.
        let Some(parent) = struct_state.parent() else {
            return;
        };
        let mut parent_walker = parent.walk();
        let mut prev_attrs: Vec<tree_sitter::Node> = Vec::new();
        for child in parent.children(&mut parent_walker) {
            if child.id() == struct_state.id() {
                break;
            }
            if child.kind() == "attribute_item" {
                prev_attrs.push(child);
            } else if matches!(child.kind(), "line_comment" | "block_comment") {
                // Comments don't separate attrs from the item they decorate.
            } else {
                // Anything else (use_declaration, extern_crate_declaration, item kinds,
                // expression_statement wrapping a top-level macro, etc.) flushes pending
                // attributes. Inverting the policy this way means we don't have to
                // enumerate every kind tree-sitter-rust exposes, and remains correct
                // when the grammar grows new item kinds.
                prev_attrs.clear();
            }
        }
        for attr_state in prev_attrs {
            collect_derives_from_attr(attr_state, source.as_bytes(), name, &mut out);
        }
    });
    out
}

/// Walk an `attribute_item` and, if it's a `#[derive(...)]`, push one
/// `(struct_name, trait_name)` per identifier in the token_tree.
fn collect_derives_from_attr(
    attr: tree_sitter::Node,
    source: &[u8],
    struct_name: &str,
    out: &mut Vec<(String, String)>,
) {
    // attribute_item -> attribute (regular child, no field name)
    let mut aw = attr.walk();
    let Some(inner) = attr.children(&mut aw).find(|c| c.kind() == "attribute") else {
        return;
    };
    if !attr_is_derive(inner, source) {
        return;
    }
    let Some(args) = inner.child_by_field_name("arguments") else {
        return;
    };
    if args.kind() != "token_tree" {
        return;
    }
    push_token_tree_idents(args, source, struct_name, out);
}

/// True when the `attribute` node's path names `derive`. The attribute's
/// path is an `identifier` child (or `scoped_identifier` — e.g.
/// `core::prelude::v1::derive`, where the last segment is the path tail);
/// only the first path-shaped child is consulted.
fn attr_is_derive(inner: tree_sitter::Node, source: &[u8]) -> bool {
    let mut iw = inner.walk();
    for child in inner.children(&mut iw) {
        if child.kind() == "identifier" {
            return child.utf8_text(source).unwrap_or("") == "derive";
        }
        if child.kind() == "scoped_identifier" {
            let txt = child.utf8_text(source).unwrap_or("");
            return txt.rsplit("::").next() == Some("derive");
        }
    }
    false
}

/// Walk the derive token_tree; identifiers inside it are derived traits.
/// Pushes one `(struct_name, trait_name)` per identifier.
fn push_token_tree_idents(
    args: tree_sitter::Node,
    source: &[u8],
    struct_name: &str,
    out: &mut Vec<(String, String)>,
) {
    let mut tw = args.walk();
    for child in args.children(&mut tw) {
        if child.kind() == "identifier" {
            let trait_name = child.utf8_text(source).unwrap_or("");
            if !trait_name.is_empty() {
                out.push((struct_name.to_string(), trait_name.to_string()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::syntax::rust::extract;

    #[test]
    fn extracts_single_derive() {
        let code = "#[derive(Debug)]\nstruct Foo {}";
        let facts = extract(code);
        assert_eq!(
            facts.struct_derives,
            vec![("Foo".to_string(), "Debug".to_string())]
        );
    }

    #[test]
    fn extracts_multiple_derives() {
        let code = "#[derive(Debug, Clone, PartialEq)]\nstruct Foo {}";
        let facts = extract(code);
        assert_eq!(
            facts.struct_derives,
            vec![
                ("Foo".to_string(), "Debug".to_string()),
                ("Foo".to_string(), "Clone".to_string()),
                ("Foo".to_string(), "PartialEq".to_string()),
            ]
        );
    }

    #[test]
    fn ignores_non_derive_attributes() {
        let code = "#[serde(rename = \"foo\")]\nstruct Foo {}";
        let facts = extract(code);
        assert!(facts.struct_derives.is_empty());
    }

    #[test]
    fn no_facts_for_struct_without_derives() {
        let code = "struct Foo {}";
        let facts = extract(code);
        assert!(facts.struct_derives.is_empty());
    }

    #[test]
    fn does_not_leak_attr_through_use_declaration() {
        // Mid-edit/illegal Rust: derive on a use statement. Must not leak the
        // derive onto the next struct.
        let code = "#[derive(Clone)]\nuse std::io;\nstruct Foo {}";
        let facts = extract(code);
        assert!(
            facts.struct_derives.is_empty(),
            "derive attached to a use should not propagate to Foo: {:?}",
            facts.struct_derives
        );
    }

    #[test]
    fn extracts_derive_on_nested_struct() {
        // Lock in that the walker handles nested structs (struct inside a fn body).
        let code = "fn outer() { #[derive(Debug)] struct Inner {} }";
        let facts = extract(code);
        assert_eq!(
            facts.struct_derives,
            vec![("Inner".to_string(), "Debug".to_string())]
        );
    }
}
