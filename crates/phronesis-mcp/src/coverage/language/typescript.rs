//! TypeScript/JavaScript coverage semantics: function sites with body
//! ranges, the production-source filter, and the one-liner rule.
//!
//! The walker trusts only node shapes the parse probe confirmed against
//! tree-sitter-typescript 0.23 (PLAN.md Task H1): `function_declaration`,
//! `generator_function_declaration`, `method_definition` (class and
//! object-literal), `variable_declarator`/`public_field_definition`/`pair`
//! as the naming parents of `arrow_function`/`function_expression`, and
//! `method_signature` (interface members and class overload signatures,
//! which carry no body) excluded by kind.

use crate::coverage::region_map::{
    FunctionSite, RawSite, assign_function_ordinals, inclusive_end_line,
};

/// Function sites for one TypeScript/JavaScript source: declared and
/// generator functions, class and object-literal methods, class fields and
/// object properties assigned a function, and arrows/function expressions
/// bound to a declarator. Anonymous callbacks (`arr.map(x => …)`) are not
/// sites — their coverage belongs to the enclosing function.
pub fn typescript_function_sites(source: &str, tsx: bool) -> anyhow::Result<Vec<FunctionSite>> {
    let parsed = crate::syntax::parsed::ParsedFile::parse_typescript(source, tsx)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter failed to parse TypeScript source"))?;
    let crate::syntax::parsed::ParsedFile::TypeScript { tree, source: text } = &parsed else {
        anyhow::bail!("parse_typescript returned a non-TypeScript tree");
    };
    let mut raw = Vec::new();
    walk(tree.root_node(), text.as_bytes(), &mut Vec::new(), &mut raw);
    Ok(assign_function_ordinals(raw))
}

/// The registry's path-blind entry point: parses as plain TypeScript.
/// `extract_function_sites_for` prefers [`function_sites_for_path`], which
/// sees the extension and picks the TSX grammar for `.tsx`/`.jsx`.
pub fn function_sites(source: &str) -> anyhow::Result<Vec<FunctionSite>> {
    typescript_function_sites(source, false)
}

/// The registry's per-path extractor: `.tsx`/`.jsx` carry JSX syntax, which
/// only the TSX grammar parses; every other claimed extension parses under
/// the plain TypeScript grammar.
pub fn function_sites_for_path(rel: &str, source: &str) -> anyhow::Result<Vec<FunctionSite>> {
    let tsx = rel.ends_with(".tsx") || rel.ends_with(".jsx");
    typescript_function_sites(source, tsx)
}

fn walk(node: tree_sitter::Node<'_>, src: &[u8], scope: &mut Vec<String>, out: &mut Vec<RawSite>) {
    let mut pushed = false;
    match node.kind() {
        "class_declaration" | "class_expression" => {
            if let Some(name) = name_of(node, src) {
                scope.push(name.to_string());
                pushed = true;
            }
        }
        "function_declaration" | "generator_function_declaration" | "method_definition" => {
            // `method_definition` covers class methods and object-literal
            // methods alike (the probe found the same node kind in both);
            // overload signatures and interface members are
            // `method_signature`, a different kind, so they never land here.
            if let Some(name) = name_of(node, src) {
                record(node, src, scope, &name, out);
                scope.push(name.to_string());
                pushed = true;
            }
        }
        "arrow_function" | "function_expression" => {
            // Only a function bound to a name of its own is a site; the
            // name comes from the binding parent, not the function.
            if let Some(name) = assigned_name(node, src) {
                record(node, src, scope, &name, out);
                scope.push(name.to_string());
                pushed = true;
            }
        }
        "variable_declarator" => {
            // `const cfg = { run() { … } }` — the object's methods qualify
            // under the declarator's name.
            if let Some(value) = node.child_by_field_name("value")
                && value.kind() == "object"
                && let Some(name) = name_of(node, src)
            {
                scope.push(name.to_string());
                pushed = true;
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(child, src, scope, out);
    }
    if pushed {
        scope.pop();
    }
}

fn name_of<'a>(node: tree_sitter::Node<'_>, src: &'a [u8]) -> Option<&'a str> {
    node.child_by_field_name("name")?.utf8_text(src).ok()
}

/// The binding name of an arrow/function expression: the `name` (or `key`)
/// of the `variable_declarator`, `public_field_definition`, or `pair` that
/// holds it. Anything else — a call's `arguments`, a callback parameter — is
/// anonymous and not a site.
fn assigned_name(node: tree_sitter::Node<'_>, src: &[u8]) -> Option<String> {
    let parent = node.parent()?;
    let named = match parent.kind() {
        "variable_declarator" | "public_field_definition" => parent.child_by_field_name("name")?,
        "pair" => parent.child_by_field_name("key")?,
        _ => return None,
    };
    named.utf8_text(src).ok().map(str::to_string)
}

fn record(
    node: tree_sitter::Node<'_>,
    src: &[u8],
    scope: &[String],
    name: &str,
    out: &mut Vec<RawSite>,
) {
    let _ = src;
    let start_line = node.start_position().row as u64 + 1;
    let body_start_line = match node.child_by_field_name("body") {
        // A block body starts on the line of its `{`, which for a normal
        // multi-line declaration is the declaration line itself.
        Some(body) if body.kind() == "statement_block" => body.start_position().row as u64 + 1,
        // An expression-bodied arrow (`const f = (x) => x + 1;`) has no
        // block: the body starts (and the site usually ends) on the
        // declaration line, a one-liner under the brace-row rule.
        _ => start_line,
    };
    let mut path = scope.join("::");
    if !path.is_empty() {
        path.push_str("::");
    }
    path.push_str(name);
    out.push(RawSite {
        path,
        start_line,
        body_start_line,
        end_line: inclusive_end_line(node),
    });
}

/// Decision 6: the eight registry extensions, minus everything that is not
/// a production source — declaration files, vendored and build output,
/// `tests/` directories, and runner/config files. The graph's `file_type`
/// classifier is the source of truth for `*.test.*`/`*.spec.*`/`__tests__`.
pub fn is_wanted_source(rel: &str) -> bool {
    let ext = std::path::Path::new(rel)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    if !matches!(
        ext,
        "ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs"
    ) {
        return false;
    }
    if rel.ends_with(".d.ts") {
        return false;
    }
    if crate::graph::typescript::file_type(rel) == "test" {
        return false;
    }
    let name = rel.rsplit('/').next().unwrap_or(rel);
    if name.contains(".config.") {
        return false;
    }
    !rel.split('/').any(|seg| {
        matches!(
            seg,
            "node_modules" | "dist" | "build" | "coverage" | "tests"
        )
    })
}

/// Revision 2 brace-row rule: a one-liner is a true single-line site. The
/// `{` shares the declaration line, so `body_start_line == start_line`
/// holds for nearly every multi-line brace function and cannot be the test;
/// `end_line == start_line` is.
pub fn is_one_liner(site: &FunctionSite) -> bool {
    site.end_line == site.start_line
}

/// A runnable command needs the imported record's tool string, which the
/// registry's `render_command` entry point cannot see; Task H3 adds
/// `render_command_with_tool` for that. Until a record carries a tool, no
/// command is rendered.
pub fn render_command(_test_id: &str, _file: &str) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typescript_sites_cover_functions_methods_nested_arrows_and_one_liners() {
        let src = "export function helper(x: number) {\n\
                   \x20   return x + 1;\n\
                   }\n\
                   export class Store {\n\
                   \x20   load(): number {\n\
                   \x20       const inner = () => {\n\
                   \x20           return 1;\n\
                   \x20       };\n\
                   \x20       return inner();\n\
                   \x20   }\n\
                   \x20   load(again: number): number {\n\
                   \x20       return again;\n\
                   \x20   }\n\
                   }\n\
                   export const oneLiner = (x: number) => x * 2;\n\
                   const cfg = { run() { return 3; } };\n\
                   [1, 2].map(x => x + 1);\n";
        let sites = typescript_function_sites(src, false).expect("parses");
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
                ("Store::load", 5, 5, 10),
                ("Store::load::inner", 6, 6, 8),
                ("Store::load.2", 11, 11, 13),
                ("oneLiner", 15, 15, 15),
                ("cfg::run", 16, 16, 16),
            ],
            "{sites:?}"
        );
        // The anonymous `.map(x => x + 1)` callback is not a site.
        assert!(!sites.iter().any(|s| s.item_path.contains("anonymous")));
    }

    #[test]
    fn tsx_parses_and_declaration_files_are_not_sources() {
        assert_eq!(
            typescript_function_sites("export const C = () => <div/>;\n", true)
                .expect("tsx")
                .len(),
            1
        );
        assert!(!(is_wanted_source)("src/types.d.ts"));
        assert!(!(is_wanted_source)("node_modules/x/index.js"));
        assert!(!(is_wanted_source)("src/store.test.ts"));
        assert!(!(is_wanted_source)("vitest.config.ts"));
        assert!((is_wanted_source)("src/store.ts"));
        assert!((is_wanted_source)("lib/util.mjs"));
    }
}
