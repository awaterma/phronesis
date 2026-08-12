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

/// True when a node is a *named* function-shaped construct — one that has
/// (or, per `defines_fn`'s existing scope, could have) an identity of its
/// own separate from whatever encloses it: a declared function, a method, or
/// a named function expression (`const f = function foo() {}`).
///
/// Deliberately excludes `arrow_function` and anonymous `function_expression`
/// — inline callbacks (`.map(x => x!.y)`, an immediately-invoked
/// `const f = () => …`) have no identity of their own and run as part of
/// the enclosing function's own control flow, so a non-null assertion
/// inside one still belongs to the enclosing function. A *named* nested
/// function does not: `defines_fn` never descends into a
/// `function_declaration` nested inside another (see `walk`'s early
/// `return` on that arm), so attributing its body's assertions to the
/// outer function would blame code that only runs if the outer function
/// explicitly calls it — a claim this extractor cannot verify.
fn is_named_function_boundary(node: Node) -> bool {
    matches!(
        node.kind(),
        "function_declaration" | "generator_function_declaration" | "method_definition"
    ) || (matches!(node.kind(), "function_expression" | "generator_function")
        && node.child_by_field_name("name").is_some())
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

    /// Shared handling for `class_declaration`, `abstract_class_declaration`,
    /// and named `class` (class-expression) nodes: push the class's own
    /// name onto scope and descend into its body.
    fn class_like(&mut self, node: Node, scope: &[String]) {
        let Some(name) = node.child_by_field_name("name") else {
            // An anonymous class expression reached here (not via the
            // `variable_declarator` or `export_statement` special cases,
            // which supply a name from their own binding) has no identity
            // to qualify its methods with. Counted rather than silently
            // flattened into the caller's scope.
            self.skipped += 1;
            return;
        };
        let mut inner = scope.to_vec();
        inner.push(text(name, self.source).to_string());
        if let Some(body) = node.child_by_field_name("body") {
            self.walk_children(body, &inner);
        }
    }

    fn walk(&mut self, node: Node, scope: &[String]) {
        match node.kind() {
            "function_declaration" | "generator_function_declaration" => {
                if let Some(name) = node.child_by_field_name("name") {
                    let qualified = self.qualify(scope, text(name, self.source));
                    let file = self.file_path.to_string();
                    self.emit("defines_fn", &[&file, &qualified]);
                    if let Some(body) = node.child_by_field_name("body")
                        && self.asserts_non_null(body)
                    {
                        self.emit("calls_api", &[&qualified, "non_null_assertion"]);
                    }
                } else {
                    self.skipped += 1;
                }
                return;
            }
            // `class_declaration` is `class Foo {}`; `abstract_class_declaration`
            // is `abstract class Foo {}` (same `name`/`body` fields, so it
            // needs no separate handling — this also fixes an `Ledger`-style
            // collision the abstract form had on its own: without this arm
            // it fell through to `walk_children` and its methods landed
            // unqualified in the caller's scope). `class` (guarded by
            // `node.is_named()`) is a class *expression* (`const C = class
            // {}`, `class Named {}` used as a value).
            //
            // The `is_named()` guard on `"class"` matters because that kind
            // string does double duty in this grammar: the class-expression
            // node is named, but the literal `class` keyword *token* — an
            // unnamed child of `abstract_class_declaration` (and any other
            // construct not otherwise matched here) — shares the same kind
            // string. Before `abstract_class_declaration` got its own arm,
            // that construct fell through to `walk_children`, which walked
            // straight into its unnamed `class` keyword child; without this
            // guard that token hit this arm, found no `name` field of its
            // own, and inflated `skipped` on every abstract class.
            //
            // All three matched forms carry the same `name`/`body` fields,
            // and all three must push a scope before descending — otherwise
            // their methods land in whatever scope the walk was already in,
            // colliding with an unrelated same-named method elsewhere in the
            // file (the object-literal/class-expression collision below).
            "class_declaration" | "abstract_class_declaration" => {
                self.class_like(node, scope);
                return;
            }
            "class" if node.is_named() => {
                self.class_like(node, scope);
                return;
            }
            "method_definition" => {
                if let Some(name) = node.child_by_field_name("name") {
                    let qualified = self.qualify(scope, text(name, self.source));
                    let file = self.file_path.to_string();
                    self.emit("defines_fn", &[&file, &qualified]);
                    if let Some(body) = node.child_by_field_name("body")
                        && self.asserts_non_null(body)
                    {
                        self.emit("calls_api", &[&qualified, "non_null_assertion"]);
                    }
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
                if let (Some(name), Some(value)) = (name, value) {
                    if is_function_node(value.kind()) {
                        let qualified = self.qualify(scope, text(name, self.source));
                        let file = self.file_path.to_string();
                        self.emit("defines_fn", &[&file, &qualified]);
                        if let Some(body) = value.child_by_field_name("body")
                            && self.asserts_non_null(body)
                        {
                            self.emit("calls_api", &[&qualified, "non_null_assertion"]);
                        }
                        return;
                    }
                    // `const obj = { m() {} }` and `const C = class { m() {} }`:
                    // neither an object literal nor an anonymous class
                    // expression has an identity of its own, so without this
                    // the `method_definition` arm below would qualify `m` by
                    // whatever scope happened to be active at this point in
                    // the walk — silently merging it with an unrelated `m`
                    // defined elsewhere in the file. The binding name is the
                    // only identity available, so borrow it.
                    if matches!(value.kind(), "class" | "object") {
                        let mut inner = scope.to_vec();
                        inner.push(text(name, self.source).to_string());
                        // `class` has its methods under a `class_body` field;
                        // an object literal's `method_definition`s are direct
                        // children of the object node itself.
                        let target = value.child_by_field_name("body").unwrap_or(value);
                        self.walk_children(target, &inner);
                        return;
                    }
                }
            }
            "import_statement" => {
                self.import(node);
                return;
            }
            "export_statement" => {
                // A re-export (`export … from "…"`) carries a `source`
                // field exactly like `import_statement` — this covers
                // `export * from`, `export { a } from`, `export type { T }
                // from`, and `export * as ns from`. A barrel `index.ts`
                // built entirely from these would otherwise emit no
                // `imports` edges at all and look like a leaf instead of a
                // hub, silently hiding any cycle routed through it.
                if node.child_by_field_name("source").is_some() {
                    self.import(node);
                    return;
                }
                // `export default function foo() {}` / `export default class
                // Foo {}` carry their declaration under a `declaration`
                // field and already have their own name, so the fallthrough
                // walk below reaches the ordinary `function_declaration` /
                // `class_declaration` arms and needs nothing extra here.
                //
                // `export default function () {}` / `export default class {}`
                // / `export default () => …` are anonymous — they carry
                // their value under a `value` field instead, with no name of
                // their own anywhere, unlike every other case this
                // extractor handles. A module's default export is one
                // addressable thing per file, so `default` is a defensible
                // identity for it; dropping it silently (the alternative)
                // would undercount the single most common export shape in
                // React components and route handlers.
                if let Some(value) = node.child_by_field_name("value") {
                    if is_function_node(value.kind()) {
                        let qualified = self.qualify(scope, "default");
                        let file = self.file_path.to_string();
                        self.emit("defines_fn", &[&file, &qualified]);
                        return;
                    }
                    if value.kind() == "class" {
                        let mut inner = scope.to_vec();
                        inner.push("default".to_string());
                        if let Some(body) = value.child_by_field_name("body") {
                            self.walk_children(body, &inner);
                        }
                        return;
                    }
                }
            }
            "call_expression" => {
                if let Some(title) = self.test_title(node) {
                    let qualified = format!("{}::{title}", self.self_module);
                    if let Some(callback) = self.test_callback(node) {
                        for callee in self.called_names(callback) {
                            self.emit("tested_by", &[&callee, &qualified]);
                        }
                    } else if self.has_reference_callback(node) {
                        // `it('t', handler)` — a named-reference callback.
                        // We cannot follow a reference to know what it
                        // calls, so this is a real analysis gap, not a
                        // clean "no coverage" — counted rather than
                        // silently dropped. `it.todo('t')`, which has no
                        // second argument at all, is not a gap — it is
                        // never expected to have a callback — so it does
                        // not land here.
                        self.skipped += 1;
                    }
                    // Fall through to the ordinary descent below: a helper
                    // defined inside a test callback (`const h = () => ...`,
                    // `class C { m() {} }`) still gets its own `defines_fn`
                    // edge, exactly as it would anywhere else in the file —
                    // preserving Task 6's contract. This is harmless for
                    // `warn-untested-risky-call`, which gates on
                    // `file_type(?file, "production")`.
                }
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

    /// Resolve an `import … from "…"` or re-export `export … from "…"` to a
    /// module edge.
    ///
    /// Dynamic `import('./y')` and `import x = require('./y')` are not
    /// tracked: the first is a call expression (no `import_statement` /
    /// `export_statement` node to hang the edge off), and the second is
    /// CommonJS interop rare enough in fresh TypeScript that the walker
    /// complexity to special-case it isn't justified yet. Both are omitted
    /// by choice, not by oversight — a future pass can add them if they
    /// turn out to matter.
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

    /// True when `function` (the `function` field of a `call_expression`)
    /// names an *active* — i.e. it will actually run — `it`/`test`
    /// invocation, in any of its common spellings:
    ///
    /// - `it(...)` / `test(...)` — bare identifier.
    /// - `it.only(...)` / `test.only(...)` — still runs (exclusively), so
    ///   still counts.
    /// - `it.each([...])(...)` / `test.each([...])(...)` — the outer call's
    ///   `function` field is itself a call expression (`it.each([...])`),
    ///   one level removed from the plain and `.only` shapes; still counts.
    /// - `it.concurrent(...)` / `test.concurrent(...)` — runs in parallel
    ///   with other concurrent tests, but still runs; counts.
    /// - `it.failing(...)` — jest's "expected to fail" marker; the
    ///   callback still executes (the test passes iff it throws), so it is
    ///   still coverage.
    /// - `test.sequential(...)` — vitest's opt-out of `--sequence.concurrent`;
    ///   still runs; counts.
    /// - `it.todo(...)` / `test.todo(...)` — a placeholder with (by
    ///   convention) no callback at all. Recognized as a test invocation
    ///   for consistency, but since there is nothing to call,
    ///   `test_callback` finds nothing and no `tested_by` edge results
    ///   either way.
    ///
    /// `it.skip(...)` / `test.skip(...)` deliberately return `false`: a
    /// skipped test never runs, so a call inside it is not evidence
    /// anything was verified. This is the one case among the member-call
    /// forms above where under-counting (treating it as "no coverage") is
    /// the *safe* direction — the opposite of every other form here, where
    /// under-counting would produce a false "untested" accusation.
    fn is_test_invocation(&self, function: Node) -> bool {
        match function.kind() {
            "identifier" => matches!(text(function, self.source), "it" | "test"),
            "member_expression" => {
                let Some(object) = function.child_by_field_name("object") else {
                    return false;
                };
                let Some(property) = function.child_by_field_name("property") else {
                    return false;
                };
                matches!(text(object, self.source), "it" | "test")
                    && matches!(
                        text(property, self.source),
                        "only" | "each" | "concurrent" | "failing" | "sequential" | "todo"
                    )
            }
            "call_expression" => function
                .child_by_field_name("function")
                .is_some_and(|f| self.is_test_invocation(f)),
            _ => false,
        }
    }

    /// The title of an active `it("…", …)` / `test("…", …)` call, if this
    /// node is one.
    fn test_title(&self, node: Node) -> Option<String> {
        if node.kind() != "call_expression" {
            return None;
        }
        let function = node.child_by_field_name("function")?;
        if !self.is_test_invocation(function) {
            return None;
        }
        let args = node.child_by_field_name("arguments")?;
        let mut cursor = args.walk();
        let first = args
            .children(&mut cursor)
            .find(|c| matches!(c.kind(), "string" | "template_string"))?;
        // Trim only the specific delimiter this literal actually opened
        // with — `trim_matches` against the whole quote set would eat a
        // trailing `"` that's part of the title itself (`'has "quotes"'`
        // would lose its own closing `"`).
        let raw = text(first, self.source);
        let title = match raw.chars().next() {
            Some(delim @ ('"' | '\'' | '`')) => raw
                .strip_prefix(delim)
                .and_then(|s| s.strip_suffix(delim))
                .unwrap_or(raw),
            _ => raw,
        };
        Some(title.to_string())
    }

    /// The callback function passed to a test invocation — the arrow
    /// function or (less commonly) `function` expression among its
    /// arguments — if this node is one.
    fn test_callback<'a>(&self, node: Node<'a>) -> Option<Node<'a>> {
        let args = node.child_by_field_name("arguments")?;
        let mut cursor = args.walk();
        args.children(&mut cursor)
            .find(|c| is_function_node(c.kind()))
    }

    /// True when a test invocation's arguments contain a real (named)
    /// second argument that is not an inline function — i.e. a callback
    /// passed by reference (`it('t', handler)`) rather than inline. Used
    /// only after `test_callback` has already failed to find an inline
    /// callback, to tell that gap apart from `it.todo('t')`, which has no
    /// second argument at all and is not a gap.
    fn has_reference_callback(&self, node: Node) -> bool {
        let Some(args) = node.child_by_field_name("arguments") else {
            return false;
        };
        let mut cursor = args.walk();
        args.children(&mut cursor)
            .any(|c| c.is_named() && !matches!(c.kind(), "string" | "template_string"))
    }

    /// Bare names of functions invoked in a body. Resolved by short name in
    /// `super::derive::untested`, exactly as Rust and Python do.
    fn called_names(&self, body: Node) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        let mut stack = vec![body];
        while let Some(n) = stack.pop() {
            if n.kind() == "call_expression"
                && let Some(f) = n.child_by_field_name("function")
            {
                let name = match f.kind() {
                    "identifier" => text(f, self.source).to_string(),
                    "member_expression" => f
                        .child_by_field_name("property")
                        .map(|p| text(p, self.source).to_string())
                        .unwrap_or_default(),
                    _ => String::new(),
                };
                if !name.is_empty() && !matches!(name.as_str(), "it" | "test" | "describe") {
                    found.insert(name);
                }
            }
            let mut cursor = n.walk();
            stack.extend(n.children(&mut cursor));
        }
        found
    }

    /// True when this subtree contains a non-null assertion, not counting
    /// one that lives inside a nested *named* function (see
    /// `is_named_function_boundary`). `node` should be the executable body
    /// of the function being checked (a `statement_block`, or an arrow
    /// function's concise expression body) — not the definition node
    /// itself, which for the `variable_declarator` case *is* an
    /// `arrow_function` and would immediately be (mis)treated as a nested
    /// boundary of itself.
    fn asserts_non_null(&self, node: Node) -> bool {
        let mut stack = vec![node];
        while let Some(n) = stack.pop() {
            if n.kind() == "non_null_expression" {
                return true;
            }
            if is_named_function_boundary(n) {
                continue;
            }
            let mut cursor = n.walk();
            stack.extend(n.children(&mut cursor));
        }
        false
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
            lua_files: Vec::new(),
            cue_files: Vec::new(),
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

    #[test]
    fn an_abstract_class_method_is_qualified_by_its_class() {
        let out = extract_typescript(
            "src/billing.ts",
            "abstract class A { m() { return 1 } }\n",
            &ctx(&["src/billing.ts"]),
        );
        assert_eq!(
            edges_of(&out, "defines_fn")[0][1],
            "typescript:myapp::billing::A::m"
        );
        assert_eq!(out.skipped, 0, "an abstract class must not inflate skipped");
    }

    #[test]
    fn an_abstract_class_does_not_inflate_skipped() {
        // Regression for the `"class"` keyword token — an unnamed child of
        // `abstract_class_declaration` — matching the class-expression arm
        // and being counted as an anonymous class with no name of its own.
        let out = extract_typescript(
            "src/billing.ts",
            "abstract class A { m() {} }\nabstract class B { n() {} }\n",
            &ctx(&["src/billing.ts"]),
        );
        assert_eq!(out.skipped, 0);
    }

    // ─── imports (including re-exports) ──────────────────────────────

    #[test]
    fn a_star_re_export_is_an_import() {
        let out = extract_typescript(
            "src/index.ts",
            "export * from './a'\n",
            &ctx(&["src/index.ts", "src/a.ts"]),
        );
        assert_eq!(
            edges_of(&out, "imports"),
            vec![vec![
                "typescript:myapp::index".to_string(),
                "typescript:myapp::a".to_string()
            ]]
        );
    }

    #[test]
    fn a_named_re_export_is_an_import() {
        let out = extract_typescript(
            "src/index.ts",
            "export { b } from './b'\n",
            &ctx(&["src/index.ts", "src/b.ts"]),
        );
        assert_eq!(
            edges_of(&out, "imports"),
            vec![vec![
                "typescript:myapp::index".to_string(),
                "typescript:myapp::b".to_string()
            ]]
        );
    }

    #[test]
    fn a_type_only_re_export_is_an_import() {
        let out = extract_typescript(
            "src/index.ts",
            "export type { T } from './c'\n",
            &ctx(&["src/index.ts", "src/c.ts"]),
        );
        assert_eq!(
            edges_of(&out, "imports"),
            vec![vec![
                "typescript:myapp::index".to_string(),
                "typescript:myapp::c".to_string()
            ]]
        );
    }

    #[test]
    fn a_barrel_file_of_only_re_exports_is_not_a_clean_leaf() {
        // The regression this whole finding is about: without re-export
        // support, the single most common index.ts shape reported zero
        // dependencies and skipped=0 — a clean leaf rather than a hub.
        let out = extract_typescript(
            "src/index.ts",
            "export * from './a'\nexport { b } from './b'\n",
            &ctx(&["src/index.ts", "src/a.ts", "src/b.ts"]),
        );
        assert_eq!(edges_of(&out, "imports").len(), 2);
    }

    // ─── default exports ───────────────────────────────────────────────

    #[test]
    fn an_anonymous_default_exported_function_is_a_defined_function() {
        let out = extract_typescript(
            "src/billing.ts",
            "export default function () { return 1 }\n",
            &ctx(&["src/billing.ts"]),
        );
        assert_eq!(
            edges_of(&out, "defines_fn")[0][1],
            "typescript:myapp::billing::default"
        );
    }

    #[test]
    fn an_anonymous_default_exported_class_method_is_qualified_by_default() {
        let out = extract_typescript(
            "src/billing.ts",
            "export default class { charge() { return 1 } }\n",
            &ctx(&["src/billing.ts"]),
        );
        assert_eq!(
            edges_of(&out, "defines_fn")[0][1],
            "typescript:myapp::billing::default::charge"
        );
    }

    // ─── object-literal / class-expression method identity ────────────

    #[test]
    fn object_and_class_expression_methods_do_not_collide_with_a_free_function() {
        let out = extract_typescript(
            "src/billing.ts",
            "function m() {}\nconst obj = { m() {} };\nconst C = class { m() {} };\n",
            &ctx(&["src/billing.ts"]),
        );
        let mut identities: Vec<String> = edges_of(&out, "defines_fn")
            .into_iter()
            .map(|a| a[1].clone())
            .collect();
        identities.sort();
        assert_eq!(
            identities,
            vec![
                "typescript:myapp::billing::C::m".to_string(),
                "typescript:myapp::billing::m".to_string(),
                "typescript:myapp::billing::obj::m".to_string(),
            ]
        );
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

    // ─── tested_by ──────────────────────────────────────────────────

    #[test]
    fn a_test_callback_records_what_it_calls() {
        // TS tests are callbacks, not named functions, so the coverage source is
        // identified by its title string — the only stable identity available.
        let out = extract_typescript(
            "src/billing.test.ts",
            "it('charges the order', () => { charge(cart) })\n",
            &ctx(&["src/billing.test.ts"]),
        );
        assert_eq!(
            edges_of(&out, "tested_by"),
            vec![vec![
                "charge".to_string(),
                "typescript:myapp::billing.test::charges the order".to_string()
            ]]
        );
    }

    #[test]
    fn a_test_spelled_with_test_rather_than_it_also_counts() {
        let out = extract_typescript(
            "src/billing.test.ts",
            "test('charges', () => { charge() })\n",
            &ctx(&["src/billing.test.ts"]),
        );
        assert_eq!(edges_of(&out, "tested_by")[0][0], "charge");
    }

    #[test]
    fn a_helper_outside_a_test_callback_is_not_coverage() {
        // A helper's calls are not evidence that anything was verified — the
        // same rule Python applies to non-`test_*` functions.
        let out = extract_typescript(
            "src/billing.test.ts",
            "function buildFixture() { charge() }\n",
            &ctx(&["src/billing.test.ts"]),
        );
        assert!(edges_of(&out, "tested_by").is_empty());
    }

    #[test]
    fn a_method_call_inside_a_test_records_the_method_name() {
        let out = extract_typescript(
            "src/billing.test.ts",
            "it('works', () => { ledger.charge() })\n",
            &ctx(&["src/billing.test.ts"]),
        );
        assert_eq!(edges_of(&out, "tested_by")[0][0], "charge");
    }

    #[test]
    fn a_test_wrapped_in_describe_still_counts() {
        // `describe` is not itself a test invocation, but does not block the
        // ordinary descent into its callback — the `it` inside is walked
        // exactly as if `describe` were not there.
        let out = extract_typescript(
            "src/billing.test.ts",
            "describe('group', () => { it('inner', () => { charge() }) })\n",
            &ctx(&["src/billing.test.ts"]),
        );
        assert_eq!(
            edges_of(&out, "tested_by"),
            vec![vec![
                "charge".to_string(),
                "typescript:myapp::billing.test::inner".to_string()
            ]]
        );
    }

    #[test]
    fn a_test_nested_two_describes_deep_still_counts() {
        let out = extract_typescript(
            "src/billing.test.ts",
            "describe('outer', () => { describe('inner', () => { it('deep', () => { charge() }) }) })\n",
            &ctx(&["src/billing.test.ts"]),
        );
        assert_eq!(edges_of(&out, "tested_by")[0][0], "charge");
    }

    #[test]
    fn an_async_test_callback_still_counts() {
        let out = extract_typescript(
            "src/billing.test.ts",
            "it('async works', async () => { await charge() })\n",
            &ctx(&["src/billing.test.ts"]),
        );
        assert_eq!(edges_of(&out, "tested_by")[0][0], "charge");
    }

    #[test]
    fn a_skipped_test_is_not_coverage() {
        // `it.skip` never runs, so a call inside it is not evidence anything
        // was verified — same principle as the bare-helper case.
        let out = extract_typescript(
            "src/billing.test.ts",
            "it.skip('nope', () => { charge() })\n",
            &ctx(&["src/billing.test.ts"]),
        );
        assert!(edges_of(&out, "tested_by").is_empty());
    }

    #[test]
    fn an_only_test_still_counts() {
        // `test.only` / `it.only` do run (exclusively), so they are coverage.
        let out = extract_typescript(
            "src/billing.test.ts",
            "test.only('yes', () => { charge() })\n",
            &ctx(&["src/billing.test.ts"]),
        );
        assert_eq!(
            edges_of(&out, "tested_by"),
            vec![vec![
                "charge".to_string(),
                "typescript:myapp::billing.test::yes".to_string()
            ]]
        );
    }

    #[test]
    fn a_parameterized_each_test_counts() {
        // `it.each([...])(title, cb)` is a real, executed test — its outer
        // call's `function` field is itself a call expression (`it.each(...)`),
        // one level removed from the plain-identifier and `it.only` shapes.
        let out = extract_typescript(
            "src/billing.test.ts",
            "it.each([1, 2])('works %s', (n) => { charge(n) })\n",
            &ctx(&["src/billing.test.ts"]),
        );
        assert_eq!(
            edges_of(&out, "tested_by"),
            vec![vec![
                "charge".to_string(),
                "typescript:myapp::billing.test::works %s".to_string()
            ]]
        );
    }

    // ─── calls_api ──────────────────────────────────────────────────

    #[test]
    fn a_non_null_assertion_is_a_watched_api_call() {
        let out = extract_typescript(
            "src/billing.ts",
            "export function charge(o?: Order) { return o!.total }\n",
            &ctx(&["src/billing.ts"]),
        );
        assert_eq!(
            edges_of(&out, "calls_api"),
            vec![vec![
                "typescript:myapp::billing::charge".to_string(),
                "non_null_assertion".to_string()
            ]]
        );
    }

    #[test]
    fn a_function_without_assertions_calls_no_watched_api() {
        let out = extract_typescript(
            "src/billing.ts",
            "export function charge(o: Order) { return o.total }\n",
            &ctx(&["src/billing.ts"]),
        );
        assert!(edges_of(&out, "calls_api").is_empty());
    }

    #[test]
    fn several_assertions_in_one_function_yield_one_edge() {
        let out = extract_typescript(
            "src/billing.ts",
            "export function charge(o?: Order) { return o!.total + o!.tax }\n",
            &ctx(&["src/billing.ts"]),
        );
        assert_eq!(edges_of(&out, "calls_api").len(), 1);
    }

    #[test]
    fn a_method_assertion_is_a_watched_api_call() {
        let out = extract_typescript(
            "src/billing.ts",
            "class Ledger { charge(o?: Order) { return o!.total } }\n",
            &ctx(&["src/billing.ts"]),
        );
        assert_eq!(
            edges_of(&out, "calls_api"),
            vec![vec![
                "typescript:myapp::billing::Ledger::charge".to_string(),
                "non_null_assertion".to_string()
            ]]
        );
    }

    #[test]
    fn an_arrow_const_assertion_is_a_watched_api_call() {
        let out = extract_typescript(
            "src/billing.ts",
            "const charge = (o?: Order) => o!.total\n",
            &ctx(&["src/billing.ts"]),
        );
        assert_eq!(
            edges_of(&out, "calls_api"),
            vec![vec![
                "typescript:myapp::billing::charge".to_string(),
                "non_null_assertion".to_string()
            ]]
        );
    }

    #[test]
    fn an_assertion_in_a_nested_named_function_does_not_count_toward_the_outer_function() {
        // Task 6 does not descend into a nested `function_declaration` for
        // `defines_fn` — `inner` gets no identity of its own inside `outer`.
        // Attributing `inner`'s assertion to `outer` would blame code that
        // never runs unless `outer` explicitly invokes `inner`, so the same
        // boundary applies to `calls_api`: deliberately not counted.
        let out = extract_typescript(
            "src/billing.ts",
            "export function outer() { function inner(o?: Order) { return o!.total } }\n",
            &ctx(&["src/billing.ts"]),
        );
        assert!(edges_of(&out, "calls_api").is_empty());
    }

    #[test]
    fn an_assertion_inside_an_inline_callback_does_count_toward_the_enclosing_function() {
        // Unlike a nested *named* function, an anonymous arrow callback
        // (`.map(x => x!.y)`, an inline `const f = () => …` immediately
        // invoked, etc.) has no identity of its own and runs as part of the
        // enclosing function's own control flow — so its assertions belong
        // to the enclosing function.
        let out = extract_typescript(
            "src/billing.ts",
            "export function outer(o?: Order) { const f = () => o!.total; return f() }\n",
            &ctx(&["src/billing.ts"]),
        );
        assert_eq!(
            edges_of(&out, "calls_api"),
            vec![vec![
                "typescript:myapp::billing::outer".to_string(),
                "non_null_assertion".to_string()
            ]]
        );
    }

    // ─── fix round 1: wider tested_by recognition ──────────────────────

    #[test]
    fn a_concurrent_test_counts() {
        let out = extract_typescript(
            "src/billing.test.ts",
            "test.concurrent('t', () => { f() })\n",
            &ctx(&["src/billing.test.ts"]),
        );
        assert_eq!(edges_of(&out, "tested_by")[0][0], "f");
    }

    #[test]
    fn a_failing_test_counts() {
        // `it.failing` still executes its callback (the test passes iff it
        // throws) — the callback running is what matters for coverage.
        let out = extract_typescript(
            "src/billing.test.ts",
            "it.failing('t', () => { f() })\n",
            &ctx(&["src/billing.test.ts"]),
        );
        assert_eq!(edges_of(&out, "tested_by")[0][0], "f");
    }

    #[test]
    fn a_sequential_test_counts() {
        let out = extract_typescript(
            "src/billing.test.ts",
            "test.sequential('t', () => { f() })\n",
            &ctx(&["src/billing.test.ts"]),
        );
        assert_eq!(edges_of(&out, "tested_by")[0][0], "f");
    }

    #[test]
    fn a_todo_test_has_no_callback_and_records_no_coverage_or_skip() {
        // `it.todo('t')` is a placeholder by convention — no callback is
        // ever expected, so this is not an analysis gap and must not
        // inflate `skipped`.
        let out = extract_typescript(
            "src/billing.test.ts",
            "it.todo('not written yet')\n",
            &ctx(&["src/billing.test.ts"]),
        );
        assert!(edges_of(&out, "tested_by").is_empty());
        assert_eq!(out.skipped, 0);
    }

    #[test]
    fn a_reference_callback_records_no_coverage_but_is_counted_as_skipped() {
        // `it('t', handler)` — a named-reference callback. We cannot
        // follow the reference to know what it calls, so this must not
        // silently look like a clean "no coverage" the way `it.todo` does.
        let out = extract_typescript(
            "src/billing.test.ts",
            "it('t', handler)\n",
            &ctx(&["src/billing.test.ts"]),
        );
        assert!(edges_of(&out, "tested_by").is_empty());
        assert_eq!(out.skipped, 1);
    }

    #[test]
    fn a_quoted_title_containing_the_opposite_quote_char_keeps_its_closing_char() {
        // Regression: `trim_matches` against the whole quote set ate the
        // title's own trailing `"` in `'has "quotes"'`. Only the specific
        // delimiter the literal opened with should be stripped.
        let out = extract_typescript(
            "src/billing.test.ts",
            "it('has \"quotes\"', () => { charge() })\n",
            &ctx(&["src/billing.test.ts"]),
        );
        assert_eq!(
            edges_of(&out, "tested_by"),
            vec![vec![
                "charge".to_string(),
                "typescript:myapp::billing.test::has \"quotes\"".to_string()
            ]]
        );
    }

    // ─── fix round 1: defines_fn survives inside a test callback ───────

    #[test]
    fn a_helper_defined_inside_a_test_callback_still_gets_a_defines_fn_edge() {
        // Regression: the `call_expression` arm used to `return` after
        // emitting `tested_by`, skipping the ordinary descent into the
        // callback's own body — silently losing `defines_fn` for anything
        // defined inside a test (Task 6's contract, which this task must
        // not change). Harmless once restored: `file_type` for this file
        // is "test", and `warn-untested-risky-call` gates on
        // `file_type(?file, "production")`.
        let out = extract_typescript(
            "src/billing.test.ts",
            "it('t', () => { const h = () => g(); h() })\n",
            &ctx(&["src/billing.test.ts"]),
        );
        assert_eq!(
            edges_of(&out, "defines_fn")[0][1],
            "typescript:myapp::billing.test::h"
        );
    }

    #[test]
    fn a_class_defined_inside_a_test_callback_still_gets_defines_fn_edges() {
        let out = extract_typescript(
            "src/billing.test.ts",
            "it('t', () => { class C { m() {} } })\n",
            &ctx(&["src/billing.test.ts"]),
        );
        assert_eq!(
            edges_of(&out, "defines_fn")[0][1],
            "typescript:myapp::billing.test::C::m"
        );
    }
}
