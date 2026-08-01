//! The TypeScript sensor: structural edges from one `.ts` / `.tsx` file.
//!
//! Separate from `super::extract` and `super::python` because almost nothing
//! is shared. TypeScript has no declared module tree — the directory layout
//! is the module tree — and its imports name paths rather than modules, so
//! `super::resolve` does work neither other extractor needs.

use super::extract::Extracted;
use super::model::Edge;
use super::resolve::{self, Resolution};
use super::unit::UnitContext;
use crate::syntax::parsed::ParsedFile;
use std::collections::BTreeSet;
use tree_sitter::Node;

/// Classification of a TypeScript file, as the `file_type` relation.
///
/// Follows the conventions jest, vitest and mocha share: `*.test.*`,
/// `*.spec.*`, or anything beneath a `__tests__` directory.
fn file_type(file_path: &str) -> &'static str {
    let name = file_path.rsplit('/').next().unwrap_or(file_path);
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
    if stem.ends_with(".test") || stem.ends_with(".spec") {
        return "test";
    }
    if file_path.starts_with("__tests__/") || file_path.contains("/__tests__/") {
        return "test";
    }
    "production"
}

fn text<'a>(node: Node, source: &'a [u8]) -> &'a str {
    node.utf8_text(source).unwrap_or("")
}

/// True when a node is a function of any TypeScript spelling.
fn is_function_node(kind: &str) -> bool {
    matches!(
        kind,
        "function_expression" | "arrow_function" | "function_declaration" | "generator_function"
    )
}

struct Sensor<'a> {
    file_path: &'a str,
    source: &'a [u8],
    unit: &'a UnitContext,
    self_module: String,
    out: BTreeSet<(String, Vec<String>)>,
    skipped: usize,
}

impl Sensor<'_> {
    fn emit(&mut self, p: &str, args: &[&str]) {
        self.out
            .insert((p.to_string(), args.iter().map(|s| s.to_string()).collect()));
    }

    fn qualify(&self, scope: &[String], name: &str) -> String {
        let mut parts = vec![self.self_module.clone()];
        parts.extend(scope.iter().cloned());
        parts.push(name.to_string());
        parts.join("::")
    }

    fn walk(&mut self, node: Node, scope: &[String]) {
        match node.kind() {
            "function_declaration" | "generator_function_declaration" => {
                if let Some(name) = node.child_by_field_name("name") {
                    let qualified = self.qualify(scope, text(name, self.source));
                    let file = self.file_path.to_string();
                    self.emit("defines_fn", &[&file, &qualified]);
                } else {
                    self.skipped += 1;
                }
                return;
            }
            "class_declaration" => {
                let Some(name) = node.child_by_field_name("name") else {
                    self.skipped += 1;
                    return;
                };
                let mut inner = scope.to_vec();
                inner.push(text(name, self.source).to_string());
                if let Some(body) = node.child_by_field_name("body") {
                    self.walk_children(body, &inner);
                }
                return;
            }
            "method_definition" => {
                if let Some(name) = node.child_by_field_name("name") {
                    let qualified = self.qualify(scope, text(name, self.source));
                    let file = self.file_path.to_string();
                    self.emit("defines_fn", &[&file, &qualified]);
                } else {
                    self.skipped += 1;
                }
                return;
            }
            "variable_declarator" => {
                // `const charge = () => …` is the dominant modern spelling;
                // treating it as a plain binding would leave most codebases
                // looking as though they define nothing.
                let name = node.child_by_field_name("name");
                let value = node.child_by_field_name("value");
                if let (Some(name), Some(value)) = (name, value)
                    && is_function_node(value.kind())
                {
                    let qualified = self.qualify(scope, text(name, self.source));
                    let file = self.file_path.to_string();
                    self.emit("defines_fn", &[&file, &qualified]);
                    return;
                }
            }
            "import_statement" => {
                self.import(node);
                return;
            }
            _ => {}
        }
        self.walk_children(node, scope);
    }

    fn walk_children(&mut self, node: Node, scope: &[String]) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor).collect::<Vec<_>>() {
            self.walk(child, scope);
        }
    }

    /// Resolve an `import … from "…"` to a module edge.
    fn import(&mut self, node: Node) {
        let Some(source_node) = node.child_by_field_name("source") else {
            return;
        };
        let specifier = text(source_node, self.source).trim_matches(['"', '\'', '`']);
        match resolve::resolve_specifier(specifier, self.file_path, self.unit) {
            Resolution::File(target_file) => {
                let target = resolve::module_path(&target_file, self.unit);
                if target != self.self_module {
                    let from = self.self_module.clone();
                    self.emit("imports", &[&from, &target]);
                }
            }
            // Third-party: a node with no definitions hanging off it is worse
            // than no node.
            Resolution::External => {}
            // Names something in this project that we could not find. Counted
            // so a broken resolver cannot look like a clean codebase.
            Resolution::Unresolved => self.skipped += 1,
        }
    }
}

/// Extract every base relation from one TypeScript file.
pub fn extract_typescript(file_path: &str, content: &str, unit: &UnitContext) -> Extracted {
    let Some(lang) = crate::graph::unit::lang_of_path(file_path) else {
        return Extracted::default();
    };
    if lang != crate::graph::unit::LANG_TYPESCRIPT {
        return Extracted::default();
    }

    let tsx = file_path.ends_with(".tsx");
    let Some(parsed) = ParsedFile::parse_typescript(content, tsx) else {
        return Extracted::unparseable();
    };
    let ParsedFile::TypeScript { tree, source } = &parsed else {
        return Extracted::unparseable();
    };
    if tree.root_node().has_error() {
        return Extracted::unparseable();
    }

    let self_module = resolve::module_path(file_path, unit);
    let mut sensor = Sensor {
        file_path,
        source: source.as_bytes(),
        unit,
        self_module: self_module.clone(),
        out: BTreeSet::new(),
        skipped: 0,
    };
    sensor.emit("file_type", &[file_path, file_type(file_path)]);
    sensor.emit("declares_module", &[file_path, &self_module]);
    sensor.walk(tree.root_node(), &[]);

    let edges = sensor
        .out
        .iter()
        .map(|(p, a)| Edge {
            p: p.clone(),
            a: a.clone(),
            src: file_path.to_string(),
            d: false,
        })
        .collect();
    Extracted {
        edges,
        skipped: sensor.skipped,
        parse_failed: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::unit::TsConfig;
    use std::collections::BTreeMap;

    fn ctx(files: &[&str]) -> UnitContext {
        UnitContext {
            id: "typescript:myapp".to_string(),
            module_base: "src".to_string(),
            siblings: BTreeMap::new(),
            ts: TsConfig {
                base_url: "src".to_string(),
                paths: BTreeMap::new(),
            },
            files: files.iter().map(|f| (*f).to_string()).collect(),
        }
    }

    fn edges_of(x: &Extracted, p: &str) -> Vec<Vec<String>> {
        x.edges
            .iter()
            .filter(|e| e.p == p)
            .map(|e| e.a.clone())
            .collect()
    }

    // ─── file_type ──────────────────────────────────────────────────

    #[test]
    fn a_plain_module_is_production() {
        assert_eq!(file_type("src/billing.ts"), "production");
    }

    #[test]
    fn a_dot_test_file_is_a_test() {
        assert_eq!(file_type("src/billing.test.ts"), "test");
        assert_eq!(file_type("src/billing.spec.tsx"), "test");
    }

    #[test]
    fn a_file_under_a_tests_directory_is_a_test() {
        assert_eq!(file_type("src/__tests__/billing.ts"), "test");
    }

    // ─── declares_module ────────────────────────────────────────────

    #[test]
    fn a_file_declares_its_own_module() {
        let out = extract_typescript(
            "src/billing.ts",
            "export const x = 1\n",
            &ctx(&["src/billing.ts"]),
        );
        assert_eq!(
            edges_of(&out, "declares_module"),
            vec![vec![
                "src/billing.ts".to_string(),
                "typescript:myapp::billing".to_string()
            ]]
        );
    }

    // ─── defines_fn ─────────────────────────────────────────────────

    #[test]
    fn a_function_declaration_is_a_defined_function() {
        let out = extract_typescript(
            "src/billing.ts",
            "export function charge() { return 1 }\n",
            &ctx(&["src/billing.ts"]),
        );
        assert_eq!(
            edges_of(&out, "defines_fn")[0][1],
            "typescript:myapp::billing::charge"
        );
    }

    #[test]
    fn an_arrow_function_assigned_to_a_const_is_a_defined_function() {
        // The dominant style in modern TypeScript; missing it would leave
        // most codebases looking empty.
        let out = extract_typescript(
            "src/billing.ts",
            "export const charge = () => 1\n",
            &ctx(&["src/billing.ts"]),
        );
        assert_eq!(
            edges_of(&out, "defines_fn")[0][1],
            "typescript:myapp::billing::charge"
        );
    }

    #[test]
    fn a_class_method_is_qualified_by_its_class() {
        let out = extract_typescript(
            "src/billing.ts",
            "export class Ledger { charge() { return 1 } }\n",
            &ctx(&["src/billing.ts"]),
        );
        assert_eq!(
            edges_of(&out, "defines_fn")[0][1],
            "typescript:myapp::billing::Ledger::charge"
        );
    }

    #[test]
    fn a_non_function_const_is_not_a_defined_function() {
        let out = extract_typescript(
            "src/billing.ts",
            "export const RATE = 0.2\n",
            &ctx(&["src/billing.ts"]),
        );
        assert!(edges_of(&out, "defines_fn").is_empty());
    }

    // ─── guards ─────────────────────────────────────────────────────

    #[test]
    fn a_non_typescript_file_yields_nothing() {
        assert_eq!(
            extract_typescript("src/a.rs", "fn f() {}", &ctx(&[])),
            Extracted::default()
        );
    }

    #[test]
    fn unparseable_source_preserves_existing_evidence() {
        // Not an empty extraction: that would erase the file's edges and
        // report the graph fresh.
        let out = extract_typescript(
            "src/billing.ts",
            "function ((( {",
            &ctx(&["src/billing.ts"]),
        );
        assert!(out.parse_failed, "must signal parse failure");
        assert!(out.edges.is_empty());
    }
}
