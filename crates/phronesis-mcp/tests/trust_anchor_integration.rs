//! SPEC-verification-artifact-generation.md S1/S3, acceptance C7: agent-seam
//! writes to the verification trust anchors are refused by the rules a plain
//! `phr-mcp init` installs. Every case runs the real `pre-check` binary
//! against the rules `init` wrote — the pack, the path facts, and the
//! command matcher are all under test together.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

/// The trust anchors the host reads (see `properties/allowlist.rs`
/// `allowlist_path`, `properties/execute.rs` `raw_execution_allowed` and
/// `detect_tier`).
const ANCHOR_PATHS: &[&str] = &[
    ".phronesis/verification-allowlist.json",
    ".phronesis/verification.json",
    "verification/templates/harness.rhai",
    "verification/templates/devcontainer.json",
];

fn init_project() -> tempfile::TempDir {
    let d = tempfile::tempdir().expect("tempdir");
    let out = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(["init", "--rules-only"])
        .current_dir(d.path())
        .output()
        .expect("spawn init");
    assert!(
        out.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    d
}

fn pre_check(root: &Path, tool: &str, input: Value) -> (i32, String) {
    let payload = json!({
        "session_id": "s-agent",
        "cwd": root.display().to_string(),
        "hook_event_name": "PreToolUse",
        "tool_name": tool,
        "tool_input": input,
    });
    let mut child = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .current_dir(root)
        .arg("pre-check")
        .env("PHRONESIS_NO_ACTION_LOG", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pre-check");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(payload.to_string().as_bytes())
        .expect("write payload");
    let out = child.wait_with_output().expect("wait");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Every file-editing tool shape the pre-check hook governs (Claude and
/// Gemini), aimed at `path` — relative and absolute.
fn file_tool_inputs(root: &Path, path: &str) -> Vec<(&'static str, Value)> {
    let abs = root.join(path).display().to_string();
    let mut out = Vec::new();
    for p in [path.to_string(), abs] {
        out.push((
            "Write",
            json!({"file_path": p, "content": "{\"raw_execution\": true}"}),
        ));
        out.push((
            "Edit",
            json!({"file_path": p, "old_string": "false", "new_string": "true"}),
        ));
        out.push((
            "MultiEdit",
            json!({"file_path": p, "edits": [{"old_string": "false", "new_string": "true"}]}),
        ));
        out.push(("write_file", json!({"file_path": p, "content": "x"})));
        out.push((
            "replace",
            json!({"file_path": p, "old_string": "a", "new_string": "b"}),
        ));
    }
    out
}

#[test]
fn file_tool_writes_to_every_trust_anchor_are_blocked() {
    let d = init_project();
    for path in ANCHOR_PATHS {
        for (tool, input) in file_tool_inputs(d.path(), path) {
            let (code, stderr) = pre_check(d.path(), tool, input.clone());
            assert_eq!(
                code, 2,
                "{tool} to trust anchor {path} must be BLOCKED ({input}): {stderr}"
            );
            assert!(
                stderr.contains("trust anchor"),
                "{tool} to {path}: blocked by the trust-anchor rule, not something else: {stderr}"
            );
        }
    }
}

#[test]
fn shell_writes_to_every_trust_anchor_are_blocked() {
    let d = init_project();
    let commands = [
        "echo '{\"raw_execution\": true}' > .phronesis/verification.json",
        "echo x >> .phronesis/verification-allowlist.json",
        "cat evil.json >| .phronesis/verification-allowlist.json",
        "echo x > verification/templates/harness.rhai",
        "printf x | tee .phronesis/verification.json",
        "printf x | tee -a verification/templates/devcontainer.json",
        "cp /tmp/evil.json .phronesis/verification-allowlist.json",
        "mv /tmp/evil.rhai verification/templates/harness.rhai",
        "sed -i '' 's/false/true/' .phronesis/verification.json",
        "sed -i.bak 's/a/b/' verification/templates/harness.rhai",
        "perl -pi -e 's/a/b/' .phronesis/verification-allowlist.json",
        "rm verification/templates/devcontainer.json",
        "cd .phronesis && echo '{\"raw_execution\":true}' > verification.json",
        "git mv x.rhai verification/templates/x.rhai",
        "echo x > \"$PWD/verification/templates/x.rhai\"",
    ];
    for tool in ["Bash", "run_shell_command"] {
        for cmd in commands {
            let (code, stderr) = pre_check(d.path(), tool, json!({"command": cmd}));
            assert_eq!(
                code, 2,
                "{tool} `{cmd}` writes a trust anchor and must be BLOCKED: {stderr}"
            );
            assert!(
                stderr.contains("trust anchor"),
                "{tool} `{cmd}`: blocked by the trust-anchor rule, not something else: {stderr}"
            );
        }
    }
}

#[test]
fn reads_and_unrelated_writes_are_not_blocked() {
    let d = init_project();
    for cmd in [
        "cat .phronesis/verification.json",
        "jq . .phronesis/verification-allowlist.json",
        "ls verification/templates",
        "git diff -- verification/templates",
        "echo x > verification/unreviewed/h.rs",
        "cargo test 2>&1 | tail -5",
    ] {
        let (code, stderr) = pre_check(d.path(), "Bash", json!({"command": cmd}));
        assert_eq!(code, 0, "`{cmd}` must be allowed: {stderr}");
    }
    for path in [
        "src/verification.rs",
        "docs/verification.json",
        "verification/unreviewed/h.rs",
        "templates/verification.md",
    ] {
        let (code, stderr) = pre_check(
            d.path(),
            "Write",
            json!({"file_path": path, "content": "fn f() {}\n"}),
        );
        assert_eq!(code, 0, "Write to {path} must be allowed: {stderr}");
    }
}
