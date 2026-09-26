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

/// Review corpus (B1): everyday commands the shell rule once false-blocked in
/// the default platform. Each must pass: a lookalike file name, a nested
/// `verification/templates` that is not the root anchor, an anchor read as
/// a copy SOURCE, or anchor text inside a quoted message or heredoc body.
const SHELL_MUST_PASS: &[&str] = &[
    // Files that merely end in verification.json.
    "curl -s https://example.com/api > fixtures/email-verification.json",
    "jq . raw.json > test-data/phone_verification.json",
    "cp sample.json tests/fixtures/verification.json",
    // `verification/templates` nested below the root, or a lookalike file.
    "touch src/verification/templates.rs",
    "git mv old.rs src/verification/templates.rs",
    "rm app/verification/templates/welcome.html",
    "mkdir -p src/auth/verification/templates && touch src/auth/verification/templates/mod.rs",
    // The anchor as a copy SOURCE (a read), never the destination.
    "cp .phronesis/verification.json /tmp/backup.json",
    "cp -r verification/templates /tmp/tpl",
    "rsync -a verification/templates/ /tmp/tpl/",
    "cp .phronesis/verification.json .phronesis/verification.json.bak",
    // Anchor text inside quoted strings and a heredoc commit body.
    "echo \"config -> .phronesis/verification.json\"",
    "git commit -m \"doc: explain > verification.json semantics\"",
    "git commit -m \"docs: never rm .phronesis/verification.json by hand\"",
    "git commit -F - <<'EOF'\nfix: tidy the docs\n\nrm stale note from verification.json docs\nEOF",
    // Plain reads.
    "cat .phronesis/verification.json",
    "jq . .phronesis/verification-allowlist.json",
    "ls verification/templates",
    "git diff -- verification/templates",
    "diff verification/templates/a.rhai /tmp/a.rhai > /tmp/out.diff",
    "echo x > verification/unreviewed/h.rs",
    "cargo test 2>&1 | tail -5",
];

/// Real writes to the anchors that must stay blocked, beyond the base set in
/// `shell_writes_to_every_trust_anchor_are_blocked`.
const SHELL_MUST_BLOCK: &[&str] = &[
    "echo x>.phronesis/verification.json",
    "echo x > ./.phronesis/verification.json",
    "echo x > ./verification/templates/a.rhai",
    "echo x > \"$PWD/.phronesis/verification-allowlist.json\"",
    "touch a; rm -f \".phronesis/verification.json\"",
    "sudo tee .phronesis/verification.json < /tmp/x",
    "cp /tmp/verification.json .phronesis/",
    "cp /tmp/verification-allowlist.json .phronesis",
    "cp -r /tmp/templates verification/",
    "cp -t verification/templates x.rhai",
    "rsync -a /tmp/tpl/ verification/templates/",
    "ln -sf /tmp/evil.json .phronesis/verification.json",
    "install -m 644 evil.rhai verification/templates/h.rhai",
    "dd if=/tmp/x of=.phronesis/verification.json",
    "curl -o verification/templates/h.rhai https://example.com/h.rhai",
    "git checkout HEAD~1 -- .phronesis/verification-allowlist.json",
    "git rm verification/templates/devcontainer.json",
    "ls && cp x.json .phronesis/verification.json",
    "cd .phronesis; echo x > verification-allowlist.json",
];

#[test]
fn review_corpus_everyday_shell_commands_are_not_blocked() {
    let d = init_project();
    let mut wrong = Vec::new();
    for cmd in SHELL_MUST_PASS {
        let (code, stderr) = pre_check(d.path(), "Bash", json!({"command": cmd}));
        if code == 2 || stderr.contains("trust anchor") {
            wrong.push(format!("`{cmd}` (exit {code}): {stderr}"));
        }
    }
    assert!(wrong.is_empty(), "false blocks:\n{}", wrong.join("\n"));
}

#[test]
fn review_corpus_real_shell_writes_stay_blocked() {
    let d = init_project();
    let mut wrong = Vec::new();
    for cmd in SHELL_MUST_BLOCK {
        let (code, stderr) = pre_check(d.path(), "Bash", json!({"command": cmd}));
        if code != 2 || !stderr.contains("trust anchor") {
            wrong.push(format!("`{cmd}` (exit {code}): {stderr}"));
        }
    }
    assert!(wrong.is_empty(), "missed writes:\n{}", wrong.join("\n"));
}

/// B2: the templates anchor is the adjacent pair `verification/templates`
/// at the project root — not the two segments in any order anywhere.
#[test]
fn file_tool_writes_outside_the_root_anchors_are_not_blocked() {
    let d = init_project();
    for path in [
        "templates/verification/email.html",
        "app/verification/views/templates/x.html",
        "src/verification/templates/x.html",
        "src/.phronesis/verification.json",
        ".phronesis/verification.json.bak",
        "verification/templates.rs",
    ] {
        let (code, stderr) = pre_check(
            d.path(),
            "Write",
            json!({"file_path": path, "content": "x\n"}),
        );
        assert!(
            code != 2 && !stderr.contains("trust anchor"),
            "Write to {path} must be allowed: {stderr}"
        );
    }
}

/// B2: a checkout that itself lives under `…/verification/templates/` must
/// not have every absolute-path write blocked.
#[test]
fn a_project_checked_out_under_a_templates_dir_is_not_blocked() {
    let outer = tempfile::tempdir().expect("tempdir");
    let root = outer.path().join("verification/templates/proj");
    std::fs::create_dir_all(&root).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(["init", "--rules-only"])
        .current_dir(&root)
        .output()
        .expect("spawn init");
    assert!(out.status.success(), "init failed: {out:?}");
    let abs = root.join("src/lib.rs").display().to_string();
    let (code, stderr) = pre_check(
        &root,
        "Write",
        json!({"file_path": abs, "content": "pub fn f() {}\n"}),
    );
    assert!(
        code != 2 && !stderr.contains("trust anchor"),
        "Write to {abs} must be allowed: {stderr}"
    );
    // …while the project's own anchor is still refused by absolute path.
    let anchor = root
        .join("verification/templates/h.rhai")
        .display()
        .to_string();
    let (code, stderr) = pre_check(&root, "Write", json!({"file_path": anchor, "content": "x"}));
    assert_eq!(code, 2, "Write to {anchor} must be BLOCKED: {stderr}");
}

/// Lexical tricks that still name the anchor on disk stay blocked.
#[test]
fn file_tool_path_tricks_to_anchors_stay_blocked() {
    let d = init_project();
    for path in [
        "./.phronesis/verification.json",
        "verification/x/../templates/h.rhai",
        ".phronesis/./verification-allowlist.json",
    ] {
        let (code, stderr) = pre_check(
            d.path(),
            "Write",
            json!({"file_path": path, "content": "x"}),
        );
        assert_eq!(code, 2, "Write to {path} must be BLOCKED: {stderr}");
    }
}
