//! A test that runs a Cargo binary target reaches that binary's `main`.
//!
//! Integration tests that spawn the package's binary through
//! `env!("CARGO_BIN_EXE_<name>")` make no Rust call the graph can follow, so
//! before this they reached nothing. These fixtures pin the edge, its
//! resolution through `Cargo.toml` bin targets, and the agreement of the
//! incremental save path with a full rebuild.

use super::*;
use crate::graph::model::Edge;
use crate::graph::store;
use std::collections::BTreeSet;
use tempfile::TempDir;

const MANIFEST: &str = r#"[package]
name = "tool"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "tool-cli"
path = "src/main.rs"
"#;

const LIB: &str = "pub fn engine() -> u32 { 1 }\n";

const MAIN: &str = "fn main() { run(); }\nfn run() { let _ = tool::engine(); }\n";

const TESTS: &str = r#"use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tool-cli"))
}

fn through_two_helpers() -> Command {
    bin()
}

#[test]
fn spawns_directly() {
    let _ = Command::new(env!("CARGO_BIN_EXE_tool-cli")).status();
}

#[test]
fn spawns_through_helper() {
    let _ = bin().status();
}

#[test]
fn spawns_through_two_helpers() {
    let _ = through_two_helpers().status();
}

#[test]
fn spawns_inside_a_macro() {
    assert!(Command::new(env!("CARGO_BIN_EXE_tool-cli")).status().is_ok());
}

#[test]
fn names_an_unknown_binary() {
    let _ = Command::new(env!("CARGO_BIN_EXE_ghost")).status();
}

#[test]
fn spawns_nothing() {
    let _ = 1 + 1;
}
"#;

const MAIN_FN: &str = "rust:tool#bin:tool::main";
const RUN_FN: &str = "rust:tool#bin:tool::run";
const ENGINE_FN: &str = "rust:tool::engine";

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(p, body).expect("write");
}

fn edges(root: &Path) -> Vec<Edge> {
    store::load(&store::graph_path(root)).expect("load")
}

fn fixture() -> TempDir {
    let d = TempDir::new().expect("tempdir");
    write(d.path(), "Cargo.toml", MANIFEST);
    write(d.path(), "src/lib.rs", LIB);
    write(d.path(), "src/main.rs", MAIN);
    write(d.path(), "tests/cli.rs", TESTS);
    d
}

fn test_id(name: &str) -> String {
    format!("rust:tool#test:cli::{name}")
}

fn has(graph: &[Edge], p: &str, a: &[&str]) -> bool {
    graph.iter().any(|edge| edge.p == p && edge.a == a)
}

/// What `test` can exercise: its own `test_reaches`, joined through
/// `bin_reaches` for every binary `main` it reaches.
fn reached_by(graph: &[Edge], test: &str) -> BTreeSet<String> {
    let direct = graph
        .iter()
        .filter(|edge| edge.p == "test_reaches" && edge.a[0] == test)
        .map(|edge| edge.a[1].clone())
        .collect::<BTreeSet<_>>();
    let through_binaries = graph
        .iter()
        .filter(|edge| edge.p == "bin_reaches" && direct.contains(&edge.a[0]))
        .map(|edge| edge.a[1].clone())
        .collect::<BTreeSet<_>>();
    direct.union(&through_binaries).cloned().collect()
}

/// `test_reaches` rows alone, without the `bin_reaches` join.
fn test_reaches_rows(graph: &[Edge], test: &str) -> BTreeSet<String> {
    graph
        .iter()
        .filter(|edge| edge.p == "test_reaches" && edge.a[0] == test)
        .map(|edge| edge.a[1].clone())
        .collect()
}

/// Every edge as a sortable tuple, so two graphs compare as sets.
fn edge_set(graph: &[Edge]) -> BTreeSet<(String, Vec<String>, String, bool)> {
    graph
        .iter()
        .map(|edge| (edge.p.clone(), edge.a.clone(), edge.src.clone(), edge.d))
        .collect()
}

#[test]
fn a_test_spawning_a_cargo_binary_reaches_its_main_and_mains_callees() {
    let d = fixture();
    let outcome = rebuild(d.path()).expect("rebuild");
    let graph = edges(d.path());

    for name in [
        "spawns_directly",
        "spawns_through_helper",
        "spawns_through_two_helpers",
        "spawns_inside_a_macro",
    ] {
        let test = test_id(name);
        assert!(
            has(&graph, "tested_by", &[MAIN_FN, &test]),
            "{name}: tested_by(main): {:?}",
            graph
                .iter()
                .filter(|e| e.p == "tested_by")
                .map(|e| &e.a)
                .collect::<Vec<_>>()
        );
        let reached = reached_by(&graph, &test);
        for function in [MAIN_FN, RUN_FN, ENGINE_FN] {
            assert!(
                reached.contains(function),
                "{name} reaches {function}: {reached:?}"
            );
        }
    }
    // main's closure is stored once, not per test.
    assert_eq!(
        test_reaches_rows(&graph, &test_id("spawns_directly")),
        BTreeSet::from([MAIN_FN.to_string()])
    );
    for function in [RUN_FN, ENGINE_FN] {
        assert!(has(&graph, "bin_reaches", &[MAIN_FN, function]));
    }
    assert!(!has(&graph, "bin_reaches", &[MAIN_FN, MAIN_FN]));
    assert!(
        reached_by(&graph, &test_id("spawns_nothing")).is_empty(),
        "a test that runs no binary reaches nothing"
    );
    assert!(
        !graph
            .iter()
            .any(|edge| edge.p == "no_direct_test" && edge.a == [MAIN_FN]),
        "main is run by a test"
    );
    // The helper that spawns the binary calls its entry point.
    assert!(has(&graph, "calls", &["rust:tool#test:cli::bin", MAIN_FN]));
    // The bin target the name resolved through is in the graph.
    assert!(has(
        &graph,
        "cargo_bin",
        &["rust:tool", "tool-cli", "rust:tool#bin:tool"]
    ));
    // `ghost` names no bin target: no edge, and counted as unresolved.
    assert!(reached_by(&graph, &test_id("names_an_unknown_binary")).is_empty());
    assert!(
        outcome
            .per_file_resolution
            .get("tests/cli.rs")
            .is_some_and(|(unresolved, _)| *unresolved >= 1),
        "{:?}",
        outcome.per_file_resolution
    );
}

#[test]
fn the_package_name_is_the_default_binary_name() {
    let d = fixture();
    write(
        d.path(),
        "Cargo.toml",
        "[package]\nname = \"tool\"\nversion = \"0.1.0\"\n",
    );
    write(
        d.path(),
        "src/bin/extra.rs",
        "fn main() { helper(); }\nfn helper() {}\n",
    );
    write(
        d.path(),
        "tests/cli.rs",
        "use std::process::Command;\n#[test]\nfn default_bin() { let _ = Command::new(env!(\"CARGO_BIN_EXE_tool\")).status(); }\n#[test]\nfn extra_bin() { let _ = Command::new(env!(\"CARGO_BIN_EXE_extra\")).status(); }\n#[test]\nfn renamed_away() { let _ = Command::new(env!(\"CARGO_BIN_EXE_tool-cli\")).status(); }\n",
    );
    rebuild(d.path()).expect("rebuild");
    let graph = edges(d.path());

    assert!(reached_by(&graph, &test_id("default_bin")).contains(ENGINE_FN));
    let extra = reached_by(&graph, &test_id("extra_bin"));
    assert!(extra.contains("rust:tool#bin:extra::main"), "{extra:?}");
    assert!(extra.contains("rust:tool#bin:extra::helper"), "{extra:?}");
    assert!(
        reached_by(&graph, &test_id("renamed_away")).is_empty(),
        "no [[bin]] names tool-cli any more"
    );
}

#[test]
fn a_binary_of_another_package_is_not_reachable_by_name() {
    // CARGO_BIN_EXE_<name> is set only for the test's own package.
    let d = fixture();
    write(
        d.path(),
        "Cargo.toml",
        "[workspace]\nmembers = [\"tool\", \"other\"]\n",
    );
    for (rel, body) in [
        ("tool/Cargo.toml", MANIFEST),
        ("tool/src/lib.rs", LIB),
        ("tool/src/main.rs", MAIN),
        (
            "other/Cargo.toml",
            "[package]\nname = \"other\"\nversion = \"0.1.0\"\n",
        ),
        ("other/src/lib.rs", ""),
        ("other/tests/cli.rs", TESTS),
    ] {
        write(d.path(), rel, body);
    }
    for rel in ["src/lib.rs", "src/main.rs", "tests/cli.rs"] {
        std::fs::remove_file(d.path().join(rel)).expect("remove root copy");
    }
    rebuild(d.path()).expect("rebuild");
    let graph = edges(d.path());
    assert!(
        reached_by(&graph, "rust:other#test:cli::spawns_directly").is_empty(),
        "{:?}",
        graph
            .iter()
            .filter(|e| e.p == "test_reaches")
            .collect::<Vec<_>>()
    );
}

/// Apply one edit through the incremental path, then compare against a full
/// rebuild of the same tree.
fn assert_incremental_matches_rebuild(root: &Path, rel: &str, body: &str) {
    write(root, rel, body);
    on_save(root, rel, body).expect("incremental save");
    let incremental = edge_set(&edges(root));
    rebuild(root).expect("rebuild");
    let full = edge_set(&edges(root));
    let only_incremental = incremental.difference(&full).collect::<Vec<_>>();
    let only_full = full.difference(&incremental).collect::<Vec<_>>();
    assert!(
        only_incremental.is_empty() && only_full.is_empty(),
        "after saving {rel}: only incremental {only_incremental:?}; only rebuild {only_full:?}"
    );
}

#[test]
fn incremental_saves_on_either_side_of_the_binary_edge_match_a_full_rebuild() {
    let d = fixture();
    rebuild(d.path()).expect("initial rebuild");

    // The test side: a new test spawns the binary.
    let tests = format!("{TESTS}\n#[test]\nfn added_later() {{ let _ = bin().status(); }}\n");
    assert_incremental_matches_rebuild(d.path(), "tests/cli.rs", &tests);
    assert!(reached_by(&edges(d.path()), &test_id("added_later")).contains(ENGINE_FN));

    // The binary side: main gains a callee.
    let main =
        "fn main() { run(); extra(); }\nfn run() { let _ = tool::engine(); }\nfn extra() {}\n";
    assert_incremental_matches_rebuild(d.path(), "src/main.rs", main);
    assert!(
        reached_by(&edges(d.path()), &test_id("spawns_directly"))
            .contains("rust:tool#bin:tool::extra")
    );

    // The manifest side: renaming the bin target unresolves the old name.
    let renamed = MANIFEST.replace("tool-cli", "tool-renamed");
    assert_incremental_matches_rebuild(d.path(), "Cargo.toml", &renamed);
    assert!(reached_by(&edges(d.path()), &test_id("spawns_directly")).is_empty());

    // And back: the hook sensor's path agrees too.
    write(d.path(), "Cargo.toml", MANIFEST);
    record_from_disk(d.path(), "Cargo.toml");
    let sensed = edge_set(&edges(d.path()));
    rebuild(d.path()).expect("rebuild");
    assert_eq!(sensed, edge_set(&edges(d.path())));
    assert!(reached_by(&edges(d.path()), &test_id("spawns_directly")).contains(ENGINE_FN));
}

#[test]
fn an_out_of_band_manifest_edit_reads_as_drift() {
    let d = fixture();
    rebuild(d.path()).expect("rebuild");
    let index = load_index(&index_path(d.path())).expect("index");
    assert_eq!(check_freshness(d.path(), &index), Freshness::Fresh);

    write(
        d.path(),
        "Cargo.toml",
        &MANIFEST.replace("tool-cli", "tool-renamed"),
    );
    assert_eq!(
        check_freshness(d.path(), &index),
        Freshness::Stale(vec!["Cargo.toml".to_string()])
    );
}
