//! The hook decision pipeline must never fail open, and never block without
//! saying why in the audit trail. Each test drives the built binary through
//! `pre-check` and, where the host has the same path, `codex-hook PreToolUse`.
//!
//! - A firing block rule always blocks, even when its message starts with `?`
//!   or names a variable no condition binds (the variable renders literally).
//! - Hook-generated fact ids are collision-free, so two rules whose patterns
//!   differ only in punctuation cannot collide into a spurious block.
//! - Every exit-2 / deny path writes a log entry naming its reason.

use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

fn spawn(root: &Path, args: &[&str], payload: &Value) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(args)
        .env("PHRONESIS_PROJECT_ROOT", root)
        .env_remove("PHRONESIS_NO_ACTION_LOG")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn phr-mcp");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(payload.to_string().as_bytes())
        .expect("write payload");
    child.wait_with_output().expect("wait for phr-mcp")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

fn setup_raw(rules: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp project");
    let phr = dir.path().join(".phronesis");
    std::fs::create_dir_all(&phr).expect("config dir");
    std::fs::write(phr.join("rules.json"), rules).expect("write rules");
    dir
}

fn setup(rules: &Value) -> tempfile::TempDir {
    setup_raw(&serde_json::to_string_pretty(rules).expect("rules JSON"))
}

fn claude_bash(command: &str) -> Value {
    json!({"tool_name": "Bash", "tool_input": {"command": command}})
}

fn claude_write(path: &str, content: &str) -> Value {
    json!({"tool_name": "Write", "tool_input": {"file_path": path, "content": content}})
}

fn codex_bash(command: &str) -> Value {
    json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "session_id": "codex-pipeline",
        "turn_id": "codex-pipeline-turn",
        "tool_use_id": "codex-pipeline-uid",
        "tool_input": {"command": command},
    })
}

fn codex_patch(path: &str, line: &str) -> Value {
    json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "apply_patch",
        "session_id": "codex-pipeline",
        "turn_id": "codex-pipeline-turn",
        "tool_use_id": "codex-pipeline-uid",
        "tool_input": {"command": format!(
            "*** Begin Patch\n*** Add File: {path}\n+{line}\n*** End Patch\n"
        )},
    })
}

/// `(permissionDecision, reason)` from a Codex PreToolUse response.
fn codex_decision(output: &Output) -> (String, String) {
    let body: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "codex-hook stdout must be JSON ({e}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    });
    let hso = &body["hookSpecificOutput"];
    (
        hso["permissionDecision"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        hso["permissionDecisionReason"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    )
}

fn log_entries(root: &Path) -> Vec<Value> {
    let path = root.join(".phronesis/log.jsonl");
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// The one hook entry with `exit: 2`, asserting there is exactly one.
fn only_block_entry(root: &Path) -> Value {
    let blocks: Vec<Value> = log_entries(root)
        .into_iter()
        .filter(|e| e["kind"] == "hook" && e["exit"] == 2)
        .collect();
    assert_eq!(
        blocks.len(),
        1,
        "exactly one exit-2 entry expected: {blocks:#?}"
    );
    blocks.into_iter().next().unwrap_or_default()
}

fn force_push_rule(message: &str) -> Value {
    json!({"rules": [{
        "id": "no-force-push",
        "phase": "pre",
        "priority": 10,
        "when": [{"bash_command_matches": "git push --force"}],
        "then": {"block": message}
    }]})
}

// --- C2: a firing block rule always blocks ---------------------------------

#[test]
fn block_message_starting_with_unbound_variable_still_blocks() {
    let dir = setup(&force_push_rule("?reason: force-pushing rewrites history"));
    let out = spawn(
        dir.path(),
        &["pre-check"],
        &claude_bash("git push --force origin main"),
    );
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(2), "must block; stderr: {err}");
    assert!(
        err.contains("BLOCKED — ?reason: force-pushing rewrites history"),
        "unbound variable renders literally: {err}"
    );
    assert!(
        err.contains("no-force-push") && err.contains("`?reason`") && err.contains("unbound"),
        "stderr names the rule and the unbound variable: {err}"
    );
}

#[test]
fn block_message_that_is_literal_question_text_still_blocks() {
    let dir = setup(&force_push_rule("?? are you sure — force push is blocked"));
    let out = spawn(
        dir.path(),
        &["pre-check"],
        &claude_bash("git push --force origin main"),
    );
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(2), "must block; stderr: {err}");
    assert!(
        err.contains("BLOCKED — ?? are you sure — force push is blocked"),
        "{err}"
    );
    assert!(
        !err.contains("unbound"),
        "`??` is text, not a variable: {err}"
    );
}

#[test]
fn codex_denies_when_block_message_starts_with_unbound_variable() {
    let dir = setup(&force_push_rule("?reason: force-pushing rewrites history"));
    let out = spawn(
        dir.path(),
        &["codex-hook", "PreToolUse"],
        &codex_bash("git push --force origin main"),
    );
    let (decision, reason) = codex_decision(&out);
    assert_eq!(decision, "deny", "stderr: {}", stderr(&out));
    assert!(reason.contains("?reason: force-pushing"), "{reason}");
}

#[test]
fn unbound_action_variable_is_flagged_at_load_even_when_the_rule_does_not_fire() {
    let dir = setup(&force_push_rule("Blocked for ?who"));
    let out = spawn(dir.path(), &["pre-check"], &claude_bash("ls"));
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(0), "rule did not fire: {err}");
    assert!(
        err.contains("no-force-push") && err.contains("`?who`") && err.contains("unbound"),
        "load-time diagnostic names the rule and variable: {err}"
    );
}

#[test]
fn bound_variables_raise_no_load_diagnostic() {
    let dir = setup(&json!({"rules": [{
        "id": "path-rule",
        "phase": "pre",
        "priority": 1,
        "when": [{"file_path_matches": "?seg"}, {"file_path_matches": "secret"}],
        "then": {"warn": "touches ?seg"}
    }]}));
    let out = spawn(dir.path(), &["pre-check"], &claude_bash("ls"));
    let err = stderr(&out);
    assert!(!err.contains("unbound"), "{err}");
}

// --- C3: collision-free fact ids -------------------------------------------

fn punctuation_twins() -> Value {
    json!({"rules": [
        {"id": "warn-dot", "phase": "pre", "priority": 1,
         "when": [{"new_content_contains": "a.b"}], "then": {"warn": "dot form"}},
        {"id": "warn-dash", "phase": "pre", "priority": 1,
         "when": [{"new_content_contains": "a-b"}], "then": {"warn": "dash form"}},
        {"id": "warn-dot-again", "phase": "pre", "priority": 1,
         "when": [{"new_content_contains": "a.b"}], "then": {"warn": "dot form again"}}
    ]})
}

#[test]
fn patterns_that_sanitize_alike_warn_instead_of_blocking() {
    let dir = setup(&punctuation_twins());
    let out = spawn(
        dir.path(),
        &["pre-check"],
        &claude_write("notes.txt", "a.b and a-b"),
    );
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(1), "warn, never block: {err}");
    assert!(!err.contains("duplicate fact id"), "{err}");
    for msg in ["dot form", "dash form", "dot form again"] {
        assert!(err.contains(msg), "missing `{msg}`: {err}");
    }
}

#[test]
fn codex_patterns_that_sanitize_alike_do_not_deny() {
    let dir = setup(&punctuation_twins());
    let bash = spawn(
        dir.path(),
        &["codex-hook", "PreToolUse"],
        &codex_bash("echo a.b a-b"),
    );
    let (decision, reason) = codex_decision(&bash);
    assert_ne!(decision, "deny", "{reason}; stderr: {}", stderr(&bash));
    let patch = spawn(
        dir.path(),
        &["codex-hook", "PreToolUse"],
        &codex_patch("notes.txt", "a.b and a-b"),
    );
    let (decision, reason) = codex_decision(&patch);
    assert_ne!(decision, "deny", "{reason}; stderr: {}", stderr(&patch));
}

// --- C4: every block is logged with its reason ------------------------------

#[test]
fn rule_block_is_logged_with_rule_and_message() {
    let dir = setup(&force_push_rule("no force push"));
    let out = spawn(
        dir.path(),
        &["pre-check"],
        &claude_bash("git push --force origin main"),
    );
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let entry = only_block_entry(dir.path());
    let reasons = entry["blocked_by"].as_array().cloned().unwrap_or_default();
    assert!(
        reasons.iter().any(|r| r["kind"] == "rule"
            && r["rule"] == "no-force-push"
            && r["message"] == "no force push"),
        "{entry:#}"
    );
}

#[test]
fn fail_closed_load_error_is_logged() {
    let dir = setup_raw("{not json");
    let out = spawn(dir.path(), &["pre-check"], &claude_bash("ls"));
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let entry = only_block_entry(dir.path());
    let reasons = entry["blocked_by"].as_array().cloned().unwrap_or_default();
    assert!(
        reasons.iter().any(|r| r["kind"] == "fail_closed"
            && r["message"]
                .as_str()
                .is_some_and(|m| m.contains("rules file"))),
        "{entry:#}"
    );
}

#[test]
fn fail_closed_provider_error_is_logged() {
    let dir = setup(&force_push_rule("no force push"));
    let predicates = dir.path().join(".phronesis/predicates");
    std::fs::create_dir_all(&predicates).expect("predicates dir");
    std::fs::write(predicates.join("broken.rhai"), "this is not rhai (((").expect("provider");
    let out = spawn(dir.path(), &["pre-check"], &claude_bash("ls"));
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let entry = only_block_entry(dir.path());
    assert_eq!(entry["event"], "pre_check", "{entry:#}");
    let reasons = entry["blocked_by"].as_array().cloned().unwrap_or_default();
    assert!(
        reasons
            .iter()
            .any(|r| r["kind"] == "fail_closed" && r["message"].as_str().is_some()),
        "{entry:#}"
    );
}

#[test]
fn codex_fail_closed_denies_are_logged() {
    let dir = setup_raw("{not json");
    for payload in [codex_bash("ls"), codex_patch("src/a.rs", "pub fn a() {}")] {
        let out = spawn(dir.path(), &["codex-hook", "PreToolUse"], &payload);
        let (decision, _) = codex_decision(&out);
        assert_eq!(decision, "deny", "stderr: {}", stderr(&out));
    }
    let blocks: Vec<Value> = log_entries(dir.path())
        .into_iter()
        .filter(|e| e["kind"] == "hook" && e["exit"] == 2)
        .collect();
    assert_eq!(blocks.len(), 2, "{blocks:#?}");
    for entry in &blocks {
        assert_eq!(entry["host"], "codex", "{entry:#}");
        let reasons = entry["blocked_by"].as_array().cloned().unwrap_or_default();
        assert!(
            reasons.iter().any(|r| r["kind"] == "fail_closed"),
            "{entry:#}"
        );
    }
}

#[test]
fn codex_rule_deny_is_logged_with_rule() {
    let dir = setup(&force_push_rule("no force push"));
    let out = spawn(
        dir.path(),
        &["codex-hook", "PreToolUse"],
        &codex_bash("git push --force origin main"),
    );
    let (decision, _) = codex_decision(&out);
    assert_eq!(decision, "deny", "stderr: {}", stderr(&out));
    let entry = only_block_entry(dir.path());
    let reasons = entry["blocked_by"].as_array().cloned().unwrap_or_default();
    assert!(
        reasons
            .iter()
            .any(|r| r["kind"] == "rule" && r["rule"] == "no-force-push"),
        "{entry:#}"
    );
}

/// No shipped pack rule names a variable its conditions leave unbound, so
/// the load-time diagnostic stays silent on a stock install.
#[test]
fn shipped_packs_raise_no_unbound_variable_diagnostic() {
    let dir = tempfile::tempdir().expect("temp project");
    let init = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args([
            "init",
            "--packs",
            "llm,rust,rhai,python,python-patterns,typescript,swift,lua,cue,json,yaml,helm3",
        ])
        .current_dir(dir.path())
        .env("PHRONESIS_PROJECT_ROOT", dir.path())
        .output()
        .expect("run init");
    assert!(init.status.success(), "{}", stderr(&init));
    for phase in ["pre-check", "post-check"] {
        let out = spawn(dir.path(), &[phase], &claude_bash("ls"));
        let err = stderr(&out);
        assert!(!err.contains("unbound"), "{phase}: {err}");
    }
}
