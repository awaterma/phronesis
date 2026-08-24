//! The Python sensor: derives structural edges from one Python source file.
//!
//! Separate from `super::extract` because almost nothing is shared. Rust's
//! module tree is declared (`mod x;`) while Python's is the directory layout;
//! Rust qualifies by compilation target while Python has none; Rust's imports
//! name modules while Python's name modules *or* the objects inside them. The
//! two extractors agree on the edge vocabulary and on `::` as the segment
//! separator, and on nothing else.

use super::model::Edge;
use super::unit::UnitContext;
use crate::syntax::parsed::ParsedFile;
use std::collections::{BTreeMap, BTreeSet};
use tree_sitter::Node;

use super::extract::Extracted;

/// Classification of a Python source file, as the `file_type` relation.
///
/// pytest's own discovery convention decides this: `test_*.py` and `*_test.py`
/// anywhere, plus anything under a `tests/` directory. `conftest.py` is
/// deliberately not a test file — it holds fixtures that production-shaped
/// rules should still see.
fn file_type(file_path: &str) -> &'static str {
    let name = file_path.rsplit('/').next().unwrap_or(file_path);
    if name.starts_with("test_") || name.ends_with("_test.py") {
        return "test";
    }
    if file_path.starts_with("tests/") || file_path.contains("/tests/") {
        return "test";
    }
    "production"
}

/// Module path for a Python file, e.g. `src/pyside/utils.py` ->
/// `python:pyside::pyside::utils`.
///
/// Segments join with `::`, not `.`, even though Python source writes dots:
/// the separator belongs to the graph's data model, and derivation bridges
/// `tested_by` to `defines_fn` by splitting on it (`super::derive`).
pub fn module_path(file_path: &str, unit: &UnitContext) -> String {
    let rel = match unit.module_base.as_str() {
        "" => file_path,
        base => file_path
            .strip_prefix(base)
            .and_then(|rest| rest.strip_prefix('/'))
            .unwrap_or(file_path),
    };
    let trimmed = rel.strip_suffix(".py").unwrap_or(rel);
    let mut segments: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();
    // `__init__.py` is its package, not a module inside it.
    if segments.last() == Some(&"__init__") {
        segments.pop();
    }
    std::iter::once(unit.id.as_str())
        .chain(segments)
        .collect::<Vec<_>>()
        .join("::")
}

/// Text of a node, or empty when it is not valid UTF-8.
fn text<'a>(node: Node, source: &'a [u8]) -> &'a str {
    node.utf8_text(source).unwrap_or("")
}

/// Collector for one file's extraction.
struct Sensor<'a> {
    file_path: &'a str,
    source: &'a [u8],
    /// Namespace prefix, e.g. `python:pyside`.
    id: &'a str,
    /// Top-level import package -> unit id, for distributions in this project
    /// other than our own. Python has no import aliasing at the distribution
    /// level, so the key is the package name exactly as source writes it.
    siblings: &'a BTreeMap<String, String>,
    /// Module segments of this file, e.g. `["pyside", "utils"]`.
    segments: Vec<String>,
    /// Module segments of the package *containing* this file. For
    /// `__init__.py` that is the file's own module, because the file is the
    /// package; for any other module it is the parent directory.
    package: Vec<String>,
    out: BTreeSet<(String, Vec<String>)>,
}

impl Sensor<'_> {
    fn emit(&mut self, p: &str, args: &[&str]) {
        self.out
            .insert((p.to_string(), args.iter().map(|s| s.to_string()).collect()));
    }

    /// Qualified name for `segments` under this unit.
    fn qualify(&self, segments: &[String]) -> String {
        std::iter::once(self.id)
            .chain(segments.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join("::")
    }

    /// The import package this file belongs to, e.g. `pyside`. Absolute
    /// imports are resolved only against this — a distribution can ship
    /// several top-level packages, and guessing which are ours would invent
    /// edges to third-party code.
    fn top_level(&self) -> Option<&str> {
        self.package
            .first()
            .or(self.segments.first())
            .map(String::as_str)
    }

    /// Walk any node, tracking the enclosing class scope.
    fn walk(&mut self, node: Node, scope: &[String]) {
        match node.kind() {
            "function_definition" => {
                self.function(node, scope);
                return;
            }
            "class_definition" => {
                let Some(name) = node.child_by_field_name("name") else {
                    return;
                };
                let mut inner = scope.to_vec();
                inner.push(text(name, self.source).to_string());
                if let Some(body) = node.child_by_field_name("body") {
                    self.walk_children(body, &inner);
                }
                return;
            }
            "import_statement" | "import_from_statement" => {
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

    fn function(&mut self, node: Node, scope: &[String]) {
        let Some(name_node) = node.child_by_field_name("name") else {
            return;
        };
        let name = text(name_node, self.source).to_string();
        let mut path = self.segments.clone();
        path.extend(scope.iter().cloned());
        path.push(name.clone());
        let qualified = self.qualify(&path);

        let Some(body) = node.child_by_field_name("body") else {
            return;
        };

        // pytest collects `test_*` functions, at module level or as methods
        // of a `Test*` class. Those are coverage sources, not subjects — a
        // test that is itself "no_direct_test" is noise, and a helper in a test
        // file is not evidence that anything was verified.
        if name.starts_with("test_") {
            let file_path = self.file_path.to_string();
            self.emit("defines_test", &[&file_path, &qualified]);
            for callee in self.called_names(body) {
                self.emit("tested_by", &[&callee, &qualified]);
            }
            return;
        }

        let file_path = self.file_path.to_string();
        self.emit("defines_fn", &[&file_path, &qualified]);
    }

    /// Bare names of functions invoked in a body. Used only for `tested_by`,
    /// which resolves by short name — see `super::derive::untested`.
    fn called_names(&self, body: Node) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        let mut stack = vec![body];
        while let Some(n) = stack.pop() {
            if n.kind() == "call"
                && let Some(f) = n.child_by_field_name("function")
            {
                let name = match f.kind() {
                    "identifier" => text(f, self.source).to_string(),
                    // `obj.method()` — the method name is what a `defines_fn`
                    // for a method ends with.
                    "attribute" => f
                        .child_by_field_name("attribute")
                        .map(|a| text(a, self.source).to_string())
                        .unwrap_or_default(),
                    _ => String::new(),
                };
                if !name.is_empty() {
                    found.insert(name);
                }
            }
            stack.extend(n.children(&mut n.walk()));
        }
        found
    }

    /// Resolve an `import` / `from … import …` statement to a module edge.
    fn import(&mut self, node: Node) {
        let from = self.qualify(&self.segments.clone());
        let mut targets: Vec<String> = Vec::new();

        if node.kind() == "import_statement" {
            // `import a.b` — absolute only; a dotted name is the module.
            let mut cursor = node.walk();
            for child in node.children(&mut cursor).collect::<Vec<_>>() {
                if child.kind() == "dotted_name"
                    && let Some(target) = self.absolute(child)
                {
                    targets.push(target);
                }
            }
        } else if let Some(module) = node.child_by_field_name("module_name") {
            match module.kind() {
                "relative_import" => {
                    targets.extend(self.relative(node, module).iter().map(|s| self.qualify(s)))
                }
                "dotted_name" => {
                    // `from a.b import c` — the module is `a.b`; `c` may be a
                    // submodule or an object, and either way `a.b` is the
                    // edge we can defend.
                    if let Some(target) = self.absolute(module) {
                        targets.push(target);
                    }
                }
                _ => {}
            }
        }

        for target in targets {
            if target == from {
                continue;
            }
            self.emit("imports", &[&from, &target]);
        }
    }

    /// Fully-qualified target for an absolute dotted name, if it names a
    /// package this project defines — ours or a sibling distribution's.
    /// Anything else is third-party: a node with no definitions hanging off
    /// it is worse than no node.
    fn absolute(&self, node: Node) -> Option<String> {
        let dotted = text(node, self.source);
        let segs: Vec<String> = dotted.split('.').map(str::to_string).collect();
        let head = segs.first()?.as_str();
        if Some(head) == self.top_level() {
            return Some(self.qualify(&segs));
        }
        // A sibling distribution in the same repository. Its packages live
        // under its own unit id, not ours.
        let sibling = self.siblings.get(head)?;
        Some(
            std::iter::once(sibling.as_str())
                .chain(segs.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join("::"),
        )
    }

    /// Segments for each module a relative import names.
    ///
    /// One leading dot is the current package, each further dot climbs one
    /// level. `from . import x` names submodule `x` of that package; `from
    /// .x import y` names module `x` within it.
    fn relative(&self, statement: Node, module: Node) -> Vec<Vec<String>> {
        let Some(base) = self.relative_base(module) else {
            return Vec::new();
        };

        // `from .helpers import Thing` — the dotted name after the dots.
        if let Some(tail) = module
            .children(&mut module.walk())
            .find(|c| c.kind() == "dotted_name")
        {
            let mut segs = base;
            segs.extend(text(tail, self.source).split('.').map(str::to_string));
            return vec![segs];
        }

        // `from . import a, b` — each imported name is a submodule.
        self.relative_names(statement, module, &base)
    }

    /// Package segments a relative import's leading dots resolve to, or
    /// `None` when the dots climb above the top-level package.
    fn relative_base(&self, module: Node) -> Option<Vec<String>> {
        let prefix = module
            .children(&mut module.walk())
            .find(|c| c.kind() == "import_prefix")
            .map(|c| text(c, self.source).chars().filter(|c| *c == '.').count())
            .unwrap_or(1);
        let climb = prefix.saturating_sub(1);
        if climb > self.package.len() {
            return None;
        }
        Some(self.package[..self.package.len() - climb].to_vec())
    }

    /// `from . import a, b` — each imported name is a submodule of `base`.
    fn relative_names(&self, statement: Node, module: Node, base: &[String]) -> Vec<Vec<String>> {
        let mut out = Vec::new();
        for child in statement
            .children(&mut statement.walk())
            .collect::<Vec<_>>()
        {
            let named = match child.kind() {
                "dotted_name" => child,
                "aliased_import" => match child.child_by_field_name("name") {
                    Some(n) => n,
                    None => continue,
                },
                _ => continue,
            };
            // Skip the module_name slot itself; only imported names remain.
            if named.id() == module.id() {
                continue;
            }
            let mut segs = base.to_vec();
            segs.push(text(named, self.source).to_string());
            out.push(segs);
        }
        out
    }
}

/// Extract every base relation from one Python file.
pub fn extract_python(file_path: &str, content: &str, unit: &UnitContext) -> Extracted {
    if !file_path.ends_with(".py") {
        return Extracted::default();
    }
    // A file we cannot parse contributes no edges at all: a partial edge set
    // reads downstream as "this function lost its test" (spec §4.6).
    let Some(parsed) = ParsedFile::parse_python(content) else {
        return Extracted::unparseable();
    };
    let ParsedFile::Python { tree, source } = &parsed else {
        return Extracted::unparseable();
    };
    if tree.root_node().has_error() {
        return Extracted::unparseable();
    }

    let self_module = module_path(file_path, unit);
    let (segments, package) = {
        let segments: Vec<String> = self_module
            .strip_prefix(unit.id.as_str())
            .and_then(|rest| rest.strip_prefix("::"))
            .map(|rest| rest.split("::").map(str::to_string).collect())
            .unwrap_or_default();
        // `__init__.py` *is* its package; any other module sits inside one.
        let is_package = file_path.ends_with("__init__.py");
        let package = if is_package || segments.is_empty() {
            segments.clone()
        } else {
            segments[..segments.len() - 1].to_vec()
        };
        (segments, package)
    };

    let mut sensor = Sensor {
        file_path,
        source: source.as_bytes(),
        id: unit.id.as_str(),
        siblings: &unit.siblings,
        segments,
        package,
        out: BTreeSet::new(),
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
        skipped: 0,
        parse_failed: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn ctx() -> UnitContext {
        UnitContext {
            id: "python:pyside".to_string(),
            module_base: "src".to_string(),
            siblings: BTreeMap::new(),
            ts: crate::graph::unit::TsConfig::default(),
            files: Vec::new(),
            lua_files: Vec::new(),
            cue_files: Vec::new(),
            test_target: false,
        }
    }

    fn run(path: &str, src: &str) -> Extracted {
        extract_python(path, src, &ctx())
    }

    fn edges_of(x: &Extracted, p: &str) -> Vec<Vec<String>> {
        x.edges
            .iter()
            .filter(|e| e.p == p)
            .map(|e| e.a.clone())
            .collect()
    }

    // ─── module naming ──────────────────────────────────────────────

    #[test]
    fn a_module_file_maps_to_its_module_path() {
        assert_eq!(
            module_path("src/pyside/utils.py", &ctx()),
            "python:pyside::pyside::utils"
        );
    }

    #[test]
    fn an_init_file_names_its_package() {
        assert_eq!(
            module_path("src/pyside/__init__.py", &ctx()),
            "python:pyside::pyside"
        );
    }

    #[test]
    fn a_nested_package_becomes_a_nested_module_path() {
        assert_eq!(
            module_path("src/pyside/db/models.py", &ctx()),
            "python:pyside::pyside::db::models"
        );
    }

    // ─── file_type ──────────────────────────────────────────────────

    #[test]
    fn a_plain_module_is_production() {
        assert_eq!(file_type("src/pyside/utils.py"), "production");
    }

    #[test]
    fn a_pytest_named_file_is_a_test() {
        assert_eq!(file_type("src/pyside/test_utils.py"), "test");
        assert_eq!(file_type("src/pyside/utils_test.py"), "test");
    }

    #[test]
    fn a_file_under_a_tests_directory_is_a_test() {
        assert_eq!(file_type("tests/check_things.py"), "test");
    }

    #[test]
    fn conftest_is_not_a_test_file() {
        // It holds fixtures, not tests; production-shaped rules should see it.
        assert_eq!(file_type("src/pyside/conftest.py"), "production");
    }

    // ─── defines_fn ─────────────────────────────────────────────────

    #[test]
    fn a_top_level_def_is_a_defined_function() {
        let out = run("src/pyside/utils.py", "def load():\n    return 1\n");
        assert_eq!(
            edges_of(&out, "defines_fn"),
            vec![vec![
                "src/pyside/utils.py".to_string(),
                "python:pyside::pyside::utils::load".to_string()
            ]]
        );
    }

    #[test]
    fn a_method_is_qualified_by_its_class() {
        let out = run(
            "src/pyside/utils.py",
            "class Loader:\n    def load(self):\n        return 1\n",
        );
        assert_eq!(
            edges_of(&out, "defines_fn")[0][1],
            "python:pyside::pyside::utils::Loader::load"
        );
    }

    #[test]
    fn an_async_def_is_a_defined_function() {
        let out = run("src/pyside/utils.py", "async def load():\n    return 1\n");
        assert_eq!(
            edges_of(&out, "defines_fn")[0][1],
            "python:pyside::pyside::utils::load"
        );
    }

    #[test]
    fn a_decorated_def_is_still_a_defined_function() {
        let out = run("src/pyside/utils.py", "@cache\ndef load():\n    return 1\n");
        assert_eq!(
            edges_of(&out, "defines_fn")[0][1],
            "python:pyside::pyside::utils::load"
        );
    }

    // ─── declares_module ────────────────────────────────────────────

    #[test]
    fn a_file_declares_its_own_module() {
        assert_eq!(
            edges_of(&run("src/pyside/utils.py", "x = 1\n"), "declares_module"),
            vec![vec![
                "src/pyside/utils.py".to_string(),
                "python:pyside::pyside::utils".to_string()
            ]]
        );
    }

    // ─── tested_by ──────────────────────────────────────────────────

    #[test]
    fn a_test_function_is_not_itself_a_defined_function() {
        // Tests are coverage sources, not coverage subjects — otherwise every
        // test is a function with no test of its own.
        let out = run("tests/test_utils.py", "def test_load():\n    load()\n");
        assert!(edges_of(&out, "defines_fn").is_empty());
    }

    #[test]
    fn a_test_function_records_what_it_calls_as_tested_by() {
        let out = run("tests/test_utils.py", "def test_load():\n    load()\n");
        assert_eq!(
            edges_of(&out, "tested_by"),
            vec![vec![
                "load".to_string(),
                "python:pyside::tests::test_utils::test_load".to_string()
            ]]
        );
    }

    #[test]
    fn a_pytest_with_no_calls_still_has_an_independent_identity() {
        let out = run("tests/test_api.py", "def test_exists():\n    assert True\n");
        assert_eq!(edges_of(&out, "defines_test").len(), 1);
    }

    #[test]
    fn a_test_method_in_a_class_still_provides_coverage() {
        // pytest collects `Test*` classes; unittest collects TestCase methods.
        let out = run(
            "tests/test_utils.py",
            "class TestLoad:\n    def test_it(self):\n        load()\n",
        );
        assert_eq!(edges_of(&out, "tested_by")[0][0], "load");
    }

    #[test]
    fn a_helper_in_a_test_file_is_not_a_coverage_source() {
        // Only `test_*` functions are collected by pytest; a helper's calls
        // are not evidence that anything was verified.
        let out = run("tests/test_utils.py", "def build_fixture():\n    load()\n");
        assert!(edges_of(&out, "tested_by").is_empty());
    }

    // ─── imports ────────────────────────────────────────────────────

    #[test]
    fn a_relative_import_resolves_against_the_current_package() {
        let out = run("src/pyside/utils.py", "from . import helpers\n");
        assert_eq!(
            edges_of(&out, "imports"),
            vec![vec![
                "python:pyside::pyside::utils".to_string(),
                "python:pyside::pyside::helpers".to_string()
            ]]
        );
    }

    #[test]
    fn a_relative_import_of_a_name_targets_the_named_module() {
        let out = run("src/pyside/utils.py", "from .helpers import Thing\n");
        assert_eq!(
            edges_of(&out, "imports")[0][1],
            "python:pyside::pyside::helpers"
        );
    }

    #[test]
    fn a_double_dot_import_climbs_to_the_parent_package() {
        let out = run("src/pyside/db/models.py", "from ..helpers import Thing\n");
        assert_eq!(
            edges_of(&out, "imports")[0][1],
            "python:pyside::pyside::helpers"
        );
    }

    #[test]
    fn an_absolute_import_of_our_own_package_resolves() {
        let out = run("src/pyside/utils.py", "from pyside.helpers import Thing\n");
        assert_eq!(
            edges_of(&out, "imports")[0][1],
            "python:pyside::pyside::helpers"
        );
    }

    #[test]
    fn an_absolute_import_of_a_sibling_distribution_resolves() {
        // A monorepo of several distributions: `imports` feeds `in_cycle`, so
        // dropping these makes a cycle *between* deployable units invisible —
        // the worst kind, since it defeats the point of splitting them.
        let unit = UnitContext {
            id: "python:app".to_string(),
            module_base: "libs/app/src".to_string(),
            siblings: BTreeMap::from([("core".to_string(), "python:core".to_string())]),
            ts: crate::graph::unit::TsConfig::default(),
            files: Vec::new(),
            lua_files: Vec::new(),
            cue_files: Vec::new(),
            test_target: false,
        };
        let out = extract_python(
            "libs/app/src/app/__init__.py",
            "from core import helper\n",
            &unit,
        );
        assert_eq!(edges_of(&out, "imports")[0][1], "python:core::core");
    }

    #[test]
    fn a_dotted_import_of_a_sibling_distribution_keeps_its_submodule() {
        let unit = UnitContext {
            id: "python:app".to_string(),
            module_base: "libs/app/src".to_string(),
            siblings: BTreeMap::from([("core".to_string(), "python:core".to_string())]),
            ts: crate::graph::unit::TsConfig::default(),
            files: Vec::new(),
            lua_files: Vec::new(),
            cue_files: Vec::new(),
            test_target: false,
        };
        let out = extract_python(
            "libs/app/src/app/__init__.py",
            "from core.db import Model\n",
            &unit,
        );
        assert_eq!(edges_of(&out, "imports")[0][1], "python:core::core::db");
    }

    #[test]
    fn a_third_party_import_is_not_an_edge() {
        // A node with no definitions hanging off it is worse than no node.
        let out = run("src/pyside/utils.py", "import requests\n");
        assert!(edges_of(&out, "imports").is_empty());
    }

    #[test]
    fn a_stdlib_import_is_not_an_edge() {
        let out = run("src/pyside/utils.py", "from pathlib import Path\n");
        assert!(edges_of(&out, "imports").is_empty());
    }

    // ─── guards ─────────────────────────────────────────────────────

    #[test]
    fn a_non_python_file_yields_nothing() {
        assert_eq!(run("src/a.rs", "fn f() {}"), Extracted::default());
    }

    #[test]
    fn unparseable_source_yields_nothing_rather_than_guesses() {
        let out = run("src/pyside/utils.py", "def (((\n");
        assert!(edges_of(&out, "defines_fn").is_empty());
    }
}
