//! C18: the packaged commit-gate rules (`confidence-low-blocks-commit`,
//! `confidence-medium-warns-commit`, `nudge-verify-before-commit`,
//! `llm-warn-git-add-all`) gate on `bash_command_matches` regexes that only
//! recognized `git` immediately followed by its subcommand. Any of git's
//! global options between the binary and the subcommand (`-C <dir>`,
//! `-c <k=v>`, `--git-dir=…`, `--work-tree=…`, `--no-pager`, `--bare`, …), an
//! absolute/relative path to the `git` binary, a backslash-escaped binary
//! name, or a `command`/`env FOO=bar` wrapper let a governed Git mutation
//! bypass every gate silently.
//!
//! These tests drive the real `phr-mcp init` output (not a hand-copied
//! fixture) through the real `pre-check` binary, so a regression in either
//! the generated pattern or the shared regex helper it's built from shows
//! up here.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

fn run_hook(subcommand: &str, payload: &str, cwd: &Path) -> (i32, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_phr-mcp"));
    cmd.arg(subcommand)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("failed to spawn hook process");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .expect("write payload");
    let output = child.wait_with_output().expect("failed to wait");
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (output.status.code().unwrap_or(-1), stderr)
}

fn bash_payload(command: &str) -> String {
    let escaped = command.replace('\\', "\\\\").replace('"', "\\\"");
    format!(r#"{{"tool_name": "Bash", "tool_input": {{"command": "{escaped}"}}}}"#)
}

fn init_with_packs(dir: &Path, packs: &str) {
    let out = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(["init", "--packs", packs])
        .current_dir(dir)
        .output()
        .expect("spawn init");
    assert!(
        out.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Every one of these should be recognized as a `git commit` invocation by
/// the packaged gate rules — none of them mention "commit" as bare adjacent
/// text to `git`, but all of them are real ways a shell line reaches
/// `git commit`.
const COMMIT_BYPASS_FORMS: &[&str] = &[
    "git -C . commit -m x",
    "git -c user.name=x commit -m x",
    "git --git-dir=.git commit -m x",
    "git --no-pager commit -m x",
    "/usr/bin/git commit -m x",
    r"\git commit -m x",
    "command git commit -m x",
    "env FOO=bar git commit -m x",
];

/// These must never be treated as a `git commit` invocation.
const COMMIT_NEGATIVE_FORMS: &[&str] = &[
    "git log --grep commit",
    r#"echo "git commit""#,
    "gitk",
    "git commit-tree abc123",
];

#[test]
fn confidence_gate_fires_on_commit_bypass_forms() {
    let dir = tempfile::tempdir().unwrap();
    init_with_packs(dir.path(), "confidence");
    for form in COMMIT_BYPASS_FORMS {
        let (code, stderr) = run_hook("pre-check", &bash_payload(form), dir.path());
        assert_eq!(
            code, 1,
            "confidence gate must warn on bypass form `{form}` (no evidence -> low band); stderr: {stderr}"
        );
        assert!(
            stderr.contains("Low confidence"),
            "form `{form}` did not trip the low-confidence gate; stderr: {stderr}"
        );
    }
}

#[test]
fn confidence_gate_does_not_fire_on_commit_negative_forms() {
    let dir = tempfile::tempdir().unwrap();
    init_with_packs(dir.path(), "confidence");
    for form in COMMIT_NEGATIVE_FORMS {
        let (code, stderr) = run_hook("pre-check", &bash_payload(form), dir.path());
        assert_eq!(
            code, 0,
            "confidence gate must not fire on `{form}`; stderr: {stderr}"
        );
    }
}

/// `nudge-verify-before-commit` self-deactivates once `.phronesis/confidence.json`
/// exists (SPEC-pack-opt-in-facts) to avoid double-warning alongside the
/// confidence gate — and every non-`none` pack selection pulls in `base`,
/// which includes `confidence`, so there is no `init --packs` combination
/// that ships the nudge without also shipping (and thus silencing it via)
/// the confidence marker. To exercise the *real* generated nudge rule in
/// isolation, pull it out of a real `init --packs llm` run and replay it
/// alone against a project that never opted into confidence.
fn extract_rule(rules: &serde_json::Value, id: &str) -> serde_json::Value {
    rules["rules"]
        .as_array()
        .expect("rules array")
        .iter()
        .find(|r| r["id"] == id)
        .unwrap_or_else(|| panic!("rule `{id}` not found"))
        .clone()
}

fn write_single_rule(dir: &Path, rule: serde_json::Value) {
    let rules = serde_json::json!({"rules": [rule]});
    std::fs::create_dir_all(dir.join(".phronesis")).unwrap();
    std::fs::write(
        dir.join(".phronesis/rules.json"),
        serde_json::to_string_pretty(&rules).unwrap(),
    )
    .unwrap();
}

fn nudge_only_dir() -> tempfile::TempDir {
    let source = tempfile::tempdir().unwrap();
    init_with_packs(source.path(), "llm");
    let generated: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(source.path().join(".phronesis/rules.json")).unwrap(),
    )
    .unwrap();
    let nudge_rule = extract_rule(&generated, "nudge-verify-before-commit");

    // A project that never ran `init --packs confidence`: no
    // `.phronesis/confidence.json`, so the nudge's self-deactivation clause
    // stays false and it can fire.
    let dir = tempfile::tempdir().unwrap();
    init_with_packs(dir.path(), "none");
    assert!(!dir.path().join(".phronesis/confidence.json").exists());
    write_single_rule(dir.path(), nudge_rule);
    dir
}

#[test]
fn nudge_verify_before_commit_fires_on_commit_bypass_forms() {
    let dir = nudge_only_dir();
    for form in COMMIT_BYPASS_FORMS {
        let (code, stderr) = run_hook("pre-check", &bash_payload(form), dir.path());
        assert_eq!(
            code, 1,
            "nudge-verify-before-commit must warn on bypass form `{form}`; stderr: {stderr}"
        );
        assert!(
            stderr.contains("About to commit"),
            "form `{form}` did not trip nudge-verify-before-commit; stderr: {stderr}"
        );
    }
}

#[test]
fn nudge_verify_before_commit_does_not_fire_on_commit_negative_forms() {
    let dir = nudge_only_dir();
    for form in COMMIT_NEGATIVE_FORMS {
        let (code, stderr) = run_hook("pre-check", &bash_payload(form), dir.path());
        assert_eq!(
            code, 0,
            "nudge-verify-before-commit must not fire on `{form}`; stderr: {stderr}"
        );
    }
}

/// `llm-warn-git-add-all` (staging-hygiene warning) has the same global-
/// option bypass shape as the commit gates.
const ADD_ALL_BYPASS_FORMS: &[&str] = &[
    "git -C . add -A",
    "git --no-pager add .",
    "/usr/bin/git add -A",
];

#[test]
fn git_add_all_warning_fires_on_bypass_forms() {
    let dir = tempfile::tempdir().unwrap();
    init_with_packs(dir.path(), "llm");
    for form in ADD_ALL_BYPASS_FORMS {
        let (code, stderr) = run_hook("pre-check", &bash_payload(form), dir.path());
        assert_eq!(
            code, 1,
            "llm-warn-git-add-all must warn on bypass form `{form}`; stderr: {stderr}"
        );
        assert!(
            stderr.contains("Stage files explicitly"),
            "form `{form}` did not trip llm-warn-git-add-all; stderr: {stderr}"
        );
    }
}
