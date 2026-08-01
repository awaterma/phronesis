//! The sensor: derives structural edges from one Rust source file.
//!
//! The Rust extractor, via tree-sitter; `super::python` is its counterpart.
//! Regex extraction is deliberately
//! rejected — it cannot tell a call inside a string literal, a comment, or a
//! `cfg`-disabled block from a real one, and each of those is a false-block
//! generator in an enforcement layer (spec §4.1).
//!
//! This tier parses the edited file *only*. Whole-graph facts (`untested`,
//! `in_cycle`) are computed separately in `super::derive`.

use super::model::Edge;
use super::unit::UnitContext;
use crate::syntax::parsed::ParsedFile;
use std::collections::BTreeSet;
use tree_sitter::Node;

/// Default watched-API list. Deliberately small and explicit: `calls_api` is
/// resolved syntactically by method name, so a broad list would over-match
/// user types that happen to share a name (spec §4.3).
pub const DEFAULT_WATCHLIST: &[&str] = &["unwrap", "expect", "panic", "todo", "unimplemented"];

/// Result of extracting one file.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Extracted {
    pub edges: Vec<Edge>,
    /// Items skipped because their qualified name could not be determined
    /// (macro-generated items, `#[path]` indirection). Counted, never guessed.
    pub skipped: usize,
    /// The file could not be parsed at all, so this result carries no
    /// evidence about it — as distinct from a file that genuinely defines
    /// nothing.
    ///
    /// The two look identical as an edge set and must not be treated alike.
    /// Compacting an empty extraction erases every function, call and import
    /// the file had, and recording its hash reports the graph fresh — so the
    /// harness keeps enforcing against evidence it just destroyed. The caller
    /// must instead leave both the graph and the index untouched, which makes
    /// freshness report the file as drifted (spec §4.6).
    pub parse_failed: bool,
}

impl Extracted {
    /// Result for a file that could not be parsed: no evidence, and an
    /// explicit signal that the caller must preserve what it already had.
    pub fn unparseable() -> Self {
        Extracted {
            edges: Vec::new(),
            skipped: 1,
            parse_failed: true,
        }
    }
}

/// Classification of a source file, as the `file_type` relation.
fn file_type(file_path: &str) -> &'static str {
    let p = file_path;
    if p == "build.rs" || p.ends_with("/build.rs") {
        "build"
    } else if p.starts_with("tests/") || p.contains("/tests/") || p.ends_with("_test.rs") {
        "test"
    } else if p.starts_with("examples/") || p.contains("/examples/") {
        "example"
    } else if p.starts_with("benches/") || p.contains("/benches/") {
        "build"
    } else {
        "production"
    }
}

/// Module path for a source file, e.g. `src/network.rs` ->
/// `rust:phronesis::network`.
///
/// The unit prefix already names the package and the compilation target, so
/// the module part is anchored at that target's own module root and carries
/// only what the prefix does not already say.
pub fn module_path(file_path: &str, unit: &UnitContext) -> String {
    let rel = module_relative(file_path, unit);
    let trimmed = rel.strip_suffix(".rs").unwrap_or(rel);
    let mut segments: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();
    // `lib.rs`, `main.rs` and `mod.rs` name their parent, not themselves.
    if matches!(segments.last(), Some(&"lib" | &"main" | &"mod")) {
        segments.pop();
    }
    std::iter::once(unit.id.as_str())
        .chain(segments)
        .collect::<Vec<_>>()
        .join("::")
}

/// Strip everything the unit prefix already encodes, leaving the part of the
/// path that is genuinely a module path.
///
/// A target's module root is the directory of its crate-root file, and the
/// crate-root file itself is the target root. `module_base` names that root
/// without the `.rs` — so `benches/graph_sync.rs` yields nothing (the target
/// root) and `benches/graph_sync/fixtures.rs` yields `fixtures`. Without
/// this, a non-`src/` target restated its entire repo-relative path inside an
/// identity that already began with `rust:pkg#bench:graph_sync`.
fn module_relative<'a>(file_path: &'a str, unit: &UnitContext) -> &'a str {
    if !unit.module_base.is_empty() {
        if file_path.strip_prefix(unit.module_base.as_str()) == Some(".rs") {
            // The crate-root file itself is the target root, not a module
            // inside it.
            return "";
        }
        // Every other module resolves against the crate-root file's
        // *directory* — `benches/x.rs` declaring `mod helpers;` means
        // `benches/helpers.rs`, not `benches/x/helpers.rs`.
        let dir = unit.module_base.rsplit_once('/').map_or("", |(dir, _)| dir);
        if let Some(rest) = file_path
            .strip_prefix(dir)
            .and_then(|rest| rest.strip_prefix('/'))
        {
            return rest;
        }
    }
    // No manifest claimed this file: fall back to anchoring on the last
    // `src/`, which resolves flat and workspace layouts alike.
    match file_path.rfind("src/") {
        Some(i) => &file_path[i + 4..],
        None => file_path
            .trim_start_matches("tests/")
            .trim_start_matches("examples/")
            .trim_start_matches("benches/"),
    }
}

/// Text of a node, or empty when it is not valid UTF-8.
fn text<'a>(node: Node, source: &'a [u8]) -> &'a str {
    node.utf8_text(source).unwrap_or("")
}

/// True when `attr` is a test marker: `#[test]`, `#[tokio::test]`, or a
/// `cfg` gate naming `test`.
///
/// Matches the attribute's **path**, not its text. A substring search for
/// `test` treats `#[tool(description = "...which tests cover...")]` as a test
/// marker, which removes the function from `defines_fn` and — far worse —
/// turns every call in its body into a `tested_by` edge, silently inflating
/// coverage across the graph.
fn is_test_attribute(attr_text: &str) -> bool {
    let inner = attr_text
        .trim()
        .trim_start_matches("#")
        .trim_start_matches("[")
        .trim_end_matches("]")
        .trim();
    // The path is everything before any argument list.
    let path = inner.split('(').next().unwrap_or("").trim();
    if path == "test" || path.ends_with("::test") {
        return true;
    }
    // `#[cfg(test)]`, `#[cfg(all(test, ...))]`.
    if path == "cfg" {
        let args = inner
            .split_once('(')
            .map(|(_, rest)| rest.strip_suffix(')').unwrap_or(rest))
            .unwrap_or("");
        return cfg_asserts_test(args);
    }
    false
}

/// True when a `cfg` predicate list *positively* asserts the bare `test` flag.
///
/// Parsed rather than scanned for a token. A flat scan cannot see the two
/// things that matter: `not(test)` marks production-only code, and
/// `feature = "test-utils"` names a feature whose value merely contains the
/// word. Both were read as test markers, which dropped the function from
/// `defines_fn` and turned every call in its body into a `tested_by` edge —
/// inflating coverage across the whole graph and hiding the untested-risky
/// calls this pack exists to surface.
fn cfg_asserts_test(args: &str) -> bool {
    for predicate in split_top_level(args) {
        let predicate = predicate.trim();
        if predicate == "test" {
            return true;
        }
        // `not(...)`: whatever it names, this is not test-gated code.
        if predicate
            .strip_prefix("not")
            .is_some_and(|rest| rest.trim_start().starts_with('('))
        {
            continue;
        }
        // `all(...)` / `any(...)`: a nested list, any branch of which may
        // assert `test`.
        for combinator in ["all", "any"] {
            if let Some(rest) = predicate.strip_prefix(combinator)
                && let Some(rest) = rest.trim_start().strip_prefix('(')
                && cfg_asserts_test(rest.strip_suffix(')').unwrap_or(rest))
            {
                return true;
            }
        }
        // Anything else (`feature = "..."`, `target_os = "..."`) names a
        // different flag, whatever its value happens to spell.
    }
    false
}

/// Split a `cfg` predicate list on top-level commas, ignoring commas nested
/// in parentheses or inside string literals.
fn split_top_level(args: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut in_quotes, mut start) = (0i32, false, 0usize);
    for (i, c) in args.char_indices() {
        match c {
            '"' => in_quotes = !in_quotes,
            '(' if !in_quotes => depth += 1,
            ')' if !in_quotes => depth -= 1,
            ',' if !in_quotes && depth == 0 => {
                out.push(&args[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(&args[start..]);
    out
}

/// True when a test-marker attribute is attached to `node`.
fn has_test_attribute(node: Node, source: &[u8]) -> bool {
    let mut sib = node.prev_sibling();
    while let Some(s) = sib {
        match s.kind() {
            "attribute_item" => {
                if is_test_attribute(text(s, source)) {
                    return true;
                }
            }
            // Doc comments may interleave with attributes.
            "line_comment" | "block_comment" => {}
            _ => break,
        }
        sib = s.prev_sibling();
    }
    false
}

/// Where in the module tree the walk currently is.
#[derive(Clone)]
struct Scope {
    /// Module prefix, already including the file's own module path.
    path: Vec<String>,
    /// Enclosing `impl` type, if any.
    impl_type: Option<String>,
}

impl Scope {
    fn qualify(&self, name: &str) -> String {
        let mut parts = self.path.clone();
        if let Some(t) = &self.impl_type {
            parts.push(t.clone());
        }
        parts.push(name.to_string());
        parts.join("::")
    }
}

/// Collector for one file's extraction.
struct Sensor<'a> {
    file_path: &'a str,
    source: &'a [u8],
    watchlist: &'a [&'a str],
    self_module: String,
    unit: &'a UnitContext,
    /// A set, so a function calling `.expect()` twice yields one edge.
    out: BTreeSet<(String, Vec<String>)>,
    skipped: usize,
}

impl Sensor<'_> {
    fn emit(&mut self, p: &str, args: &[&str]) {
        self.out
            .insert((p.to_string(), args.iter().map(|s| s.to_string()).collect()));
    }

    /// Walk any node, tracking module and impl scope.
    fn walk(&mut self, node: Node, scope: &Scope) {
        match node.kind() {
            "mod_item" => {
                let Some(name) = node.child_by_field_name("name") else {
                    self.skipped += 1;
                    return;
                };
                let mut inner = scope.clone();
                inner.path.push(text(name, self.source).to_string());
                self.walk_children(node, &inner);
            }
            "impl_item" => {
                let mut inner = scope.clone();
                inner.impl_type = node
                    .child_by_field_name("type")
                    .map(|t| text(t, self.source).to_string());
                self.walk_children(node, &inner);
            }
            "function_item" => self.visit_function(node, scope),
            "use_declaration" => self.visit_use(node, scope),
            _ => self.walk_children(node, scope),
        }
    }

    fn walk_children(&mut self, node: Node, scope: &Scope) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.walk(child, scope);
        }
    }

    fn visit_function(&mut self, node: Node, scope: &Scope) {
        let Some(name_node) = node.child_by_field_name("name") else {
            self.skipped += 1;
            return;
        };
        let name = text(name_node, self.source);
        if name.is_empty() {
            self.skipped += 1;
            return;
        }
        let qualified = scope.qualify(name);
        let Some(body) = node.child_by_field_name("body") else {
            // A trait signature declares; it does not define.
            return;
        };

        if has_test_attribute(node, self.source) {
            // Test functions are coverage sources, not coverage subjects.
            for callee in self.called_names(body) {
                self.emit("tested_by", &[&callee, &qualified]);
            }
            return;
        }

        let file_path = self.file_path.to_string();
        self.emit("defines_fn", &[&file_path, &qualified]);
        for api in self.watched_calls(body) {
            self.emit("calls_api", &[&qualified, &api]);
        }
    }

    /// Bare names of functions invoked in a body. Used only for `tested_by`,
    /// which resolves by short name — see `derive::untested`.
    fn called_names(&self, body: Node) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        let mut stack = vec![body];
        while let Some(n) = stack.pop() {
            if n.kind() == "call_expression"
                && let Some(f) = n.child_by_field_name("function")
            {
                let name = match f.kind() {
                    "identifier" => text(f, self.source).to_string(),
                    "scoped_identifier" | "field_expression" => f
                        .child_by_field_name("name")
                        .or_else(|| f.child_by_field_name("field"))
                        .map(|x| text(x, self.source).to_string())
                        .unwrap_or_default(),
                    _ => String::new(),
                };
                if !name.is_empty() {
                    found.insert(name);
                }
            }
            let mut cursor = n.walk();
            stack.extend(n.children(&mut cursor));
        }
        found
    }

    /// Watched method calls and macro invocations within a body.
    fn watched_calls(&self, body: Node) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        let mut stack = vec![body];
        while let Some(n) = stack.pop() {
            let name = match n.kind() {
                "call_expression" => n
                    .child_by_field_name("function")
                    .filter(|f| f.kind() == "field_expression")
                    .and_then(|f| f.child_by_field_name("field"))
                    .map(|x| text(x, self.source).to_string()),
                "macro_invocation" => n
                    .child_by_field_name("macro")
                    .map(|x| text(x, self.source).to_string()),
                _ => None,
            };
            if let Some(name) = name
                && self.watchlist.contains(&name.as_str())
            {
                found.insert(name);
            }
            let mut cursor = n.walk();
            stack.extend(n.children(&mut cursor));
        }
        found
    }

    /// Intra-crate `use` declarations become `imports` edges. External crates
    /// are ignored: only intra-crate edges can form the cycles we detect.
    ///
    /// Relative anchors (`super::`, `self::`) resolve against `scope`, the
    /// module the statement is written in — not the file. `#[cfg(test)] mod
    /// tests { use super::*; }` is the most common use statement in Rust, and
    /// resolving it against the file would invent an edge to the file's
    /// parent instead of correctly recognizing a self-import.
    fn visit_use(&mut self, node: Node, scope: &Scope) {
        let Some(arg) = node.child_by_field_name("argument") else {
            return;
        };
        let raw = text(arg, self.source);
        // Take the path prefix before any brace group or glob.
        let head = raw.split(['{', '*']).next().unwrap_or("").trim();
        let mut segments: Vec<&str> = head
            .split("::")
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();

        // Resolve the leading anchor to an absolute module path.
        let mut resolved: Vec<String> = match segments.first() {
            Some(&"crate") => {
                segments.remove(0);
                vec![self.unit.id.clone()]
            }
            Some(&"self") => {
                segments.remove(0);
                scope.path.clone()
            }
            Some(&"super") => {
                let mut base = scope.path.clone();
                while segments.first() == Some(&"super") {
                    segments.remove(0);
                    // `crate` is the root; climbing past it is not a path we
                    // can name, so the statement contributes no edge.
                    if base.len() <= 1 {
                        return;
                    }
                    base.pop();
                }
                base
            }
            Some(alias) => {
                let Some(target) = self.unit.siblings.get(*alias) else {
                    // An external crate, or a bare item already in scope.
                    return;
                };
                segments.remove(0);
                vec![target.clone()]
            }
            None => return,
        };
        resolved.extend(segments.iter().map(|s| (*s).to_string()));

        // Drop the imported item, leaving its module. A trailing `::` means a
        // brace group or glob followed, so we are already at the module.
        if !head.ends_with("::") && resolved.len() > 1 {
            resolved.pop();
        }
        let target = resolved.join("::");
        if target.is_empty() || target == self.self_module {
            return;
        }
        let from = self.self_module.clone();
        self.emit("imports", &[&from, &target]);
    }
}

/// Extract every base relation from one Rust file.
pub fn extract_rust(
    file_path: &str,
    content: &str,
    watchlist: &[&str],
    unit: &UnitContext,
) -> Extracted {
    if !file_path.ends_with(".rs") {
        return Extracted::default();
    }
    // A file we cannot parse contributes no edges at all: a partial edge set
    // is indistinguishable from a shrinking one, and would read downstream as
    // "this function lost its test" (spec §4.6).
    let Some(parsed) = ParsedFile::parse_rust(content) else {
        return Extracted::unparseable();
    };
    let ParsedFile::Rust { tree, source } = &parsed else {
        return Extracted::unparseable();
    };
    // A tree with error nodes is a half-edited file. Publishing its partial
    // edges would silently drop whatever the parser could not reach, which
    // reads downstream as "these functions lost their tests".
    if tree.root_node().has_error() {
        return Extracted::unparseable();
    }

    let self_module = module_path(file_path, unit);
    let mut sensor = Sensor {
        file_path,
        source: source.as_bytes(),
        watchlist,
        self_module: self_module.clone(),
        unit,
        out: BTreeSet::new(),
        skipped: 0,
    };
    sensor.emit("file_type", &[file_path, file_type(file_path)]);
    // Links a file to its module, so a rule matching on module-keyed
    // relations (`in_cycle`) can be scoped to the file being edited.
    sensor.emit("declares_module", &[file_path, &self_module]);

    let root_scope = Scope {
        path: self_module.split("::").map(str::to_string).collect(),
        impl_type: None,
    };
    sensor.walk(tree.root_node(), &root_scope);

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

    fn edges_of(x: &Extracted, p: &str) -> Vec<Vec<String>> {
        x.edges
            .iter()
            .filter(|e| e.p == p)
            .map(|e| e.a.clone())
            .collect()
    }

    fn run(path: &str, src: &str) -> Extracted {
        extract_rust(path, src, DEFAULT_WATCHLIST, &UnitContext::default())
    }

    // ─── module naming ──────────────────────────────────────────────

    #[test]
    fn a_module_file_maps_to_its_module_path() {
        assert_eq!(
            module_path("src/network.rs", &UnitContext::default()),
            "rust:crate::network"
        );
    }

    #[test]
    fn lib_rs_maps_to_the_crate_root() {
        assert_eq!(
            module_path("src/lib.rs", &UnitContext::default()),
            "rust:crate"
        );
    }

    #[test]
    fn a_nested_directory_becomes_a_nested_module_path() {
        assert_eq!(
            module_path("src/graph/store.rs", &UnitContext::default()),
            "rust:crate::graph::store"
        );
    }

    #[test]
    fn a_mod_rs_file_names_its_directory() {
        assert_eq!(
            module_path("src/graph/mod.rs", &UnitContext::default()),
            "rust:crate::graph"
        );
    }

    #[test]
    fn a_targets_module_path_does_not_restate_its_own_path() {
        // The unit prefix already says `#bench:graph_sync`; repeating
        // `crates::phronesis_mcp::benches::graph_sync` after it is noise that
        // also makes the identity depend on where the package sits in the
        // repo.
        let unit = UnitContext {
            id: "rust:app#bench:sync".to_string(),
            module_base: "crates/app/benches/sync".to_string(),
            siblings: BTreeMap::new(),
            ts: TsConfig::default(),
            files: Vec::new(),
        };
        assert_eq!(
            module_path("crates/app/benches/sync.rs", &unit),
            "rust:app#bench:sync"
        );
    }

    #[test]
    fn a_module_beside_a_crate_root_file_is_named_relative_to_it() {
        // Rust resolves `mod helper;` in a target root against the root
        // file's directory, so the graph must too.
        let unit = UnitContext {
            id: "rust:app#test:hooks".to_string(),
            module_base: "crates/app/tests/hooks".to_string(),
            siblings: BTreeMap::new(),
            ts: TsConfig::default(),
            files: Vec::new(),
        };
        assert_eq!(
            module_path("crates/app/tests/helper.rs", &unit),
            "rust:app#test:hooks::helper"
        );
    }

    #[test]
    fn a_library_module_is_named_relative_to_the_source_root() {
        let unit = UnitContext {
            id: "rust:app".to_string(),
            module_base: "crates/app/src/lib".to_string(),
            siblings: BTreeMap::new(),
            ts: TsConfig::default(),
            files: Vec::new(),
        };
        assert_eq!(module_path("crates/app/src/lib.rs", &unit), "rust:app");
        assert_eq!(
            module_path("crates/app/src/graph/store.rs", &unit),
            "rust:app::graph::store"
        );
        assert_eq!(
            module_path("crates/app/src/graph/mod.rs", &unit),
            "rust:app::graph"
        );
    }

    // ─── file_type ──────────────────────────────────────────────────

    #[test]
    fn a_src_file_is_production() {
        assert_eq!(
            edges_of(&run("src/a.rs", "fn f() {}"), "file_type"),
            vec![vec!["src/a.rs".to_string(), "production".to_string()]]
        );
    }

    #[test]
    fn a_file_under_tests_is_a_test_file() {
        assert_eq!(
            edges_of(&run("tests/a.rs", "fn f() {}"), "file_type"),
            vec![vec!["tests/a.rs".to_string(), "test".to_string()]]
        );
    }

    #[test]
    fn a_file_under_examples_is_an_example() {
        assert_eq!(
            edges_of(&run("examples/a.rs", "fn f() {}"), "file_type"),
            vec![vec!["examples/a.rs".to_string(), "example".to_string()]]
        );
    }

    #[test]
    fn build_rs_is_a_build_file() {
        assert_eq!(
            edges_of(&run("build.rs", "fn main() {}"), "file_type"),
            vec![vec!["build.rs".to_string(), "build".to_string()]]
        );
    }

    // ─── defines_fn ─────────────────────────────────────────────────

    #[test]
    fn a_file_declares_its_own_module() {
        assert_eq!(
            edges_of(&run("src/network.rs", "fn f() {}"), "declares_module"),
            vec![vec![
                "src/network.rs".to_string(),
                "rust:crate::network".to_string()
            ]]
        );
    }

    #[test]
    fn a_free_function_is_defined_with_its_qualified_name() {
        let out = run("src/network.rs", "fn fire() {}");
        assert_eq!(
            edges_of(&out, "defines_fn"),
            vec![vec![
                "src/network.rs".to_string(),
                "rust:crate::network::fire".to_string()
            ]]
        );
    }

    #[test]
    fn an_inline_mod_block_extends_the_qualified_name() {
        let out = run("src/network.rs", "mod inner { fn fire() {} }");
        assert_eq!(
            edges_of(&out, "defines_fn")[0][1],
            "rust:crate::network::inner::fire"
        );
    }

    #[test]
    fn an_impl_method_is_qualified_by_its_type() {
        let out = run("src/network.rs", "impl Network { fn fire(&self) {} }");
        assert_eq!(
            edges_of(&out, "defines_fn")[0][1],
            "rust:crate::network::Network::fire"
        );
    }

    #[test]
    fn a_trait_signature_without_a_body_is_not_a_definition() {
        let out = run("src/a.rs", "trait T { fn f(&self); }");
        assert!(edges_of(&out, "defines_fn").is_empty());
    }

    // ─── calls_api ──────────────────────────────────────────────────

    // Fixtures use `expect` rather than `unwrap` as the watched method: the
    // repo's own `enforce-no-unwrap-in-src` rule scans whole-file content and
    // cannot tell a fixture string from real code.
    #[test]
    fn a_watched_method_call_is_recorded_against_its_caller() {
        let out = run("src/a.rs", "fn f() { let x = g().expect(\"m\"); }");
        assert_eq!(
            edges_of(&out, "calls_api"),
            vec![vec!["rust:crate::a::f".to_string(), "expect".to_string()]]
        );
    }

    #[test]
    fn a_watched_macro_is_recorded_against_its_caller() {
        let out = run("src/a.rs", "fn f() { panic!(\"boom\"); }");
        assert_eq!(
            edges_of(&out, "calls_api"),
            vec![vec!["rust:crate::a::f".to_string(), "panic".to_string()]]
        );
    }

    #[test]
    fn an_unwatched_method_call_is_ignored() {
        let out = run("src/a.rs", "fn f() { g().into_iter(); }");
        assert!(edges_of(&out, "calls_api").is_empty());
    }

    #[test]
    fn a_call_named_in_a_string_literal_is_not_a_call() {
        // The reason regex extraction was rejected.
        let out = run("src/a.rs", "fn f() { let s = \"call .expect() here\"; }");
        assert!(edges_of(&out, "calls_api").is_empty());
    }

    #[test]
    fn a_call_inside_a_comment_is_not_a_call() {
        let out = run("src/a.rs", "fn f() { // x.expect(\"m\")\n }");
        assert!(edges_of(&out, "calls_api").is_empty());
    }

    #[test]
    fn the_watchlist_bounds_what_is_recorded() {
        let out = extract_rust(
            "src/a.rs",
            "fn f() { g().expect(\"m\"); }",
            &["unwrap"],
            &UnitContext::default(),
        );
        assert!(edges_of(&out, "calls_api").is_empty());
    }

    #[test]
    fn repeated_calls_collapse_to_one_edge() {
        let out = run(
            "src/a.rs",
            "fn f() { a().expect(\"m\"); b().expect(\"m\"); }",
        );
        assert_eq!(edges_of(&out, "calls_api").len(), 1);
    }

    // ─── imports ────────────────────────────────────────────────────

    #[test]
    fn a_use_declaration_becomes_an_import_edge() {
        let out = run("src/a.rs", "use crate::network::Thing;");
        assert_eq!(
            edges_of(&out, "imports"),
            vec![vec![
                "rust:crate::a".to_string(),
                "rust:crate::network".to_string()
            ]]
        );
    }

    #[test]
    fn a_self_import_is_not_recorded() {
        let out = run("src/a.rs", "use crate::a::Thing;");
        assert!(edges_of(&out, "imports").is_empty());
    }

    #[test]
    fn a_super_import_resolves_against_the_enclosing_module() {
        // ~40% of this repo's intra-crate `use` statements are `super::`.
        // Dropping them understates fan-in and hides import cycles.
        let out = run("src/syntax/rust/signatures.rs", "use super::walk::helper;");
        assert_eq!(
            edges_of(&out, "imports"),
            vec![vec![
                "rust:crate::syntax::rust::signatures".to_string(),
                "rust:crate::syntax::rust::walk".to_string()
            ]]
        );
    }

    #[test]
    fn a_super_import_of_a_bare_item_targets_the_parent_module() {
        let out = run("src/syntax/rust/signatures.rs", "use super::Thing;");
        assert_eq!(edges_of(&out, "imports")[0][1], "rust:crate::syntax::rust");
    }

    #[test]
    fn a_doubled_super_climbs_two_levels() {
        let out = run(
            "src/syntax/rust/signatures.rs",
            "use super::super::facts::F;",
        );
        assert_eq!(edges_of(&out, "imports")[0][1], "rust:crate::syntax::facts");
    }

    #[test]
    fn a_super_glob_targets_the_parent_module() {
        let out = run("src/syntax/rust/signatures.rs", "use super::*;");
        assert_eq!(edges_of(&out, "imports")[0][1], "rust:crate::syntax::rust");
    }

    #[test]
    fn a_test_modules_super_glob_is_a_self_import_not_a_parent_edge() {
        // `#[cfg(test)] mod tests { use super::*; }` is the single most common
        // use statement in Rust. Resolving it against the file rather than the
        // inline module would invent an edge to the file's parent.
        let out = run("src/a.rs", "#[cfg(test)]\nmod tests { use super::*; }");
        assert!(
            edges_of(&out, "imports").is_empty(),
            "expected no edge, got {:?}",
            edges_of(&out, "imports")
        );
    }

    #[test]
    fn a_self_import_resolves_to_the_current_module() {
        let out = run("src/syntax/rust/signatures.rs", "use self::inner::Thing;");
        assert_eq!(
            edges_of(&out, "imports")[0][1],
            "rust:crate::syntax::rust::signatures::inner"
        );
    }

    #[test]
    fn a_super_import_that_would_climb_above_the_crate_root_is_ignored() {
        let out = run("src/a.rs", "use super::super::super::Thing;");
        assert!(edges_of(&out, "imports").is_empty());
    }

    #[test]
    fn external_crate_imports_are_not_recorded() {
        // Only intra-crate edges matter for cycle detection.
        let out = run("src/a.rs", "use serde::Serialize;");
        assert!(edges_of(&out, "imports").is_empty());
    }

    #[test]
    fn a_sibling_dependency_alias_resolves_to_the_siblings_unit() {
        let unit = UnitContext {
            id: "rust:app".to_string(),
            module_base: "crates/app/src/lib".to_string(),
            siblings: BTreeMap::from([("core".to_string(), "rust:core-lib".to_string())]),
            ts: TsConfig::default(),
            files: Vec::new(),
        };
        let out = extract_rust(
            "crates/app/src/a.rs",
            "use core::network::Thing;",
            DEFAULT_WATCHLIST,
            &unit,
        );
        assert_eq!(
            edges_of(&out, "imports"),
            vec![vec![
                "rust:app::a".to_string(),
                "rust:core-lib::network".to_string()
            ]]
        );
    }

    // ─── cfg gating ─────────────────────────────────────────────────

    #[test]
    fn a_cfg_test_gate_marks_a_test_function() {
        let out = run("src/a.rs", "#[cfg(test)]\nfn t() { fire(); }");
        assert!(edges_of(&out, "defines_fn").is_empty());
        assert_eq!(edges_of(&out, "tested_by")[0][0], "fire");
    }

    #[test]
    fn a_cfg_all_gate_naming_test_marks_a_test_function() {
        let out = run("src/a.rs", "#[cfg(all(test, unix))]\nfn t() { fire(); }");
        assert_eq!(edges_of(&out, "tested_by")[0][0], "fire");
    }

    #[test]
    fn a_negated_test_gate_is_production_code() {
        // `#[cfg(not(test))]` is the idiomatic way to mark production-only
        // code. Reading it as a test marker removed the function from
        // `defines_fn` and turned its calls into coverage edges.
        let out = run(
            "src/a.rs",
            "#[cfg(not(test))]\nfn prod() { g().expect(\"m\"); }",
        );
        assert_eq!(edges_of(&out, "defines_fn")[0][1], "rust:crate::a::prod");
        assert!(
            edges_of(&out, "tested_by").is_empty(),
            "production-only code provides no coverage"
        );
    }

    #[test]
    fn a_negated_test_gate_nested_in_all_is_still_production_code() {
        let out = run(
            "src/a.rs",
            "#[cfg(all(not(test), unix))]\nfn prod() { g().expect(\"m\"); }",
        );
        assert_eq!(edges_of(&out, "defines_fn").len(), 1);
        assert!(edges_of(&out, "tested_by").is_empty());
    }

    #[test]
    fn a_feature_whose_name_contains_test_is_not_a_test_gate() {
        let out = run(
            "src/a.rs",
            "#[cfg(feature = \"test-utils\")]\nfn helper() { g().expect(\"m\"); }",
        );
        assert_eq!(edges_of(&out, "defines_fn").len(), 1);
        assert!(edges_of(&out, "tested_by").is_empty());
    }

    #[test]
    fn a_feature_gate_beside_a_real_test_gate_still_marks_a_test() {
        let out = run(
            "src/a.rs",
            "#[cfg(all(feature = \"x\", test))]\nfn t() { fire(); }",
        );
        assert_eq!(edges_of(&out, "tested_by")[0][0], "fire");
    }

    // ─── tested_by ──────────────────────────────────────────────────

    #[test]
    fn a_test_function_calling_a_function_covers_it() {
        let src = "#[test]\nfn t_fire() { fire(); }";
        let out = run("tests/net.rs", src);
        let e = edges_of(&out, "tested_by");
        assert_eq!(e.len(), 1);
        assert_eq!(e[0][0], "fire");
    }

    #[test]
    fn a_non_test_function_does_not_create_coverage() {
        let out = run("src/a.rs", "fn helper() { fire(); }");
        assert!(edges_of(&out, "tested_by").is_empty());
    }

    #[test]
    fn an_attribute_that_merely_mentions_tests_is_not_a_test_attribute() {
        // `#[tool(description = "...which tests cover...")]` in server.rs was
        // read as `#[test]` by a substring match. The function then vanished
        // from `defines_fn` and — far worse — every call in its body became a
        // `tested_by` edge, silently inflating coverage.
        let src =
            "#[tool(description = \"which tests cover a function\")]\nfn handler() { helper(); }";
        let out = run("src/a.rs", src);
        assert!(
            edges_of(&out, "tested_by").is_empty(),
            "must not emit coverage: {:?}",
            edges_of(&out, "tested_by")
        );
        assert_eq!(
            edges_of(&out, "defines_fn")[0][1],
            "rust:crate::a::handler",
            "must still be a defined function"
        );
    }

    #[test]
    fn a_doc_comment_mentioning_tests_is_not_a_test_attribute() {
        let src = "#[doc = \"run the tests first\"]\nfn helper() { inner(); }";
        let out = run("src/a.rs", src);
        assert!(edges_of(&out, "tested_by").is_empty());
        assert_eq!(edges_of(&out, "defines_fn").len(), 1);
    }

    #[test]
    fn a_cfg_test_module_function_counts_as_a_test() {
        let src = "#[cfg(test)]\nmod tests {\n #[test]\n fn t() { fire(); }\n}";
        let out = run("src/a.rs", src);
        assert!(!edges_of(&out, "tested_by").is_empty());
    }

    #[test]
    fn a_tokio_test_counts_as_a_test() {
        let src = "#[tokio::test]\nasync fn t() { fire(); }";
        let out = run("tests/a.rs", src);
        assert!(!edges_of(&out, "tested_by").is_empty());
    }

    // ─── provenance & failure ───────────────────────────────────────

    #[test]
    fn every_edge_carries_the_source_file_as_provenance() {
        let out = run(
            "src/a.rs",
            "use crate::b::T;\nfn f() { g().expect(\"m\"); }",
        );
        assert!(!out.edges.is_empty());
        assert!(out.edges.iter().all(|e| e.src == "src/a.rs" && !e.d));
    }

    #[test]
    fn the_extractor_never_emits_derived_edges() {
        let out = run("src/a.rs", "fn f() {}");
        assert!(
            out.edges
                .iter()
                .all(|e| e.p != "untested" && e.p != "in_cycle")
        );
    }
}
