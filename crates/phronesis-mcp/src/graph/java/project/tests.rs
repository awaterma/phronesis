use super::*;

fn project(files: &[(&str, &str)]) -> Project {
    Project::from_inputs(
        &files
            .iter()
            .map(|(path, body)| (path.to_string(), body.to_string()))
            .collect(),
    )
}

fn has(out: &crate::graph::extract::Extracted, predicate: &str, args: &[&str]) -> bool {
    out.edges
        .iter()
        .any(|edge| edge.p == predicate && edge.a == args)
}

#[test]
fn maven_project_emits_package_edges_and_canonical_static_test_coverage() {
    let project = project(&[
        (
            "pom.xml",
            "<project><groupId>example</groupId><artifactId>app</artifactId></project>",
        ),
        (
            "src/main/java/p/Service.java",
            "package p; public class Service { public static void run() {} public static void overloaded() {} public static void overloaded(int i) {} }",
        ),
        (
            "src/test/java/q/Check.java",
            "package q; import p.Service; class Check { @Test void check() { Service.run(); Service.overloaded(); } void helper() { Service.run(); } }",
        ),
    ]);
    let out = project.extract("src/test/java/q/Check.java");
    assert_eq!(out.skipped, 0);
    assert!(has(
        &out,
        "imports",
        &["java:example:app::q", "java:example:app::p"]
    ));
    assert!(has(
        &out,
        "tested_by",
        &[
            "java:example:app::p::Service::run",
            "java:example:app::q::Check::check"
        ]
    ));
    assert_eq!(
        out.edges
            .iter()
            .filter(|edge| edge.p == "tested_by")
            .count(),
        1
    );
    let definition = project.extract("src/main/java/p/Service.java");
    assert!(has(
        &definition,
        "defines_fn",
        &[
            "src/main/java/p/Service.java",
            "java:example:app::p::Service::run"
        ]
    ));
}

#[test]
fn watched_api_calls_require_a_receiver_zero_arguments_and_an_executed_body() {
    let project = project(&[
        (
            "pom.xml",
            "<project><groupId>example</groupId><artifactId>app</artifactId></project>",
        ),
        (
            "src/main/java/p/Service.java",
            r#"package p; class Service {
                void watched() {
                    value.get(); value.orElseThrow(); value.getAsInt();
                    value.getAsLong(); value.getAsDouble();
                }
                void excluded() {
                    get(); orElseThrow(); getAsInt(); getAsLong(); getAsDouble();
                    value.get(1); value.orElseThrow(factory); value.getAsInt(1);
                    value.getAsLong(1); value.getAsDouble(1);
                    Runnable deferred = () -> value.get();
                    Object anonymous = new Object() { void hidden() { value.get(); } };
                    class Local { void hidden() { value.get(); } }
                }
            }"#,
        ),
    ]);
    let out = project.extract("src/main/java/p/Service.java");
    let calls = out
        .edges
        .iter()
        .filter(|edge| edge.p == "calls_api")
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), 5);
    for name in ["get", "orElseThrow", "getAsInt", "getAsLong", "getAsDouble"] {
        assert!(has(
            &out,
            "calls_api",
            &["java:example:app::p::Service::watched", name]
        ));
    }
}

#[test]
fn shadowed_receivers_and_unresolved_calls_do_not_create_coverage() {
    let project = project(&[
        ("pom.xml", "<project><artifactId>app</artifactId></project>"),
        (
            "src/main/java/p/Service.java",
            "package p; class Service { static void run() {} }",
        ),
        (
            "src/test/java/q/Test.java",
            "package q; import p.Service; class Test { @Test void check(Object Service) { Service.run(); unknown.run(); } }",
        ),
    ]);
    assert!(
        !project
            .extract("src/test/java/q/Test.java")
            .edges
            .iter()
            .any(|edge| edge.p == "tested_by")
    );
}

#[test]
fn coverage_does_not_discard_ambiguous_receivers_or_overloaded_candidates() {
    for source in [
        "package checks; import static p.Service.run; class Checks { @Test void verifies() { this.run(); } }",
        "package checks; import p.*; import q.*; class Checks { @Test void verifies() { Service.run(); } }",
        "package checks; import static p.Service.*; import static q.Overloads.*; class Checks { @Test void verifies() { run(); } }",
    ] {
        let project = project(&[
            ("pom.xml", "<project><artifactId>app</artifactId></project>"),
            (
                "src/main/java/p/Service.java",
                "package p; class Service { static void run() {} }",
            ),
            (
                "src/main/java/q/Service.java",
                "package q; class Service { static void different() {} }",
            ),
            (
                "src/main/java/q/Overloads.java",
                "package q; class Overloads { static void run() {} static void run(int n) {} }",
            ),
            ("src/test/java/checks/Checks.java", source),
        ]);
        assert!(
            !project
                .extract("src/test/java/checks/Checks.java")
                .edges
                .iter()
                .any(|edge| edge.p == "tested_by"),
            "false coverage for {source}"
        );
    }
}

#[test]
fn declared_methods_take_precedence_over_static_imports() {
    let project = project(&[
        ("pom.xml", "<project><artifactId>app</artifactId></project>"),
        (
            "src/main/java/p/Service.java",
            "package p; class Service { static void run() {} }",
        ),
        (
            "src/test/java/checks/Checks.java",
            "package checks; import static p.Service.run; class Checks { void run() {} @Test void verifies() { run(); this.run(); } }",
        ),
    ]);
    let out = project.extract("src/test/java/checks/Checks.java");
    let coverage = out
        .edges
        .iter()
        .filter(|edge| edge.p == "tested_by")
        .collect::<Vec<_>>();
    assert_eq!(coverage.len(), 1);
    assert_eq!(
        coverage[0].a,
        [
            "java:app::checks::Checks::run",
            "java:app::checks::Checks::verifies"
        ]
    );
}

#[test]
fn explicit_type_imports_precede_wildcards_and_lexical_types_precede_imports() {
    for (body, target) in [
        (
            "import static p.Service.run; import static q.Service.*; class Checks { @Test void verifies() { run(); } }",
            "p::Service::run",
        ),
        (
            "import p.Service; import q.*; class Checks { @Test void verifies() { Service.run(); } }",
            "p::Service::run",
        ),
        (
            "import p.Service; class Checks { static class Service { static void run() {} } @Test void verifies() { Service.run(); } }",
            "checks::Checks::Service::run",
        ),
    ] {
        let project = project(&[
            ("pom.xml", "<project><artifactId>app</artifactId></project>"),
            (
                "src/main/java/p/Service.java",
                "package p; class Service { static void run() {} }",
            ),
            (
                "src/main/java/q/Service.java",
                "package q; class Service { static void run() {} }",
            ),
            (
                "src/test/java/checks/Checks.java",
                &format!("package checks; {body}"),
            ),
        ]);
        let out = project.extract("src/test/java/checks/Checks.java");
        let coverage = out
            .edges
            .iter()
            .filter(|edge| edge.p == "tested_by")
            .collect::<Vec<_>>();
        assert_eq!(coverage.len(), 1, "{body}");
        assert_eq!(coverage[0].a[0], format!("java:app::{target}"));
    }
}

#[test]
fn directly_constructed_receivers_resolve_without_guessing_variable_types() {
    for (expression, expected) in [
        ("new Service<String>().run()", true),
        ("(new p.Service<String>()).run()", true),
        (
            "new Service<String>() { public void run() {} }.run()",
            false,
        ),
        ("factory().run()", false),
        ("service.run()", false),
    ] {
        let test = format!(
            "package checks; import p.Service; class Checks {{ @Test void verifies() {{ Object Service = null; {expression}; }} }}"
        );
        let project = project(&[
            ("pom.xml", "<project><artifactId>app</artifactId></project>"),
            (
                "src/main/java/p/Service.java",
                "package p; public class Service<T> { public void run() {} }",
            ),
            ("src/test/java/checks/Checks.java", &test),
        ]);
        let out = project.extract("src/test/java/checks/Checks.java");
        assert_eq!(
            has(
                &out,
                "tested_by",
                &[
                    "java:app::p::Service::run",
                    "java:app::checks::Checks::verifies"
                ]
            ),
            expected,
            "{expression}"
        );
    }
}

#[test]
fn unresolved_inherited_methods_cannot_fall_through_to_static_imports() {
    for (declaration, invocation) in [
        ("class Checks extends UnknownBase", "run()"),
        ("class Checks implements UnknownInterface", "run()"),
        ("class Checks", "toString()"),
    ] {
        let test = format!(
            "package checks; import static p.Service.*; {declaration} {{ @Test void verifies() {{ {invocation}; }} }}"
        );
        let project = project(&[
            ("pom.xml", "<project><artifactId>app</artifactId></project>"),
            (
                "src/main/java/p/Service.java",
                "package p; class Service { static void run() {} static String toString() { return null; } }",
            ),
            ("src/test/java/checks/Checks.java", &test),
        ]);
        assert!(
            !project
                .extract("src/test/java/checks/Checks.java")
                .edges
                .iter()
                .any(|edge| edge.p == "tested_by"),
            "{declaration}"
        );
    }
}

#[test]
fn bazel_targets_restrict_imports_even_inside_the_same_package_unit() {
    let project = project(&[
        (
            "BUILD",
            "java_library(name='a', srcs=['a/A.java'])\njava_library(name='b', srcs=['b/B.java'])",
        ),
        ("a/A.java", "package a; import b.B; class A {}"),
        ("b/B.java", "package b; class B {}"),
    ]);
    let out = project.extract("a/A.java");
    assert_eq!(out.skipped, 1);
    assert!(!out.edges.iter().any(|edge| edge.p == "imports"));
}

#[test]
fn mismatched_path_is_diagnostic_and_does_not_change_declared_identity() {
    let project = project(&[(
        "wrong/A.java",
        "package actual; class A { void getValue() { optional.get(); optional.get(1); } }",
    )]);
    let out = project.extract("wrong/A.java");
    assert_eq!(out.skipped, 1);
    assert!(has(
        &out,
        "declares_module",
        &["wrong/A.java", "java:project::actual"]
    ));
    assert!(has(
        &out,
        "calls_api",
        &["java:project::actual::A::getValue", "get"]
    ));
}

#[test]
fn module_and_versioned_sources_are_excluded_and_build_overlap_is_diagnosed() {
    let project = project(&[
        (
            "pom.xml",
            "<project><groupId>e</groupId><artifactId>app</artifactId></project>",
        ),
        (
            "BUILD",
            "java_library(name='app', srcs=glob(['**/*.java']))",
        ),
        ("src/main/java/p/A.java", "package p; class A {}"),
        ("src/main/java/module-info.java", "module example {}"),
        ("src/main/java11/p/A.java", "package p; class A {}"),
    ]);
    assert_eq!(project.files.len(), 1);
    assert_eq!(project.files["src/main/java/p/A.java"].owner.unit, "e:app");
    assert_eq!(project.diagnostics.count("mixed_build_owner_selected"), 1);
    assert_eq!(project.diagnostics.count("module_info_skipped"), 1);
    assert_eq!(project.diagnostics.count("multi_release_root_ignored"), 1);
}

#[test]
fn declaration_cache_checks_bytes_even_when_length_and_mtime_are_unchanged() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("A.java");
    std::fs::write(&path, "package a; class A {}").unwrap();
    let timestamp = std::fs::metadata(&path).unwrap().modified().unwrap();
    let first = Project::discover(root.path(), None).unwrap();
    let warm = Project::discover(root.path(), None).unwrap();
    // Concurrent discovery may evict this root from the bounded cache.
    // Reuse itself is checked with isolated cache entries in cache::tests.
    assert_eq!(first.files["A.java"].owner, warm.files["A.java"].owner);
    std::fs::write(&path, "package b; class B {}").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(timestamp)
        .unwrap();
    let changed = Project::discover(root.path(), None).unwrap();
    assert_eq!(changed.files["A.java"].owner.module, "java:project::b");
    assert!(!Arc::ptr_eq(
        &warm.files["A.java"].source,
        &changed.files["A.java"].source
    ));
    std::fs::remove_file(&path).unwrap();
    assert!(
        Project::discover(root.path(), None)
            .unwrap()
            .files
            .is_empty()
    );
}

#[test]
fn in_memory_overlay_updates_importers_without_writing_the_source() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("A.java"), "package a; class A {}").unwrap();
    std::fs::write(
        root.path().join("B.java"),
        "package b; import a.A; class B {}",
    )
    .unwrap();
    let before = Project::discover(root.path(), None).unwrap();
    assert!(
        before
            .extract("B.java")
            .edges
            .iter()
            .any(|edge| edge.p == "imports")
    );
    let after = Project::discover(root.path(), Some(("A.java", "package c; class C {}"))).unwrap();
    assert!(
        !after
            .extract("B.java")
            .edges
            .iter()
            .any(|edge| edge.p == "imports")
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("A.java")).unwrap(),
        "package a; class A {}"
    );
}

#[test]
fn package_named_java11_is_not_mistaken_for_a_multi_release_source_root() {
    let project = project(&[
        ("pom.xml", "<project><artifactId>app</artifactId></project>"),
        (
            "src/main/java/example/java11/A.java",
            "package example.java11; import module java.sql; class A {}",
        ),
    ]);
    assert_eq!(project.files.len(), 1);
    assert_eq!(project.diagnostics.count("multi_release_root_ignored"), 0);
    assert_eq!(project.diagnostics.count("module_import_ignored"), 1);
}

#[test]
fn bazel_build_inside_a_java_package_uses_the_ancestor_source_root() {
    let project = project(&[
        (
            "src/main/java/example/BUILD",
            "java_library(name='lib', srcs=['A.java'])",
        ),
        (
            "src/main/java/example/A.java",
            "package example; class A {}",
        ),
    ]);
    assert_eq!(project.extract("src/main/java/example/A.java").skipped, 0);
}

#[test]
fn non_utf8_java_is_counted_without_aborting_other_files() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("Invalid.java"), [0xff, 0xfe]).unwrap();
    std::fs::write(root.path().join("Valid.java"), "class Valid {}").unwrap();
    let project = Project::discover(root.path(), None).unwrap();
    assert!(project.extract("Invalid.java").parse_failed);
    assert!(!project.extract("Valid.java").parse_failed);
    assert_eq!(project.diagnostics.count("unreadable_input"), 1);
    assert!(!project.input_hashes.contains_key("Invalid.java"));
}
