//! Standalone JavaScript packages must participate in graph rebuild and freshness.
use phronesis_mcp::graph::{store, sync};

#[test]
fn standalone_javascript_extensions_index_sources_tests_and_changes() {
    for extension in ["js", "jsx", "mjs", "cjs"] {
        let dir = tempfile::tempdir().expect("project");
        let root = dir.path();
        std::fs::write(root.join("package.json"), r#"{"name":"example-app"}"#).expect("package");
        std::fs::create_dir(root.join("src")).expect("src");
        std::fs::create_dir(root.join("tests")).expect("tests");
        let source = format!("src/store.{extension}");
        let test = format!("tests/store.test.{extension}");
        std::fs::write(root.join(&source), "export function load() { return 1; }\n")
            .expect("source");
        std::fs::write(root.join(&test), "test('testLoad', () => { load(); });\n").expect("test");
        sync::rebuild(root).expect("rebuild");
        let graph = store::load(&store::graph_path(root)).expect("graph");
        assert!(
            graph.iter().any(|edge| edge.p == "defines_test"
                && edge.a.iter().any(|arg| arg.contains("testLoad"))),
            "{extension}: no test: {graph:?}"
        );
        assert!(
            graph
                .iter()
                .any(|edge| edge.p == "defines_fn"
                    && edge.a.iter().any(|arg| arg.ends_with("::load"))),
            "{extension}: no function: {graph:?}"
        );
        let index = sync::load_index(&root.join(".phronesis/graph.index")).expect("index");
        assert!(
            index.entries.contains_key(&source),
            "{extension}: source missing"
        );
        assert!(
            index.entries.contains_key(&test),
            "{extension}: test missing"
        );
        assert!(matches!(
            sync::check_freshness(root, &index),
            sync::Freshness::Fresh
        ));
        std::fs::write(root.join(&source), "export function load() { return 2; }\n").expect("edit");
        assert!(
            !matches!(sync::check_freshness(root, &index), sync::Freshness::Fresh),
            "{extension}: edit not detected"
        );
    }
}
