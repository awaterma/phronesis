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
}

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
    assert_ne!(
        code, 0,
        "the filesystem root must be rejected as a project root"
    );
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
    assert_ne!(
        code, 0,
        "a relative project root is ambiguous and must be rejected"
    );
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

#[test]
fn suspected_secret_blocks_write_before_backup() {
    let dir = tempfile::tempdir().expect("tempdir");
    let capture = dir.path().join("payloads.jsonl");
    let original = r#"{"ts":1,"phase":"pre","raw":{"tool_input":{"command":"export API_KEY=SUPERSECRETVALUE123456"}}}"#;
    std::fs::write(&capture, original).expect("write capture");

    let (code, stdout, stderr) = run_scrub(&[
        capture.to_str().expect("utf8"),
        "--write",
        "--home",
        "/Users/alicejones",
        "--project-root",
        "/Users/alicejones/Git/myproject",
    ]);
    assert_ne!(code, 0, "a suspected secret must fail the run");
    assert!(stdout.is_empty(), "no scrubbed output on a detector error");
    assert!(
        stderr.contains("token/secret assignment"),
        "stderr names the leak class: {stderr}"
    );
    assert!(
        !stderr.contains("SECRETVALUE123456"),
        "diagnostics must not echo the full secret: {stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(&capture).expect("read back"),
        original,
        "a failed run must not rewrite the input"
    );
    assert!(
        !dir.path().join("payloads.jsonl.bak").exists(),
        "errors must abort before the backup is made"
    );
}

#[test]
fn pem_private_key_blocks_run() {
    let dir = tempfile::tempdir().expect("tempdir");
    let capture = dir.path().join("payloads.jsonl");
    std::fs::write(
        &capture,
        r#"{"ts":1,"phase":"pre","raw":{"tool_input":{"new_string":"-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEAfake"}}}"#,
    )
    .expect("write capture");

    let (code, stdout, stderr) = run_scrub(&[
        capture.to_str().expect("utf8"),
        "--write",
        "--home",
        "/Users/alicejones",
        "--project-root",
        "/Users/alicejones/Git/myproject",
    ]);
    assert_ne!(code, 0, "a private-key header must fail the run");
    assert!(stdout.is_empty());
    assert!(
        stderr.contains("private-key header"),
        "stderr names the leak class: {stderr}"
    );
}

#[test]
fn absolute_path_outside_home_blocks_run() {
    let dir = tempfile::tempdir().expect("tempdir");
    let capture = dir.path().join("payloads.jsonl");
    std::fs::write(
        &capture,
        r#"{"ts":1,"phase":"pre","raw":{"tool_input":{"command":"cat /private/tmp/cap/payloads.jsonl"}}}"#,
    )
    .expect("write capture");

    let (code, _, stderr) = run_scrub(&[
        capture.to_str().expect("utf8"),
        "--home",
        "/Users/alicejones",
        "--project-root",
        "/Users/alicejones/Git/myproject",
    ]);
    assert_ne!(code, 0, "an out-of-home absolute path must fail the run");
    assert!(
        stderr.contains("absolute path"),
        "stderr names the leak class: {stderr}"
    );
}

#[test]
fn email_address_warns_but_exits_zero() {
    let dir = tempfile::tempdir().expect("tempdir");
    let capture = dir.path().join("payloads.jsonl");
    std::fs::write(
        &capture,
        r#"{"ts":1,"phase":"pre","raw":{"tool_input":{"command":"git log --author=someone@example.com"}}}"#,
    )
    .expect("write capture");

    let (code, stdout, stderr) = run_scrub(&[
        capture.to_str().expect("utf8"),
        "--home",
        "/Users/alicejones",
        "--project-root",
        "/Users/alicejones/Git/myproject",
    ]);
    assert_eq!(
        code, 0,
        "email addresses are warnings, not errors: {stderr}"
    );
    assert!(
        stderr.contains("email address"),
        "the warning must be visible on stderr: {stderr}"
    );
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).expect("stdout is JSON");
    assert_eq!(v["ts"], 1);
}

#[test]
fn benign_password_prose_passes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let capture = dir.path().join("payloads.jsonl");
    std::fs::write(
        &capture,
        r#"{"ts":1,"phase":"pre","raw":{"tool_input":{"command":"grep -n password src/lib.rs"}}}"#,
    )
    .expect("write capture");

    let (code, stdout, stderr) = run_scrub(&[
        capture.to_str().expect("utf8"),
        "--home",
        "/Users/alicejones",
        "--project-root",
        "/Users/alicejones/Git/myproject",
    ]);
    assert_eq!(
        code, 0,
        "the bare word password must not fail the run: {stderr}"
    );
    assert!(
        !stderr.contains("scrub error"),
        "no error-severity finding for a bare keyword: {stderr}"
    );
    assert!(
        stdout.contains("password"),
        "project content passes through"
    );
}

// ---------- lifecycle::scrub::scrub_prompt (Task 6) ----------

use std::sync::{Mutex, MutexGuard, OnceLock};

/// Serializes every test in this file that mutates `$HOME`.
fn home_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Sets `$HOME` (or removes it when `value` is `None`) and restores the previous
/// value on drop, including when the test panics.
struct HomeGuard {
    previous: Option<String>,
    _lock: MutexGuard<'static, ()>,
}

impl HomeGuard {
    fn set(value: Option<&str>) -> Self {
        let guard = Self {
            previous: std::env::var("HOME").ok(),
            _lock: home_lock(),
        };
        // SAFETY: edition 2024 requires `unsafe` for env mutation because it is
        // not thread-safe; `home_lock` is what makes it safe here, and every
        // other `$HOME` mutation in this file goes through this guard.
        unsafe {
            match value {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
        guard
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.previous {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
    }
}

#[test]
fn scrub_prompt_removes_bare_session_ids_transcripts_and_home_paths() {
    let fake_home = tempfile::tempdir().unwrap();
    let home = fake_home.path().display().to_string();
    let _guard = HomeGuard::set(Some(&home));
    let root = tempfile::tempdir().unwrap();
    let text = format!(
        "resume session 0f3c9a1e-1234-4bcd-9ef0-abcdefabcdef please, transcript at {home}/.claude/projects/x/abc.jsonl and file {home}/secret/notes.txt"
    );
    let out = phronesis_mcp::lifecycle::scrub::scrub_prompt(root.path(), &text);
    assert!(
        !out.contains("0f3c9a1e-1234-4bcd-9ef0-abcdefabcdef"),
        "{out}"
    );
    assert!(!out.contains("/.claude/projects/x/abc.jsonl"), "{out}");
    assert!(!out.contains(&format!("{home}/secret")), "{out}");
    assert!(out.contains("sess-00000000"), "{out}");
    assert!(out.contains("resume session"), "{out}");
}

/// The three regexes are unconditional: a bare UUID with no `session` word
/// beside it, a phronesis sid, and a relative transcript path all go.
#[test]
fn scrub_prompt_removes_ids_with_no_surrounding_context() {
    let root = tempfile::tempdir().unwrap();
    let out = phronesis_mcp::lifecycle::scrub::scrub_prompt(
        root.path(),
        "compare 0f3c9a1e-1234-4bcd-9ef0-abcdefabcdef with s-2026-09-18-3a9f1c, see .codex/sessions/x.jsonl",
    );
    assert!(!out.contains("0f3c9a1e"), "{out}");
    assert!(!out.contains("s-2026-09-18-3a9f1c"), "{out}");
    assert!(!out.contains(".codex/sessions/x.jsonl"), "{out}");
    assert_eq!(out.matches("sess-00000000").count(), 2, "{out}");
    assert!(out.contains("compare") && out.contains("with"), "{out}");
}

#[test]
fn scrub_prompt_without_home_still_scrubs_project_root() {
    let _guard = HomeGuard::set(None);
    let root = tempfile::tempdir().unwrap();
    let text = format!("edit {}/src/main.rs now", root.path().display());
    let out = phronesis_mcp::lifecycle::scrub::scrub_prompt(root.path(), &text);
    assert!(!out.contains(&root.path().display().to_string()), "{out}");
    assert!(out.contains("src/main.rs"), "{out}");
}
