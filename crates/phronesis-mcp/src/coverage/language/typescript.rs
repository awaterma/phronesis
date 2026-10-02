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
                record(node, src, scope, name, out);
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
    if parent.kind() == "assignment_expression" {
        if parent.child_by_field_name("right") != Some(node) {
            return None;
        }
        return static_assignment_name(parent.child_by_field_name("left")?, src);
    }
    let named = match parent.kind() {
        "variable_declarator" | "public_field_definition" => parent.child_by_field_name("name")?,
        "pair" => parent.child_by_field_name("key")?,
        _ => return None,
    };
    named.utf8_text(src).ok().map(str::to_string)
}

/// A statically named assignment target; computed properties cannot supply
/// a stable region identity without evaluating JavaScript.
fn static_assignment_name(node: tree_sitter::Node<'_>, src: &[u8]) -> Option<String> {
    match node.kind() {
        "identifier" => node.utf8_text(src).ok().map(str::to_string),
        "member_expression" => {
            let object = static_assignment_name(node.child_by_field_name("object")?, src)?;
            let property = node.child_by_field_name("property")?;
            if property.kind() != "property_identifier" {
                return None;
            }
            Some(format!("{object}::{}", property.utf8_text(src).ok()?))
        }
        _ => None,
    }
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
        Some(body) if body.kind() == "statement_block" => {
            let mut cursor = body.walk();
            body.named_children(&mut cursor)
                .find(|child| child.kind() != "comment")
                .map_or(start_line, |statement| {
                    statement.start_position().row as u64 + 1
                })
        }
        // Expression bodies and empty functions have no separate statement
        // line with which to distinguish declaration from invocation.
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

/// Require function-count evidence when the body's first statement shares
/// the declaration line, even if later statements span additional lines.
/// Loading a module can hit that line without invoking the function.
pub fn is_one_liner(site: &FunctionSite) -> bool {
    site.end_line == site.start_line
        || site.body_start_line == site.start_line
        || site.body_start_line >= site.end_line
}

/// A runnable command needs the imported record's tool string, which the
/// registry's `render_command` entry point cannot see; Task H3 adds
/// `render_command_with_tool` for that. Until a record carries a tool, no
/// command is rendered.
pub fn render_command(_test_id: &str, _file: &str) -> Option<String> {
    None
}

/// The runner-native command for `select`, chosen by the imported record's
/// tool string (producer + runner) so `select` never re-detects the runner:
/// `c8+vitest` → vitest, `istanbul+jest` → jest, `c8+node` → `node --test`.
/// Anything else names no runner and renders nothing.
pub fn render_command_with_tool(test_id: &str, file: &str, tool: &str) -> Option<String> {
    let title = runner_name(test_id, file)?;
    let pattern =
        crate::coverage::collect_js::quote(&crate::coverage::collect_js::exact_pattern(&title));
    let file = crate::coverage::collect_js::quote(file);
    match tool {
        "c8+vitest" => Some(format!("npx vitest run {file} -t {pattern}")),
        "istanbul+jest" => Some(format!("npx jest --runTestsByPath {file} -t {pattern}")),
        "c8+node" => Some(format!("node --test --test-name-pattern={pattern} {file}")),
        _ => None,
    }
}

/// The runner-native test name: the `it()` title that ends the graph's
/// `defines_test` id, extracted the way `python::render_command` extracts
/// its pytest node id — the suffix after the test file's module marker,
/// never a naive last-segment split (titles contain spaces, and may even
/// contain `::`; plan decision 5).
pub fn runner_name(test_id: &str, file: &str) -> Option<String> {
    // Mirrors the graph's module identity (`resolve::strip_known_extension`):
    // only a literal trailing `.ts` leaves the module segment, every other
    // extension stays in it, so `.mts`/`.cts` reduce to `.m`/`.c` exactly
    // as the graph's ids already do.
    let module = file.strip_suffix(".ts").unwrap_or(file).replace('/', "::");
    let marker = format!("::{module}::");
    let (_, title) = test_id.rsplit_once(&marker)?;
    Some(title.to_string())
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
                ("helper", 1, 2, 3),
                ("Store::load", 5, 6, 10),
                ("Store::load::inner", 6, 7, 8),
                ("Store::load.2", 11, 12, 13),
                ("oneLiner", 15, 15, 15),
                ("cfg::run", 16, 16, 16),
            ],
            "{sites:?}"
        );
        // The anonymous `.map(x => x + 1)` callback is not a site.
        assert!(!sites.iter().any(|s| s.item_path.contains("anonymous")));
    }

    #[test]
    fn commonjs_and_identifier_assignments_have_stable_coverage_regions() {
        let source = "exports.load = function load() { return 7; };\n\
            module.exports.save = () => {\n\
              return 8;\n\
            };\n\
            read = function named() { return 9; };\n\
            exports[key] = () => 10;\n\
            module[key].hidden = function () { return 11; };\n\
            exports['computed'] = () => 12;\n";
        let sites = typescript_function_sites(source, false).expect("parse CJS");
        assert_eq!(
            sites
                .iter()
                .map(|site| site.item_path.as_str())
                .collect::<Vec<_>>(),
            ["exports::load", "module::exports::save", "read"]
        );
        assert_eq!((sites[1].start_line, sites[1].end_line), (2, 4));
        assert!(is_one_liner(&sites[0]));
        assert!(!is_one_liner(&sites[1]));
    }

    #[test]
    fn javascript_declaration_hits_do_not_credit_unexecuted_function_bodies() {
        let source = "exports.load = function load() {\n  // body comment\n  return 7;\n};\nexports.inline = () => { return 8;\n  return 9;\n};\n";
        let sites = typescript_function_sites(source, false).expect("sites");
        assert_eq!(sites[0].body_start_line, 3);
        assert!(is_one_liner(&sites[1]), "header/body overlap requires FNDA");
        let language = crate::coverage::language::language_for_path("store.js").expect("language");
        let parsed = crate::coverage::lcov::parse_lcov(
            "SF:store.js\nDA:1,1\nDA:3,0\nDA:4,1\nDA:5,1\nDA:6,1\nend_of_record\n",
        )
        .expect("lcov");
        let (hits, _) = crate::coverage::lcov::hit_sites(&sites, &parsed.files[0], language);
        assert!(
            hits.is_empty(),
            "declaration and inline header cannot imply invocation"
        );
        let parsed = crate::coverage::lcov::parse_lcov(
            "SF:store.js\nFNDA:0,load\nDA:1,1\nDA:3,1\nDA:4,1\nend_of_record\n",
        )
        .expect("lcov");
        let (hits, _) = crate::coverage::lcov::hit_sites(&sites, &parsed.files[0], language);
        assert!(
            hits.is_empty(),
            "explicit zero invocation vetoes misleading line hits"
        );
        let parsed =
            crate::coverage::lcov::parse_lcov("SF:store.js\nDA:1,1\nDA:3,1\nend_of_record\n")
                .expect("lcov");
        let (hits, _) = crate::coverage::lcov::hit_sites(&sites, &parsed.files[0], language);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].item_path, "exports::load");
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

    #[test]
    fn runner_name_extracts_the_title_after_the_test_files_module_marker() {
        // The graph's `defines_test` ids end with the `it()` title, which
        // may contain spaces and even `::`, so the extraction mirrors
        // `python::render_command`'s module-marker suffix rather than a
        // last-segment split (plan decision 5).
        assert_eq!(
            runner_name(
                "typescript:myapp::tests::store.test::Store loads",
                "tests/store.test.ts"
            )
            .as_deref(),
            Some("Store loads")
        );
        assert_eq!(
            runner_name(
                "typescript:myapp::tests::store.test.js::loads (slow)::again",
                "tests/store.test.js"
            )
            .as_deref(),
            Some("loads (slow)::again")
        );
        // Only a literal trailing `.ts` leaves the module segment, exactly
        // as the graph's `strip_known_extension` behaves.
        assert_eq!(
            runner_name(
                "typescript:myapp::src::app.tsx::Store renders",
                "src/app.tsx"
            )
            .as_deref(),
            Some("Store renders")
        );
        assert_eq!(
            runner_name(
                "typescript:myapp::other::Store loads",
                "tests/store.test.ts"
            ),
            None
        );
    }

    #[test]
    fn render_command_with_tool_chooses_the_runner_from_the_tool_string() {
        let id = "typescript:myapp#test:store.test.ts::tests::store.test::Store loads";
        let file = "tests/store.test.ts";
        assert_eq!(
            render_command_with_tool(id, file, "c8+vitest").as_deref(),
            Some("npx vitest run 'tests/store.test.ts' -t '^Store loads$'")
        );
        assert_eq!(
            render_command_with_tool(id, file, "istanbul+jest").as_deref(),
            Some("npx jest --runTestsByPath 'tests/store.test.ts' -t '^Store loads$'")
        );
        assert_eq!(
            render_command_with_tool(id, file, "c8+node").as_deref(),
            Some("node --test --test-name-pattern='^Store loads$' 'tests/store.test.ts'")
        );
        assert_eq!(
            render_command_with_tool(id, file, "cargo-llvm-cov"),
            None,
            "a tool that names no JS runner renders no command"
        );
        assert_eq!(
            render_command_with_tool(id, "tests/other.test.ts", "c8+vitest"),
            None,
            "an id that does not belong to the file renders no command"
        );
        // The path-blind entry point stays empty for the typescript row:
        // the runner is known only from the imported record's tool string.
        assert_eq!(render_command(id, file), None);
    }
}
