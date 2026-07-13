//! Integration tests for `phr-mcp scrub-payload` (plan Task 3), driving the
//! real binary end-to-end: JSONL-to-stdout scrubbing, in-place rewrite with a
//! `.bak` backup, the warn-but-exit-0 free-text-username contract, error
//! reporting, `--write` idempotence (the C1 pin: output must be JSONL that a
//! second run can re-parse), and single-JSON fixture round-tripping (M3).

use std::process::Command;

fn run_scrub(args: &[&str]) -> (i32, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .arg("scrub-payload")
        .args(args)
        .output()
        .expect("run scrub-payload");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

#[test]
fn scrubs_capture_jsonl_to_stdout() {
    let dir = tempfile::tempdir().expect("tempdir");
    let capture = dir.path().join("payloads.jsonl");
    std::fs::write(
        &capture,
        r#"{"ts":1,"phase":"pre","raw":{"session_id":"abc","cwd":"/Users/alicejones/Git/myproject","tool_input":{"file_path":"/Users/alicejones/Git/myproject/src/lib.rs"}}}"#,
    )
    .expect("write capture");

    let (code, stdout, _) = run_scrub(&[
        capture.to_str().expect("utf8"),
        "--home",
        "/Users/alicejones",
        "--project-root",
        "/Users/alicejones/Git/myproject",
    ]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).expect("stdout is JSON");
    assert_eq!(v["raw"]["session_id"], "sess-00000000");
    assert_eq!(v["raw"]["cwd"], "/home/dev/project");
    assert_eq!(
        v["raw"]["tool_input"]["file_path"],
        "/home/dev/project/src/lib.rs"
    );
    assert!(!stdout.contains("alicejones"));
}

#[test]
fn write_flag_rewrites_in_place_with_backup() {
    let dir = tempfile::tempdir().expect("tempdir");
    let capture = dir.path().join("payloads.jsonl");
    let original = r#"{"ts":1,"phase":"pre","raw":{"cwd":"/Users/alicejones/Git/myproject"}}"#;
    std::fs::write(&capture, original).expect("write capture");

    let (code, _, _) = run_scrub(&[
        capture.to_str().expect("utf8"),
        "--write",
        "--home",
        "/Users/alicejones",
        "--project-root",
        "/Users/alicejones/Git/myproject",
    ]);
    assert_eq!(code, 0);
    let rewritten = std::fs::read_to_string(&capture).expect("read back");
    assert!(rewritten.contains("/home/dev/project"));
    assert!(!rewritten.contains("alicejones"));
    let backup = std::fs::read_to_string(dir.path().join("payloads.jsonl.bak")).expect("backup");
    assert_eq!(backup.trim(), original);
}

#[test]
fn username_as_free_token_warns_but_exits_zero() {
    // Finding #1: a captured command that mentions the username as a word is
    // scrubbed-and-shipped with a warning, not failed. (Here the username is
    // long enough to be replaced in free text, so it becomes "dev"; the point
    // is the run still exits 0 and the pipeline is idempotent.)
    let dir = tempfile::tempdir().expect("tempdir");
    let capture = dir.path().join("payloads.jsonl");
    std::fs::write(
        &capture,
        r#"{"ts":1,"phase":"pre","raw":{"tool_input":{"command":"git commit -m 'thanks alicejones'"}}}"#,
    )
    .expect("write capture");
    let (code, stdout, _) = run_scrub(&[
        capture.to_str().expect("utf8"),
        "--home",
        "/Users/alicejones",
        "--project-root",
        "/Users/alicejones/Git/myproject",
    ]);
    assert_eq!(code, 0, "free-text username must not fail the run");
    assert!(!stdout.contains("alicejones"));
}

#[test]
fn missing_file_exits_nonzero_with_message() {
    let (code, _, stderr) = run_scrub(&[
        "/nonexistent/nowhere.jsonl",
        "--home",
        "/Users/x",
        "--project-root",
        "/Users/x/p",
    ]);
    assert_ne!(code, 0);
    assert!(stderr.contains("nowhere.jsonl"));
}

#[test]
fn non_json_line_errors_with_line_number_and_writes_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let capture = dir.path().join("payloads.jsonl");
    let original = concat!(
        r#"{"ts":1,"phase":"pre","raw":{"cwd":"/Users/alicejones/Git/myproject"}}"#,
        "\n",
        "this line is not JSON {",
        "\n",
    );
    std::fs::write(&capture, original).expect("write capture");

    let (code, _, stderr) = run_scrub(&[
        capture.to_str().expect("utf8"),
        "--write",
        "--home",
        "/Users/alicejones",
        "--project-root",
        "/Users/alicejones/Git/myproject",
    ]);
    assert_ne!(code, 0, "a corrupt capture line must fail the run");
    assert!(
        stderr.contains("line 2: not JSON:"),
        "stderr must name the bad line: {stderr}"
    );
    // Abort means abort: the file is untouched and no backup was made.
    assert_eq!(
        std::fs::read_to_string(&capture).expect("read back"),
        original,
        "a failed run must not rewrite the input"
    );
    assert!(!dir.path().join("payloads.jsonl.bak").exists());
}

#[test]
fn write_twice_is_a_fixpoint() {
    // C1 pin: output is JSONL, so a second --write run re-parses every line
    // and leaves the file byte-for-byte identical — never truncates it.
    let dir = tempfile::tempdir().expect("tempdir");
    let capture = dir.path().join("payloads.jsonl");
    std::fs::write(
        &capture,
        concat!(
            r#"{"ts":1,"phase":"pre","raw":{"session_id":"abc","cwd":"/Users/alicejones/Git/myproject","tool_input":{"file_path":"/Users/alicejones/Git/myproject/src/lib.rs"}}}"#,
            "\n",
            r#"{"ts":2,"phase":"post","raw":{"session_id":"def","tool_response":{"stdout":"ok"}}}"#,
            "\n",
        ),
    )
    .expect("write capture");

    let args = [
        capture.to_str().expect("utf8"),
        "--write",
        "--home",
        "/Users/alicejones",
        "--project-root",
        "/Users/alicejones/Git/myproject",
    ];
    let (code, _, _) = run_scrub(&args);
    assert_eq!(code, 0);
    let once = std::fs::read_to_string(&capture).expect("read after first run");
    assert_eq!(once.lines().count(), 2, "both records survive: {once}");

    let (code, _, _) = run_scrub(&args);
    assert_eq!(code, 0, "second run must succeed on its own output");
    let twice = std::fs::read_to_string(&capture).expect("read after second run");
    assert_eq!(twice, once, "scrub --write must be a fixpoint");
}

#[test]
fn single_json_fixture_round_trips_without_envelope() {
    // M3: a bare fixture (no `raw` key), even pretty-printed across multiple
    // lines, is scrubbed as-is — never wrapped in a {ts, phase, raw} record.
    let dir = tempfile::tempdir().expect("tempdir");
    let fixture = dir.path().join("fixture.json");
    std::fs::write(
        &fixture,
        "{\n  \"session_id\": \"abc\",\n  \"tool_name\": \"Edit\",\n  \"tool_input\": {\n    \"file_path\": \"/Users/alicejones/Git/myproject/src/lib.rs\"\n  }\n}\n",
    )
    .expect("write fixture");

    let (code, stdout, _) = run_scrub(&[
        fixture.to_str().expect("utf8"),
        "--home",
        "/Users/alicejones",
        "--project-root",
        "/Users/alicejones/Git/myproject",
    ]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).expect("stdout is JSON");
    assert!(v.get("raw").is_none(), "must not gain a capture envelope");
    assert!(v.get("ts").is_none(), "must not gain a capture envelope");
    assert_eq!(v["session_id"], "sess-00000000");
    assert_eq!(v["tool_name"], "Edit");
    assert_eq!(v["tool_input"]["file_path"], "/home/dev/project/src/lib.rs");


#[test]
fn filesystem_root_project_root_exits_nonzero_and_writes_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let capture = dir.path().join("payloads.jsonl");
    let original = r#"{"ts":1,"phase":"pre","raw":{"cwd":"/Users/alicejones/Git/myproject"}}"#;
    std::fs::write(&capture, original).expect("write capture");

    let (code, stdout, stderr) = run_scrub(&[
        capture.to_str().expect("utf8"),
        "--write",
        "--home",
        "/Users/alicejones",
        "--project-root",
        "/",
    ]);
    assert_ne!(code, 0, "the filesystem root must be rejected as a project root");
    assert!(stdout.is_empty(), "no scrubbed output on a config error");
    assert!(
        stderr.contains("project root"),
        "diagnostic must name the bad root: {stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(&capture).expect("read back"),
        original,
        "a rejected configuration must not rewrite the input"
    );
    assert!(!dir.path().join("payloads.jsonl.bak").exists());
}

#[test]
fn relative_project_root_exits_nonzero() {
    let dir = tempfile::tempdir().expect("tempdir");
    let capture = dir.path().join("payloads.jsonl");
    std::fs::write(&capture, r#"{"ts":1,"phase":"pre","raw":{}}"#).expect("write capture");

    let (code, _, stderr) = run_scrub(&[
        capture.to_str().expect("utf8"),
        "--home",
        "/Users/alicejones",
        "--project-root",
        "Git/myproject",
    ]);
    assert_ne!(code, 0, "a relative project root is ambiguous and must be rejected");
    assert!(
        stderr.contains("absolute path"),
        "diagnostic must explain the rejection: {stderr}"
    );
}

#[test]
fn whitespace_home_exits_nonzero() {
    let dir = tempfile::tempdir().expect("tempdir");
    let capture = dir.path().join("payloads.jsonl");
    std::fs::write(&capture, r#"{"ts":1,"phase":"pre","raw":{}}"#).expect("write capture");

    let (code, _, stderr) = run_scrub(&[
        capture.to_str().expect("utf8"),
        "--home",
        "   ",
        "--project-root",
        "/Users/alicejones/Git/myproject",
    ]);
    assert_ne!(code, 0, "a whitespace-only home must be rejected");
    assert!(
        stderr.contains("home directory"),
        "diagnostic must name the bad root: {stderr}"
    );
}

}
