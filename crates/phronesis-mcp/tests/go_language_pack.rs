//! The Go starter pack, exercised entirely through the public language-pack
//! contract (`language_packs::{builtin, extract, records}`) — no bespoke Go
//! extractor. Run with `--features go-grammar` (the dependency the plan's
//! shared grammar-MSRV probe already verified; see
//! `artifacts/grammar-msrv.log`).

#![cfg(all(feature = "go-grammar", feature = "rhai"))]

use phronesis_mcp::language_packs::{
    Extraction, FileContext, PackRecord, ReferenceMode, builtin_packs, extract,
};

fn go_pack() -> phronesis_mcp::language_packs::ValidatedPack {
    let mut packs = builtin_packs().expect("go builtin pack compiles");
    assert_eq!(packs.len(), 1);
    packs.remove(0)
}

fn ctx(file_rel: &str) -> FileContext {
    FileContext {
        file_rel: file_rel.to_string(),
        context_files: Vec::new(),
    }
}

fn ctx_with_gomod(file_rel: &str, module_dir_rel: &str, module: &str) -> FileContext {
    FileContext {
        file_rel: file_rel.to_string(),
        context_files: vec![phronesis_mcp::language_packs::ContextFile {
            name: "go.mod".to_string(),
            file_rel: if module_dir_rel == "." {
                "go.mod".to_string()
            } else {
                format!("{module_dir_rel}/go.mod")
            },
            directory_rel: module_dir_rel.to_string(),
            content: format!("module {module}\n\ngo 1.23\n"),
        }],
    }
}

fn declarations(extraction: &Extraction) -> Vec<&PackRecord> {
    extraction
        .records
        .iter()
        .filter(|r| matches!(r, PackRecord::Declaration { .. }))
        .collect()
}

fn decl_named<'a>(extraction: &'a Extraction, name: &str) -> &'a PackRecord {
    extraction
        .records
        .iter()
        .find(|r| matches!(r, PackRecord::Declaration { name: n, .. } if n == name))
        .unwrap_or_else(|| panic!("no declaration named {name}"))
}

// ── free functions, methods, receivers ──────────────────────────────────

#[test]
fn free_function_is_declared_in_its_package_scope() {
    let pack = go_pack();
    let source = "package main\n\nfunc greet() string {\n\treturn \"hi\"\n}\n";
    let extraction = extract(&pack, &ctx("greet.go"), source).unwrap();
    assert!(
        extraction.diagnostics.is_empty(),
        "{:?}",
        extraction.diagnostics
    );
    let decl = decl_named(&extraction, "greet");
    match decl {
        PackRecord::Declaration {
            scope,
            receiver,
            test,
            command,
            ..
        } => {
            assert_eq!(scope, "local/.::main");
            assert_eq!(*receiver, None);
            assert!(!test);
            assert!(command.is_none());
        }
        _ => unreachable!(),
    }
}

#[test]
fn pointer_value_and_generic_receivers_are_normalized_to_their_base_type() {
    let pack = go_pack();
    let source = "\
package main

type T struct{}
type List[E any] struct{}

func (t T) ByValue() {}
func (t *T) ByPointer() {}
func (l *List[E]) Generic() {}
";
    let extraction = extract(&pack, &ctx("recv.go"), source).unwrap();
    assert!(
        extraction.diagnostics.is_empty(),
        "{:?}",
        extraction.diagnostics
    );
    let receiver_of = |name: &str| match decl_named(&extraction, name) {
        PackRecord::Declaration { receiver, .. } => receiver.clone(),
        _ => unreachable!(),
    };
    assert_eq!(receiver_of("ByValue"), Some("T".to_string()));
    assert_eq!(receiver_of("ByPointer"), Some("T".to_string()));
    assert_eq!(receiver_of("Generic"), Some("List".to_string()));
}

#[test]
fn site_identity_preserves_duplicate_function_names_across_build_tag_variants() {
    // Two files in the same package, same name — e.g. `foo_linux.go` /
    // `foo_darwin.go` under build tags. The pack does not collapse these:
    // each file's declaration keeps its own (file, start_byte) site.
    let pack = go_pack();
    let source = "package pkg\n\nfunc Foo() {}\n";
    let a = extract(&pack, &ctx("foo_linux.go"), source).unwrap();
    let b = extract(&pack, &ctx("foo_darwin.go"), source).unwrap();
    assert_eq!(declarations(&a).len(), 1);
    assert_eq!(declarations(&b).len(), 1);
    // Same qualified scope (same package), but each file's own extraction —
    // ambiguity resolution across the two sites is left to the graph layer.
    let scope_of = |e: &Extraction| match &e.records[0] {
        PackRecord::Declaration { scope, .. } => scope.clone(),
        _ => unreachable!(),
    };
    assert_eq!(scope_of(&a), scope_of(&b));
}

// ── imports ──────────────────────────────────────────────────────────────

#[test]
fn grouped_imports_with_alias_blank_and_dot_are_all_recorded() {
    let pack = go_pack();
    let source = "\
package main

import (
\t\"fmt\"
\tstr \"strings\"
\t_ \"unsafe\"
\t. \"errors\"
)

func use() { _ = fmt.Sprintf; _ = str.ToUpper }
";
    let extraction = extract(&pack, &ctx("imp.go"), source).unwrap();
    assert!(
        extraction.diagnostics.is_empty(),
        "{:?}",
        extraction.diagnostics
    );
    let imports: Vec<&PackRecord> = extraction
        .records
        .iter()
        .filter(|r| matches!(r, PackRecord::Import { .. }))
        .collect();
    let targets: Vec<&str> = imports
        .iter()
        .map(|r| match r {
            PackRecord::Import { target, .. } => target.as_str(),
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(targets, ["fmt", "strings", "unsafe", "errors"]);
    let aliases: Vec<Option<&str>> = imports
        .iter()
        .map(|r| match r {
            PackRecord::Import { alias, .. } => alias.as_deref(),
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(aliases, [None, Some("str"), Some("_"), Some(".")]);
}

#[test]
fn dot_import_emits_the_advisory_fact_and_only_from_a_real_capture() {
    let pack = go_pack();
    let with_dot = "package main\n\nimport . \"errors\"\n\nfunc f() {}\n";
    let extraction = extract(&pack, &ctx("dot.go"), with_dot).unwrap();
    let facts: Vec<&PackRecord> = extraction
        .records
        .iter()
        .filter(|r| matches!(r, PackRecord::Fact { .. }))
        .collect();
    assert_eq!(facts.len(), 1);
    match facts[0] {
        PackRecord::Fact { name, args, .. } => {
            assert_eq!(name, "dot_import");
            assert_eq!(args, &vec!["errors".to_string()]);
        }
        _ => unreachable!(),
    }

    // Named, blank, and lookalike text (comment/string) never trigger it.
    let without_dot = "\
package main

// import . \"errors\" (comment lookalike)
var s = \"import . \\\"errors\\\"\"

import (
\t\"fmt\"
\t_ \"unsafe\"
)

func f() { _ = fmt.Sprintf; _ = s }
";
    let extraction = extract(&pack, &ctx("no_dot.go"), without_dot).unwrap();
    assert!(
        extraction
            .records
            .iter()
            .all(|r| !matches!(r, PackRecord::Fact { .. })),
        "{:?}",
        extraction.records
    );
}

// ── calls: same-scope candidates, selector refs, shadow suppression ─────

#[test]
fn unqualified_call_is_a_same_scope_candidate_and_selector_calls_are_unresolved() {
    let pack = go_pack();
    let source = "\
package main

func helper() {}

func caller() {
\thelper()
\tfmt.Println(\"x\")
}
";
    let extraction = extract(&pack, &ctx("calls.go"), source).unwrap();
    let refs: Vec<&PackRecord> = extraction
        .records
        .iter()
        .filter(|r| matches!(r, PackRecord::Reference { .. }))
        .collect();
    assert_eq!(refs.len(), 2, "{:?}", refs);
    let by_target = |t: &str| {
        refs.iter()
            .find(|r| matches!(r, PackRecord::Reference { target, .. } if target == t))
            .unwrap_or_else(|| panic!("no reference to {t}"))
    };
    match by_target("helper") {
        PackRecord::Reference { resolution, .. } => {
            assert_eq!(*resolution, ReferenceMode::SameScope);
        }
        _ => unreachable!(),
    }
    match by_target("fmt.Println") {
        PackRecord::Reference { resolution, .. } => {
            assert_eq!(*resolution, ReferenceMode::Unresolved);
        }
        _ => unreachable!(),
    }
}

#[test]
fn local_parameter_and_dot_import_shadowing_suppress_same_scope_resolution() {
    let pack = go_pack();

    // A local variable shadows the call.
    let local_shadow = "\
package main

func helper() {}

func caller() {
\thelper := func() {}
\thelper()
}
";
    let extraction = extract(&pack, &ctx("local.go"), local_shadow).unwrap();
    let resolved: Vec<&PackRecord> = extraction
        .records
        .iter()
        .filter(|r| {
            matches!(r, PackRecord::Reference { target, resolution, .. }
                if target == "helper" && *resolution == ReferenceMode::SameScope)
        })
        .collect();
    assert!(
        resolved.is_empty(),
        "a call through the local `helper` must not resolve to the package function: {resolved:?}"
    );

    // A parameter shadows the call.
    let param_shadow = "\
package main

func helper() {}

func caller(helper func()) {
\thelper()
}
";
    let extraction = extract(&pack, &ctx("param.go"), param_shadow).unwrap();
    let refs: Vec<&PackRecord> = extraction
        .records
        .iter()
        .filter(|r| matches!(r, PackRecord::Reference { .. }))
        .collect();
    assert_eq!(refs.len(), 1);
    match refs[0] {
        PackRecord::Reference { resolution, .. } => {
            assert_eq!(*resolution, ReferenceMode::Unresolved);
        }
        _ => unreachable!(),
    }

    // A dot import makes every unqualified call in the file ambiguous.
    let dot_shadow = "\
package main

import . \"fmt\"

func helper() {}

func caller() {
\thelper()
}
";
    let extraction = extract(&pack, &ctx("dotshadow.go"), dot_shadow).unwrap();
    let refs: Vec<&PackRecord> = extraction
        .records
        .iter()
        .filter(|r| matches!(r, PackRecord::Reference { .. }))
        .collect();
    assert_eq!(refs.len(), 1);
    match refs[0] {
        PackRecord::Reference { resolution, .. } => {
            assert_eq!(*resolution, ReferenceMode::Unresolved);
        }
        _ => unreachable!(),
    }
}

// ── standard `testing.T` recognition ────────────────────────────────────

#[test]
fn standard_test_function_is_recognized_with_a_safe_rendered_command() {
    let pack = go_pack();
    let source = "\
package pkg

import \"testing\"

func TestAdd(t *testing.T) {
\tif 1+1 != 2 {
\t\tt.Fatal(\"math is broken\")
\t}
}
";
    let extraction = extract(
        &pack,
        &ctx_with_gomod("sub/pkg_test.go", ".", "example.com/demo"),
        source,
    )
    .unwrap();
    let decl = decl_named(&extraction, "TestAdd");
    match decl {
        PackRecord::Declaration { test, command, .. } => {
            assert!(*test);
            let command = command.as_ref().expect("rendered test command");
            assert_eq!(command.program, "go");
            assert_eq!(command.args, vec!["test", "./sub", "-run", "^TestAdd$"]);
            assert_eq!(command.cwd, ".");
        }
        _ => unreachable!(),
    }
}

#[test]
fn aliased_testing_import_is_still_recognized() {
    let pack = go_pack();
    let source = "\
package pkg

import gotesting \"testing\"

func TestAliased(t *gotesting.T) {}
";
    let extraction = extract(&pack, &ctx("pkg_test.go"), source).unwrap();
    match decl_named(&extraction, "TestAliased") {
        PackRecord::Declaration { test, .. } => assert!(*test),
        _ => unreachable!(),
    }
}

#[test]
fn lowercase_after_test_is_rejected_like_real_go_vet() {
    let pack = go_pack();
    // `go vet`'s own rule: `TestHelper` is a test, `Testhelper` is not.
    let source = "\
package pkg

import \"testing\"

func TestHelper(t *testing.T) {}
func Testhelper(t *testing.T) {}
";
    let extraction = extract(&pack, &ctx("vet_test.go"), source).unwrap();
    match decl_named(&extraction, "TestHelper") {
        PackRecord::Declaration { test, .. } => assert!(*test, "TestHelper must be recognized"),
        _ => unreachable!(),
    }
    match decl_named(&extraction, "Testhelper") {
        PackRecord::Declaration { test, .. } => {
            assert!(
                !*test,
                "Testhelper (lowercase after Test) must not be recognized, matching `go vet`"
            );
        }
        _ => unreachable!(),
    }
}

#[test]
fn go_test_signatures_follow_real_go_runner() {
    let pack = go_pack();
    let source = "package pkg\nimport \"testing\"\n\nfunc TestUnnamed(*testing.T) {}\nfunc TestBlank(_ *testing.T) {}\nfunc TestReturns(t *testing.T) int { return 0 }\nfunc TestGeneric[T any](t *testing.T) {}\nfunc Testé(t *testing.T) {}\nfunc Testƛ(t *testing.T) {}\n";
    let extraction = extract(&pack, &ctx("signature_test.go"), source).unwrap();
    for name in ["TestUnnamed", "TestBlank"] {
        match decl_named(&extraction, name) {
            PackRecord::Declaration { test, command, .. } => {
                assert!(*test, "{name} is valid in real Go");
                assert!(command.is_some());
            }
            _ => unreachable!(),
        }
    }
    for name in ["TestReturns", "TestGeneric", "Testé", "Testƛ"] {
        match decl_named(&extraction, name) {
            PackRecord::Declaration { test, command, .. } => {
                assert!(!*test, "{name} is invalid in real Go");
                assert!(command.is_none());
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn test_recognition_requires_a_test_go_file() {
    let pack = go_pack();
    let source = "package pkg\n\nimport \"testing\"\n\nfunc TestAdd(t *testing.T) {}\n";
    // Same source, but not a `_test.go` file: never classified as a test.
    let extraction = extract(&pack, &ctx("not_a_test_file.go"), source).unwrap();
    match decl_named(&extraction, "TestAdd") {
        PackRecord::Declaration { test, .. } => assert!(!*test),
        _ => unreachable!(),
    }
}

#[test]
fn methods_are_never_recognized_as_standard_tests() {
    let pack = go_pack();
    let source = "\
package pkg

import \"testing\"

type Suite struct{}

func (s *Suite) TestAdd(t *testing.T) {}
";
    let extraction = extract(&pack, &ctx("suite_test.go"), source).unwrap();
    match decl_named(&extraction, "TestAdd") {
        PackRecord::Declaration { test, .. } => assert!(!*test),
        _ => unreachable!(),
    }
}

// ── nested modules, external test packages ───────────────────────────────

#[test]
fn nested_go_mod_scopes_a_file_to_its_own_module() {
    let pack = go_pack();
    let source = "package inner\n\nfunc F() {}\n";
    let extraction = extract(
        &pack,
        &ctx_with_gomod("vendor/sub/inner.go", "vendor/sub", "example.com/inner"),
        source,
    )
    .unwrap();
    match decl_named(&extraction, "F") {
        PackRecord::Declaration { scope, .. } => {
            assert_eq!(scope, "example.com/inner::inner");
        }
        _ => unreachable!(),
    }
}

#[test]
fn external_test_package_gets_a_distinct_scope_from_the_main_package() {
    let pack = go_pack();
    let main_src = "package pkg\n\nfunc F() {}\n";
    let test_src = "package pkg_test\n\nfunc G() {}\n";
    let main_extraction = extract(
        &pack,
        &ctx_with_gomod("pkg/a.go", ".", "example.com/demo"),
        main_src,
    )
    .unwrap();
    let test_extraction = extract(
        &pack,
        &ctx_with_gomod("pkg/a_external_test.go", ".", "example.com/demo"),
        test_src,
    )
    .unwrap();
    let scope_of = |e: &Extraction| match &e.records[0] {
        PackRecord::Declaration { scope, .. } => scope.clone(),
        _ => unreachable!(),
    };
    assert_ne!(scope_of(&main_extraction), scope_of(&test_extraction));
    assert_eq!(scope_of(&main_extraction), "example.com/demo/pkg::pkg");
    assert_eq!(scope_of(&test_extraction), "example.com/demo/pkg::pkg_test");
}

// ── negatives: comments, raw strings, malformed, Unicode ─────────────────

#[test]
fn comments_and_raw_strings_never_produce_declarations_or_calls() {
    let pack = go_pack();
    let source = "\
package main

// func lookalike() {}
// helper()

var s = `
func alsoNotReal() {}
helper()
`

func real() {
\t_ = s
}
";
    let extraction = extract(&pack, &ctx("lookalike.go"), source).unwrap();
    let names: Vec<&str> = declarations(&extraction)
        .iter()
        .map(|r| match r {
            PackRecord::Declaration { name, .. } => name.as_str(),
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(names, ["real"]);
    assert!(
        extraction
            .records
            .iter()
            .all(|r| !matches!(r, PackRecord::Reference { .. })),
        "{:?}",
        extraction.records
    );
}

#[test]
fn unicode_identifiers_round_trip() {
    let pack = go_pack();
    let source = "package main\n\nfunc 日本語関数() {}\n";
    let extraction = extract(&pack, &ctx("unicode.go"), source).unwrap();
    let decl = decl_named(&extraction, "日本語関数");
    assert!(matches!(decl, PackRecord::Declaration { .. }));
}

#[test]
fn malformed_go_source_still_extracts_without_fabricating_results() {
    let pack = go_pack();
    // The shared extractor fails closed on syntax errors; no fabricated
    // declarations or references are returned for a broken source file.
    let source = "package main\n\nfunc broken( {\n";
    let err = extract(&pack, &ctx("broken.go"), source).expect_err("invalid Go must fail closed");
    assert!(err.to_string().contains("syntax errors"), "{err}");
}

// ── go build / go vet / go test toolchain outcomes ───────────────────────

mod toolchain_outcomes {
    use phronesis_mcp::outcomes::facts::OutcomeFact;
    use phronesis_mcp::outcomes::toolchain::{CompiledDef, DefSource, load_project_defs};
    use std::process::Command;

    fn run_init(dir: &std::path::Path) {
        let binary = std::env::var_os("PHRONESIS_TEST_BIN")
            .unwrap_or_else(|| env!("CARGO_BIN_EXE_phr-mcp").into());
        let out = Command::new(binary)
            .args(["init", "--packs", "go"])
            .current_dir(dir)
            .output()
            .expect("spawn phr-mcp init");
        assert!(
            out.status.success(),
            "init --packs go failed: {:?}",
            out.stderr
        );
    }

    fn compiled(dir: &std::path::Path, id: &str) -> CompiledDef {
        let defs = load_project_defs(dir);
        let def = defs
            .iter()
            .find(|d| d.id == id)
            .unwrap_or_else(|| panic!("{id} def in toolchains.json"))
            .clone();
        CompiledDef::compile(def, DefSource::Project).expect("compile def")
    }

    /// `(passed, failed)` from the `test_outcome` fact, if any.
    fn test_counts(facts: &[OutcomeFact]) -> Option<(usize, usize)> {
        let fact = facts.iter().find(|f| f.predicate == "test_outcome")?;
        Some((fact.args[1].parse().ok()?, fact.args[2].parse().ok()?))
    }

    fn build_outcome(facts: &[OutcomeFact]) -> &str {
        facts
            .iter()
            .find(|f| f.predicate == "build_outcome")
            .map(|f| f.args[1].as_str())
            .expect("build_outcome fact")
    }

    #[test]
    fn go_pack_writes_build_vet_and_test_toolchain_defs() {
        let dir = tempfile::tempdir().unwrap();
        run_init(dir.path());
        let build = compiled(dir.path(), "go.build");
        let vet = compiled(dir.path(), "go.vet");
        let test = compiled(dir.path(), "go.test");
        assert!(build.handles("go build ./..."));
        assert!(vet.handles("go vet ./..."));
        assert!(test.handles("go test ./..."));
        assert!(!build.handles("echo go build"));
    }

    #[test]
    fn go_build_and_vet_recognize_compile_failures_and_quiet_success() {
        let dir = tempfile::tempdir().unwrap();
        run_init(dir.path());
        let build = compiled(dir.path(), "go.build");

        let fail_output = "# example.com/demo\n./main.go:5:2: undefined: Foo\n";
        let facts = build.parse("s", "go build ./...", fail_output, Some(2));
        assert_eq!(build_outcome(&facts), "fail");

        // `go build` prints nothing on success; the exit code alone must
        // still ground a pass.
        let facts = build.parse("s", "go build ./...", "", Some(0));
        assert_eq!(build_outcome(&facts), "pass");

        // No exit code, no output at all: unknown, never a fabricated pass.
        let facts = build.parse("s", "go build ./...", "", None);
        assert_eq!(build_outcome(&facts), "unknown");
    }

    #[test]
    fn go_test_recognizes_pass_fail_no_tests_and_cached_success() {
        let dir = tempfile::tempdir().unwrap();
        run_init(dir.path());
        let test = compiled(dir.path(), "go.test");

        // Successful run (exit 0).
        let facts = test.parse(
            "s",
            "go test ./...",
            "ok  \texample.com/demo\t0.003s\n",
            Some(0),
        );
        assert_eq!(build_outcome(&facts), "pass");

        // Cached success, no exit code captured: compile_success evidence.
        let facts = test.parse(
            "s",
            "go test ./...",
            "ok  \texample.com/demo\t(cached)\n",
            None,
        );
        assert_eq!(build_outcome(&facts), "pass");

        // A quiet success prints no per-test lines, so it proves the build
        // but not how many tests ran: no test count is manufactured.
        assert!(test_counts(&facts).is_none());

        // No test files: still a pass (compiled fine, nothing ran) and no
        // positive test count is ever manufactured.
        let facts = test.parse(
            "s",
            "go test ./...",
            "?   \texample.com/demo\t[no test files]\n",
            Some(0),
        );
        assert_eq!(build_outcome(&facts), "pass");
        assert!(test_counts(&facts).is_none());

        // A verbose success counts its per-test lines; skipped tests are
        // neither passed nor failed.
        let pass_output = "=== RUN   TestAdd\n--- PASS: TestAdd (0.00s)\n=== RUN   TestSkip\n--- SKIP: TestSkip (0.00s)\nPASS\nok  \texample.com/demo\t0.002s\n";
        let facts = test.parse("s", "go test -v ./...", pass_output, Some(0));
        assert_eq!(build_outcome(&facts), "pass");
        assert_eq!(test_counts(&facts), Some((1, 0)));

        // A failing test compiles fine and fails its assertion: build pass,
        // test fail. Never the same verdict as a compile error.
        let fail_output =
            "=== RUN   TestAdd\n--- FAIL: TestAdd (0.00s)\nFAIL\nFAIL\texample.com/demo\t0.002s\n";
        let facts = test.parse("s", "go test -v ./...", fail_output, Some(1));
        assert_eq!(build_outcome(&facts), "pass");
        assert_eq!(test_counts(&facts), Some((0, 1)));

        // A build failure surfaced through `go test` (compile error, not a
        // test assertion failure).
        let build_fail = "# example.com/demo [example.com/demo.test]\n./main.go:5:2: undefined: Foo\nFAIL\texample.com/demo [build failed]\n";
        let facts = test.parse("s", "go test ./...", build_fail, Some(2));
        assert_eq!(build_outcome(&facts), "fail");
        assert!(test_counts(&facts).is_none());

        // Unknown exit, no recognizable text at all (e.g. truncated output).
        let facts = test.parse("s", "go test ./...", "...truncated", None);
        assert_eq!(build_outcome(&facts), "unknown");
    }

    #[test]
    fn language_pack_toolchains_merge_preserves_user_edits_for_go() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".phronesis")).unwrap();
        std::fs::write(
            dir.path().join(".phronesis/toolchains.json"),
            r#"[{"id":"go.build","matches":"user-edited"}]"#,
        )
        .unwrap();
        run_init(dir.path());
        let defs = load_project_defs(dir.path());
        let build = defs.iter().find(|d| d.id == "go.build").unwrap();
        assert_eq!(build.matches, "user-edited", "user edit must survive merge");
        assert!(
            defs.iter().any(|d| d.id == "go.vet"),
            "missing ids are appended"
        );
        assert!(
            defs.iter().any(|d| d.id == "go.test"),
            "missing ids are appended"
        );
    }
}

/// Every Go binding form that can shadow a package-level function name must
/// suppress same-scope resolution of a call through it (review finding 4):
/// otherwise the graph invents `calls` and `tested_by` edges to the package
/// function. Each source calls `Work()` through a local of that name.
#[test]
fn every_local_binding_form_shadows_a_package_function_call() {
    let pack = go_pack();
    let cases = [
        (
            "short_var",
            "func caller() {\n\tWork := func() {}\n\tWork()\n}\n",
        ),
        (
            "range",
            "func caller(cs []func()) {\n\tfor _, Work := range cs {\n\t\tWork()\n\t}\n}\n",
        ),
        (
            "func_literal_param",
            "func caller() {\n\tf := func(Work func()) { Work() }\n\t_ = f\n}\n",
        ),
        (
            "func_literal_variadic",
            "func caller() {\n\tf := func(Work ...func()) { Work[0]() }\n\t_ = f\n}\n",
        ),
        (
            "receiver",
            "type F func()\n\nfunc (Work F) Run() { Work() }\n",
        ),
        (
            "named_result",
            "func caller() (Work func()) {\n\tWork = func() {}\n\tWork()\n\treturn\n}\n",
        ),
        (
            "type_switch",
            "func caller(v any) {\n\tswitch Work := v.(type) {\n\tcase func():\n\t\tWork()\n\t}\n}\n",
        ),
        (
            "select_receive",
            "func caller(ch chan func()) {\n\tselect {\n\tcase Work := <-ch:\n\t\tWork()\n\t}\n}\n",
        ),
    ];
    for (name, body) in cases {
        let source = format!("package main\n\nfunc Work() {{}}\n\n{body}");
        let extraction = extract(&pack, &ctx("shadow.go"), &source).unwrap();
        let same_scope: Vec<&PackRecord> = extraction
            .records
            .iter()
            .filter(|r| {
                matches!(r, PackRecord::Reference { target, resolution, .. }
                    if target == "Work" && *resolution == ReferenceMode::SameScope)
            })
            .collect();
        assert!(
            same_scope.is_empty(),
            "{name}: a call through a local `Work` resolved to the package function: {same_scope:?}"
        );
    }

    // Control: the same call with no local binding still resolves.
    let source = "package main\n\nfunc Work() {}\n\nfunc caller() {\n\tWork()\n}\n";
    let extraction = extract(&pack, &ctx("plain.go"), source).unwrap();
    assert!(extraction.records.iter().any(|r| matches!(r,
        PackRecord::Reference { target, resolution, .. }
            if target == "Work" && *resolution == ReferenceMode::SameScope)));
}
