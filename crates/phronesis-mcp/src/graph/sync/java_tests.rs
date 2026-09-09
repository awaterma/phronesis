use super::*;
use crate::graph::{Edge, store};

fn write(root: &Path, file: &str, body: &str) {
    let path = root.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn edges(root: &Path) -> Vec<Edge> {
    store::load(&store::graph_path(root)).unwrap()
}

fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        "pom.xml",
        "<project><groupId>e</groupId><artifactId>app</artifactId></project>",
    );
    write(
        root.path(),
        "src/main/java/a/A.java",
        "package a; import b.B; class A { static void run() {} }",
    );
    write(
        root.path(),
        "src/main/java/b/B.java",
        "package b; import a.A; class B {}",
    );
    root
}

#[test]
fn java_overlays_respect_repository_boundaries_and_normalize_absolute_paths() {
    let root = fixture();
    let outside = tempfile::tempdir().unwrap();
    rebuild(root.path()).unwrap();
    let before = edges(root.path());
    let outside_file = outside.path().join("Outside.java");
    for path in [
        "../Outside.java",
        "node_modules/lib/Outside.java",
        ".hidden/Outside.java",
        "node_modules/lib/pom.xml",
        "../BUILD",
        outside_file.to_str().unwrap(),
    ] {
        on_save(root.path(), path, "package outside; class Outside {}").unwrap();
        assert_eq!(edges(root.path()), before, "accepted {path}");
        let project = crate::graph::java::project::Project::discover(
            root.path(),
            Some((path, "package outside; class Outside {}")),
        )
        .unwrap();
        assert!(!project.input_hashes.contains_key(path));
    }
    let new_file = root.path().join("src/main/java/newpkg/New.java");
    on_save(
        root.path(),
        new_file.to_str().unwrap(),
        "package newpkg; class New {}",
    )
    .unwrap();
    assert!(edges(root.path()).iter().any(|edge| {
        edge.p == "declares_module"
            && edge.a == ["src/main/java/newpkg/New.java", "java:e:app::newpkg"]
    }));
    assert!(!new_file.exists());
}

#[cfg(unix)]
#[test]
fn java_overlay_cannot_follow_a_symlink_outside_the_repository() {
    let root = fixture();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("linked")).unwrap();
    rebuild(root.path()).unwrap();
    let before = edges(root.path());
    on_save(root.path(), "linked/New.java", "class New {}").unwrap();
    assert_eq!(edges(root.path()), before);
    let project = crate::graph::java::project::Project::discover(
        root.path(),
        Some(("linked/New.java", "class New {}")),
    )
    .unwrap();
    assert!(!project.files.contains_key("linked/New.java"));
}

#[test]
fn java_rebuild_persists_package_cycles_and_manifest_freshness() {
    let root = fixture();
    let outcome = rebuild(root.path()).unwrap();
    assert_eq!(outcome.skipped, 0);
    let graph = edges(root.path());
    assert!(
        graph
            .iter()
            .any(|e| e.p == "in_cycle" && e.a.contains(&"java:e:app::a".into()))
    );
    assert!(graph.iter().any(|e| e.p == "declares_module" && e.a == ["src/main/java/a/A.java", "java:e:app::a"]));
    assert!(!graph.iter().any(|e| e.src == "pom.xml"));
    let index = load_index(&index_path(root.path())).unwrap();
    assert!(index.entries.contains_key("pom.xml"));
    assert_eq!(check_freshness(root.path(), &index), Freshness::Fresh);
    write(
        root.path(),
        "pom.xml",
        "<project><groupId>e</groupId><artifactId>renamed</artifactId></project>",
    );
    assert!(
        matches!(check_freshness(root.path(), &index), Freshness::Stale(files) if files.contains(&"pom.xml".into()))
    );
}

#[test]
fn manifest_only_save_replaces_old_units_and_uses_supplied_content() {
    let root = fixture();
    rebuild(root.path()).unwrap();
    let changed = "<project><groupId>e</groupId><artifactId>new</artifactId><build><testSourceDirectory>src/main/java</testSourceDirectory><sourceDirectory>elsewhere</sourceDirectory></build></project>";
    on_save(root.path(), "pom.xml", changed).unwrap();
    let graph = edges(root.path());
    assert!(
        !graph
            .iter()
            .any(|e| e.a.iter().any(|a| a.starts_with("java:e:app")))
    );
    assert!(
        graph
            .iter()
            .any(|e| e.p == "file_type" && e.a == ["src/main/java/a/A.java", "test"])
    );
    assert!(
        graph
            .iter()
            .any(|e| e.p == "declares_module" && e.a.contains(&"java:e:new::a".into()))
    );
    // The caller has not written the overlay. The graph honestly reports it
    // differs from disk rather than overwriting the POM behind the caller.
    let index = load_index(&index_path(root.path())).unwrap();
    assert!(matches!(
        check_freshness(root.path(), &index),
        Freshness::Stale(_)
    ));
    write(root.path(), "pom.xml", changed);
    assert_eq!(check_freshness(root.path(), &index), Freshness::Fresh);
}

#[test]
fn declaration_rename_and_deletion_refresh_untouched_importers() {
    let root = fixture();
    rebuild(root.path()).unwrap();
    let changed = "package c; class C {}";
    on_save(root.path(), "src/main/java/a/A.java", changed).unwrap();
    assert!(
        !edges(root.path())
            .iter()
            .any(|e| e.p == "imports" || e.p == "in_cycle")
    );
    write(root.path(), "src/main/java/a/A.java", changed);
    let after_save = edges(root.path());
    rebuild(root.path()).unwrap();
    assert_eq!(after_save, edges(root.path()));
    std::fs::remove_file(root.path().join("src/main/java/a/A.java")).unwrap();
    record_from_disk(root.path(), "src/main/java/a/A.java");
    assert!(
        !edges(root.path())
            .iter()
            .any(|e| e.src == "src/main/java/a/A.java")
    );
    assert_eq!(
        check_freshness(root.path(), &load_index(&index_path(root.path())).unwrap()),
        Freshness::Fresh
    );
}

#[test]
fn adding_and_removing_ambiguity_recomputes_imports() {
    let root = fixture();
    rebuild(root.path()).unwrap();
    write(
        root.path(),
        "src/main/java/a/Duplicate.java",
        "package a; class A {}",
    );
    record_from_disk(root.path(), "src/main/java/a/Duplicate.java");
    assert!(
        !edges(root.path())
            .iter()
            .any(|e| e.p == "imports" && e.src == "src/main/java/b/B.java")
    );
    std::fs::remove_file(root.path().join("src/main/java/a/Duplicate.java")).unwrap();
    record_from_disk(root.path(), "src/main/java/a/Duplicate.java");
    assert!(
        edges(root.path())
            .iter()
            .any(|e| e.p == "imports" && e.src == "src/main/java/b/B.java")
    );
}

#[test]
fn invalid_java_save_preserves_graph_and_explicit_rebuild_reports_staleness() {
    let root = fixture();
    rebuild(root.path()).unwrap();
    let before = edges(root.path());
    let old_index = load_index(&index_path(root.path())).unwrap();
    let invalid = "package ; class A {";
    on_save(root.path(), "src/main/java/a/A.java", invalid).unwrap();
    assert_eq!(edges(root.path()), before);
    assert_eq!(load_index(&index_path(root.path())).unwrap(), old_index);
    write(root.path(), "src/main/java/a/A.java", invalid);
    let outcome = rebuild(root.path()).unwrap();
    assert!(outcome.skipped > 0);
    assert!(matches!(
        check_freshness(root.path(), &load_index(&index_path(root.path())).unwrap()),
        Freshness::Stale(_)
    ));
}

#[test]
fn four_language_graph_and_java_test_coverage_survive_persistence_validation() {
    let root = fixture();
    write(root.path(), "src/lib.rs", "pub fn rust_function() {}");
    write(
        root.path(),
        "script.py",
        "def python_function():\n    pass\n",
    );
    write(
        root.path(),
        "script.ts",
        "export function typescriptFunction() {}",
    );
    write(
        root.path(),
        "src/test/java/check/Test.java",
        "package check; import a.A; class Test { @Test void verifies() { A.run(); } }",
    );
    rebuild(root.path()).unwrap();
    let graph = edges(root.path());
    for language in ["rust:", "python:", "typescript:", "java:"] {
        assert!(
            graph
                .iter()
                .any(|e| e.p == "defines_fn" && e.a[1].starts_with(language))
        );
    }
    assert!(graph.iter().any(|e| e.p == "tested_by"
        && e.a == ["java:e:app::a::A::run", "java:e:app::check::Test::verifies"]));
}
