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
fn capture_preserves_non_json_stdin_as_string() {
    let dir = tempfile::tempdir().expect("tempdir");
    // Malformed payload: hook exits 0 (allow) per existing behavior, but the
    // capture must still record the raw bytes — broken payloads are exactly
    // what we want ground truth on.
    run_hook_with_env(
        "pre-check",
        "not json at all",
        &[("PHRONESIS_CAPTURE_DIR", dir.path().to_str().expect("utf8"))],
    );
    let records = read_capture(dir.path());
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["raw"], "not json at all");
}
