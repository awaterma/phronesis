use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

fn run_hook(subcommand: &str, payload: &str) -> (i32, String) {
    run_hook_in(subcommand, payload, None)
}

fn run_hook_in(subcommand: &str, payload: &str, cwd: Option<&Path>) -> (i32, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_phr-mcp"));
    cmd.arg(subcommand)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let mut child = cmd.spawn().expect("failed to spawn hook process");

    // Tolerate BrokenPipe when the hook closes stdin early (e.g., after hitting
    // the stdin size cap — see Finding #4 test). All other write errors should
    // still panic.
    let mut stdin = child.stdin.take().unwrap();
    match stdin.write_all(payload.as_bytes()) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
        Err(e) => panic!("unexpected write error: {}", e),
    }
    drop(stdin);

    let output = child.wait_with_output().expect("failed to wait");
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let code = output.status.code().unwrap_or(-1);
    (code, stderr)
}

fn write_rules_file(dir: &Path, contents: &str) {
    let phronesis = dir.join(".phronesis");
    std::fs::create_dir_all(&phronesis).unwrap();
    std::fs::write(phronesis.join("rules.json"), contents).unwrap();
}

#[test]
fn pre_check_allows_non_edit_tool() {
    let payload = r#"{"tool_name": "Read", "tool_input": {"file_path": "src/main.rs"}}"#;
    let (code, _) = run_hook("pre-check", payload);
    assert_eq!(code, 0);
}

#[test]
fn pre_check_allows_edit_without_rules_file() {
    let payload = r#"{
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/main.rs",
            "old_string": "old",
            "new_string": "new"
        }
    }"#;
    let (code, _) = run_hook("pre-check", payload);
    assert_eq!(code, 0);
}

#[test]
fn post_check_allows_non_edit_tool() {
    let payload = r#"{"tool_name": "Bash", "tool_input": {"command": "ls"}}"#;
    let (code, _) = run_hook("post-check", payload);
    assert_eq!(code, 0);
}

#[test]
fn post_check_allows_edit_without_rules_file() {
    let payload = r#"{
        "tool_name": "Write",
        "tool_input": {
            "file_path": "src/main.rs",
            "content": "fn main() {}"
        }
    }"#;
    let (code, _) = run_hook("post-check", payload);
    assert_eq!(code, 0);
}

// ─────────────────────────────────────────────────────────────────────────
// Security: fail-closed on malformed rules.json (Finding #7)
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn pre_check_fails_closed_on_malformed_rules_json() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(dir.path(), "{not valid json");
    let payload = r#"{
        "tool_name": "Edit",
        "tool_input": { "file_path": "src/x.rs", "old_string": "a", "new_string": "b" }
    }"#;
    let (code, stderr) = run_hook_in("pre-check", payload, Some(dir.path()));
    assert_eq!(code, 2, "expected BLOCK on malformed rules");
    assert!(stderr.contains("malformed"), "stderr: {}", stderr);
}

#[test]
fn post_check_warns_on_malformed_rules_json() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(dir.path(), "{also bad");
    let payload = r#"{
        "tool_name": "Write",
        "tool_input": { "file_path": "src/x.rs", "content": "x" }
    }"#;
    let (code, stderr) = run_hook_in("post-check", payload, Some(dir.path()));
    assert_eq!(code, 1, "expected WARN on malformed rules");
    assert!(stderr.contains("malformed"), "stderr: {}", stderr);
}

#[test]
fn pre_check_allows_when_rules_file_is_empty_array() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(dir.path(), r#"{"rules": []}"#);
    let payload = r#"{
        "tool_name": "Edit",
        "tool_input": { "file_path": "src/x.rs", "old_string": "a", "new_string": "b" }
    }"#;
    let (code, _) = run_hook_in("pre-check", payload, Some(dir.path()));
    assert_eq!(code, 0, "empty rules array should allow");
}

// ─────────────────────────────────────────────────────────────────────────
// Security: stdin size cap (Finding #4)
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn pre_check_rejects_oversized_stdin() {
    // 12MB of garbage exceeds the 10MB cap.
    let big = "a".repeat(12 * 1024 * 1024);
    let (code, stderr) = run_hook("pre-check", &big);
    assert_eq!(code, 2, "oversized stdin should BLOCK");
    assert!(
        stderr.contains("invalid hook payload"),
        "stderr: {}",
        stderr
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Security: path traversal in post-check (Finding #2)
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn post_check_rejects_dot_dot_traversal_in_file_path() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"any","phase":"post","priority":1,
            "when":[{"hook_phase":"post"}],
            "then":{"log":"ok"}
        }]}"#,
    );
    let payload = r#"{
        "tool_name": "Write",
        "tool_input": { "file_path": "../../../etc/passwd", "content": "x" }
    }"#;
    let (code, stderr) = run_hook_in("post-check", payload, Some(dir.path()));
    assert_eq!(code, 1, "traversal path should WARN");
    assert!(
        stderr.contains("outside project root"),
        "stderr: {}",
        stderr
    );
}

#[test]
fn post_check_rejects_absolute_path_outside_root() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"any","phase":"post","priority":1,
            "when":[{"hook_phase":"post"}],
            "then":{"log":"ok"}
        }]}"#,
    );
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("secret.rs");
    std::fs::write(&secret, "secret content").unwrap();
    let payload = format!(
        r#"{{
            "tool_name": "Write",
            "tool_input": {{ "file_path": {:?}, "content": "x" }}
        }}"#,
        secret.to_string_lossy()
    );
    let (code, stderr) = run_hook_in("post-check", &payload, Some(dir.path()));
    assert_eq!(code, 1, "absolute path outside root should WARN");
    assert!(
        stderr.contains("outside project root"),
        "stderr: {}",
        stderr
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Diff-aware facts + test_exists_for (TDD-style rules)
// ─────────────────────────────────────────────────────────────────────────

fn tdd_rules() -> &'static str {
    r#"{"rules":[{
        "id":"tdd-required","phase":"pre","priority":10,
        "when":[
            {"function_added":["?file","?fn"]},
            {"no_test_for":"?fn"}
        ],
        "then":{"block":"Write a failing test for `?fn` before implementing it in ?file"}
    }]}"#
}

fn run_hook_with_root(payload: &str, root: &Path) -> (i32, String) {
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_phr-mcp"));
    cmd.arg("pre-check")
        .env("PHRONESIS_PROJECT_ROOT", root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let _ = stdin.write_all(payload.as_bytes());
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn tdd_rule_blocks_new_function_without_test() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::create_dir_all(dir.path().join("tests")).unwrap();
    std::fs::write(dir.path().join("src/server.rs"), "pub fn existing() {}\n").unwrap();
    write_rules_file(dir.path(), tdd_rules());

    let payload = r#"{
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/server.rs",
            "old_string": "pub fn existing() {}",
            "new_string": "pub fn existing() {}\npub fn frobnicate() {}"
        }
    }"#;
    let (code, stderr) = run_hook_with_root(payload, dir.path());
    assert_eq!(code, 2, "should block: {}", stderr);
    assert!(stderr.contains("frobnicate"), "stderr: {}", stderr);
    assert!(
        stderr.contains("Write a failing test"),
        "stderr: {}",
        stderr
    );
}

#[test]
fn tdd_rule_allows_when_test_exists() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::create_dir_all(dir.path().join("tests")).unwrap();
    std::fs::write(dir.path().join("src/server.rs"), "pub fn existing() {}\n").unwrap();
    std::fs::write(
        dir.path().join("tests/server_test.rs"),
        "#[test]\nfn test_frob() { frobnicate(); }\n",
    )
    .unwrap();
    write_rules_file(dir.path(), tdd_rules());

    let payload = r#"{
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/server.rs",
            "old_string": "pub fn existing() {}",
            "new_string": "pub fn existing() {}\npub fn frobnicate() {}"
        }
    }"#;
    let (code, stderr) = run_hook_with_root(payload, dir.path());
    assert_eq!(code, 0, "should allow when test exists; stderr: {}", stderr);
}

#[test]
fn tdd_rule_blocks_python_function_without_test() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/lib.py"), "def existing(): pass\n").unwrap();
    write_rules_file(dir.path(), tdd_rules());

    let payload = r#"{
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/lib.py",
            "old_string": "def existing(): pass",
            "new_string": "def existing(): pass\ndef calculate(): return 42"
        }
    }"#;
    let (code, stderr) = run_hook_with_root(payload, dir.path());
    assert_eq!(code, 2, "should block: {}", stderr);
    assert!(stderr.contains("calculate"), "stderr: {}", stderr);
}

#[test]
fn diff_facts_not_asserted_for_unchanged_functions() {
    // Adding nothing new (same function in old and new) should NOT trigger
    // the TDD rule.
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/server.rs"), "pub fn existing() {}\n").unwrap();
    write_rules_file(dir.path(), tdd_rules());

    let payload = r#"{
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/server.rs",
            "old_string": "pub fn existing() {}",
            "new_string": "pub fn existing() { /* same name, new body */ }"
        }
    }"#;
    let (code, stderr) = run_hook_with_root(payload, dir.path());
    assert_eq!(code, 0, "no new functions → no fire; stderr: {}", stderr);
}

#[test]
fn post_check_allows_relative_path_inside_root() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"any","phase":"post","priority":1,
            "when":[{"hook_phase":"post"}],
            "then":{"log":"ok"}
        }]}"#,
    );
    let inside = dir.path().join("file.rs");
    std::fs::write(&inside, "fn main() {}").unwrap();
    let payload = r#"{
        "tool_name": "Write",
        "tool_input": { "file_path": "file.rs", "content": "fn main() {}" }
    }"#;
    let (code, stderr) = run_hook_in("post-check", payload, Some(dir.path()));
    assert_eq!(code, 0, "in-root path should pass; stderr: {}", stderr);
}

// ─────────────────────────────────────────────────────────────────────────
// MultiEdit support
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn pre_check_blocks_multiedit_introducing_unwrap() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"no-unwrap","phase":"pre","priority":10,
            "when":[
                {"new_content_contains":".unwrap()"},
                {"file_path_matches":"src"}
            ],
            "then":{"block":"no unwrap"}
        }]}"#,
    );
    // MultiEdit with two edits — the second introduces `.unwrap()`.
    let payload = r#"{
        "tool_name": "MultiEdit",
        "tool_input": {
            "file_path": "src/lib.rs",
            "edits": [
                { "old_string": "fn foo()", "new_string": "fn bar()" },
                { "old_string": "return 1;", "new_string": "return result.unwrap();" }
            ]
        }
    }"#;
    let (code, stderr) = run_hook_with_root(payload, dir.path());
    assert_eq!(
        code, 2,
        "MultiEdit with .unwrap() should block; stderr: {}",
        stderr
    );
    assert!(stderr.contains("no unwrap"), "stderr: {}", stderr);
}

#[test]
fn pre_check_allows_clean_multiedit() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"no-unwrap","phase":"pre","priority":10,
            "when":[
                {"new_content_contains":".unwrap()"},
                {"file_path_matches":"src"}
            ],
            "then":{"block":"no unwrap"}
        }]}"#,
    );
    let payload = r#"{
        "tool_name": "MultiEdit",
        "tool_input": {
            "file_path": "src/lib.rs",
            "edits": [
                { "old_string": "fn foo()", "new_string": "fn bar()" },
                { "old_string": "return 1;", "new_string": "return result.map_err(Self::err)?;" }
            ]
        }
    }"#;
    let (code, stderr) = run_hook_with_root(payload, dir.path());
    assert_eq!(code, 0, "clean MultiEdit should allow; stderr: {}", stderr);
}

// ─────────────────────────────────────────────────────────────────────────
// Bash support — catching deflective language in commit messages etc.
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn pre_check_blocks_pre_existing_issue_in_bash_command() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"no-blame-deflection","phase":"pre","priority":10,
            "when":[
                {"new_content_contains":"pre-existing issue"}
            ],
            "then":{"block":"Don't deflect with the pre-existing-phrase — fix it or call it out as scope"}
        }]}"#,
    );
    let payload = r#"{
        "tool_name": "Bash",
        "tool_input": {
            "command": "git commit -m 'fix: cleanup (note: clippy warning is a pre-existing issue not from our changes)'"
        }
    }"#;
    let (code, stderr) = run_hook_with_root(payload, dir.path());
    assert_eq!(code, 2, "should block: {}", stderr);
    assert!(stderr.contains("deflect"), "stderr: {}", stderr);
}

#[test]
fn pre_check_allows_clean_bash_command() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"no-blame-deflection","phase":"pre","priority":10,
            "when":[
                {"new_content_contains":"pre-existing issue"}
            ],
            "then":{"block":"nope"}
        }]}"#,
    );
    let payload = r#"{
        "tool_name": "Bash",
        "tool_input": { "command": "cargo test" }
    }"#;
    let (code, _) = run_hook_with_root(payload, dir.path());
    assert_eq!(code, 0);
}

// ─────────────────────────────────────────────────────────────────────────
// Warning-severity rules (constraint_warning action_type)
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn pre_check_warning_rule_exits_one_and_allows() {
    // A rule using `constraint_warning` warns but doesn't block.
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"warn-magic-number","phase":"pre","priority":5,
            "when":[
                {"new_content_contains":"12345"},
                {"file_path_matches":"src"}
            ],
            "then":{"warn":"Consider extracting the magic number"}
        }]}"#,
    );
    let payload = r#"{
        "tool_name":"Edit",
        "tool_input":{"file_path":"src/x.rs","old_string":"a","new_string":"const X = 12345;"}
    }"#;
    let (code, stderr) = run_hook_with_root(payload, dir.path());
    assert_eq!(code, 1, "warning rule must exit 1 (allow-with-message)");
    assert!(stderr.contains("WARNING"), "stderr: {}", stderr);
    assert!(
        stderr.contains("extracting the magic number"),
        "stderr: {}",
        stderr
    );
}

#[test]
fn pre_check_violation_wins_over_warning() {
    // Both a violation and a warning fire on the same edit. Violation takes
    // precedence: exit 2, but the warning still appears in stderr and log.
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[
            {
              "id":"block-unwrap","phase":"pre","priority":10,
              "when":[
                {"new_content_contains":".unwrap()"},
                {"file_path_matches":"src"}
              ],
              "then":{"block":"no unwrap"}
            },
            {
              "id":"warn-magic","phase":"pre","priority":5,
              "when":[
                {"new_content_contains":"12345"},
                {"file_path_matches":"src"}
              ],
              "then":{"warn":"magic number"}
            }
        ]}"#,
    );
    let payload = r#"{
        "tool_name":"Edit",
        "tool_input":{"file_path":"src/x.rs","old_string":"a",
                      "new_string":"let x = foo.unwrap(); const N = 12345;"}
    }"#;
    let (code, stderr) = run_hook_with_root(payload, dir.path());
    assert_eq!(code, 2, "violation must block even when warning also fires");
    assert!(stderr.contains("BLOCKED"), "stderr: {}", stderr);
    assert!(
        stderr.contains("WARNING"),
        "warnings should also surface: {}",
        stderr
    );
}

#[test]
fn pre_check_only_warnings_is_exit_one_not_two() {
    // Regression test: when only warnings fire (no violations), exit must be
    // 1 not 2. The edit is allowed; the agent sees the message.
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"warn-only","phase":"pre","priority":5,
            "when":[
                {"new_content_contains":"TODO:"},
                {"file_path_matches":"src"}
            ],
            "then":{"warn":"TODO in src"}
        }]}"#,
    );
    let payload = r#"{
        "tool_name":"Edit",
        "tool_input":{"file_path":"src/x.rs","old_string":"a","new_string":"// TODO: clean up"}
    }"#;
    let (code, _) = run_hook_with_root(payload, dir.path());
    assert_eq!(code, 1, "warning-only must exit 1, never 2");
}

#[test]
fn pre_check_log_entry_distinguishes_violations_from_warnings() {
    // Verify the JSONL entry's `consequences` array tags each fired action
    // with its action_type, so violations and warnings can be told apart.
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[
            {
              "id":"block","phase":"pre","priority":10,
              "when":[{"new_content_contains":"BAD"}],
              "then":{"block":"hard"}
            },
            {
              "id":"warn","phase":"pre","priority":5,
              "when":[{"new_content_contains":"MEH"}],
              "then":{"warn":"soft"}
            }
        ]}"#,
    );
    let payload = r#"{
        "tool_name":"Edit",
        "tool_input":{"file_path":"src/x.rs","old_string":"a","new_string":"BAD and MEH"}
    }"#;
    run_hook_with_root(payload, dir.path());

    let log = std::fs::read_to_string(dir.path().join(".phronesis/log.jsonl")).unwrap();
    let entry: serde_json::Value = serde_json::from_str(log.lines().next().unwrap()).unwrap();
    assert_eq!(entry["exit"], 2);
    let consequences = entry["consequences"].as_array().unwrap();
    let vs: Vec<&str> = consequences
        .iter()
        .filter(|c| c["action_type"].as_str() == Some("constraint_violation"))
        .map(|c| c["message"].as_str().unwrap())
        .collect();
    let ws: Vec<&str> = consequences
        .iter()
        .filter(|c| c["action_type"].as_str() == Some("constraint_warning"))
        .map(|c| c["message"].as_str().unwrap())
        .collect();
    assert_eq!(vs, vec!["hard"]);
    assert_eq!(ws, vec!["soft"]);
}

// ─────────────────────────────────────────────────────────────────────────
// OR operator — end-to-end through rules_file::read → unfold_or → hook
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn or_rule_fires_on_either_branch() {
    // A v2 rules.json with an OR clause: fires when EITHER branch matches.
    // The payload matches ONLY the second branch ("cargo nextest"), not the
    // first ("cargo test"): "cargo nextest run" does not contain the
    // substring "cargo test".
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id": "block-test-cmd", "phase": "pre", "priority": 5,
            "when": [ { "or": [
                { "new_content_contains": "cargo test" },
                { "new_content_contains": "cargo nextest" }
            ] } ],
            "then": { "block": "use the workspace test runner" }
        }]}"#,
    );
    let payload = r#"{
        "tool_name": "Bash",
        "tool_input": { "command": "cargo nextest run" }
    }"#;
    let (code, stderr) = run_hook_with_root(payload, dir.path());
    assert_eq!(code, 2, "OR second branch must block; stderr: {stderr}");
    assert!(
        stderr.contains("workspace test runner"),
        "block message must appear in stderr: {stderr}",
    );
}

// ─────────────────────────────────────────────────────────────────────────
// v1 backward-compatibility: the hook must still load and fire v1-shape rules
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn v1_legacy_rules_still_load() {
    // Explicit v1-shape rules.json (conditions/actions/predicate/action_type).
    // The read path must parse this, unfold it, and fire the rule — proving
    // backward compatibility at the integration layer.
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"v1-compat","phase":"pre","priority":10,
            "conditions":[
                {"predicate":"new_content_contains","args":[".unwrap()"]},
                {"predicate":"file_path_matches","args":["src"]}
            ],
            "actions":[{"action_type":"constraint_violation","params":["v1 rule fired"]}]
        }]}"#,
    );
    let payload = r#"{
        "tool_name": "Edit",
        "tool_input": {"file_path":"src/x.rs","old_string":"a","new_string":"foo.unwrap()"}
    }"#;
    let (code, stderr) = run_hook_with_root(payload, dir.path());
    assert_eq!(code, 2, "v1 rule must still block; stderr: {}", stderr);
    assert!(stderr.contains("v1 rule fired"), "stderr: {}", stderr);
}
