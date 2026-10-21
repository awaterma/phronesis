use std::fs;
use std::path::Path;

use phronesis_mcp::graph::{store, sync};
use phronesis_mcp::language_packs::{
    Extraction, FileContext, PackRecord, ReferenceMode, SourceSpan, compile_pack, file_context,
    graph_records, install, read_assets, resolve_pack_references,
};
use serde_json::json;

fn write(root: &Path, rel: &str, bytes: &[u8]) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn pack(dir: &Path) -> phronesis_mcp::language_packs::ValidatedPack {
    write(
        dir,
        "queries/decls.scm",
        b"(function_definition name: (identifier) @name) @def",
    );
    write(
        dir,
        "pack.json",
        json!({
            "schema_version": 1, "adapter_api": 1, "id": "demo_graph", "version": "0.1.0",
            "language": "demo", "extensions": ["demograph"], "grammar": "python",
            "capabilities": ["graph"],
            "queries": [{"id": "declarations", "path": "queries/decls.scm", "emit": [
                {"kind": "declaration", "name": "${name}", "scope": "pkg", "span_capture": "def"}
            ]}],
            "context_files": ["demo.mod"], "fixtures": []
        })
        .to_string()
        .as_bytes(),
    );
    compile_pack(read_assets(dir).unwrap()).unwrap()
}

fn span(source: &str, start: usize, end: usize) -> SourceSpan {
    SourceSpan::from_bytes(source, start, end).unwrap()
}

#[test]
fn nearest_context_changes_by_source_directory_and_refuses_symlink() {
    let source_pack = tempfile::tempdir().unwrap();
    let pack = pack(source_pack.path());
    let project = tempfile::tempdir().unwrap();
    write(project.path(), "demo.mod", b"outer");
    write(project.path(), "pkg/demo.mod", b"inner");
    let context = file_context(project.path(), "pkg/sub/a.demograph", &pack).unwrap();
    assert_eq!(context.context_files[0].file_rel, "pkg/demo.mod");
    assert_eq!(context.context_files[0].content, "inner");
    let context = file_context(project.path(), "other/a.demograph", &pack).unwrap();
    assert_eq!(context.context_files[0].file_rel, "demo.mod");
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        fs::create_dir_all(project.path().join("pkg/sub")).unwrap();
        symlink(
            project.path().join("demo.mod"),
            project.path().join("pkg/sub/demo.mod"),
        )
        .unwrap();
        assert!(file_context(project.path(), "pkg/sub/a.demograph", &pack).is_err());
    }
}

#[test]
fn duplicate_declaration_sites_and_unknown_receivers_do_not_resolve() {
    let source_pack = tempfile::tempdir().unwrap();
    let pack = pack(source_pack.path());
    let source = "def f(): pass\ndef f(): pass\ndef caller(): f()\n";
    let decl = |start, name: &str, test: bool| PackRecord::Declaration {
        name: name.into(),
        scope: "pkg".into(),
        receiver: None,
        span: span(source, start, start + 3),
        test,
        command: None,
    };
    let context = FileContext {
        file_rel: "a.demograph".into(),
        context_files: vec![],
    };
    let extraction = Extraction {
        records: vec![
            decl(0, "f", false),
            decl(14, "f", false),
            decl(28, "caller", true),
            PackRecord::Reference {
                owner_start_byte: 28,
                target: "f".into(),
                scope: "pkg".into(),
                resolution: ReferenceMode::SameScope,
                span: span(source, 42, 43),
            },
            PackRecord::Reference {
                owner_start_byte: 28,
                target: "missing".into(),
                scope: "pkg".into(),
                resolution: ReferenceMode::Unresolved,
                span: span(source, 42, 43),
            },
        ],
        diagnostics: vec![],
    };
    let base = graph_records(&pack, &context, &extraction).unwrap();
    assert_eq!(base.iter().filter(|e| e.p == "pack_declaration").count(), 3);
    assert!(resolve_pack_references(&base).is_empty());
}

#[test]
fn installed_asset_and_new_nearer_context_invalidate_freshness() {
    let source_pack = tempfile::tempdir().unwrap();
    let _ = pack(source_pack.path());
    let project = tempfile::tempdir().unwrap();
    write(project.path(), "demo.mod", b"root");
    write(
        project.path(),
        "pkg/a.demograph",
        b"def work():\n    pass\n",
    );
    install(project.path(), source_pack.path(), false).unwrap();
    sync::rebuild(project.path()).unwrap();
    let index = sync::load_index(&sync::index_path(project.path())).unwrap();
    assert_eq!(
        sync::check_freshness(project.path(), &index),
        sync::Freshness::Fresh
    );
    let edges = store::load(&store::graph_path(project.path())).unwrap();
    assert!(edges.iter().any(|e| e.p == "pack_declaration"));
    write(project.path(), "pkg/demo.mod", b"nearer");
    assert!(matches!(
        sync::check_freshness(project.path(), &index),
        sync::Freshness::Stale(_)
    ));
    sync::rebuild(project.path()).unwrap();
    let index = sync::load_index(&sync::index_path(project.path())).unwrap();
    assert_eq!(
        sync::check_freshness(project.path(), &index),
        sync::Freshness::Fresh
    );
    write(
        project.path(),
        ".phronesis/packs/demo_graph/queries/decls.scm",
        b"(identifier) @changed",
    );
    assert!(matches!(
        sync::check_freshness(project.path(), &index),
        sync::Freshness::Stale(_)
    ));
    assert!(sync::rebuild(project.path()).is_err());
    let edges = store::load(&store::graph_path(project.path())).unwrap();
    assert!(!edges.iter().any(|e| e.p == "pack_file_adapter"));
}

#[test]
fn cross_file_same_scope_reference_resolves_with_source_attribution() {
    let source_pack = tempfile::tempdir().unwrap();
    let pack = pack(source_pack.path());
    let a = FileContext {
        file_rel: "pkg/a.demograph".into(),
        context_files: vec![],
    };
    let b = FileContext {
        file_rel: "pkg/b.demograph".into(),
        context_files: vec![],
    };
    let decl = |name: &str, scope: &str, test: bool| PackRecord::Declaration {
        name: name.into(),
        scope: scope.into(),
        receiver: None,
        span: SourceSpan {
            start_byte: 0,
            end_byte: 1,
            start_line: 1,
            end_line: 1,
        },
        test,
        command: None,
    };
    let a_edges = graph_records(
        &pack,
        &a,
        &Extraction {
            records: vec![decl("work", "pkg", false)],
            diagnostics: vec![],
        },
    )
    .unwrap();
    let b_edges = graph_records(
        &pack,
        &b,
        &Extraction {
            records: vec![
                decl("TestWork", "pkg", true),
                PackRecord::Reference {
                    owner_start_byte: 0,
                    target: "work".into(),
                    scope: "pkg".into(),
                    resolution: ReferenceMode::SameScope,
                    span: SourceSpan {
                        start_byte: 2,
                        end_byte: 3,
                        start_line: 1,
                        end_line: 1,
                    },
                },
                PackRecord::Reference {
                    owner_start_byte: 0,
                    target: "work".into(),
                    scope: "other".into(),
                    resolution: ReferenceMode::SameScope,
                    span: SourceSpan {
                        start_byte: 4,
                        end_byte: 5,
                        start_line: 1,
                        end_line: 1,
                    },
                },
            ],
            diagnostics: vec![],
        },
    )
    .unwrap();
    let mut base = a_edges;
    base.extend(b_edges);
    let resolved = resolve_pack_references(&base);
    assert_eq!(resolved.len(), 1, "{resolved:?}");
    assert_eq!(resolved[0].p, "tested_by");
    assert_eq!(resolved[0].src, "pkg/b.demograph");
    assert_eq!(
        resolved[0].a,
        [
            "demo:demo_graph::pkg::work",
            "demo:demo_graph::pkg::TestWork"
        ]
    );
}

#[test]
fn malformed_pack_source_removes_old_edges_and_marks_graph_incomplete() {
    let source_pack = tempfile::tempdir().unwrap();
    let _ = pack(source_pack.path());
    let project = tempfile::tempdir().unwrap();
    write(project.path(), "a.demograph", b"def work():\n    pass\n");
    install(project.path(), source_pack.path(), false).unwrap();
    sync::rebuild(project.path()).unwrap();
    write(project.path(), "a.demograph", b"def work(:\n");
    let outcome = sync::rebuild(project.path()).unwrap();
    assert!(outcome.skipped > 0, "{outcome:?}");
    let edges = store::load(&store::graph_path(project.path())).unwrap();
    assert!(!edges.iter().any(|edge| edge.p == "pack_declaration"));
    let index = sync::load_index(&sync::index_path(project.path())).unwrap();
    assert!(matches!(
        sync::check_freshness(project.path(), &index),
        sync::Freshness::Stale(_)
    ));
}

#[cfg(all(feature = "go-grammar", feature = "rhai"))]
#[test]
fn go_multi_parameter_declaration_resolves_one_site_and_reaches_test() {
    let project = tempfile::tempdir().unwrap();
    write(
        project.path(),
        "go.mod",
        b"module example.com/calc\n\ngo 1.23\n",
    );
    write(
        project.path(),
        "calc.go",
        b"package calc\nfunc Add(a, b int) int { return a + b }\n",
    );
    write(
        project.path(),
        "calc_test.go",
        b"package calc\nimport \"testing\"\nfunc TestAdd(t *testing.T) { _ = Add(1, 2) }\n",
    );
    sync::rebuild(project.path()).unwrap();
    let edges = store::load(&store::graph_path(project.path())).unwrap();
    let add = edges
        .iter()
        .find(|edge| {
            edge.p == "pack_declaration" && edge.a.get(3).is_some_and(|name| name == "Add")
        })
        .expect("Add declaration");
    let test = edges
        .iter()
        .find(|edge| {
            edge.p == "pack_declaration" && edge.a.get(3).is_some_and(|name| name == "TestAdd")
        })
        .expect("TestAdd declaration");
    assert!(
        edges.iter().any(
            |edge| edge.p == "tested_by" && edge.a == vec![add.a[1].clone(), test.a[1].clone()]
        ),
        "missing tested_by for real Go multi-parameter declaration"
    );
    assert!(
        edges
            .iter()
            .any(|edge| edge.p == "test_reaches"
                && edge.a == vec![test.a[1].clone(), add.a[1].clone()]),
        "missing static reach"
    );
}
