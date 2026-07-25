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

fn write_predicate_provider(root: &std::path::Path, name: &str, script: &str) {
    let predicates = root.join(".phronesis/predicates");
    fs::create_dir_all(&predicates).expect("predicate provider dir");
    fs::write(predicates.join(name), script).expect("predicate provider");
}

#[test]
fn codex_hook_uses_rhai_provider_predicates() {
    let project = tempfile::tempdir().expect("temp project");
    write_rules(
        project.path(),
        json!({"version": 2, "rules": [{
            "id": "release-command", "phase": "pre", "priority": 10,
            "when": [{"release_attempted": "?command"}],
            "then": {"block": "Release policy applies to ?command"}
        }]}),
    );
    write_predicate_provider(
        project.path(),
        "release.rhai",
        r#"
            if event.tool_name == "Bash" && event.command.starts_with("cargo publish") {
                emit_fact("release_attempted", [event.command]);
            }
        "#,
    );
    let payload = json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "session_id": "s",
        "turn_id": "t",
        "tool_use_id": "u",
        "tool_input": {"command": "cargo publish --dry-run"}
    });

    let body = response(&run_hook(project.path(), &payload));
    assert_eq!(
        body["hookSpecificOutput"]["permissionDecision"], "deny",
        "Codex must consume provider-derived predicates: {body}"
    );
    assert!(
        body["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .is_some_and(|reason| reason.contains("cargo publish --dry-run"))
    );
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
fn codex_hook_cli_nonzero_exit_grounds_a_failed_outcome() {
    let project = tempfile::tempdir().expect("temp project");
    fs::create_dir_all(project.path().join(".phronesis")).expect("confidence config dir");
    fs::write(project.path().join(".phronesis/confidence.json"), "{}").expect("enable confidence");
    let payload = json!({
        "hook_event_name": "PostToolUse",
        "tool_name": "Bash",
        "session_id": "failed-session",
        "turn_id": "failed-turn",
        "tool_use_id": "failed-tool",
        "tool_input": {"command": "cargo test --workspace"},
        "tool_response": {
            "stdout": "",
            "stderr": "command exited without a parseable cargo diagnostic",
            "exit_code": 101
        }
    });

    assert_eq!(response(&run_hook(project.path(), &payload)), json!({}));
    let journal = fs::read_to_string(project.path().join(".phronesis/journey/events.jsonl"))
        .expect("failed command journal");
    let record: Value =
        serde_json::from_str(journal.lines().last().expect("journal line")).expect("journal JSON");

    assert_eq!(record["command_exit"], 101);
    assert!(
        record["tags"]
            .as_array()
            .expect("tags")
            .iter()
            .any(|tag| tag == "outcome:compile_error"),
        "a nonzero Codex command exit must ground a failed outcome: {record}"
    );
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
fn codex_stop_blocks_low_confidence_but_is_inert_without_an_open_work_unit() {
    let project = tempfile::tempdir().expect("temp project");
    fs::create_dir_all(project.path().join(".phronesis/outcomes")).expect("outcomes dir");
    fs::write(project.path().join(".phronesis/confidence.json"), "{}").expect("enable confidence");
    let stop = json!({
        "hook_event_name": "Stop",
        "session_id": "s",
        "turn_id": "t"
    });

    assert_eq!(response(&run_hook(project.path(), &stop)), json!({}));

    fs::write(
        project.path().join(".phronesis/outcomes/current"),
        "active-unit",
    )
    .expect("open work unit");
    let blocked = response(&run_hook(project.path(), &stop));
    assert_eq!(blocked["continue"], false);
    assert!(
        blocked["stopReason"]
            .as_str()
            .is_some_and(|s| s.contains("Low confidence"))
    );
    assert!(blocked["systemMessage"].is_string());

    let mut subagent_stop = stop.clone();
    subagent_stop["hook_event_name"] = json!("SubagentStop");
    assert_eq!(
        response(&run_hook(project.path(), &subagent_stop))["continue"],
        false
    );

    let journal_dir = project.path().join(".phronesis/journey");
    fs::create_dir_all(&journal_dir).expect("journey dir");
    let records = [
        ("outcome:compile_ok", 1),
        ("outcome:test_pass", 2),
        ("outcome:bug_caught:known", 3),
    ]
    .map(|(tag, seq)| {
        json!({
            "v": 1, "ts": seq, "sid": "s", "seq": seq, "tool": "Bash",
            "path": "", "tags": [tag], "subject": "active-unit"
        })
        .to_string()
    });
    fs::write(
        journal_dir.join("events.jsonl"),
        format!("{}\n", records[..2].join("\n")),
    )
    .expect("medium-confidence journal");
    let medium = response(&run_hook(project.path(), &stop));
    assert!(medium.get("continue").is_none());
    assert!(
        medium["systemMessage"]
            .as_str()
            .is_some_and(|s| s.contains("Medium confidence"))
    );

    fs::write(
        journal_dir.join("events.jsonl"),
        format!("{}\n", records.join("\n")),
    )
    .expect("high-confidence journal");
    assert_eq!(response(&run_hook(project.path(), &stop)), json!({}));
}

#[test]
fn init_merges_codex_hooks_and_mcp_idempotently_and_dry_run_is_read_only() {
    let project = tempfile::tempdir().expect("temp project");
    fs::create_dir_all(project.path().join(".codex")).expect("codex dir");
    fs::write(
        project.path().join(".codex/hooks.json"),
        r#"{
          "description":"keep me",
          "hooks":{
            "Stop":[{"matcher":"","hooks":[{"type":"command","command":"other"}]}],
            "SessionStart":[{"matcher":"","hooks":[{"type":"command","command":"phr-mcp codex-hook SessionStart"}]}],
            "PreCompact":[{"matcher":"","hooks":[{"type":"command","command":"phr-mcp codex-hook PreCompact"}]}]
          }
        }"#,
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
    assert!(
        hooks["hooks"]["Stop"]
            .as_array()
            .expect("Stop hooks")
            .iter()
            .any(|entry| entry["hooks"][0]["command"] == "other")
    );
    assert!(hooks["hooks"]["PreToolUse"].is_array());
    let session = hooks["hooks"]["SessionStart"]
        .as_array()
        .expect("SessionStart hooks");
    assert!(session.iter().any(|entry| {
        entry["matcher"] == "startup|resume|clear"
            && entry["hooks"][0]["command"] == "phr-mcp codex-hook SessionStart"
    }));
    assert_eq!(
        session
            .iter()
            .filter(|entry| { entry["hooks"][0]["command"] == "phr-mcp codex-hook SessionStart" })
            .count(),
        1,
        "legacy Phronesis SessionStart entry must be migrated, not duplicated"
    );
    for event in ["PreCompact", "PostCompact"] {
        assert!(
            hooks["hooks"][event]
                .as_array()
                .expect("compact hooks")
                .iter()
                .any(|entry| entry["matcher"] == "manual|auto"),
            "{event} must use the documented compact matcher"
        );
    }
    for event in ["Stop", "SubagentStop"] {
        assert!(
            hooks["hooks"][event]
                .as_array()
                .expect("completion hooks")
                .iter()
                .any(|entry| {
                    entry["hooks"][0]["command"] == format!("phr-mcp codex-hook {event}")
                }),
            "{event} completion gate must be wired"
        );
    }
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
