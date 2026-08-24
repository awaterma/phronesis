//! The Swift sensor: derives structural edges from one Swift source file.
//!
//! Swift has no source-level module tree — a module is a compilation target,
//! and every file in it shares one namespace. The graph needs a stable
//! identity per file anyway, so the module path follows the directory
//! layout, as the Python extractor does. Types qualify the methods they
//! contain; an `extension Foo` qualifies under `Foo` the same way, so a
//! method is named the same whether it lives in the type or an extension.
//!
//! Tests are recognised by both XCTest (`test*` methods) and the Swift
//! Testing `@Test` attribute. Their called names become `tested_by` edges,
//! which derivation resolves by short name (`super::derive::untested`).
//!
//! Every file in a Swift target sees every declaration in it without an
//! `import`, and a test target reaches production through `@testable
//! import`. Both are stated as `imports` edges from the file's module to a
//! whole unit, which `super::derive` reads as unit-wide visibility: one to
//! the file's own unit, and one per `import Foo` whose `Foo` is a target
//! declared in this repository's `Package.swift` (`unit.siblings`, keyed by
//! target name). Imports of anything else — Foundation, XCTest, UIKit, a
//! package dependency — are dropped, since a node with no definitions is
//! worse than no node.
//!
//! Units come from `Package.swift` (`super::unit::parse_package_swift`): a
//! file under `Sources/App` is `swift:App::…`, and a `.testTarget`'s files
//! are all `file_type test`. Without a manifest — an Xcode project, whose
//! `.pbxproj` is not parsed — every file falls back to `swift:project` and
//! the filename/directory heuristic in [`file_type`] decides what is a test.
//!
//! Production bodies emit `calls` (caller identity, bare callee name — the
//! same shape `tested_by` uses, resolved by `super::derive`) and `calls_api`
//! for the calls on [`SWIFT_WATCHLIST`].

use super::extract::Extracted;
use super::model::Edge;
use super::unit::UnitContext;
use crate::syntax::parsed::ParsedFile;
use std::collections::BTreeSet;
use tree_sitter::Node;

/// Classification of a Swift source file, as the `file_type` relation.
///
/// A file in a SwiftPM `.testTarget` is a test whatever it is called.
/// Otherwise Xcode and SwiftPM convention decides: a `*Tests.swift` /
/// `*Test.swift` file, or anything under a directory named `Tests` /
/// `*Tests`.
fn file_type(file_path: &str, unit: &UnitContext) -> &'static str {
    if unit.test_target {
        return "test";
    }
    let name = file_path.rsplit('/').next().unwrap_or(file_path);
    if name.ends_with("Tests.swift") || name.ends_with("Test.swift") {
        return "test";
    }
    let in_test_dir = file_path
        .split('/')
        .take_while(|seg| !seg.ends_with(".swift"))
        .any(|seg| seg == "Tests" || seg.ends_with("Tests"));
    if in_test_dir { "test" } else { "production" }
}

/// Module path for a Swift file, e.g. `App/Overlay/NPCOverlay.swift` ->
/// `swift:project::App::Overlay::NPCOverlay`.
pub fn module_path(file_path: &str, unit: &UnitContext) -> String {
    let rel = match unit.module_base.as_str() {
        "" => file_path,
        base => file_path
            .strip_prefix(base)
            .and_then(|rest| rest.strip_prefix('/'))
            .unwrap_or(file_path),
    };
    let trimmed = rel.strip_suffix(".swift").unwrap_or(rel);
    std::iter::once(unit.id.as_str())
        .chain(trimmed.split('/').filter(|s| !s.is_empty()))
        .collect::<Vec<_>>()
        .join("::")
}

/// Watched Swift APIs, as the `calls_api` relation. Deliberately small and
/// explicit, like the Rust `DEFAULT_WATCHLIST`: each entry is matched
/// syntactically by name, so a broad list would over-match user types.
///
/// - `fatalError` / `preconditionFailure`: unconditional traps at the call
///   site — Swift's `panic!`.
/// - `exit`: terminates the process, bypassing every `defer` and cleanup.
/// - `unsafeBitCast` and the `UnsafePointer` / `UnsafeMutablePointer` /
///   `UnsafeRawPointer` / `UnsafeMutableRawPointer` initializers and static
///   members (`.allocate`): reinterpret or hand-manage memory with no bounds
///   or lifetime checks. A generic static spelling
///   (`UnsafeMutablePointer<Int>.allocate`) is not recognised — the grammar
///   parses it as comparisons.
/// - `Thread.sleep` and `wait` (`DispatchSemaphore` / `DispatchGroup`):
///   block the calling thread, a deadlock when that thread is main or an
///   actor executor. `wait` is matched by method name only, so a user-defined
///   `wait()` also matches — the same caveat Rust's `expect` carries.
///
/// A `Type.method` entry matches only the static form spelled that way;
/// a bare entry matches a free call or a method call of that name. `try!`
/// is an operator, not a call, and is left to the `audit-swift-*` rules.
pub const SWIFT_WATCHLIST: &[&str] = &[
    "fatalError",
    "preconditionFailure",
    "exit",
    "unsafeBitCast",
    "UnsafePointer",
    "UnsafeMutablePointer",
    "UnsafeRawPointer",
    "UnsafeMutableRawPointer",
    "Thread.sleep",
    "wait",
];

fn text<'a>(node: Node, source: &'a [u8]) -> &'a str {
    node.utf8_text(source).unwrap_or("")
}

struct Sensor<'a> {
    file_path: &'a str,
    source: &'a [u8],
    module: &'a str,
    unit: &'a UnitContext,
    out: BTreeSet<(String, Vec<String>)>,
}

impl Sensor<'_> {
    fn emit(&mut self, p: &str, args: &[&str]) {
        self.out
            .insert((p.to_string(), args.iter().map(|s| s.to_string()).collect()));
    }

    /// Declared name of a `class_declaration` (class, struct, enum, actor,
    /// or extension — tree-sitter-swift uses one kind for all of them). An
    /// extension names its target through a `user_type`.
    fn type_name(&self, node: Node) -> Option<String> {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "type_identifier" => return Some(text(child, self.source).to_string()),
                "user_type" => {
                    let mut inner = child.walk();
                    if let Some(id) = child
                        .children(&mut inner)
                        .find(|c| c.kind() == "type_identifier")
                    {
                        return Some(text(id, self.source).to_string());
                    }
                }
                _ => {}
            }
        }
        None
    }

    fn function_name(&self, node: Node) -> Option<String> {
        node.child_by_field_name("name")
            .or_else(|| {
                let mut cursor = node.walk();
                node.children(&mut cursor)
                    .find(|c| c.kind() == "simple_identifier")
            })
            .map(|n| text(n, self.source).to_string())
            .filter(|n| !n.is_empty())
    }

    fn has_test_attribute(&self, node: Node) -> bool {
        let mut stack = vec![node];
        while let Some(n) = stack.pop() {
            if n.kind() == "attribute" && text(n, self.source).trim_start_matches('@') == "Test" {
                return true;
            }
            if matches!(
                n.kind(),
                "modifiers" | "attribute" | "user_type" | "function_declaration"
            ) {
                let mut cursor = n.walk();
                stack.extend(
                    n.children(&mut cursor)
                        .filter(|c| c.kind() != "function_body"),
                );
            }
        }
        false
    }

    /// `import Foo` / `@testable import Foo.Bar`: the module is the first
    /// path component. Only a sibling target of this project yields an edge.
    fn import(&mut self, node: Node) {
        let mut cursor = node.walk();
        let Some(id) = node
            .children(&mut cursor)
            .find(|c| c.kind() == "identifier")
        else {
            return;
        };
        let module = text(id, self.source)
            .split('.')
            .next()
            .unwrap_or_default()
            .trim();
        if let Some(target) = self.unit.siblings.get(module) {
            let target = target.clone();
            self.emit("imports", &[self.module, &target]);
        }
    }

    fn walk(&mut self, node: Node, scope: &[String]) {
        match node.kind() {
            "import_declaration" => {
                self.import(node);
                return;
            }
            "function_declaration" => {
                self.function(node, scope);
                return;
            }
            "class_declaration" | "protocol_declaration" => {
                let inner = {
                    let mut inner = scope.to_vec();
                    if let Some(name) = self.type_name(node) {
                        inner.push(name);
                    }
                    inner
                };
                for child in node.children(&mut node.walk()).collect::<Vec<_>>() {
                    self.walk(child, &inner);
                }
                return;
            }
            _ => {}
        }
        for child in node.children(&mut node.walk()).collect::<Vec<_>>() {
            self.walk(child, scope);
        }
    }

    fn function(&mut self, node: Node, scope: &[String]) {
        let Some(name) = self.function_name(node) else {
            return;
        };
        let qualified = std::iter::once(self.module)
            .chain(scope.iter().map(String::as_str))
            .chain(std::iter::once(name.as_str()))
            .collect::<Vec<_>>()
            .join("::");
        let file_path = self.file_path.to_string();

        let Some(body) = node
            .children(&mut node.walk())
            .find(|c| c.kind() == "function_body")
        else {
            // Protocol requirements have no body: a declaration, not a definition.
            return;
        };

        let is_xctest = name.starts_with("test") && file_type(self.file_path, self.unit) == "test";
        if is_xctest || self.has_test_attribute(node) {
            self.emit("defines_test", &[&file_path, &qualified]);
            for callee in self.called_names(body) {
                self.emit("tested_by", &[&callee, &qualified]);
            }
            return;
        }
        self.emit("defines_fn", &[&file_path, &qualified]);
        for callee in self.called_names(body) {
            self.emit("calls", &[&qualified, &callee]);
        }
        for api in self.watched_calls(body) {
            self.emit("calls_api", &[&qualified, &api]);
        }
    }

    /// Bare names of functions invoked in a body, for `tested_by` and `calls`.
    fn called_names(&self, body: Node) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        let mut stack = vec![body];
        while let Some(n) = stack.pop() {
            if n.kind() == "call_expression"
                && let Some(callee) = n.named_child(0)
            {
                let name = match callee.kind() {
                    "simple_identifier" => text(callee, self.source).to_string(),
                    // `obj.method()` — the last navigation suffix is the method.
                    "navigation_expression" => callee
                        .children(&mut callee.walk())
                        .filter(|c| c.kind() == "navigation_suffix")
                        .last()
                        .and_then(|s| s.named_child(0))
                        .map(|id| text(id, self.source).to_string())
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

    /// Watchlist entries invoked in a body, for `calls_api`. The static
    /// `Type.method` spelling is tried first so `Thread.sleep` can be told
    /// from an arbitrary `sleep`, then the bare callee name.
    fn watched_calls(&self, body: Node) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        let mut stack = vec![body];
        while let Some(n) = stack.pop() {
            let candidates = match n.kind() {
                "call_expression" => n
                    .named_child(0)
                    .map(|callee| self.callee_spellings(callee))
                    .unwrap_or_default(),
                "constructor_expression" => self.constructed_type_name(n).into_iter().collect(),
                _ => Vec::new(),
            };
            for candidate in candidates {
                if SWIFT_WATCHLIST.contains(&candidate.as_str()) {
                    found.insert(candidate);
                }
            }
            stack.extend(n.children(&mut n.walk()));
        }
        found
    }

    /// How a callee may be spelled against the watchlist: for a free call
    /// the name; for `Receiver.method` the `Receiver.method` form, the bare
    /// method, and — when the receiver is a plain identifier — the receiver
    /// itself, so a static member on a watched type
    /// (`UnsafeMutablePointer.allocate(capacity:)`) matches the type. An
    /// initializer call (`UnsafePointer<Int>(q)`) is a
    /// `constructor_expression`, whose type identifier is the name.
    fn callee_spellings(&self, callee: Node) -> Vec<String> {
        let mut out = Vec::new();
        match callee.kind() {
            "simple_identifier" => out.push(text(callee, self.source).to_string()),
            "navigation_expression" => {
                let mut cursor = callee.walk();
                let children = callee.children(&mut cursor).collect::<Vec<_>>();
                let receiver = children
                    .first()
                    .filter(|c| c.kind() == "simple_identifier")
                    .map(|c| text(*c, self.source));
                let method = children
                    .iter()
                    .rfind(|c| c.kind() == "navigation_suffix")
                    .and_then(|s| s.named_child(0))
                    .map(|id| text(id, self.source));
                if let (Some(recv), Some(method)) = (receiver, method) {
                    out.push(format!("{recv}.{method}"));
                    out.push(recv.to_string());
                }
                out.extend(method.map(str::to_string));
            }
            _ => {}
        }
        out
    }

    /// Type name of a `constructor_expression` (`Foo<T>(...)` is `Foo`).
    fn constructed_type_name(&self, node: Node) -> Option<String> {
        let ty = node.child_by_field_name("constructed_type")?;
        let mut cursor = ty.walk();
        ty.children(&mut cursor)
            .find(|c| c.kind() == "type_identifier")
            .map(|id| text(id, self.source).to_string())
    }
}

/// Extract every base relation from one Swift file.
pub fn extract_swift(file_path: &str, content: &str, unit: &UnitContext) -> Extracted {
    if !file_path.ends_with(".swift") {
        return Extracted::default();
    }
    let Some(parsed) = ParsedFile::parse_swift(content) else {
        return Extracted::unparseable();
    };
    let ParsedFile::Swift { tree, source } = &parsed else {
        return Extracted::unparseable();
    };
    if tree.root_node().has_error() {
        return Extracted::unparseable();
    }

    let module = module_path(file_path, unit);
    let mut sensor = Sensor {
        file_path,
        source: source.as_bytes(),
        module: &module,
        unit,
        out: BTreeSet::new(),
    };
    sensor.emit("file_type", &[file_path, file_type(file_path, unit)]);
    sensor.emit("declares_module", &[file_path, &module]);
    sensor.emit("imports", &[&module, unit.id.as_str()]);
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

    fn run(path: &str, src: &str) -> Vec<(String, Vec<String>)> {
        let unit = UnitContext::unnamed_for(super::super::unit::LANG_SWIFT);
        extract_swift(path, src, &unit)
            .edges
            .into_iter()
            .map(|e| (e.p, e.a))
            .collect()
    }

    fn has(edges: &[(String, Vec<String>)], p: &str, a: &[&str]) -> bool {
        edges
            .iter()
            .any(|(ep, ea)| ep == p && ea.iter().map(String::as_str).eq(a.iter().copied()))
    }

    #[test]
    fn file_type_follows_xcode_conventions() {
        let unit = UnitContext::unnamed_for(super::super::unit::LANG_SWIFT);
        assert_eq!(file_type("App/Foo.swift", &unit), "production");
        assert_eq!(file_type("AppTests/FooTests.swift", &unit), "test");
        assert_eq!(file_type("Tests/Unit/Helpers.swift", &unit), "test");
        assert_eq!(file_type("App/Tester.swift", &unit), "production");
    }

    #[test]
    fn methods_qualify_under_type_and_extension() {
        let e = run(
            "App/S.swift",
            "struct S { func m() {} }\nextension S { func e() {} }\nfunc free() {}\n",
        );
        assert!(
            has(
                &e,
                "defines_fn",
                &["App/S.swift", "swift:project::App::S::S::m"]
            ),
            "{e:?}"
        );
        assert!(
            has(
                &e,
                "defines_fn",
                &["App/S.swift", "swift:project::App::S::S::e"]
            ),
            "{e:?}"
        );
        assert!(
            has(
                &e,
                "defines_fn",
                &["App/S.swift", "swift:project::App::S::free"]
            ),
            "{e:?}"
        );
    }

    #[test]
    fn every_file_imports_its_whole_unit() {
        let e = run("App/S.swift", "func f() {}\n");
        assert!(
            has(&e, "imports", &["swift:project::App::S", "swift:project"]),
            "{e:?}"
        );
    }

    #[test]
    fn protocol_requirements_are_not_definitions() {
        let e = run("App/P.swift", "protocol P { func required() }\n");
        assert!(!e.iter().any(|(p, _)| p == "defines_fn"), "{e:?}");
    }

    #[test]
    fn swift_testing_attribute_marks_a_test() {
        let e = run(
            "App/Thing.swift",
            "import Testing\n@Test func checks() { thing.work() }\n",
        );
        assert!(
            has(
                &e,
                "defines_test",
                &["App/Thing.swift", "swift:project::App::Thing::checks"]
            ),
            "{e:?}"
        );
        assert!(
            has(
                &e,
                "tested_by",
                &["work", "swift:project::App::Thing::checks"]
            ),
            "{e:?}"
        );
    }

    #[test]
    fn production_functions_emit_calls_to_free_and_method_callees() {
        let e = run(
            "App/C.swift",
            "struct C {\n    func run() { helper(); self.other.finish(); sleep(1) }\n}\n",
        );
        let caller = "swift:project::App::C::C::run";
        for callee in ["helper", "finish", "sleep"] {
            assert!(has(&e, "calls", &[caller, callee]), "{callee}: {e:?}");
        }
    }

    #[test]
    fn test_functions_do_not_emit_calls() {
        let e = run(
            "AppTests/CTests.swift",
            "final class CTests: XCTestCase { func testIt() { helper() } }\n",
        );
        assert!(!e.iter().any(|(p, _)| p == "calls"), "{e:?}");
        assert!(!e.iter().any(|(p, _)| p == "calls_api"), "{e:?}");
    }

    #[test]
    fn watched_apis_emit_calls_api_by_free_static_and_method_shape() {
        let e = run(
            "App/R.swift",
            "func risky(p: UnsafeRawPointer) {\n\
             \x20   fatalError(\"x\")\n\
             \x20   let q = UnsafeMutablePointer.allocate(capacity: 1)\n\
             \x20   let r = UnsafePointer<Int>(q)\n\
             \x20   let n = unsafeBitCast(r, to: Int.self)\n\
             \x20   Thread.sleep(forTimeInterval: 1)\n\
             \x20   semaphore.wait()\n\
             \x20   exit(1)\n\
             \x20   preconditionFailure()\n\
             }\n",
        );
        let f = "swift:project::App::R::risky";
        for api in [
            "fatalError",
            "preconditionFailure",
            "unsafeBitCast",
            "UnsafeMutablePointer",
            "UnsafePointer",
            "Thread.sleep",
            "wait",
            "exit",
        ] {
            assert!(has(&e, "calls_api", &[f, api]), "{api}: {e:?}");
        }
    }

    #[test]
    fn unwatched_calls_emit_no_calls_api() {
        let e = run("App/S.swift", "func safe() { print(\"ok\"); helper() }\n");
        assert!(!e.iter().any(|(p, _)| p == "calls_api"), "{e:?}");
    }

    fn target_ctx(
        id: &str,
        base: &str,
        test_target: bool,
        siblings: &[(&str, &str)],
    ) -> UnitContext {
        let mut unit = UnitContext::unnamed_for(super::super::unit::LANG_SWIFT);
        unit.id = id.to_string();
        unit.module_base = base.to_string();
        unit.test_target = test_target;
        unit.siblings = siblings
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        unit
    }

    fn run_in(path: &str, src: &str, unit: &UnitContext) -> Vec<(String, Vec<String>)> {
        extract_swift(path, src, unit)
            .edges
            .into_iter()
            .map(|e| (e.p, e.a))
            .collect()
    }

    #[test]
    fn a_swiftpm_target_roots_module_paths_at_the_target_directory() {
        let unit = target_ctx("swift:App", "Sources/App", false, &[]);
        assert_eq!(
            module_path("Sources/App/Overlay/NPCOverlay.swift", &unit),
            "swift:App::Overlay::NPCOverlay"
        );
    }

    #[test]
    fn a_test_target_types_every_file_as_test() {
        let unit = target_ctx("swift:CoreSpecs", "Specs/Core", true, &[]);
        let e = run_in("Specs/Core/Helpers.swift", "func h() {}\n", &unit);
        assert!(
            has(&e, "file_type", &["Specs/Core/Helpers.swift", "test"]),
            "{e:?}"
        );
        let prod = target_ctx("swift:App", "Sources/App", false, &[]);
        let e = run_in("Sources/App/Helpers.swift", "func h() {}\n", &prod);
        assert!(
            has(
                &e,
                "file_type",
                &["Sources/App/Helpers.swift", "production"]
            ),
            "{e:?}"
        );
    }

    #[test]
    fn an_xctest_method_in_a_test_target_is_a_test_whatever_the_file_is_called() {
        let unit = target_ctx("swift:CoreSpecs", "Specs/Core", true, &[]);
        let e = run_in(
            "Specs/Core/Helpers.swift",
            "final class H: XCTestCase { func testIt() { work() } }\n",
            &unit,
        );
        assert!(
            has(
                &e,
                "tested_by",
                &["work", "swift:CoreSpecs::Helpers::H::testIt"]
            ),
            "{e:?}"
        );
    }

    #[test]
    fn imports_of_in_repo_targets_become_unit_imports_and_frameworks_do_not() {
        let unit = target_ctx(
            "swift:AppTests",
            "Tests/AppTests",
            true,
            &[("App", "swift:App"), ("Core", "swift:Core")],
        );
        let e = run_in(
            "Tests/AppTests/T.swift",
            "import XCTest\nimport Foundation\n@testable import App\nimport Core.Sub\n// import Nope\n",
            &unit,
        );
        let m = "swift:AppTests::T";
        assert!(has(&e, "imports", &[m, "swift:AppTests"]), "{e:?}");
        assert!(has(&e, "imports", &[m, "swift:App"]), "{e:?}");
        assert!(has(&e, "imports", &[m, "swift:Core"]), "{e:?}");
        let targets: Vec<&str> = e
            .iter()
            .filter(|(p, _)| p == "imports")
            .map(|(_, a)| a[1].as_str())
            .collect();
        assert_eq!(targets.len(), 3, "{targets:?}");
    }

    #[test]
    fn unparseable_source_contributes_nothing() {
        let r = extract_swift(
            "App/Bad.swift",
            "func {{{",
            &UnitContext::unnamed_for("swift"),
        );
        assert!(r.parse_failed);
        assert!(r.edges.is_empty());
    }
}
