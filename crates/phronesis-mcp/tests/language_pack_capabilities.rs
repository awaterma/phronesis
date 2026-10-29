//! Declared capabilities decide what a pack runs (review finding 10): a
//! pack without `structural_facts` contributes no facts and is never parsed
//! for hooks or audit; a pack without `graph` contributes no graph edges or
//! static selection. A toolchains-only pack is never parsed at all.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use phronesis_mcp::language_packs::install::install;
use serde_json::{Value, json};

fn write(dir: &Path, rel: &str, bytes: &[u8]) {
    let path = dir.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

/// A python-grammar pack on `.capx` that emits one declaration and one fact
/// per function definition, declaring only `capabilities`.
fn structural_pack(capabilities: &[&str]) -> tempfile::TempDir {
    let src = tempfile::tempdir().unwrap();
    write(
        src.path(),
        "queries/defs.scm",
        b"(function_definition name: (identifier) @name) @def",
    );
    let manifest = json!({
        "schema_version": 1, "adapter_api": 1, "id": "capdemo", "version": "0.1.0",
        "language": "capdemo", "extensions": ["capx"], "grammar": "python",
        "capabilities": capabilities,
        "queries": [{"id": "defs", "path": "queries/defs.scm", "emit": [
            {"kind": "declaration", "name": "${name}", "scope": "", "span_capture": "def"},
            {"kind": "fact", "name": "def_seen", "args": ["${name}"], "span_capture": "def"}
        ]}],
        "context_files": [], "fixtures": []
    });
    write(src.path(), "pack.json", manifest.to_string().as_bytes());
    src
}

fn toolchains_only_pack() -> tempfile::TempDir {
    let src = tempfile::tempdir().unwrap();
    write(
        src.path(),
        "toolchains.json",
        json!([{"id": "capdemo.check", "matches": "^capcheck(\\s|$)"}])
            .to_string()
            .as_bytes(),
    );
    let manifest = json!({
        "schema_version": 1, "adapter_api": 1, "id": "capdemo", "version": "0.1.0",
        "language": "capdemo", "extensions": ["capx"], "grammar": "python",
        "capabilities": ["toolchains"], "queries": [], "toolchains": "toolchains.json",
        "context_files": [], "fixtures": []
    });
    write(src.path(), "pack.json", manifest.to_string().as_bytes());
    src
}

fn project(pack: &Path) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), ".phronesis/rules.json", br#"{"rules": []}"#);
    write(root.path(), "a.capx", b"def one():\n    pass\n");
    install(root.path(), pack, false).unwrap();
    root
}

fn phr(root: &Path, args: &[&str], stdin: &str) -> (i32, String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(args)
        .current_dir(root)
        .env("PHRONESIS_PROJECT_ROOT", root)
        .env("PHRONESIS_NO_ACTION_LOG", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        use std::io::Write;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
    }
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn graph_rows(root: &Path, relation: &str) -> usize {
    let (code, _, err) = phr(root, &["graph", "rebuild"], "");
    assert_eq!(code, 0, "{err}");
    let (_, out, _) = phr(root, &["graph", "query", relation, "--json"], "");
    let rows: Value = serde_json::from_str(&out).unwrap_or(Value::Null);
    rows["total"].as_u64().unwrap_or(0) as usize
}

fn pack_facts(root: &Path) -> usize {
    let registry = phronesis_mcp::language_packs::load(root).unwrap();
    let Ok(Some(pack)) = registry.for_path("a.capx") else {
        return 0;
    };
    let context = phronesis_mcp::language_packs::file_context(root, "a.capx", pack).unwrap();
    phronesis_mcp::language_packs::facts::facts_with_lines(
        &registry,
        &context,
        "def one():\n    pass\n",
    )
    .unwrap()
    .len()
}

#[test]
fn graph_needs_the_graph_capability() {
    let with = project(structural_pack(&["structural_facts", "graph"]).path());
    assert_eq!(graph_rows(with.path(), "pack_declaration"), 1);
    let without = project(structural_pack(&["structural_facts"]).path());
    assert_eq!(graph_rows(without.path(), "pack_declaration"), 0);
}

#[test]
fn facts_need_the_structural_facts_capability() {
    let with = project(structural_pack(&["structural_facts", "graph"]).path());
    assert_eq!(pack_facts(with.path()), 1);
    let without = project(structural_pack(&["graph"]).path());
    assert_eq!(pack_facts(without.path()), 0);
}

#[test]
fn a_toolchains_only_pack_never_parses_source() {
    let root = project(toolchains_only_pack().path());
    // Invalid python: a structural pack would fail to parse and block.
    let payload = json!({"tool_name": "Write", "tool_input": {
        "file_path": root.path().join("b.capx").display().to_string(),
        "content": "def (:\n"}});
    let (code, _, err) = phr(root.path(), &["pre-check"], &payload.to_string());
    assert_eq!(code, 0, "{err}");
}
