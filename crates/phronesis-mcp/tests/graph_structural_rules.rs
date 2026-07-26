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
          {{"untested":["?func"]}}
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
    let payload = format!(
        r#"{{"session_id":"s","cwd":"{}","hook_event_name":"PreToolUse","tool_name":"Edit",
            "tool_input":{{"file_path":"src/risky.rs","old_string":"a","new_string":"b"}}}}"#,
        dir.display()
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
    assert!(stderr.contains("crate::risky::danger"), "{stderr}");
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
fn a_covered_function_produces_no_verdict() {
    // The negative case: add a direct test and the rule must go silent.
    let d = project("block");
    std::fs::create_dir_all(d.path().join("tests")).expect("mkdir tests");
    std::fs::write(
        d.path().join("tests/risky_test.rs"),
        "#[test]\nfn covers() { danger(vec![1]); }\n",
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
    assert!(stderr.contains("crate::risky::danger"), "{stderr}");
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
    assert!(stderr.contains("crate::a"), "{stderr}");
    assert!(
        !stderr.contains("Module `crate::b`"),
        "editing a.rs must not warn about b.rs: {stderr}"
    );
}
