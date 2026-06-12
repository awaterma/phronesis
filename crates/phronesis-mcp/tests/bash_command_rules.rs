//! `bash_command_matches` — regex predicate over Bash tool command text.
//!
//! The predicate fires only for command tools (Bash / run_shell_command):
//! a file edit whose *content* happens to contain the same text must not
//! trigger it. An invalid regex in a rule is skipped with a warning and
//! never blocks (rule-author typo must not brick the project).

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

fn run_hook_in(subcommand: &str, payload: &str, cwd: &Path) -> (i32, String) {
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

fn write_rules_file(dir: &Path, contents: &str) {
    let phronesis = dir.join(".phronesis");
    std::fs::create_dir_all(&phronesis).unwrap();
    std::fs::write(phronesis.join("rules.json"), contents).unwrap();
}

const GIT_ADD_GUARD: &str = r#"{
  "rules": [
    {
      "id": "llm-warn-git-add-all",
      "phase": "pre",
      "priority": 5,
      "when": [
        { "bash_command_matches": "(^|[;&|]\\s*)git\\s+add\\s+(-A\\b|\\.($|\\s))" }
      ],
      "then": { "warn": "Stage files explicitly — git add -A / git add . sweeps unrelated changes in." }
    }
  ]
}"#;

fn bash_payload(command: &str) -> String {
    format!(r#"{{"tool_name": "Bash", "tool_input": {{"command": "{command}"}}}}"#)
}

#[test]
fn pre_check_warns_on_matching_bash_command() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(dir.path(), GIT_ADD_GUARD);
    let (code, stderr) = run_hook_in("pre-check", &bash_payload("git add -A"), dir.path());
    assert_eq!(code, 1, "expected WARN; stderr: {stderr}");
    assert!(
        stderr.contains("Stage files explicitly"),
        "stderr: {stderr}"
    );
}

#[test]
fn pre_check_matches_command_after_chain_operator() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(dir.path(), GIT_ADD_GUARD);
    let (code, stderr) = run_hook_in(
        "pre-check",
        &bash_payload("cargo fmt && git add ."),
        dir.path(),
    );
    assert_eq!(code, 1, "expected WARN; stderr: {stderr}");
}

#[test]
fn pre_check_allows_explicit_git_add() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(dir.path(), GIT_ADD_GUARD);
    let (code, stderr) = run_hook_in(
        "pre-check",
        &bash_payload("git add src/lib.rs tests/it.rs"),
        dir.path(),
    );
    assert_eq!(code, 0, "explicit paths are fine; stderr: {stderr}");
}

#[test]
fn file_content_mentioning_pattern_does_not_fire_command_rule() {
    // The predicate is about the *command*, not file text: editing a doc
    // that quotes `git add -A` must pass.
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(dir.path(), GIT_ADD_GUARD);
    let payload = r#"{
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "docs/howto.md",
            "old_string": "old",
            "new_string": "Never run git add -A blindly."
        }
    }"#;
    let (code, stderr) = run_hook_in("pre-check", payload, dir.path());
    assert_eq!(
        code, 0,
        "file edits must not trip command rules; stderr: {stderr}"
    );
}

#[test]
fn invalid_regex_is_skipped_with_warning_not_block() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{
  "rules": [
    {
      "id": "broken-regex-rule",
      "phase": "pre",
      "priority": 5,
      "when": [ { "bash_command_matches": "([unclosed" } ],
      "then": { "warn": "never reached" }
    }
  ]
}"#,
    );
    let (code, stderr) = run_hook_in("pre-check", &bash_payload("git status"), dir.path());
    assert_eq!(
        code, 0,
        "author typo must not brick the hook; stderr: {stderr}"
    );
    assert!(
        stderr.contains("invalid bash_command_matches"),
        "author should see the typo surfaced; stderr: {stderr}"
    );
}

#[test]
fn post_check_also_warns_on_matching_bash_command() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        // Same rule but post-phase so the post hook loads it.
        r#"{
  "rules": [
    {
      "id": "llm-warn-git-add-all-post",
      "phase": "post",
      "priority": 5,
      "when": [
        { "bash_command_matches": "git\\s+add\\s+-A\\b" }
      ],
      "then": { "warn": "Stage files explicitly." }
    }
  ]
}"#,
    );
    let (code, stderr) = run_hook_in("post-check", &bash_payload("git add -A"), dir.path());
    assert_eq!(code, 1, "expected WARN; stderr: {stderr}");
}
