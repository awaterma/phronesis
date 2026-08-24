use super::super::parsed::ParsedFile;
use super::walk::in_test_code;

/// Stable names for crate/item attributes governed by starter rules.
pub(super) fn extract_governed_attributes(parsed: &ParsedFile) -> Vec<String> {
    let ParsedFile::Rust { tree, source } = parsed else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cursor = tree.walk();
    walk(&mut cursor, source.as_bytes(), &mut out);
    out
}

fn walk(cursor: &mut tree_sitter::TreeCursor, source: &[u8], out: &mut Vec<String>) {
    let node = cursor.node();
    if node.kind() == "inner_attribute_item" || node.kind() == "attribute_item" {
        let normalized = node
            .utf8_text(source)
            .unwrap_or("")
            .chars()
            .filter(|ch| !ch.is_whitespace())
            .collect::<String>();
        if normalized == "#![deny(warnings)]" && !in_test_code(node, source) {
            out.push("deny_warnings".to_string());
        }
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

#[cfg(test)]
mod tests {
    use crate::syntax::rust::extract;

    #[test]
    fn detects_deny_warnings_across_whitespace() {
        let facts = extract("#![ deny ( warnings ) ]\nfn main() {}\n");
        assert_eq!(facts.rust_governed_attributes, vec!["deny_warnings"]);
    }

    #[test]
    fn ignores_governed_attributes_inside_test_code() {
        let facts = extract(
            r#"
#![deny(warnings)]

#[test]
fn t() {
    #![deny(warnings)]
}

#[cfg(test)]
mod tests {
    #![deny(warnings)]
}
"#,
        );
        assert_eq!(
            facts.rust_governed_attributes,
            vec!["deny_warnings"],
            "only the crate-root attribute should be reported, not the #[test] fn or #[cfg(test)] mod"
        );
    }

    #[test]
    fn ignores_comments_strings_and_other_lints() {
        let facts = extract(
            r##"
// #![deny(warnings)]
const TEXT: &str = "#![deny(warnings)]";
#![warn(warnings)]
"##,
        );
        assert!(facts.rust_governed_attributes.is_empty());
    }
}
