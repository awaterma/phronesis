//! Call-resolution soundness on small real crates, rebuilt end to end.
//!
//! Each case is a repro from review of the no-guessed-edges resolver: a call
//! the graph must not attribute to a project function it does not reach. A
//! false `calls` or `tested_by` edge fabricates coverage evidence, so these
//! pin the absence of the edge and, where the same shape can be real, its
//! presence in a positive control.

use std::path::Path;

use phronesis_mcp::graph::{store, sync};
use tempfile::TempDir;

/// Write `files` (relative path, contents) under a fresh crate named `app`,
/// rebuild the graph, and return its `(relation, args)` pairs.
fn graph_of(files: &[(&str, &str)]) -> Vec<(String, Vec<String>)> {
    let dir = TempDir::new().expect("tempdir");
    write(
        dir.path(),
        "Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
    );
    for (path, body) in files {
        write(dir.path(), path, body);
    }
    sync::rebuild(dir.path()).expect("graph rebuild");
    store::load(&store::graph_path(dir.path()))
        .expect("load graph")
        .into_iter()
        .map(|edge| (edge.p, edge.a))
        .collect()
}

fn write(root: &Path, path: &str, body: &str) {
    let path = root.join(path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(path, body).expect("write fixture");
}

fn has(graph: &[(String, Vec<String>)], p: &str, a: &str, b: &str) -> bool {
    graph
        .iter()
        .any(|(rel, args)| rel == p && args.len() == 2 && args[0] == a && args[1] == b)
}

#[test]
fn a_module_path_bound_to_an_external_crate_never_resolves_to_a_project_module() {
    let graph = graph_of(&[
        ("src/lib.rs", "pub mod fs;\npub mod sync;\n"),
        (
            "src/fs.rs",
            "pub fn write(_p: &str, _c: &str) {}\n\
             pub fn read_to_string(_p: &str) -> String { String::new() }\n",
        ),
        (
            "src/sync.rs",
            "use std::fs;\n\
             pub fn save(p: &str) { fs::write(p, \"x\").unwrap(); }\n\
             pub fn load(p: &str) -> String { fs::read_to_string(p).unwrap() }\n\
             pub fn local() { crate::fs::write(\"a\", \"b\"); }\n",
        ),
        (
            "tests/it.rs",
            "use std::fs;\n#[test]\nfn uses_std_fs() { fs::write(\"/tmp/x\", \"x\").unwrap(); }\n",
        ),
    ]);
    assert!(!has(
        &graph,
        "calls",
        "rust:app::sync::save",
        "rust:app::fs::write"
    ));
    assert!(!has(
        &graph,
        "calls",
        "rust:app::sync::load",
        "rust:app::fs::read_to_string"
    ));
    assert!(!has(
        &graph,
        "tested_by",
        "rust:app::fs::write",
        "rust:app#test:it::uses_std_fs"
    ));
    // Positive control: the crate-anchored path is the project module.
    assert!(
        has(
            &graph,
            "calls",
            "rust:app::sync::local",
            "rust:app::fs::write"
        ),
        "{graph:?}"
    );
}

#[test]
fn crate_and_super_anchors_resolve_against_the_right_module() {
    let graph = graph_of(&[
        ("src/lib.rs", "pub mod capsule;\npub mod context;\n"),
        ("src/capsule.rs", "pub fn load() {}\npub fn save() {}\n"),
        (
            "src/context/mod.rs",
            "pub mod capsule;\npub mod other;\npub mod render;\n",
        ),
        (
            "src/context/capsule.rs",
            "pub fn load() {}\npub fn save() {}\n",
        ),
        ("src/context/other.rs", "pub fn x() {}\n"),
        (
            "src/context/render.rs",
            "use super::other;\n\
             pub fn go() { crate::capsule::load(); other::x(); }\n\
             pub fn up() { super::capsule::save(); }\n\
             #[cfg(test)]\nmod tests {\n    #[test]\n    fn t() { crate::capsule::load(); }\n}\n",
        ),
    ]);
    let go = "rust:app::context::render::go";
    assert!(
        has(&graph, "calls", go, "rust:app::capsule::load"),
        "{graph:?}"
    );
    assert!(!has(
        &graph,
        "calls",
        go,
        "rust:app::context::capsule::load"
    ));
    assert!(has(&graph, "calls", go, "rust:app::context::other::x"));
    let up = "rust:app::context::render::up";
    assert!(has(&graph, "calls", up, "rust:app::context::capsule::save"));
    assert!(!has(&graph, "calls", up, "rust:app::capsule::save"));
    let t = "rust:app::context::render::tests::t";
    assert!(has(&graph, "tested_by", "rust:app::capsule::load", t));
    assert!(!has(
        &graph,
        "tested_by",
        "rust:app::context::capsule::load",
        t
    ));
}

#[test]
fn a_module_path_reaches_a_nested_function_only_through_a_pub_use() {
    let graph = graph_of(&[
        ("src/lib.rs", "pub mod hooks;\n"),
        (
            "src/hooks.rs",
            "pub use std::process::exit;\n\
             pub use inner::real;\n\
             pub mod inner {\n    pub fn exit(_: i32) {}\n    pub fn real() {}\n}\n\
             pub fn go() { crate::hooks::exit(0); }\n\
             pub fn run() { crate::hooks::real(); }\n",
        ),
    ]);
    assert!(!has(
        &graph,
        "calls",
        "rust:app::hooks::go",
        "rust:app::hooks::inner::exit"
    ));
    assert!(
        has(
            &graph,
            "calls",
            "rust:app::hooks::run",
            "rust:app::hooks::inner::real"
        ),
        "{graph:?}"
    );
}

#[test]
fn a_let_that_shadows_a_typed_parameter_clears_its_receiver_type() {
    let graph = graph_of(&[
        ("src/lib.rs", "pub mod config;\n"),
        (
            "src/config.rs",
            "pub struct Config;\n\
             impl Config {\n    pub fn render(&self) -> String { String::new() }\n    \
             pub fn len(&self) -> usize { 0 }\n}\n\
             pub fn shadow(c: Config) -> usize { let c = c.render(); c.len() }\n",
        ),
    ]);
    let shadow = "rust:app::config::shadow";
    assert!(has(
        &graph,
        "calls",
        shadow,
        "rust:app::config::Config::render"
    ));
    assert!(!has(
        &graph,
        "calls",
        shadow,
        "rust:app::config::Config::len"
    ));
}

#[test]
fn a_qualified_external_type_never_falls_back_to_the_callers_own_type() {
    let graph = graph_of(&[
        ("src/lib.rs", "pub mod err;\n"),
        (
            "src/err.rs",
            "use std::io;\npub struct Error;\n\
             impl Error {\n    pub fn kind(&self) -> u8 { 0 }\n    \
             pub fn from_io(e: io::Error) -> u8 { let _ = e.kind(); 0 }\n    \
             pub fn own(&self) -> u8 { self.kind() }\n}\n",
        ),
    ]);
    assert!(!has(
        &graph,
        "calls",
        "rust:app::err::Error::from_io",
        "rust:app::err::Error::kind"
    ));
    assert!(has(
        &graph,
        "calls",
        "rust:app::err::Error::own",
        "rust:app::err::Error::kind"
    ));
}
