//! End-to-end proof that structural graph rules reach a verdict through the
//! real binary — extractor, durable graph, derivation, hydration, RETE join,
//! and the staleness demotion, with no LLM in the loop.
//!
//! The unit tests cover each stage in isolation. This file pins the wiring
//! between them, which is where the earlier `graph_fresh` design silently
//! failed: every stage passed its own tests while the composed path did the
//! wrong thing.
//!
//! Spec: `docs/specs/SPEC-triple-store-rete.md` §8 task 8.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use phronesis_mcp::graph::store;
use tempfile::TempDir;

/// A production function calling a watched API, with no test anywhere.
const RISKY_SOURCE: &str = r#"
pub fn danger(v: Vec<u32>) -> u32 {
    let first = v.first().expect("empty");
    *first
}
"#;

/// Four conditions across four relations, joined on `?file` and `?func`.
/// `severity` is the action verb, so one fixture drives both the warn and
/// block cases.
fn rules_json(severity: &str) -> String {
    format!(
        r#"{{"rules":[{{
        "id":"structural-untested-risky-call","phase":"pre","priority":100,
        "when":[
          {{"file_type":["?file","production"]}},
          {{"defines_fn":["?file","?func"]}},
          {{"calls_api":["?func","expect"]}},
          {{"no_direct_test":["?func"]}}
        ],
        "then":{{"{severity}":"`?file` defines `?func`, which calls a panicking API and has no direct test."}}
    }}]}}"#
    )
}

fn project(severity: &str) -> TempDir {
    let d = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(d.path().join("src")).expect("mkdir src");
    std::fs::create_dir_all(d.path().join(".phronesis")).expect("mkdir .phronesis");
    std::fs::write(d.path().join("src/risky.rs"), RISKY_SOURCE).expect("write source");
    std::fs::write(d.path().join(".phronesis/rules.json"), rules_json(severity))
        .expect("write rules");
    rebuild_graph(d.path());
    d
}

fn rebuild_graph(dir: &Path) {
    let status = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .current_dir(dir)
        .args(["graph", "rebuild", "--path", "."])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run graph rebuild");
    assert!(status.success(), "graph rebuild failed");
}

/// Run the real pre-check hook over an Edit of `src/risky.rs`.
fn pre_check(dir: &Path) -> (i32, String) {
    pre_check_content(dir, "b")
}

fn pre_check_content(dir: &Path, new_string: &str) -> (i32, String) {
    let payload = format!(
        r#"{{"session_id":"s","cwd":"{}","hook_event_name":"PreToolUse","tool_name":"Edit",
            "tool_input":{{"file_path":"src/risky.rs","old_string":"a","new_string":{}}}}}"#,
        dir.display(),
        serde_json::to_string(new_string).expect("json string")
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .current_dir(dir)
        .arg("pre-check")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pre-check");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(payload.as_bytes())
        .expect("write payload");
    let out = child.wait_with_output().expect("wait");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// Simulate `git checkout` / a shell edit: content changes, hook never runs.
fn edit_outside_the_hook(dir: &Path) {
    let mut body = RISKY_SOURCE.to_string();
    body.push_str("\npub fn added_outside_the_hook() {}\n");
    std::fs::write(dir.join("src/risky.rs"), body).expect("write");
}

#[test]
fn a_structural_rule_blocks_on_a_fresh_graph() {
    let d = project("block");
    let (code, stderr) = pre_check(d.path());
    assert_eq!(code, 2, "fresh graph must carry block authority: {stderr}");
    assert!(stderr.contains("BLOCKED"), "{stderr}");
}

#[test]
fn the_verdict_names_the_bound_file_and_function() {
    // Proves the join actually bound ?file and ?func rather than matching
    // something degenerate.
    let d = project("block");
    let (_, stderr) = pre_check(d.path());
    assert!(stderr.contains("src/risky.rs"), "{stderr}");
    assert!(stderr.contains("rust:crate::risky::danger"), "{stderr}");
}

#[test]
fn a_drifted_graph_demotes_a_blocking_rule_to_a_warning() {
    let d = project("block");
    edit_outside_the_hook(d.path());
    let (code, stderr) = pre_check(d.path());
    assert_eq!(code, 1, "stale evidence must not block: {stderr}");
    assert!(stderr.contains("WARNING"), "{stderr}");
    assert!(!stderr.contains("BLOCKED"), "{stderr}");
}

#[test]
fn a_drifted_graph_says_how_to_resync() {
    let d = project("block");
    edit_outside_the_hook(d.path());
    let (_, stderr) = pre_check(d.path());
    assert!(stderr.contains("stale"), "{stderr}");
    assert!(stderr.contains("graph rebuild"), "{stderr}");
}

#[test]
fn rebuilding_restores_block_authority() {
    let d = project("block");
    edit_outside_the_hook(d.path());
    assert_eq!(
        pre_check(d.path()).0,
        1,
        "precondition: demoted while stale"
    );
    rebuild_graph(d.path());
    let (code, stderr) = pre_check(d.path());
    assert_eq!(code, 2, "resync must restore enforcement: {stderr}");
}

#[test]
fn a_warn_severity_rule_warns_on_a_fresh_graph() {
    let d = project("warn");
    let (code, stderr) = pre_check(d.path());
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("WARNING"), "{stderr}");
}

#[test]
fn a_rule_warns_after_its_previously_bound_referent_is_deleted() {
    let d = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(d.path().join("src")).expect("src");
    std::fs::create_dir_all(d.path().join(".phronesis")).expect("state");
    std::fs::write(d.path().join("src/risky.rs"), "pub fn legacy_call() {}\n").expect("source");
    std::fs::write(
        d.path().join(".phronesis/rules.json"),
        r#"{"rules":[{"id":"legacy-contract","phase":"pre","priority":10,"when":[{"new_content_contains":"legacy_call("}],"then":{"block":"legacy contract"}}]}"#,
    )
    .expect("rules");
    rebuild_graph(d.path());

    let (code, stderr) = pre_check_content(d.path(), "legacy_call(");
    assert_eq!(code, 2, "bound rule must initially block: {stderr}");

    std::fs::write(d.path().join("src/risky.rs"), "pub fn replacement() {}\n")
        .expect("replace referent");
    rebuild_graph(d.path());
    let (code, stderr) = pre_check_content(d.path(), "legacy_call(");
    assert_eq!(code, 1, "stale rule must warn: {stderr}");
    assert!(stderr.contains("legacy_call"), "{stderr}");
    assert!(stderr.contains("will warn, not block"), "{stderr}");
}

fn stale_bound_rule_project() -> TempDir {
    let d = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(d.path().join("src")).expect("src");
    std::fs::create_dir_all(d.path().join(".phronesis")).expect("state");
    std::fs::write(d.path().join("src/risky.rs"), "pub fn legacy_call() {}\n").expect("source");
    std::fs::write(
        d.path().join(".phronesis/rules.json"),
        r#"{"rules":[{"id":"legacy-contract","phase":"pre","priority":10,"when":[{"new_content_contains":"legacy_call("}],"then":{"block":"legacy contract"}}]}"#,
    )
    .expect("rules");
    rebuild_graph(d.path());
    std::fs::write(d.path().join("src/risky.rs"), "pub fn replacement() {}\n")
        .expect("replace referent");
    rebuild_graph(d.path());
    assert_eq!(pre_check_content(d.path(), "legacy_call(").0, 1);
    d
}

#[test]
fn malformed_binding_evidence_preserves_block_authority() {
    let d = stale_bound_rule_project();
    std::fs::write(d.path().join(".phronesis/bindings.json"), "not json\n")
        .expect("corrupt bindings");

    let (code, stderr) = pre_check_content(d.path(), "legacy_call(");
    assert_eq!(code, 2, "malformed evidence must fail closed: {stderr}");
    assert!(stderr.contains("BLOCKED"), "{stderr}");
}

#[test]
fn generation_mismatched_binding_evidence_preserves_block_authority() {
    let d = stale_bound_rule_project();
    let path = d.path().join(".phronesis/bindings.json");
    let mut bindings: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read bindings"))
            .expect("parse bindings");
    bindings["generation"] = serde_json::json!(bindings["generation"].as_u64().unwrap() + 1);
    std::fs::write(
        &path,
        format!("{}\n", serde_json::to_string_pretty(&bindings).unwrap()),
    )
    .expect("write mismatched bindings");

    let (code, stderr) = pre_check_content(d.path(), "legacy_call(");
    assert_eq!(code, 2, "mismatched evidence must fail closed: {stderr}");
    assert!(stderr.contains("BLOCKED"), "{stderr}");
}

#[test]
fn a_covered_function_produces_no_verdict() {
    // The negative case: add a real same-module direct test and the rule must
    // go silent. A bare call in an unrelated integration-test module is not
    // valid Rust name-resolution evidence.
    let d = project("block");
    std::fs::write(
        d.path().join("src/risky.rs"),
        format!(
            "{RISKY_SOURCE}\n#[cfg(test)]\nmod tests {{\n    use super::*;\n    #[test]\n    fn covers() {{ danger(vec![1]); }}\n}}\n"
        ),
    )
    .expect("write test");
    rebuild_graph(d.path());
    let (code, stderr) = pre_check(d.path());
    assert_eq!(code, 0, "a tested function must not be flagged: {stderr}");
}

// ─── the shipped `structural` pack ──────────────────────────────────
//
// The tests above drive hand-written rules. These drive the pack a user
// actually gets from `phr-mcp init --packs structural`, which is the only
// thing that proves the shipped JSON is well-formed and scoped.

/// A project wired by the real `init`, with a risky file, an import cycle,
/// and a file with neither.
fn packaged_project() -> TempDir {
    let d = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(d.path().join("src")).expect("mkdir src");
    std::fs::write(d.path().join("src/risky.rs"), RISKY_SOURCE).expect("risky");
    std::fs::write(
        d.path().join("src/a.rs"),
        "use crate::b::Thing;\npub fn from_a() -> Thing { Thing }\n",
    )
    .expect("a");
    std::fs::write(
        d.path().join("src/b.rs"),
        "use crate::a::from_a;\npub struct Thing;\npub fn from_b() { from_a(); }\n",
    )
    .expect("b");
    std::fs::write(
        d.path().join("src/clean.rs"),
        "pub fn clean() -> u32 { 1 }\n",
    )
    .expect("clean");

    let status = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .current_dir(d.path())
        .args(["init", "--packs", "structural", "."])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run init");
    assert!(status.success(), "init failed");
    rebuild_graph(d.path());
    d
}

/// Run pre-check over an Edit of an arbitrary file.
fn pre_check_file(dir: &Path, rel: &str) -> (i32, String) {
    let payload = format!(
        r#"{{"session_id":"s","cwd":"{}","hook_event_name":"PreToolUse","tool_name":"Edit",
            "tool_input":{{"file_path":"{rel}","old_string":"a","new_string":"b"}}}}"#,
        dir.display()
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .current_dir(dir)
        .arg("pre-check")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(payload.as_bytes())
        .expect("write");
    let out = child.wait_with_output().expect("wait");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn the_shipped_pack_flags_a_risky_function() {
    let d = packaged_project();
    let (code, stderr) = pre_check_file(d.path(), "src/risky.rs");
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("rust:crate::risky::danger"), "{stderr}");
    assert!(stderr.contains("expect"), "{stderr}");
}

#[test]
fn the_shipped_pack_flags_an_import_cycle() {
    let d = packaged_project();
    let (code, stderr) = pre_check_file(d.path(), "src/a.rs");
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("import cycle"), "{stderr}");
}

#[test]
fn editing_an_unrelated_file_stays_silent() {
    // The scoping guarantee. Graph relations are repo-wide, so without the
    // `edited_file` join every edit would re-report every violation in the
    // project — which is how a pack earns itself an uninstall.
    let d = packaged_project();
    let (code, stderr) = pre_check_file(d.path(), "src/clean.rs");
    assert_eq!(
        code, 0,
        "a clean file must produce no structural noise: {stderr}"
    );
    assert!(!stderr.contains("WARNING"), "{stderr}");
}

#[test]
fn a_cycle_warning_names_only_the_edited_modules_cycle() {
    // Both modules are in the cycle; editing one must not report the other.
    let d = packaged_project();
    let (_, stderr) = pre_check_file(d.path(), "src/a.rs");
    assert!(stderr.contains("rust:crate::a"), "{stderr}");
    assert!(
        !stderr.contains("Module `rust:crate::b`"),
        "editing a.rs must not warn about b.rs: {stderr}"
    );
}

#[test]
fn init_alone_is_enough_for_the_pack_to_fire() {
    // `packaged_project` calls rebuild explicitly; this asserts the path a
    // real user takes, where `init` is the only command they run. A pack that
    // needs an undocumented second step reads as broken, not as unbuilt.
    let d = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(d.path().join("src")).expect("mkdir src");
    std::fs::write(d.path().join("src/risky.rs"), RISKY_SOURCE).expect("risky");

    let status = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .current_dir(d.path())
        .args(["init", "--packs", "structural", "."])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run init");
    assert!(status.success(), "init failed");

    // No rebuild_graph() call here — that is the point of the test.
    let (code, stderr) = pre_check_file(d.path(), "src/risky.rs");
    assert_eq!(code, 1, "pack must work straight after init: {stderr}");
    assert!(stderr.contains("rust:crate::risky::danger"), "{stderr}");
}

#[test]
fn workspace_members_with_the_same_modules_keep_distinct_graph_identities() {
    let d = TempDir::new().expect("tempdir");
    std::fs::write(
        d.path().join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/alpha\", \"crates/beta\"]\nresolver = \"2\"\n",
    )
    .expect("workspace manifest");

    for package in ["alpha", "beta"] {
        let root = d.path().join("crates").join(package);
        std::fs::create_dir_all(root.join("src")).expect("mkdir package");
        std::fs::write(
            root.join("Cargo.toml"),
            format!("[package]\nname = \"{package}\"\nversion = \"0.1.0\"\n"),
        )
        .expect("package manifest");
        std::fs::write(root.join("src/lib.rs"), "mod left;\nmod right;\n").expect("lib");
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("main");
        std::fs::write(
            root.join("src/left.rs"),
            "use crate::right::Right;\npub struct Left(pub Right);\n",
        )
        .expect("left");
        std::fs::write(
            root.join("src/right.rs"),
            "use crate::left::Left;\npub struct Right(pub Option<Box<Left>>);\n",
        )
        .expect("right");
    }

    rebuild_graph(d.path());
    let edges = store::load(&store::graph_path(d.path())).expect("load graph");
    let modules: Vec<_> = edges
        .iter()
        .filter(|edge| edge.p == "declares_module")
        .filter_map(|edge| edge.a.get(1))
        .cloned()
        .collect();
    assert!(
        modules.contains(&"rust:alpha::left".to_string()),
        "{modules:?}"
    );
    assert!(
        modules.contains(&"rust:beta::left".to_string()),
        "{modules:?}"
    );
    assert!(modules.contains(&"rust:alpha".to_string()), "{modules:?}");
    assert!(
        modules.contains(&"rust:alpha#bin:alpha".to_string()),
        "library and default binary are separate compilation units: {modules:?}"
    );

    let cycles: Vec<_> = edges
        .iter()
        .filter(|edge| edge.p == "in_cycle")
        .map(|edge| edge.a.clone())
        .collect();
    assert!(
        cycles
            .iter()
            .any(|args| args.first().is_some_and(|m| m == "rust:alpha::left")),
        "{cycles:?}"
    );
    assert!(
        cycles
            .iter()
            .any(|args| args.first().is_some_and(|m| m == "rust:beta::left")),
        "{cycles:?}"
    );
    assert_eq!(
        cycles
            .iter()
            .filter_map(|args| args.get(1))
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        2,
        "each package must retain its own strongly connected component: {cycles:?}"
    );
}

#[test]
fn a_mixed_language_repository_yields_one_graph_per_language() {
    // The `<lang>:` prefix exists so two extractors can share one graph. This
    // is the first test that actually exercises it: a Rust crate and a Python
    // distribution, each with a module named `utils` defining a function
    // named `load`. Under a language-blind identity the two collapse into one
    // node, and every relation hanging off them merges.
    let d = TempDir::new().expect("tempdir");

    std::fs::create_dir_all(d.path().join("rust-side/src")).expect("mkdir rust");
    std::fs::write(
        d.path().join("rust-side/Cargo.toml"),
        "[package]\nname = \"rust-side\"\nversion = \"0.1.0\"\n",
    )
    .expect("cargo manifest");
    std::fs::write(d.path().join("rust-side/src/lib.rs"), "pub mod utils;\n").expect("lib");
    std::fs::write(
        d.path().join("rust-side/src/utils.rs"),
        "pub fn load() -> u32 { 1 }\n",
    )
    .expect("rust utils");

    std::fs::create_dir_all(d.path().join("py-side/src/pyside")).expect("mkdir python");
    std::fs::write(
        d.path().join("py-side/pyproject.toml"),
        "[project]\nname = \"py-side\"\nversion = \"0.1.0\"\n",
    )
    .expect("pyproject");
    std::fs::write(
        d.path().join("py-side/src/pyside/__init__.py"),
        "from . import utils\n",
    )
    .expect("python init");
    std::fs::write(
        d.path().join("py-side/src/pyside/utils.py"),
        "def load():\n    return 1\n",
    )
    .expect("python utils");

    rebuild_graph(d.path());
    let edges = store::load(&store::graph_path(d.path())).expect("load graph");

    let functions: Vec<String> = edges
        .iter()
        .filter(|e| e.p == "defines_fn")
        .filter_map(|e| e.a.get(1).cloned())
        .collect();

    assert!(
        functions.contains(&"rust:rust-side::utils::load".to_string()),
        "the Rust extractor must still name its own entities: {functions:?}"
    );
    // `::` separates segments in every language, not just Rust: derivation
    // bridges `tested_by`'s bare callee names to `defines_fn`'s qualified
    // ones by splitting on it, so a dotted Python identity would report every
    // tested Python function as untested.
    assert!(
        functions.contains(&"python:py-side::pyside::utils::load".to_string()),
        "the Python extractor must name entities under its own language tag: {functions:?}"
    );
    assert_eq!(
        functions
            .iter()
            .filter(|f| f.ends_with("utils::load"))
            .count(),
        2,
        "same-named functions in different languages must stay distinct: {functions:?}"
    );
}

// ─── risky-call rules stay scoped to their own language ──────────────
//
// `calls_api` is not language-scoped by itself — both extractors write to
// it, Rust with `unwrap`/`expect`/`panic`/`todo`/`unimplemented`, TypeScript
// with `non_null_assertion`. Before this fix `warn-untested-risky-call` left
// its `?api` argument unconstrained, so it matched TypeScript's
// `non_null_assertion` too and fired its Rust-flavoured "can panic" message
// on TypeScript files, alongside the correct `warn-ts-untested-risky-call`
// warning. These tests pin one warning per language, so a third language
// reusing `calls_api` without its own watchlist constraint cannot silently
// reintroduce the collision.

/// A project with an untested, non-null-asserting TypeScript function and an
/// untested, unwrapping Rust function side by side.
fn mixed_language_risky_project() -> TempDir {
    let d = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(d.path().join("src")).expect("mkdir src");
    std::fs::write(d.path().join("package.json"), r#"{"name": "myapp"}"#).expect("package.json");
    std::fs::write(
        d.path().join("src/billing.ts"),
        "export function charge(o?: { total: number }) { return o!.total }\n",
    )
    .expect("ts");
    std::fs::write(d.path().join("src/risky.rs"), RISKY_SOURCE).expect("rust");

    let status = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .current_dir(d.path())
        .args(["init", "--packs", "structural", "."])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run init");
    assert!(status.success(), "init failed");
    rebuild_graph(d.path());
    d
}

#[test]
fn a_typescript_file_fires_only_the_typescript_risky_call_rule() {
    let d = mixed_language_risky_project();
    let (code, stderr) = pre_check_file(d.path(), "src/billing.ts");
    assert_eq!(code, 1, "{stderr}");
    assert_eq!(
        stderr.matches("WARNING").count(),
        1,
        "exactly one warning expected, not the Rust rule firing too: {stderr}"
    );
    assert!(stderr.contains("non-null assertion"), "{stderr}");
    assert!(
        !stderr.contains("panicking API"),
        "the Rust risky-call rule must not fire on a TypeScript file: {stderr}"
    );
}

#[test]
fn a_rust_file_fires_only_the_rust_risky_call_rule() {
    let d = mixed_language_risky_project();
    let (code, stderr) = pre_check_file(d.path(), "src/risky.rs");
    assert_eq!(code, 1, "{stderr}");
    assert_eq!(
        stderr.matches("WARNING").count(),
        1,
        "exactly one warning expected, not the TypeScript rule firing too: {stderr}"
    );
    assert!(stderr.contains("panicking API"), "{stderr}");
    assert!(
        !stderr.contains("non-null assertion"),
        "the TypeScript risky-call rule must not fire on a Rust file: {stderr}"
    );
}

/// Run the real post-check hook over an Edit of `rel`.
fn post_check(dir: &Path, rel: &str) -> i32 {
    let payload = format!(
        r#"{{"session_id":"s","cwd":"{root}","hook_event_name":"PostToolUse","tool_name":"Edit",
            "tool_input":{{"file_path":"{root}/{rel}","old_string":"a","new_string":"b"}}}}"#,
        root = dir.display()
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .current_dir(dir)
        .arg("post-check")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn post-check");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(payload.as_bytes())
        .expect("write payload");
    child.wait().expect("wait").code().unwrap_or(-1)
}

fn graph_is_fresh(dir: &Path) -> bool {
    let out = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .current_dir(dir)
        .args(["graph", "status", "--path", "."])
        .output()
        .expect("run graph status");
    String::from_utf8_lossy(&out.stdout).contains("fresh")
}

fn graph_defines_function(dir: &Path, function_name: &str) -> bool {
    store::load(&store::graph_path(dir))
        .expect("load graph")
        .iter()
        .any(|edge| {
            edge.p == "defines_fn"
                && edge
                    .a
                    .get(1)
                    .is_some_and(|function| function.ends_with(function_name))
        })
}

#[test]
fn claude_post_check_updates_graph_from_an_absolute_path_with_only_pre_rules() {
    // The structural pack ships `phase: "pre"` rules exclusively, so a
    // project on `--packs structural` has no post rules at all. The graph
    // sensor is machinery, not a rule — gating it on rule phase means the
    // graph goes stale on the very first edit and the pack demotes itself to
    // warnings forever. Rejecting absolute host paths in `record_from_disk`
    // would also make this fail.
    let d = packaged_project();
    assert!(graph_is_fresh(d.path()), "precondition: freshly built");

    std::fs::write(
        d.path().join("src/risky.rs"),
        format!("{RISKY_SOURCE}\npub fn added_later() -> u32 {{ 7 }}\n"),
    )
    .expect("edit");
    assert_eq!(post_check(d.path(), "src/risky.rs"), 0, "post-check clean");

    assert!(
        graph_is_fresh(d.path()),
        "the post sensor must have recorded the edit"
    );
    assert!(
        graph_defines_function(d.path(), "::added_later"),
        "the sensor must extract the newly added function"
    );
}

/// Drive the real Codex adapter with an `apply_patch` PreToolUse payload.
fn codex_pre_patch(dir: &Path, rel: &str, body: &str) -> String {
    let patch = format!("*** Begin Patch\n*** Update File: {rel}\n+{body}\n*** End Patch");
    let payload = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "apply_patch",
        "tool_input": {"command": patch},
    })
    .to_string();
    let mut child = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .current_dir(dir)
        .args(["codex-hook", "PreToolUse"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn codex-hook");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(payload.as_bytes())
        .expect("write payload");
    let out = child.wait_with_output().expect("wait");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// Drive the real Codex adapter with an `apply_patch` PostToolUse payload.
fn codex_post_patch(dir: &Path, file_path: &Path, body: &str) -> String {
    let patch = format!(
        "*** Begin Patch\n*** Update File: {}\n+{body}\n*** End Patch",
        file_path.display()
    );
    let payload = serde_json::json!({
        "hook_event_name": "PostToolUse",
        "tool_name": "apply_patch",
        "tool_input": {"command": patch},
    })
    .to_string();
    let mut child = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .current_dir(dir)
        .args(["codex-hook", "PostToolUse"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn codex-hook");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(payload.as_bytes())
        .expect("write payload");
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "codex-hook must return structured output"
    );
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn the_structural_pack_fires_under_codex_too() {
    // The Codex pre-hook never hydrated the graph, so both shipped rules —
    // which open on `edited_file`, a fact asserted only inside `hydrate` —
    // joined against nothing. The pack looked installed and produced zero
    // warnings, while the post sensor dutifully maintained a graph that
    // nothing read.
    let d = packaged_project();
    let output = codex_pre_patch(d.path(), "src/risky.rs", "// touched");
    assert!(
        output.contains("rust:crate::risky::danger"),
        "structural rules must fire under Codex: {output}"
    );
}

#[cfg(unix)]
#[test]
fn codex_post_hook_updates_graph_from_a_symlinked_absolute_path() {
    // `project_root()` resolves the process cwd, while hosts can preserve a
    // symlink in the absolute path they send. Removing canonicalization from
    // `repo_relative`, rejecting absolute paths in `record_from_disk`, or
    // skipping the Codex post sensor would make this fail.
    let d = packaged_project();
    let links = TempDir::new().expect("symlink parent");
    let linked_root = links.path().join("linked-project");
    std::os::unix::fs::symlink(d.path(), &linked_root).expect("symlink project");
    let linked_file = linked_root.join("src/risky.rs");

    std::fs::write(
        d.path().join("src/risky.rs"),
        format!("{RISKY_SOURCE}\npub fn added_by_codex() -> u32 {{ 9 }}\n"),
    )
    .expect("edit");
    let output = codex_post_patch(
        d.path(),
        &linked_file,
        "pub fn added_by_codex() -> u32 { 9 }",
    );

    assert!(
        graph_is_fresh(d.path()),
        "the Codex post sensor must record the symlinked path: {output}"
    );
    assert!(
        graph_defines_function(d.path(), "::added_by_codex"),
        "the Codex post sensor must extract the newly added function: {output}"
    );
}
