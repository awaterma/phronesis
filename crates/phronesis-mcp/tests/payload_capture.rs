use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

fn run_hook_with_env(subcommand: &str, payload: &str, envs: &[(&str, &str)]) -> i32 {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_phr-mcp"));
    cmd.arg(subcommand)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("spawn hook");
    let mut stdin = child.stdin.take().expect("stdin handle");
    stdin.write_all(payload.as_bytes()).expect("write payload");
    drop(stdin);
    let output = child.wait_with_output().expect("wait");
    output.status.code().unwrap_or(-1)
}

fn read_capture(dir: &Path) -> Vec<serde_json::Value> {
    let raw = std::fs::read_to_string(dir.join("payloads.jsonl")).unwrap_or_default();
    raw.lines()
        .map(|l| serde_json::from_str(l).expect("capture line is JSON"))
        .collect()
}

#[test]
fn capture_dir_set_tees_raw_payload() {
    let dir = tempfile::tempdir().expect("tempdir");
    let payload =
        r#"{"tool_name": "Read", "tool_input": {"file_path": "src/main.rs"}, "session_id": "abc"}"#;
    let code = run_hook_with_env(
        "pre-check",
        payload,
        &[("PHRONESIS_CAPTURE_DIR", dir.path().to_str().expect("utf8"))],
    );
    assert_eq!(code, 0, "capture must not change hook behavior");

    let records = read_capture(dir.path());
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["phase"], "pre");
    // Raw payload preserved verbatim, including fields HookPayload ignores.
    assert_eq!(records[0]["raw"]["session_id"], "abc");
    assert_eq!(records[0]["raw"]["tool_name"], "Read");
}

#[test]
fn capture_appends_across_invocations_and_stamps_phase() {
    let dir = tempfile::tempdir().expect("tempdir");
    let envs = [("PHRONESIS_CAPTURE_DIR", dir.path().to_str().expect("utf8"))];
    run_hook_with_env(
        "pre-check",
        r#"{"tool_name": "Read", "tool_input": {}}"#,
        &envs,
    );
    run_hook_with_env(
        "post-check",
        r#"{"tool_name": "Read", "tool_input": {}}"#,
        &envs,
    );

    let records = read_capture(dir.path());
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["phase"], "pre");
    assert_eq!(records[1]["phase"], "post");
}

#[test]
fn capture_unset_writes_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    run_hook_with_env(
        "pre-check",
        r#"{"tool_name": "Read", "tool_input": {}}"#,
        &[],
    );
    assert!(
        !dir.path().join("payloads.jsonl").exists(),
        "no capture file without the env var"
    );
}

#[test]
fn capture_skips_non_json_stdin_entirely() {
    let dir = tempfile::tempdir().expect("tempdir");
    // Malformed payload: the hook still fails closed (exit 2), but the capture
    // cannot redact what it cannot parse, so it writes nothing rather than a
    // verbatim copy of what may be truncated free text.
    let code = run_hook_with_env(
        "pre-check",
        "not json at all",
        &[("PHRONESIS_CAPTURE_DIR", dir.path().to_str().expect("utf8"))],
    );
    let records = read_capture(dir.path());
    assert!(records.is_empty(), "non-JSON stdin is never captured");
    assert_eq!(code, 2, "pre-check fails closed on malformed JSON");
}

#[test]
fn capture_redacts_every_free_text_key_at_any_depth() {
    let raw = r#"{"hook_event_name":"UserPromptSubmit","session_id":"s","prompt":"top secret words","tool_name":"Bash","tool_input":{"command":"ls"}}"#;
    let out = phronesis_mcp::hook::redact_for_capture(raw).expect("valid JSON is captured");
    assert!(!out.contains("top secret"));
    assert!(out.contains(r#""prompt":"<redacted:16 bytes>""#));
    assert!(out.contains(r#""tool_input":{"command":"ls"}"#));

    // Nested: Gemini's invoke_agent carries the whole sub-agent task under
    // `tool_input.prompt`, which a top-level-only redaction would miss.
    let nested = r#"{"hook_event_name":"BeforeTool","tool_name":"invoke_agent","tool_input":{"agent_name":"reviewer","prompt":"review the auth module"}}"#;
    let out = phronesis_mcp::hook::redact_for_capture(nested).expect("valid JSON");
    assert!(!out.contains("review the auth"), "{out}");
    assert!(out.contains(r#""prompt":"<redacted:22 bytes>""#), "{out}");
    assert!(out.contains(r#""agent_name":"reviewer""#), "{out}");

    // Inside an array, too.
    let deep =
        r#"{"messages":[{"role":"user","prompt":"abc"},{"nested":{"prompt_response":"defg"}}]}"#;
    let out = phronesis_mcp::hook::redact_for_capture(deep).expect("valid JSON");
    assert!(out.contains(r#""prompt":"<redacted:3 bytes>""#), "{out}");
    assert!(
        out.contains(r#""prompt_response":"<redacted:4 bytes>""#),
        "{out}"
    );

    // Gemini AfterAgent: `prompt_response` is the model's whole answer.
    let after = r#"{"hook_event_name":"AfterAgent","prompt":"hi","prompt_response":"a long answer","stop_hook_active":false}"#;
    let out = phronesis_mcp::hook::redact_for_capture(after).expect("valid JSON");
    assert!(!out.contains("a long answer"), "{out}");

    // Not valid JSON: it cannot be redacted, so it is not captured at all.
    assert_eq!(phronesis_mcp::hook::redact_for_capture("not json"), None);
}

#[test]
fn prompt_text_never_reaches_the_capture_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let payload = r#"{"hook_event_name":"PreToolUse","session_id":"s","prompt":"zzz-secret","tool_name":"Read","tool_input":{"file_path":"src/main.rs"}}"#;
    let code = run_hook_with_env(
        "pre-check",
        payload,
        &[("PHRONESIS_CAPTURE_DIR", dir.path().to_str().expect("utf8"))],
    );
    assert_eq!(code, 0, "capture must not change hook behavior");
    let raw = std::fs::read_to_string(dir.path().join("payloads.jsonl")).expect("capture file");
    assert!(!raw.contains("zzz-secret"), "{raw}");
    assert!(raw.contains("<redacted:10 bytes>"), "{raw}");
    let records = read_capture(dir.path());
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["phase"], "pre");
    // Everything else is still captured verbatim.
    assert_eq!(records[0]["raw"]["tool_name"], "Read");
    assert_eq!(records[0]["raw"]["session_id"], "s");
}
