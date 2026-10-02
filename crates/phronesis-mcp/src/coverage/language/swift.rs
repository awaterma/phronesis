//! Swift function sites: `swift_function_sites` names every function-like
//! declaration the way the graph's Swift ids do — free functions, methods,
//! inits and deinits, nested functions — with extension methods qualified
//! under the extended type. llvm-cov's lcov `FN` names are mangled, so the
//! registry row says `one_liner_needs_fnda: false` and one-line Swift
//! functions are reported `unattributable` rather than guessed.

use crate::coverage::region_map::{
    FunctionSite, RawSite, assign_function_ordinals, inclusive_end_line,
};
use crate::syntax::parsed::ParsedFile;
use tree_sitter::Node;

/// Declared name of a `class_declaration` (class, struct, enum, actor, or
/// extension — tree-sitter-swift uses one kind for all of them), mirroring
/// the graph's `type_name` (`graph/swift.rs:131-149`): an extension names
/// its target through a `user_type` whose first `type_identifier` wins.
fn type_name(node: Node<'_>, src: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "type_identifier" => return node_text(child, src).map(str::to_string),
            "user_type" => {
                let mut inner = child.walk();
                if let Some(id) = child
                    .children(&mut inner)
                    .find(|c| c.kind() == "type_identifier")
                {
                    return node_text(id, src).map(str::to_string);
                }
            }
            _ => {}
        }
    }
    None
}

fn node_text<'a>(node: Node<'_>, src: &'a [u8]) -> Option<&'a str> {
    node.utf8_text(src).ok().filter(|t| !t.is_empty())
}

fn function_name(node: Node<'_>, src: &[u8]) -> Option<String> {
    if node.kind() == "init_declaration" || node.kind() == "deinit_declaration" {
        return Some(
            node.child_by_field_name("name")
                .and_then(|n| node_text(n, src))
                .unwrap_or(if node.kind() == "init_declaration" {
                    "init"
                } else {
                    "deinit"
                })
                .to_string(),
        );
    }
    node.child_by_field_name("name")
        .and_then(|n| node_text(n, src))
        .or_else(|| {
            let mut cursor = node.walk();
            node.children(&mut cursor).find_map(|c| match c.kind() {
                "simple_identifier" => node_text(c, src),
                _ => None,
            })
        })
        .map(str::to_string)
}

fn walk(node: Node<'_>, src: &[u8], scope: &mut Vec<String>, out: &mut Vec<RawSite>) {
    match node.kind() {
        "class_declaration" | "protocol_declaration" => {
            let pushed = type_name(node, src).is_some_and(|name| {
                scope.push(name);
                true
            });
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                walk(child, src, scope, out);
            }
            if pushed {
                scope.pop();
            }
            return;
        }
        "function_declaration" | "init_declaration" | "deinit_declaration" => {
            let body = node
                .child_by_field_name("body")
                .filter(|b| b.kind() == "function_body");
            // Protocol requirements (and other bodyless declarations) are
            // declarations, not definitions: no site.
            if let Some(body) = body {
                let start_line = node.start_position().row as u64 + 1;
                let name = function_name(node, src).unwrap_or_else(|| {
                    if node.kind() == "init_declaration" {
                        "init"
                    } else {
                        "deinit"
                    }
                    .to_string()
                });
                let mut path = scope.clone();
                path.push(name.clone());
                out.push(RawSite {
                    path: path.join("::"),
                    start_line,
                    body_start_line: body.start_position().row as u64 + 1,
                    end_line: inclusive_end_line(node),
                });
                scope.push(name);
                walk(body, src, scope, out);
                scope.pop();
                return;
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(child, src, scope, out);
    }
}

pub fn swift_function_sites(source: &str) -> anyhow::Result<Vec<FunctionSite>> {
    let parsed = ParsedFile::parse_swift(source)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter failed to parse Swift source"))?;
    let ParsedFile::Swift { tree, source: text } = &parsed else {
        anyhow::bail!("parse_swift returned a non-Swift tree")
    };
    let mut raw = Vec::new();
    walk(tree.root_node(), text.as_bytes(), &mut Vec::new(), &mut raw);
    Ok(assign_function_ordinals(raw))
}

/// A Swift production source, agreeing with the graph's `file_type`
/// classifier (`graph/swift.rs:41-59`): test-classified files (target
/// context aside — a path alone cannot know that), `Package.swift`, and
/// build artefacts under `.build/` carry no production regions.
pub fn is_wanted_source(rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    if name == "Package.swift" {
        return false;
    }
    if name.ends_with("Tests.swift") || name.ends_with("Test.swift") {
        return false;
    }
    let in_test_dir = rel
        .split('/')
        .take_while(|seg| !seg.ends_with(".swift"))
        .any(|seg| seg == "Tests" || seg.ends_with("Tests"));
    if in_test_dir {
        return false;
    }
    !rel.split('/').any(|seg| seg == ".build")
}

/// Brace rows use `end_line == start_line` (Revision 2): the `{` shares the
/// declaration line, so `body_start_line == start_line` would swallow every
/// multi-line brace function.
pub fn is_one_liner(site: &FunctionSite) -> bool {
    site.end_line == site.start_line
}

/// `select` renders a runnable command from the graph test id; the fixture
/// file itself is not needed (the filter selects by test name). The selector
/// shape is probe-pinned: `^<module>.<scope>/<name>$` matched both an XCTest
/// class and a Swift Testing suite live on Swift 6.4.
pub fn render_command(test_id: &str, _file: &str) -> Option<String> {
    selector_from_test_id(test_id).map(|sel| format!("swift test --filter '{sel}'"))
}

/// Parse a graph Swift test id into (module, scope, name). Real shape
/// (`graph/swift.rs:238-257`, asserted at `:493-501`):
/// `swift:<unit>::<file segments>::[<type scopes>::]<name>` — the unit is
/// the SwiftPM target name, the second-to-last segment is the declaring
/// scope (an XCTestCase class or a Swift Testing suite).
pub(crate) fn test_id_parts(test_id: &str) -> Option<(String, String, String)> {
    let rest = test_id.strip_prefix("swift:")?;
    let segments: Vec<&str> = rest.split("::").collect();
    if segments.len() < 3 {
        return None;
    }
    let name = segments[segments.len() - 1].to_string();
    let scope = segments[segments.len() - 2].to_string();
    let module = segments[0].replace('-', "_");
    Some((module, scope, name))
}

/// The `swift test --filter` selector for a graph test id, anchored so a
/// regex prefix cannot drag in sibling tests (probe: the unanchored form
/// `StoreTests/testLoad` also matched `testLoadAgain`). Shape
/// `^<module>.<scope>/<name>$` — the same form matched both an XCTest class
/// and a Swift Testing suite live (Swift 6.4); the module name is the
/// SwiftPM target with `-` mapped to `_` (probe: `store-kitTests` →
/// `store_kitTests`).
pub fn selector_from_test_id(test_id: &str) -> Option<String> {
    let (module, scope, name) = test_id_parts(test_id)?;
    Some(format!(
        "^{}\\.{}/{}$",
        regex::escape(&module),
        regex::escape(&scope),
        regex::escape(&name)
    ))
}

/// The runner-native name for `# node:` records: the unanchored selector.
pub fn runner_native_name(test_id: &str) -> Option<String> {
    let (module, scope, name) = test_id_parts(test_id)?;
    Some(format!("{module}.{scope}/{name}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coverage::language::language_for_path;

    const FIXTURE: &str = "func helper(_ x: Int) -> Int {\n\
                   \x20   return x + 1\n\
                   }\n\
                   struct Store {\n\
                   \x20   init() {\n\
                   \x20   }\n\
                   \x20   func load() -> Int {\n\
                   \x20       func inner() -> Int {\n\
                   \x20           return 1\n\
                   \x20       }\n\
                   \x20       return inner()\n\
                   \x20   }\n\
                   \x20   func load(_ again: Int) -> Int {\n\
                   \x20       return again\n\
                   \x20   }\n\
                   \x20   func oneLiner() -> Int { 3 }\n\
                   }\n\
                   extension Store {\n\
                   \x20   static func make() -> Store { Store() }\n\
                   }\n";

    #[test]
    fn swift_sites_cover_free_functions_methods_inits_nested_and_overloads() {
        let sites = swift_function_sites(FIXTURE).expect("parses");
        let rows: Vec<(&str, u64, u64, u64)> = sites
            .iter()
            .map(|s| {
                (
                    s.item_path.as_str(),
                    s.start_line,
                    s.body_start_line,
                    s.end_line,
                )
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                ("helper", 1, 1, 3),
                ("Store::init", 5, 5, 6),
                ("Store::load", 7, 7, 12),
                ("Store::load::inner", 8, 8, 10),
                ("Store::load.2", 13, 13, 15),
                ("Store::oneLiner", 16, 16, 16),
                ("Store::make", 19, 19, 19),
            ],
            "{sites:?}"
        );
    }

    #[test]
    fn swift_protocol_requirements_closures_and_properties_are_not_sites() {
        let src = "protocol P {\n\
                   \x20   func required()\n\
                   }\n\
                   struct S {\n\
                   \x20   var count: Int {\n\
                   \x20       get { 1 }\n\
                   \x20   }\n\
                   \x20   func f() {\n\
                   \x20       let c = { 2 }\n\
                   \x20       _ = c\n\
                   \x20   }\n\
                   }\n";
        let sites = swift_function_sites(src).expect("parses");
        assert_eq!(
            sites
                .iter()
                .map(|s| s.item_path.as_str())
                .collect::<Vec<_>>(),
            vec!["S::f"],
            "{sites:?}"
        );
    }

    #[test]
    fn swift_row_rejects_test_and_manifest_sources_and_uses_the_brace_one_liner_rule() {
        let row = language_for_path("Sources/Store/Store.swift").expect("swift row");
        assert_eq!(row.id, "swift");
        assert_eq!(row.test_id_prefix, "swift:");
        assert!(!row.one_liner_needs_fnda, "llvm-cov FN names are mangled");
        assert!((row.is_wanted_source)("Sources/Store/Store.swift"));
        assert!(
            (row.is_wanted_source)("App/NPCOverlay.swift"),
            "target context, not only Sources/"
        );
        assert!(!(row.is_wanted_source)("AppTests/FooTests.swift"));
        assert!(!(row.is_wanted_source)("Tests/Unit/Helpers.swift"));
        assert!(
            (row.is_wanted_source)("App/Tester.swift"),
            "*Test.swift naming, not *Tester.swift"
        );
        assert!(!(row.is_wanted_source)("Package.swift"));
        assert!(!(row.is_wanted_source)(
            ".build/out/Products/Debug/Store.swift"
        ));
        let one_line = FunctionSite {
            item_path: "f".into(),
            start_line: 4,
            body_start_line: 4,
            end_line: 4,
        };
        let multi_line = FunctionSite {
            item_path: "g".into(),
            start_line: 4,
            body_start_line: 4,
            end_line: 5,
        };
        assert!((row.is_one_liner)(&one_line), "brace rows: end==start");
        assert!(
            !(row.is_one_liner)(&multi_line),
            "multi-line brace fn is not a one-liner"
        );
    }

    #[test]
    fn swift_selector_from_the_graph_id_is_anchored_and_runner_native() {
        // Real graph shape: unit :: file stem :: class :: method (probe:
        // `swift:store-kitTests::StoreTests::StoreTests::testLoad`).
        assert_eq!(
            selector_from_test_id("swift:store-kitTests::StoreTests::StoreTests::testLoad")
                .as_deref(),
            Some("^store_kitTests\\.StoreTests/testLoad$")
        );
        // The plan's shorter shape derives the same way.
        assert_eq!(
            selector_from_test_id("swift:store-kit::StoreTests::testLoad").as_deref(),
            Some("^store_kit\\.StoreTests/testLoad$")
        );
        assert_eq!(selector_from_test_id("rust:phronesis#test:x"), None);
        assert_eq!(selector_from_test_id("swift:only-two"), None);
        assert_eq!(
            runner_native_name("swift:store-kitTests::StoreTests::StoreTests::testLoad").as_deref(),
            Some("store_kitTests.StoreTests/testLoad")
        );
    }

    #[test]
    fn swift_render_command_derives_the_probed_filter_from_the_id() {
        assert_eq!(
            render_command(
                "swift:StoreTests::StoreTests::StoreTests::testLoad",
                "Tests/StoreTests/StoreTests.swift"
            )
            .as_deref(),
            Some("swift test --filter '^StoreTests\\.StoreTests/testLoad$'")
        );
        assert_eq!(
            render_command("outcomes::tests::x", "crates/x/src/lib.rs"),
            None
        );
    }
}
