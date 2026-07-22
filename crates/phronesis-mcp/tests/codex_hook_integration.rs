use std::fs;
use std::io::Write as _;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

fn run_hook(root: &std::path::Path, payload: &Value) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .arg("codex-hook")
        .env("PHRONESIS_PROJECT_ROOT", root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn phr-mcp codex-hook");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(payload.to_string().as_bytes())
        .expect("write payload");
    child.wait_with_output().expect("wait for hook")
}

fn run_raw_hook(root: &std::path::Path, event: &str, raw: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(["codex-hook", event])
        .env("PHRONESIS_PROJECT_ROOT", root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn raw hook");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(raw)
        .expect("write raw payload");
    child.wait_with_output().expect("wait for raw hook")
}

fn response(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("hook stdout is JSON")
}

fn fixture_payload(raw: &str) -> Value {
    serde_json::from_str::<Value>(raw).expect("fixture JSON")["payload"].clone()
}

fn write_rules(root: &std::path::Path, rules: Value) {
    fs::create_dir_all(root.join(".phronesis")).expect("create config dir");
    fs::write(
        root.join(".phronesis/rules.json"),
        serde_json::to_vec_pretty(&rules).expect("serialize rules"),
    )
    .expect("write rules");
}

fn block_rule() -> Value {
    json!({"version": 2, "rules": [{
        "id": "block-unwrap", "phase": "pre", "priority": 10,
        "when": [{"new_content_contains": ".unwrap()"}],
        "then": {"block": "unwrap is blocked"}
    }]})
}

fn warning_rule() -> Value {
    json!({"version": 2, "rules": [{
        "id": "warn-cargo", "phase": "pre", "priority": 10,
        "when": [{"new_content_contains": "cargo test"}],
        "then": {"warn": "workspace test advised"}
    }]})
}

#[test]
fn codex_hook_cli_decodes_current_pretooluse_and_denies() {
    let project = tempfile::tempdir().expect("temp project");
    write_rules(project.path(), block_rule());
    let payload = fixture_payload(include_str!(
        "fixtures/payloads/codex/pre-bash-unwrap-with-deny.json"
    ));

    let output = run_hook(project.path(), &payload);
    let body = response(&output);
    assert_eq!(body["hookSpecificOutput"]["hookEventName"], "PreToolUse");
    assert_eq!(body["hookSpecificOutput"]["permissionDecision"], "deny");
}

#[test]
fn codex_hook_cli_warning_is_advisory_and_clean_is_empty() {
    let project = tempfile::tempdir().expect("temp project");
    write_rules(project.path(), warning_rule());
    let warning = fixture_payload(include_str!("fixtures/payloads/codex/pre-bash-clean.json"));
    let warning_output = run_hook(project.path(), &warning);
    assert!(warning_output.status.success());
    let warning_body = response(&warning_output);
    assert_eq!(
        warning_body["hookSpecificOutput"]["hookEventName"],
        "PreToolUse"
    );
    assert!(warning_body["hookSpecificOutput"]["additionalContext"].is_string());
    assert!(
        warning_body["hookSpecificOutput"]
            .get("permissionDecision")
            .is_none()
    );

    write_rules(project.path(), json!({"version": 2, "rules": []}));
    assert_eq!(response(&run_hook(project.path(), &warning)), json!({}));
}

#[test]
fn codex_hook_cli_apply_patch_denies_violation_and_unsafe_paths() {
    let project = tempfile::tempdir().expect("temp project");
    write_rules(project.path(), block_rule());
    for fixture in [
        include_str!("fixtures/payloads/codex/pre-patch-unwrap.json"),
        include_str!("fixtures/payloads/codex/pre-patch-traversal.json"),
        include_str!("fixtures/payloads/codex/pre-patch-absolute-path.json"),
    ] {
        let payload = fixture_payload(fixture);
        assert_eq!(
            response(&run_hook(project.path(), &payload))["hookSpecificOutput"]["permissionDecision"],
            "deny"
        );
    }
}

#[test]
fn codex_hook_cli_multi_file_patch_combines_decisions() {
    let project = tempfile::tempdir().expect("temp project");
    write_rules(project.path(), block_rule());
    let payload = json!({
        "hook_event_name": "PreToolUse", "tool_name": "apply_patch",
        "session_id": "s", "turn_id": "t", "tool_use_id": "u",
        "tool_input": {"command": "*** Begin Patch\n*** Add File: src/clean.rs\n+pub fn clean() {}\n*** Add File: src/bad.rs\n+pub fn bad() { thing.unwrap(); }\n*** End Patch\n"}
    });
    assert_eq!(
        response(&run_hook(project.path(), &payload))["hookSpecificOutput"]["permissionDecision"],
        "deny"
    );
}

#[test]
fn codex_hook_cli_posttooluse_captures_output_and_journals_executed_call() {
    let project = tempfile::tempdir().expect("temp project");
    let payload = fixture_payload(include_str!(
        "fixtures/payloads/codex/post-bash-cargo-test.json"
    ));
    let output = run_hook(project.path(), &payload);
    assert!(output.status.success());
    assert_eq!(response(&output), json!({}));

    let journal = fs::read_to_string(project.path().join(".phronesis/journey/events.jsonl"))
        .expect("executed call journal");
    let record: Value =
        serde_json::from_str(journal.lines().last().expect("journal line")).expect("journal JSON");
    assert_eq!(record["sid"], "codex-s-004");
    assert_eq!(record["tool"], "Bash");

    let log = fs::read_to_string(project.path().join(".phronesis/log.jsonl")).expect("action log");
    let entry: Value =
        serde_json::from_str(log.lines().last().expect("log line")).expect("log JSON");
    assert_eq!(entry["host"], "codex");
    assert_eq!(entry["tool_use_id"], "codex-uid-004");
}

#[test]
fn codex_hook_cli_post_patch_journals_each_file() {
    let project = tempfile::tempdir().expect("temp project");
    let payload = json!({
        "hook_event_name": "PostToolUse", "tool_name": "apply_patch",
        "session_id": "patch-session", "turn_id": "t", "tool_use_id": "u",
        "tool_input": {"command": "*** Begin Patch\n*** Add File: src/a.rs\n+pub fn a() {}\n*** Add File: tests/a.rs\n+#[test] fn a() {}\n*** End Patch\n"},
        "tool_response": {"output": "Done!"}
    });
    assert_eq!(response(&run_hook(project.path(), &payload)), json!({}));
    let journal = fs::read_to_string(project.path().join(".phronesis/journey/events.jsonl"))
        .expect("patch journals");
    let paths: Vec<String> = journal
        .lines()
        .map(|line| {
            serde_json::from_str::<Value>(line).expect("record")["path"]
                .as_str()
                .expect("path")
                .to_string()
        })
        .collect();
    assert_eq!(paths, ["src/a.rs", "tests/a.rs"]);
}

#[test]
fn codex_hook_cli_unsupported_tool_is_safe_noop() {
    let project = tempfile::tempdir().expect("temp project");
    let payload = fixture_payload(include_str!(
        "fixtures/payloads/codex/unsupported-tool.json"
    ));
    let output = run_hook(project.path(), &payload);
    assert!(output.status.success());
    assert_eq!(response(&output), json!({}));
}

#[test]
fn codex_hook_cli_malformed_payload_fails_closed_pre_and_advises_post() {
    let project = tempfile::tempdir().expect("temp project");
    let pre = response(&run_raw_hook(project.path(), "PreToolUse", b"not-json"));
    assert_eq!(pre["hookSpecificOutput"]["permissionDecision"], "deny");
    let post = response(&run_raw_hook(project.path(), "PostToolUse", b"not-json"));
    assert!(
        post["systemMessage"]
            .as_str()
            .is_some_and(|s| s.contains("invalid"))
    );
    assert!(post.get("continue").is_none());
}

#[test]
fn codex_hook_cli_context_uses_pascal_event_and_is_bounded() {
    let project = tempfile::tempdir().expect("temp project");
    write_rules(project.path(), warning_rule());
    fs::write(
        project.path().join(".phronesis/durable.md"),
        "important durable guidance\n".repeat(20_000),
    )
    .expect("write durable context");
    for event in [
        "SessionStart",
        "UserPromptSubmit",
        "PreCompact",
        "PostCompact",
        "SubagentStart",
    ] {
        let payload = json!({"hook_event_name": event, "session_id": "s", "turn_id": "t"});
        let body = response(&run_hook(project.path(), &payload));
        assert_eq!(body["hookSpecificOutput"]["hookEventName"], event);
        let context = body["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .expect("context string");
        assert!(context.len() <= phronesis_mcp::context::DEFAULT_MAX_BYTES);
    }
}

#[test]
fn init_merges_codex_hooks_and_mcp_idempotently_and_dry_run_is_read_only() {
    let project = tempfile::tempdir().expect("temp project");
    fs::create_dir_all(project.path().join(".codex")).expect("codex dir");
    fs::write(
        project.path().join(".codex/hooks.json"),
        r#"{"description":"keep me","hooks":{"Stop":[{"hooks":[{"type":"command","command":"other"}]}]}}"#,
    )
    .expect("existing hooks");
    fs::write(
        project.path().join(".codex/config.toml"),
        "model = \"keep-me\"\n",
    )
    .expect("existing config");

    let run_init = || {
        Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
            .args(["init", "--hooks-only"])
            .current_dir(project.path())
            .output()
            .expect("run init")
    };
    assert!(run_init().status.success());
    let first_hooks =
        fs::read_to_string(project.path().join(".codex/hooks.json")).expect("merged hooks");
    let first_config =
        fs::read_to_string(project.path().join(".codex/config.toml")).expect("merged config");
    let hooks: Value = serde_json::from_str(&first_hooks).expect("hooks JSON");
    assert_eq!(hooks["description"], "keep me");
    assert!(hooks["hooks"]["Stop"].is_array());
    assert!(hooks["hooks"]["PreToolUse"].is_array());
    assert!(first_config.contains("model = \"keep-me\""));
    assert!(first_config.contains("[mcp_servers.phronesis]"));

    assert!(run_init().status.success());
    assert_eq!(
        fs::read_to_string(project.path().join(".codex/hooks.json")).expect("hooks again"),
        first_hooks
    );
    assert_eq!(
        fs::read_to_string(project.path().join(".codex/config.toml")).expect("config again"),
        first_config
    );

    let dry = tempfile::tempdir().expect("dry project");
    let output = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(["init", "--hooks-only", "--dry-run"])
        .current_dir(dry.path())
        .output()
        .expect("dry init");
    assert!(output.status.success());
    assert!(!dry.path().join(".codex").exists());
}
