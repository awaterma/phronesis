use super::super::parsed::ParsedFile;
use super::walk::in_test_code;

#[derive(Default)]
pub(super) struct GovernedTypes {
    pub primitive_id_fields: Vec<(String, String)>,
    pub rc_refcell_count: usize,
}

pub(super) fn extract_governed_types(parsed: &ParsedFile) -> GovernedTypes {
    let ParsedFile::Rust { tree, source } = parsed else {
        return GovernedTypes::default();
    };
    let mut out = GovernedTypes::default();
    let mut cursor = tree.walk();
    walk(&mut cursor, source.as_bytes(), &mut out);
    out
}

fn walk(cursor: &mut tree_sitter::TreeCursor, source: &[u8], out: &mut GovernedTypes) {
    let node = cursor.node();
    if node.kind() == "field_declaration"
        && let Some(name) = node.child_by_field_name("name")
        && let Some(ty) = node.child_by_field_name("type")
        && let (Ok(name), Ok(ty)) = (name.utf8_text(source), ty.utf8_text(source))
        && name.ends_with("_id")
        && ty.trim() == "u64"
        && !in_test_code(node, source)
    {
        out.primitive_id_fields
            .push((name.to_string(), ty.trim().to_string()));
    }
    if node.kind() == "generic_type"
        && node.utf8_text(source).is_ok_and(|text| {
            text.chars()
                .filter(|ch| !ch.is_whitespace())
                .collect::<String>()
                .starts_with("Rc<RefCell<")
        })
        && !in_test_code(node, source)
    {
        out.rc_refcell_count += 1;
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
    fn extracts_primitive_id_fields_and_rc_refcell_shapes() {
        let facts = extract(
            "struct Row { user_id: u64, external_id: String, id: u64, value: Rc < RefCell <u8> > }",
        );
        assert_eq!(
            facts.rust_primitive_id_fields,
            vec![("user_id".to_string(), "u64".to_string())]
        );
        assert_eq!(facts.rust_rc_refcell_count, 1);
    }

    #[test]
    fn ignores_governed_types_inside_test_code() {
        let facts = extract(
            r#"
struct Prod { user_id: u64, value: Rc<RefCell<u8>> }

#[test]
fn t() {
    struct Local { session_id: u64, value: Rc<RefCell<u8>> }
}

#[cfg(test)]
mod tests {
    struct Fixture { account_id: u64, value: Rc<RefCell<u8>> }
}
"#,
        );
        assert_eq!(
            facts.rust_primitive_id_fields,
            vec![("user_id".to_string(), "u64".to_string())],
            "only the production id field should be reported, not the #[test] fn or #[cfg(test)] mod"
        );
        assert_eq!(
            facts.rust_rc_refcell_count, 1,
            "only the production Rc<RefCell<>> should be counted"
        );
    }

    #[test]
    fn ignores_comments_strings_and_neighboring_types() {
        let facts = extract(
            r#"
// struct Fake { user_id: u64, value: Rc<RefCell<u8>> }
const NOTE: &str = "Rc<RefCell< user_id: u64";
struct Safe { user: u64, value: Arc<Mutex<u8>> }
"#,
        );
        assert!(facts.rust_primitive_id_fields.is_empty());
        assert_eq!(facts.rust_rc_refcell_count, 0);
    }
}
