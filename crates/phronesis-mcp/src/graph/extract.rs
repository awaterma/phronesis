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
use super::ownership::config::OwnershipConfig;
use super::ownership::extract::FileOwnership;
use super::unit::UnitContext;
use crate::syntax::parsed::ParsedFile;
use std::collections::BTreeSet;
use std::sync::LazyLock;
use tree_sitter::Node;

static MACRO_CALL_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\b([_A-Za-z][_A-Za-z0-9]*)\s*\(").expect("static Rust macro call regex")
});

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

/// True when `node` is lexically inside a test-gated module.
///
/// `has_test_attribute` only looks at a node's own preceding attributes, so a
/// plain helper inside `#[cfg(test)] mod tests` looks like production code to
/// it. Ownership extraction needs the enclosing view (D14): a site in a test
/// module is fixture text, and indexing it would let test modules dominate
/// ownership edge volume.
fn within_test_module(node: Node, source: &[u8]) -> bool {
    let mut current = node.parent();
    while let Some(ancestor) = current {
        if ancestor.kind() == "mod_item" && has_test_attribute(ancestor, source) {
            return true;
        }
        current = ancestor.parent();
    }
    false
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
    /// Present only when the project opted into ownership enrichment for this
    /// file. It rides along inside this walk rather than re-parsing or
    /// re-deriving function ids, per decision D13.
    ownership: Option<FileOwnership<'a>>,
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
            "function_item" => {
                self.visit_function(node, scope);
                // A function body is not walked as items — that would invent
                // nested `defines_fn` entries for closures and inner helpers.
                // But function-local `use` is how a dependency needed by one
                // function is conventionally scoped, and skipping it makes a
                // file that visibly imports a module report no edge to it.
                self.visit_nested_uses(node, scope);
            }
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
            let file_path = self.file_path.to_string();
            self.emit("defines_test", &[&file_path, &qualified]);
            for callee in self.called_names(body) {
                self.emit("tested_by", &[&callee, &qualified]);
            }
            return;
        }

        let file_path = self.file_path.to_string();
        self.emit("defines_fn", &[&file_path, &qualified]);
        if scope.impl_type.is_some() {
            self.emit("defines_method", &[&file_path, &qualified]);
        }
        for callee in self.called_names(body) {
            self.emit("calls", &[&qualified, &callee]);
        }
        for api in self.watched_calls(body) {
            self.emit("calls_api", &[&qualified, &api]);
        }

        // The ownership hook (D13). It runs here, after `defines_fn`, so it
        // receives the identical `Scope::qualify` output — reconstructing that
        // id independently diverges on generic impls and `#[path]` modules —
        // and it never runs for a function the graph has no `defines_fn` for.
        // `is_some` first: `within_test_module` walks the ancestor chain, and
        // with ownership disabled that walk buys nothing, on every function in
        // the repository.
        if self.ownership.is_some()
            && !within_test_module(node, self.source)
            && let Some(collector) = self.ownership.as_mut()
        {
            collector.visit_function(&qualified, body);
        }
    }

    /// Bare names of functions invoked in a body. Persistence resolves these
    /// against canonical definitions using same-module/import evidence.
    fn called_names(&self, body: Node) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        let mut receiver_types = std::collections::BTreeMap::new();
        let mut declarations = vec![body];
        while let Some(node) = declarations.pop() {
            if node.kind() == "let_declaration"
                && let Some(pattern) = node.child_by_field_name("pattern")
                && pattern.kind() == "identifier"
            {
                let inferred = node
                    .child_by_field_name("type")
                    .map(|ty| text(ty, self.source).to_string())
                    .or_else(|| {
                        let value = node.child_by_field_name("value")?;
                        if value.kind() == "call_expression" {
                            let function = value.child_by_field_name("function")?;
                            if function.kind() == "scoped_identifier" {
                                return function
                                    .child_by_field_name("path")
                                    .map(|path| text(path, self.source).to_string());
                            }
                        }
                        None
                    })
                    .and_then(|ty| ty.rsplit("::").next().map(str::to_string));
                if let Some(inferred) = inferred {
                    receiver_types.insert(text(pattern, self.source).to_string(), inferred);
                }
            }
            let mut cursor = node.walk();
            declarations.extend(node.children(&mut cursor));
        }
        let mut stack = vec![body];
        while let Some(n) = stack.pop() {
            if n.kind() == "call_expression"
                && let Some(f) = n.child_by_field_name("function")
            {
                let name = match f.kind() {
                    "identifier" => text(f, self.source).to_string(),
                    "scoped_identifier" => f
                        .child_by_field_name("name")
                        .map(|x| text(x, self.source).to_string())
                        .unwrap_or_default(),
                    "field_expression" => f
                        .child_by_field_name("field")
                        .map(|x| {
                            let method = text(x, self.source);
                            let receiver_type = f
                                .child_by_field_name("value")
                                .filter(|receiver| receiver.kind() == "identifier")
                                .and_then(|receiver| {
                                    receiver_types.get(text(receiver, self.source))
                                });
                            receiver_type.map_or_else(
                                || format!("@method:{method}"),
                                |ty| format!("@method:{ty}:{method}"),
                            )
                        })
                        .unwrap_or_default(),
                    _ => String::new(),
                };
                if !name.is_empty() {
                    found.insert(name);
                }
            } else if n.kind() == "macro_invocation" {
                found.extend(self.called_names_in_macro(n));
            }
            let mut cursor = n.walk();
            stack.extend(n.children(&mut cursor));
        }
        found
    }

    /// Calls nested in a Rust macro token tree are opaque to tree-sitter's
    /// ordinary `call_expression` nodes. Scan only that syntax node after
    /// masking its parsed string/comment descendants; whole-file regexes are
    /// deliberately avoided.
    fn called_names_in_macro(&self, macro_node: Node) -> BTreeSet<String> {
        let start = macro_node.start_byte();
        let end = macro_node.end_byte();
        let mut shaped = self.source[start..end].to_vec();
        let mut pending = vec![macro_node];
        while let Some(node) = pending.pop() {
            if node != macro_node
                && (node.kind().contains("string") || node.kind().contains("comment"))
            {
                let from = node.start_byte().saturating_sub(start);
                let to = node.end_byte().saturating_sub(start).min(shaped.len());
                shaped[from..to].fill(b' ');
                continue;
            }
            let mut cursor = node.walk();
            pending.extend(node.children(&mut cursor));
        }
        let shaped = String::from_utf8_lossy(&shaped);
        MACRO_CALL_RE
            .captures_iter(&shaped)
            .filter_map(|captures| captures.get(1).map(|name| name.as_str().to_string()))
            .collect()
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

    /// Record every `use` nested inside `node`, without treating anything else
    /// in it as an item.
    ///
    /// The scope stays the enclosing one: a `use` in a function body resolves
    /// against the module the function is written in, exactly as the compiler
    /// resolves it.
    fn visit_nested_uses(&mut self, node: Node, scope: &Scope) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "use_declaration" {
                self.visit_use(child, scope);
            } else {
                self.visit_nested_uses(child, scope);
            }
        }
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
        if node
            .named_child(0)
            .is_some_and(|child| child.kind() == "visibility_modifier")
        {
            self.visit_reexport(raw, scope);
        }
        // Take the path prefix before any brace group or glob.
        let head = raw.split(['{', '*']).next().unwrap_or("").trim();
        let mut segments: Vec<&str> = head
            .split("::")
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();

        // Resolve the leading anchor to an absolute module path.
        //
        // `floor` is the shortest path the item-dropping step below may leave.
        // Inside our own crate it is 2 — one segment past the crate root —
        // because `use crate::rules_file;` names a *module*, and popping it
        // would both lose the dependency and leave a crate-to-crate self-edge
        // that `in_cycle` reports as a cycle. For a sibling crate the root is
        // a legitimate target: `use phr::Rule;` depends on `phr`, not on a
        // module named `Rule`.
        let mut floor = 2usize;
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
                floor = 1;
                vec![target.clone()]
            }
            None => return,
        };
        resolved.extend(segments.iter().map(|s| (*s).to_string()));

        // Drop the imported item, leaving its module. A trailing `::` means a
        // brace group or glob followed, so we are already at the module.
        if !head.ends_with("::") && resolved.len() > floor {
            resolved.pop();
        }
        let target = resolved.join("::");
        if target.is_empty() || target == self.self_module {
            return;
        }
        let from = self.self_module.clone();
        self.emit("imports", &[&from, &target]);
    }

    /// Record bounded public re-exports so a test's `use super::*` can resolve
    /// names that the parent module deliberately places in its public scope.
    fn visit_reexport(&mut self, raw: &str, scope: &Scope) {
        let Some((prefix, group)) = raw.split_once("::{") else {
            return;
        };
        let Some(group) = group.strip_suffix('}') else {
            return;
        };
        let mut target = match prefix.split("::").next() {
            Some("crate") => vec![self.unit.id.clone()],
            Some("self") => scope.path.clone(),
            Some("super") => {
                let mut path = scope.path.clone();
                if path.len() <= 1 {
                    return;
                }
                path.pop();
                path
            }
            Some(_) => scope.path.clone(),
            None => return,
        };
        let mut parts = prefix
            .split("::")
            .filter(|part| !part.is_empty() && !matches!(*part, "crate" | "self" | "super"));
        target.extend(parts.by_ref().map(str::to_string));
        let target = target.join("::");
        let from = self.self_module.clone();
        for item in group.split(',') {
            let item = item.split_whitespace().next().unwrap_or("");
            if !item.is_empty() && item != "*" && !item.contains("::") {
                self.emit("reexports", &[&from, &target, item]);
            }
        }
    }
}

/// Extract every base relation from one Rust file.
pub fn extract_rust(
    file_path: &str,
    content: &str,
    watchlist: &[&str],
    unit: &UnitContext,
) -> Extracted {
    extract_rust_at_module(file_path, content, watchlist, unit, None)
}

pub fn extract_rust_at_module(
    file_path: &str,
    content: &str,
    watchlist: &[&str],
    unit: &UnitContext,
    module_override: Option<&str>,
) -> Extracted {
    extract_rust_at_module_with_ownership(
        file_path,
        content,
        watchlist,
        unit,
        module_override,
        &OwnershipConfig::disabled(),
    )
}

/// Extract with the opt-in ownership enrichment applied.
///
/// Ownership is off unless a project's `[ownership.rust]` section turns it on,
/// so the callers that do not pass a config keep exactly their current output
/// (spec §4.2: the enrichment is an opt-in, never an unconditional expansion of
/// the Rust language pack).
pub fn extract_rust_at_module_with_ownership(
    file_path: &str,
    content: &str,
    watchlist: &[&str],
    unit: &UnitContext,
    module_override: Option<&str>,
    ownership: &OwnershipConfig,
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

    let self_module = module_override
        .map(str::to_string)
        .unwrap_or_else(|| module_path(file_path, unit));
    // Include/exclude filter `sync::tracked_files`, never a directory walk of
    // their own (D16); applying them here as well keeps a caller that forgot
    // the filter from indexing an out-of-scope file.
    let collector = (ownership.enabled && ownership.matches(file_path))
        .then(|| FileOwnership::new(file_path, source.as_bytes(), ownership));
    let mut sensor = Sensor {
        file_path,
        source: source.as_bytes(),
        watchlist,
        self_module: self_module.clone(),
        unit,
        out: BTreeSet::new(),
        ownership: collector,
        skipped: 0,
    };
    sensor.emit("file_type", &[file_path, file_type(file_path)]);
    // Links a file to its module, so a rule matching on module-keyed
    // relations (`in_cycle`) can be scoped to the file being edited.
    sensor.emit("declares_module", &[file_path, &self_module]);
    if unit.id != "rust:crate" || !unit.module_base.is_empty() {
        sensor.emit("build_member", &[file_path, &unit.id]);
    }

    let root_scope = Scope {
        path: self_module.split("::").map(str::to_string).collect(),
        impl_type: None,
    };
    sensor.walk(tree.root_node(), &root_scope);

    // Rhai's host API is string-dispatched. Record only bounded literal
    // registrations and literal script paths; dynamic names remain unknown.
    // Restrict regexes to syntax nodes so examples in comments and string
    // fixtures cannot masquerade as executable registrations.
    let mut boundary_nodes = Vec::new();
    let mut pending = vec![tree.root_node()];
    while let Some(node) = pending.pop() {
        if node.kind() == "call_expression"
            && let Some(function) = node.child_by_field_name("function")
            && let Ok(function_name) = function.utf8_text(source.as_bytes())
            && matches!(
                function_name
                    .rsplit(['.', ':'])
                    .find(|part| !part.is_empty()),
                Some("register_fn" | "compile_file" | "compile" | "eval_file")
            )
            && let Ok(text) = node.utf8_text(source.as_bytes())
        {
            boundary_nodes.push(text.to_string());
        } else if node.kind() == "macro_invocation"
            && let Ok(text) = node.utf8_text(source.as_bytes())
            && text.trim_start().starts_with("register_state_proxy!")
        {
            boundary_nodes.push(text.to_string());
        }
        let mut cursor = node.walk();
        pending.extend(node.children(&mut cursor));
    }
    let boundary_source = boundary_nodes.join("\n");
    let registration = regex::Regex::new(r#"register_fn\s*\(\s*"([_A-Za-z][_A-Za-z0-9]*)"\s*,"#)
        .expect("static Rhai registration regex");
    for captures in registration.captures_iter(&boundary_source) {
        let Some(name) = captures.get(1).map(|value| value.as_str()) else {
            continue;
        };
        sensor.emit(
            "exposes",
            &[&self_module, &format!("rhai:callable::{name}")],
        );
    }
    let named_registration = regex::Regex::new(
        r#"register_fn\s*\(\s*"([_A-Za-z][_A-Za-z0-9]*)"\s*,\s*([_A-Za-z][_A-Za-z0-9:]*)\s*\)"#,
    )
    .expect("static named Rhai registration regex");
    for captures in named_registration.captures_iter(&boundary_source) {
        let (Some(name), Some(backing)) = (
            captures.get(1).map(|value| value.as_str()),
            captures.get(2).map(|value| value.as_str()),
        ) else {
            continue;
        };
        sensor.emit(
            "rhai_callable_backing",
            &[
                &format!("rhai:callable::{name}"),
                backing.rsplit("::").next().unwrap_or(backing),
            ],
        );
    }
    let proxy_registration =
        regex::Regex::new(r#"register_state_proxy!\s*\([^,]+,[^,]+,\s*"([_A-Za-z][_A-Za-z0-9]*)""#)
            .expect("static Rhai proxy registration regex");
    for captures in proxy_registration.captures_iter(&boundary_source) {
        if let Some(name) = captures.get(1).map(|value| value.as_str()) {
            sensor.emit(
                "exposes",
                &[&self_module, &format!("rhai:callable::{name}")],
            );
        }
    }
    let proxy_backing = regex::Regex::new(
        r#"register_state_proxy!\s*\([^,]+,[^,]+,\s*"([_A-Za-z][_A-Za-z0-9]*)"\s*,\s*([_A-Za-z][_A-Za-z0-9]*)"#,
    )
    .expect("static Rhai proxy backing regex");
    for captures in proxy_backing.captures_iter(&boundary_source) {
        let (Some(name), Some(backing)) = (
            captures.get(1).map(|value| value.as_str()),
            captures.get(2).map(|value| value.as_str()),
        ) else {
            continue;
        };
        sensor.emit(
            "rhai_callable_backing",
            &[&format!("rhai:callable::{name}"), backing],
        );
    }
    let loader = regex::Regex::new(
        r#"(?:compile_file|compile|eval_file)\s*\(\s*(?:PathBuf::from\s*\(\s*)?"([^"\r\n]+\.rhai)""#,
    )
    .expect("static Rhai loader regex");
    for captures in loader.captures_iter(&boundary_source) {
        if let Some(path) = captures.get(1).map(|value| value.as_str()) {
            sensor.emit("loads_rhai_script", &[&self_module, path]);
        }
    }

    let mut edges: Vec<Edge> = sensor
        .out
        .iter()
        .map(|(p, a)| Edge {
            p: p.clone(),
            a: a.clone(),
            src: file_path.to_string(),
            d: false,
        })
        .collect();
    // `finish` appends the file's `ownership_analysis_status`, so it must run
    // even for a file that produced no sites: "we looked and found nothing" is
    // not the same claim as "we never looked" (spec §3, §10).
    if let Some(collector) = sensor.ownership.take() {
        edges.extend(collector.finish());
    }
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

    #[test]
    fn literal_rhai_registration_and_loader_are_graph_evidence() {
        let out = run(
            "src/bridge.rs",
            r#"
            fn state_attempt_stunning_strike() {}
            fn install(engine: &mut Engine) {
                engine.register_fn("state_attempt_stunning_strike", state_attempt_stunning_strike);
                engine.compile_file("scripts/combat.rhai");
            }
            "#,
        );
        assert_eq!(
            edges_of(&out, "exposes"),
            vec![vec![
                "rust:crate::bridge".to_string(),
                "rhai:callable::state_attempt_stunning_strike".to_string()
            ]]
        );
        assert_eq!(
            edges_of(&out, "loads_rhai_script")[0][1],
            "scripts/combat.rhai"
        );
        assert_eq!(
            edges_of(&out, "rhai_callable_backing"),
            vec![vec![
                "rhai:callable::state_attempt_stunning_strike".to_string(),
                "state_attempt_stunning_strike".to_string()
            ]]
        );
    }

    #[test]
    fn closure_and_proxy_macro_registrations_are_exposed_rhai_names() {
        let out = run(
            "src/bridge.rs",
            r#"
            fn install(engine: &mut Engine) {
                engine.register_fn("ability_modifier", |score: i64| score - 10);
                register_state_proxy!(engine, state, "state_get_hp", get_hp);
            }
            "#,
        );
        let exposed = edges_of(&out, "exposes");
        assert!(
            exposed
                .iter()
                .any(|args| args[1] == "rhai:callable::ability_modifier")
        );
        assert!(
            exposed
                .iter()
                .any(|args| args[1] == "rhai:callable::state_get_hp")
        );
        assert_eq!(
            edges_of(&out, "rhai_callable_backing"),
            vec![vec![
                "rhai:callable::state_get_hp".to_string(),
                "get_hp".to_string()
            ]]
        );
    }

    #[test]
    fn rhai_boundary_examples_in_rust_comments_and_strings_are_not_evidence() {
        let out = run(
            "src/docs.rs",
            r##"
            fn example() {
                // engine.register_fn("comment_only", comment_only);
                let fixture = r#"engine.register_fn("fixture_only", fixture_only);"#;
            }
            "##,
        );
        assert!(edges_of(&out, "exposes").is_empty());
        assert!(edges_of(&out, "rhai_callable_backing").is_empty());
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
            lua_files: Vec::new(),
            cue_files: Vec::new(),
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
            lua_files: Vec::new(),
            cue_files: Vec::new(),
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
            lua_files: Vec::new(),
            cue_files: Vec::new(),
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
    fn a_module_only_use_keeps_the_module_as_the_target() {
        // `use crate::rules_file;` names a module, not an item in one. Popping
        // the last segment here collapses the edge onto the crate root, which
        // both loses the real dependency and manufactures a crate-to-crate
        // self-edge that `in_cycle` then reports as a cycle.
        let out = run("src/a.rs", "use crate::rules_file;");
        assert_eq!(
            edges_of(&out, "imports"),
            vec![vec![
                "rust:crate::a".to_string(),
                "rust:crate::rules_file".to_string()
            ]]
        );
    }

    #[test]
    fn a_use_inside_a_function_body_becomes_an_import_edge() {
        // Function-local `use` is how a dependency needed by exactly one
        // function is conventionally scoped. Skipping the body means a file
        // that visibly imports a module reports no edge to it.
        let out = run(
            "src/a.rs",
            "fn f() { use crate::network::Thing; let _ = Thing; }",
        );
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
            lua_files: Vec::new(),
            cue_files: Vec::new(),
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
        assert_eq!(edges_of(&out, "defines_test").len(), 1);
    }

    #[test]
    fn a_test_with_no_calls_still_has_an_independent_identity() {
        let out = run("tests/net.rs", "#[test]\nfn exists() { assert!(true); }");
        assert_eq!(
            edges_of(&out, "defines_test")[0][1],
            "rust:crate::net::exists"
        );
    }

    #[test]
    fn production_functions_emit_raw_calls_for_whole_graph_resolution() {
        let out = run("src/net.rs", "fn entry() { helper(); } fn helper() {}");
        assert_eq!(edges_of(&out, "calls")[0][1], "helper");
    }

    #[test]
    fn inherent_methods_and_receiver_calls_keep_method_evidence() {
        let out = run(
            "src/state.rs",
            "struct State; impl State { fn apply(&mut self) {} } fn use_it(state: &mut State) { state.apply(); }",
        );
        assert_eq!(edges_of(&out, "defines_method").len(), 1);
        assert!(
            edges_of(&out, "calls")
                .iter()
                .any(|args| args[1] == "@method:apply")
        );
    }

    #[test]
    fn a_test_call_inside_an_assertion_macro_is_coverage_evidence() {
        let out = run(
            "tests/net.rs",
            r#"#[test]
fn t_fire() { assert_eq!(fire(1), 2, "fake_call() is prose"); }"#,
        );
        let tested = edges_of(&out, "tested_by");
        assert!(tested.iter().any(|args| args[0] == "fire"));
        assert!(!tested.iter().any(|args| args[0] == "fake_call"));
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
                .all(|e| e.p != "no_direct_test" && e.p != "in_cycle")
        );
    }
}
