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
