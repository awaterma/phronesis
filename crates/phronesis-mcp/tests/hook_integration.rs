use serde_json::Value;
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

fn write_predicate_provider(dir: &Path, name: &str, script: &str) {
    let predicates = dir.join(".phronesis/predicates");
    std::fs::create_dir_all(&predicates).unwrap();
    std::fs::write(predicates.join(name), script).unwrap();
}

#[test]
fn later_rule_layer_wins_and_override_provenance_is_matchable() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{"id":"policy","phase":"pre","priority":1,
            "when":[{"file_path_matches":"src/"}],
            "then":{"block":"project policy"}}]}"#,
    );
    std::fs::write(
        dir.path().join("personal.json"),
        r#"{"rules":[{"id":"policy","phase":"pre","priority":10,
            "when":[{"rule_overridden":["policy","project","?old_path","personal","?new_path","ADR-personal"]}],
            "then":{"block":"personal policy with override provenance"}}]}"#,
    )
    .unwrap();
    std::fs::write(
        dir.path().join(".phronesis/loader.json"),
        r#"{"version":1,"layers":[
            {"name":"project","path":".phronesis/rules.json"},
            {"name":"personal","path":"personal.json","decision":"ADR-personal"}
        ]}"#,
    )
    .unwrap();
    let payload = r#"{"tool_name":"Edit","tool_input":{"file_path":"src/lib.rs","old_string":"old","new_string":"new"}}"#;

    let (code, stderr) = run_hook_in("pre-check", payload, Some(dir.path()));
    assert_eq!(code, 2, "layered rules should block: {stderr}");
    assert!(
        stderr.contains("personal policy with override provenance"),
        "{stderr}"
    );
    assert!(!stderr.contains("project policy"), "{stderr}");
}

#[test]
fn rules_can_govern_a_three_override_chain() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[
            {"id":"policy","phase":"pre","priority":1,
             "when":[{"file_path_matches":"src/"}],"then":{"log":"project"}},
            {"id":"too-many-policy-overrides","phase":"pre","priority":20,
             "when":[{"__script__":"facts_count('rule_overridden', ['policy','*','*','*','*','*']) >= 3"}],
             "then":{"block":"policy was overridden at least three times"}}
        ]}"#,
    );
    for (file, priority) in [("team.json", 2), ("org.json", 3), ("personal.json", 4)] {
        std::fs::write(
            dir.path().join(file),
            format!(
                r#"{{"rules":[{{"id":"policy","phase":"pre","priority":{priority},
                    "when":[{{"file_path_matches":"src/"}}],"then":{{"log":"layer"}}}}]}}"#
            ),
        )
        .unwrap();
    }
    std::fs::write(
        dir.path().join(".phronesis/loader.json"),
        r#"{"layers":[
            {"name":"project","path":".phronesis/rules.json"},
            {"name":"team","path":"team.json","decision":"ADR-team"},
            {"name":"org","path":"org.json","decision":"ADR-org"},
            {"name":"personal","path":"personal.json","decision":"ADR-personal"}
        ]}"#,
    )
    .unwrap();
    let payload = r#"{"tool_name":"Edit","tool_input":{"file_path":"src/lib.rs","old_string":"old","new_string":"new"}}"#;

    let (code, stderr) = run_hook_in("pre-check", payload, Some(dir.path()));
    assert_eq!(code, 2, "override-count rule must block: {stderr}");
    assert!(
        stderr.contains("policy was overridden at least three times"),
        "{stderr}"
    );
}

#[test]
fn pre_hook_persists_emit_capsule_with_rule_provenance_and_bound_body() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{"id":"capsule-rule","phase":"pre","priority":5,
            "when":[{"file_path_matches":["?path"]}],
            "then":{"emit_capsule":{"id":"review-edit","body":"Review ?path","lifecycle":"session","priority":50}}}]}"#,
    );
    let payload = r#"{"tool_name":"Edit","tool_input":{"file_path":"src/lib.rs","old_string":"old","new_string":"new"}}"#;
    let (code, stderr) = run_hook_in("pre-check", payload, Some(dir.path()));
    assert_eq!(code, 0, "{stderr}");
    let value: serde_json::Value = serde_json::from_slice(
        &std::fs::read(dir.path().join(".phronesis/emitted-capsules.json")).unwrap(),
    )
    .unwrap();
    let record = &value["capsules"]["review-edit"];
    assert_eq!(record["body"], "Review lib.rs");
    assert_eq!(record["emitted_by"], "capsule-rule");
    assert_eq!(record["bindings"]["?path"], "lib.rs");
}

#[test]
fn rhai_provider_extends_the_pre_hook_lhs() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"parser-change","phase":"pre","priority":10,
            "when":[{"parser_changed":"?file"}],
            "then":{"block":"Parser policy applies to ?file"}
        }]}"#,
    );
    write_predicate_provider(
        dir.path(),
        "parser.rhai",
        r#"
            if event.tool_name == "Edit" && event.file_path.starts_with("src/parser/") {
                emit_fact("parser_changed", [event.file_path]);
            }
        "#,
    );
    let payload = r#"{
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/parser/mod.rs",
            "old_string": "old",
            "new_string": "new"
        }
    }"#;

    let (code, stderr) = run_hook_in("pre-check", payload, Some(dir.path()));
    assert_eq!(code, 2, "provider-derived predicate must fire: {stderr}");
    assert!(stderr.contains("Parser policy applies to src/parser/mod.rs"));
}

#[test]
fn broken_rhai_provider_fails_closed_before_edit() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"any-edit","phase":"pre","priority":1,
            "when":[{"tool_name":"Edit"}],
            "then":{"warn":"ordinary rule"}
        }]}"#,
    );
    write_predicate_provider(dir.path(), "broken.rhai", "this is not valid Rhai !!!");
    let payload = r#"{
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/lib.rs",
            "old_string": "old",
            "new_string": "new"
        }
    }"#;

    let (code, stderr) = run_hook_in("pre-check", payload, Some(dir.path()));
    assert_eq!(code, 2, "broken provider must block: {stderr}");
    assert!(stderr.contains("broken.rhai"), "stderr: {stderr}");
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

/// Like `run_hook_in`, but for the two-argument `claude-hook` form and
/// returning stdout — the adapter's contract is its stdout JSON.
fn run_claude_hook(dir: &Path, event: &str, payload: &str) -> (i32, String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .arg("claude-hook")
        .arg(event)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn claude-hook");
    let mut stdin = child.stdin.take().unwrap();
    let _ = stdin.write_all(payload.as_bytes());
    drop(stdin);
    let out = child.wait_with_output().expect("wait");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn claude_hook_always_prints_one_json_object() {
    let dir = tempfile::tempdir().unwrap();
    for (event, payload) in [
        (
            "SessionStart",
            r#"{"hook_event_name":"SessionStart","session_id":"s1"}"#,
        ),
        (
            "SessionEnd",
            r#"{"hook_event_name":"SessionEnd","session_id":"s1"}"#,
        ),
        (
            "SubagentStart",
            r#"{"hook_event_name":"SubagentStart","agent_id":"a1","agent_type":"Explore"}"#,
        ),
        (
            "SubagentStop",
            r#"{"hook_event_name":"SubagentStop","agent_id":"a1"}"#,
        ),
        ("Stop", r#"{"hook_event_name":"Stop","session_id":"s1"}"#),
        (
            "UserPromptSubmit",
            r#"{"hook_event_name":"UserPromptSubmit","prompt":"hi"}"#,
        ),
    ] {
        let (code, stdout, stderr) = run_claude_hook(dir.path(), event, payload);
        assert_eq!(code, 0, "{event}: {stderr}");
        let v: serde_json::Value = serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("{event}: {e}: {stdout:?}"));
        assert!(v.is_object(), "{event}: {stdout:?}");
    }
}

#[test]
fn claude_hook_bad_stdin_is_empty_json_exit_zero() {
    let dir = tempfile::tempdir().unwrap();
    for event in ["UserPromptSubmit", "SessionStart", "Stop", "SubagentStop"] {
        let (code, stdout, _) = run_claude_hook(dir.path(), event, "not json at all");
        assert_eq!(code, 0, "{event} must never fail the host");
        assert_eq!(stdout.trim(), "{}", "{event}");
    }
}

#[test]
fn claude_hook_stop_blocks_on_low_confidence_and_honors_stop_hook_active() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".phronesis/outcomes")).unwrap();
    std::fs::write(dir.path().join(".phronesis/confidence.json"), "{}").unwrap();
    std::fs::write(dir.path().join(".phronesis/outcomes/current"), "unit-1").unwrap();

    let (code, stdout, _) = run_claude_hook(dir.path(), "Stop", r#"{"hook_event_name":"Stop"}"#);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(v["decision"], "block", "{stdout}");
    assert!(v["reason"].as_str().unwrap().contains("unit-1"), "{stdout}");

    let (_, stdout, _) = run_claude_hook(
        dir.path(),
        "Stop",
        r#"{"hook_event_name":"Stop","stop_hook_active":true}"#,
    );
    assert_eq!(
        stdout.trim(),
        "{}",
        "stop_hook_active must short-circuit the gate"
    );
}

#[test]
fn claude_hook_delegates_tool_events_to_the_existing_runners() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{"id":"no-src","phase":"pre","priority":1,
            "when":[{"file_path_matches":"src"}],
            "then":{"block":"no edits under src"}}]}"#,
    );
    let payload = r#"{"hook_event_name":"PreToolUse","tool_name":"Edit",
        "tool_input":{"file_path":"src/lib.rs","old_string":"a","new_string":"b"}}"#;
    let (code, _, stderr) = run_claude_hook(dir.path(), "PreToolUse", payload);
    assert_eq!(
        code, 2,
        "tool events keep the pre-check exit contract: {stderr}"
    );
    assert!(stderr.contains("no edits under src"), "{stderr}");
}

/// The delegation must be byte-for-byte the existing runner, not a wrapper that
/// rewrites its exit code. Clap also exits 2 on an unknown subcommand, so the
/// block case alone cannot prove the subcommand exists — this differential
/// covers the allow path (0) on both phases too.
#[test]
fn claude_hook_tool_events_match_the_direct_subcommands() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{"id":"no-src","phase":"pre","priority":1,
            "when":[{"file_path_matches":"src"}],
            "then":{"block":"no edits under src"}}]}"#,
    );
    let blocked = r#"{"tool_name":"Edit","tool_input":{"file_path":"src/lib.rs","old_string":"a","new_string":"b"}}"#;
    let allowed = r#"{"tool_name":"Edit","tool_input":{"file_path":"docs/a.md","old_string":"a","new_string":"b"}}"#;
    for (direct, aliases, payload) in [
        ("pre-check", ["PreToolUse", "BeforeTool"], blocked),
        ("pre-check", ["PreToolUse", "BeforeTool"], allowed),
        ("post-check", ["PostToolUse", "AfterTool"], allowed),
    ] {
        let (want, _) = run_hook_in(direct, payload, Some(dir.path()));
        for alias in aliases {
            let (got, _, stderr) = run_claude_hook(dir.path(), alias, payload);
            assert_eq!(got, want, "{alias} vs {direct}: {stderr}");
        }
    }
}

/// The gate must not fire on a Gemini-mapped stop even when it would block on
/// Claude: same project state, two hosts, two answers.
#[test]
fn the_confidence_gate_does_not_run_on_a_gemini_after_agent() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".phronesis/outcomes")).unwrap();
    std::fs::write(dir.path().join(".phronesis/confidence.json"), "{}").unwrap();
    std::fs::write(dir.path().join(".phronesis/outcomes/current"), "unit-1").unwrap();

    let (_, claude_out, _) = run_claude_hook(dir.path(), "Stop", r#"{"hook_event_name":"Stop"}"#);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(claude_out.trim()).unwrap()["decision"],
        "block",
        "the same state blocks on Claude: {claude_out}"
    );

    let (code, gemini_out, stderr) = run_claude_hook(
        dir.path(),
        "AfterAgent",
        r#"{"hook_event_name":"AfterAgent","session_id":"g1","prompt":"hi","prompt_response":"done"}"#,
    );
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        gemini_out.trim(),
        "{}",
        "Gemini gets no decision, only the record"
    );
}

#[test]
fn claude_hook_accepts_gemini_event_names() {
    let dir = tempfile::tempdir().unwrap();
    for event in ["BeforeAgent", "AfterAgent"] {
        let (code, stdout, stderr) = run_claude_hook(
            dir.path(),
            event,
            &format!(r#"{{"hook_event_name":"{event}","prompt":"hi"}}"#),
        );
        assert_eq!(code, 0, "{event}: {stderr}");
        assert!(
            serde_json::from_str::<serde_json::Value>(stdout.trim()).is_ok(),
            "{event}: {stdout:?}"
        );
    }
}

fn journal_records(dir: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(dir.join(".phronesis/journey/events.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn log_entries(dir: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(dir.join(".phronesis/log.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn inflight_keys(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join(".phronesis/journey/inflight"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|v| v["key"].as_str().map(String::from))
        .collect()
}

#[test]
fn claude_hook_prompt_records_fresh_and_opens_turn() {
    let dir = tempfile::tempdir().unwrap();
    run_claude_hook(
        dir.path(),
        "UserPromptSubmit",
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt_id":"p1","prompt":"add a test"}"#,
    );
    let recs = journal_records(dir.path());
    let prompt = recs
        .iter()
        .find(|r| r["kind"] == "prompt")
        .expect("prompt record");
    assert_eq!(prompt["tool"], "__lifecycle");
    assert_eq!(prompt["mode"], "fresh");
    assert_eq!(prompt["host"], "claude");
    assert_eq!(prompt["turn"], "p1");
    assert!(
        !std::fs::read_to_string(dir.path().join(".phronesis/journey/events.jsonl"))
            .unwrap()
            .contains("add a test"),
        "prompt text must never reach the journal"
    );
    let log = log_entries(dir.path());
    let entry = log
        .iter()
        .find(|e| e["event"] == "prompt")
        .expect("log entry");
    assert_eq!(entry["prompt"], "add a test");
    assert_eq!(entry["kind"], "lifecycle");
    // the turn is now open
    let turn: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".phronesis/journey/turn")).unwrap(),
    )
    .unwrap();
    assert_eq!(turn["open"], true);
}

/// The Claude-only transcript-marker branch. Claude fires no hook on interrupt,
/// so this and the `inflight` branch are the only evidence there is; if
/// `handle_prompt` ever drops `transcript_path` from `PromptContext` the whole
/// branch goes dark with a green suite.
#[test]
fn claude_hook_prompt_after_transcript_marker_is_a_correction() {
    let dir = tempfile::tempdir().unwrap();
    run_claude_hook(
        dir.path(),
        "UserPromptSubmit",
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"go"}"#,
    );
    // The host wrote the interrupt marker into the transcript after the prompt
    // opened the turn. `timestamp` is absent, which `classify_prompt` accepts.
    let transcript = dir.path().join("t.jsonl");
    std::fs::write(
        &transcript,
        "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"[Request interrupted by user]\"}}\n",
    )
    .unwrap();
    let payload = format!(
        r#"{{"hook_event_name":"UserPromptSubmit","session_id":"s1","transcript_path":{},"prompt":"do this instead"}}"#,
        serde_json::Value::String(transcript.display().to_string())
    );
    run_claude_hook(dir.path(), "UserPromptSubmit", &payload);

    let recs = journal_records(dir.path());
    let modes: Vec<&str> = recs
        .iter()
        .filter(|r| r["kind"] == "prompt")
        .map(|r| r["mode"].as_str().unwrap())
        .collect();
    assert_eq!(modes, vec!["fresh", "correction"]);
    let entry = log_entries(dir.path())
        .into_iter()
        .find(|e| e["event"] == "interrupt")
        .expect("interrupt entry");
    assert_eq!(entry["inferred_from"], "transcript");
}

/// The adapter must scrub before the text reaches the log, and must not leak it
/// to stdout either — Plan 1 tests `scrub_prompt` itself, this tests the wiring.
#[test]
fn claude_hook_scrubs_prompt_text_before_the_log_and_never_prints_it() {
    let dir = tempfile::tempdir().unwrap();
    let secret =
        "resume session 0f3c9a1e-1234-4bcd-9ef0-abcdefabcdef and read /home/somebody/notes.txt";
    let payload = format!(
        r#"{{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":{}}}"#,
        serde_json::Value::String(secret.to_string())
    );
    let (_, stdout, _) = run_claude_hook(dir.path(), "UserPromptSubmit", &payload);
    assert!(
        !stdout.contains("0f3c9a1e"),
        "prompt text on stdout: {stdout}"
    );
    let entry = log_entries(dir.path())
        .into_iter()
        .find(|e| e["event"] == "prompt")
        .expect("prompt entry");
    let text = entry["prompt"].as_str().expect("prompt text");
    assert!(
        !text.contains("0f3c9a1e-1234-4bcd-9ef0-abcdefabcdef"),
        "{text}"
    );
    assert!(text.contains("sess-00000000"), "{text}");
    assert!(text.contains("resume session"), "{text}");
}

#[test]
fn claude_hook_stop_records_and_closes_the_turn() {
    let dir = tempfile::tempdir().unwrap();
    run_claude_hook(
        dir.path(),
        "UserPromptSubmit",
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"go"}"#,
    );
    run_claude_hook(
        dir.path(),
        "Stop",
        r#"{"hook_event_name":"Stop","session_id":"s1"}"#,
    );
    assert!(
        journal_records(dir.path())
            .iter()
            .any(|r| r["kind"] == "stop")
    );
    let turn: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".phronesis/journey/turn")).unwrap(),
    )
    .unwrap();
    assert_eq!(turn["open"], false);
    assert_eq!(turn["last_event"], "stop");
    // …and the next prompt is fresh again.
    run_claude_hook(
        dir.path(),
        "UserPromptSubmit",
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"again"}"#,
    );
    let recs = journal_records(dir.path());
    let modes: Vec<&str> = recs
        .iter()
        .filter(|r| r["kind"] == "prompt")
        .map(|r| r["mode"].as_str().unwrap())
        .collect();
    assert_eq!(modes, vec!["fresh", "fresh"]);
}

#[test]
fn claude_hook_subagent_pair_records_duration_and_match() {
    let dir = tempfile::tempdir().unwrap();
    run_claude_hook(
        dir.path(),
        "SubagentStart",
        r#"{"hook_event_name":"SubagentStart","session_id":"s1","agent_id":"a1","agent_type":"Explore"}"#,
    );
    run_claude_hook(
        dir.path(),
        "SubagentStop",
        r#"{"hook_event_name":"SubagentStop","session_id":"s1","agent_id":"a1","agent_type":"Explore"}"#,
    );
    let recs = journal_records(dir.path());
    let start = recs.iter().find(|r| r["kind"] == "subagent_start").unwrap();
    assert_eq!(start["agent"], "a1");
    // `agent_type` is sanitized at the boundary (Plan 1 Task 4): lowercased,
    // then kept only if it matches `[a-z0-9][a-z0-9_.:-]{0,63}`. `Explore` →
    // `explore` in the tag AND in the stored field.
    assert_eq!(start["agent_type"], "explore");
    assert!(
        start["tags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t == "lifecycle:agent:explore")
    );
    let stop = log_entries(dir.path())
        .into_iter()
        .find(|e| e["event"] == "subagent_stop")
        .unwrap();
    assert_eq!(stop["matched_start"], true);
    assert!(stop["duration_secs"].is_u64());

    // An unmatched stop is recorded honestly.
    run_claude_hook(
        dir.path(),
        "SubagentStop",
        r#"{"hook_event_name":"SubagentStop","agent_id":"ghost"}"#,
    );
    let unmatched = log_entries(dir.path())
        .into_iter()
        .rfind(|e| e["event"] == "subagent_stop")
        .unwrap();
    assert_eq!(unmatched["matched_start"], false);
    assert!(unmatched.get("duration_secs").is_none());
}

#[test]
fn claude_hook_session_begin_overwrites_sid_and_truncates_state() {
    let dir = tempfile::tempdir().unwrap();
    run_hook_in(
        "pre-check",
        r#"{"tool_name":"Bash","tool_use_id":"tu-9","tool_input":{"command":"ls"}}"#,
        Some(dir.path()),
    );
    run_claude_hook(
        dir.path(),
        "SessionStart",
        r#"{"hook_event_name":"SessionStart","session_id":"claude-s-42","source":"startup"}"#,
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join(".phronesis/journey/session"))
            .unwrap()
            .trim(),
        "claude-s-42"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join(".phronesis/journey/inflight"))
            .unwrap()
            .trim(),
        ""
    );
}

/// Source gating: `compact` and `fork` continue the session, so its open
/// sub-agents and in-flight tools are real and must survive. Without the gate a
/// mid-session compaction orphans every open sub-agent and discards every
/// in-flight tool — and, because the Codex `SessionStart` matcher widens to `""`
/// in Plan 3, this is reachable on two hosts.
#[test]
fn claude_hook_session_start_on_compact_or_fork_touches_no_state() {
    for source in ["compact", "fork"] {
        let dir = tempfile::tempdir().unwrap();
        run_claude_hook(
            dir.path(),
            "SessionStart",
            r#"{"hook_event_name":"SessionStart","session_id":"claude-s-1","source":"startup"}"#,
        );
        run_claude_hook(
            dir.path(),
            "UserPromptSubmit",
            r#"{"hook_event_name":"UserPromptSubmit","session_id":"claude-s-1","prompt":"go"}"#,
        );
        run_hook_in(
            "pre-check",
            r#"{"tool_name":"Bash","tool_use_id":"tu-live","tool_input":{"command":"sleep 100"}}"#,
            Some(dir.path()),
        );
        run_claude_hook(
            dir.path(),
            "SubagentStart",
            r#"{"hook_event_name":"SubagentStart","session_id":"claude-s-1","agent_id":"a1","agent_type":"Explore"}"#,
        );

        run_claude_hook(
            dir.path(),
            "SessionStart",
            &format!(
                r#"{{"hook_event_name":"SessionStart","session_id":"claude-s-2","source":"{source}"}}"#
            ),
        );

        assert_eq!(
            std::fs::read_to_string(dir.path().join(".phronesis/journey/session"))
                .unwrap()
                .trim(),
            "claude-s-1",
            "{source} must not mint a new sid"
        );
        assert_eq!(
            inflight_keys(dir.path()),
            vec!["tu-live".to_string()],
            "{source}"
        );
        assert!(
            !std::fs::read_to_string(dir.path().join(".phronesis/journey/agents"))
                .unwrap()
                .trim()
                .is_empty(),
            "{source}: the open sub-agent must survive"
        );
        let turn: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join(".phronesis/journey/turn")).unwrap(),
        )
        .unwrap();
        assert_eq!(turn["open"], true, "{source}: the turn is still running");
    }
}

/// Quitting out of an aborted turn must not be recorded as a completed turn, and
/// the session file is **not** truncated: truncation would let any stray hook
/// between sessions mint a throwaway sid.
#[test]
fn claude_hook_session_end_detects_an_interrupt_and_leaves_the_session_file() {
    let dir = tempfile::tempdir().unwrap();
    run_claude_hook(
        dir.path(),
        "SessionStart",
        r#"{"hook_event_name":"SessionStart","session_id":"claude-s-7","source":"startup"}"#,
    );
    run_claude_hook(
        dir.path(),
        "UserPromptSubmit",
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"claude-s-7","prompt":"go"}"#,
    );
    // A tool is still running when the human quits: that is an abort, not a
    // completed turn.
    run_hook_in(
        "pre-check",
        r#"{"tool_name":"Bash","tool_use_id":"tu-q","tool_input":{"command":"sleep 100"}}"#,
        Some(dir.path()),
    );
    let (code, stdout, stderr) = run_claude_hook(
        dir.path(),
        "SessionEnd",
        r#"{"hook_event_name":"SessionEnd","session_id":"claude-s-7"}"#,
    );
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout.trim(), "{}", "SessionEnd prints the empty object");

    let recs = journal_records(dir.path());
    let kinds: Vec<&str> = recs.iter().filter_map(|r| r["kind"].as_str()).collect();
    assert!(kinds.contains(&"interrupt"), "{kinds:?}");
    assert!(
        !kinds.contains(&"stop"),
        "an aborted turn is not a completed one: {kinds:?}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join(".phronesis/journey/session"))
            .unwrap()
            .trim(),
        "claude-s-7",
        "SessionEnd does not truncate the session file"
    );
}

/// The other half: a session that ends with nothing in flight ended cleanly, so
/// it records a `stop`.
#[test]
fn claude_hook_session_end_with_no_evidence_records_a_stop() {
    let dir = tempfile::tempdir().unwrap();
    run_claude_hook(
        dir.path(),
        "UserPromptSubmit",
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"claude-s-8","prompt":"go"}"#,
    );
    run_claude_hook(
        dir.path(),
        "SessionEnd",
        r#"{"hook_event_name":"SessionEnd"}"#,
    );
    let recs = journal_records(dir.path());
    let kinds: Vec<&str> = recs.iter().filter_map(|r| r["kind"].as_str()).collect();
    assert!(kinds.contains(&"stop"), "{kinds:?}");
    assert!(!kinds.contains(&"interrupt"), "{kinds:?}");
}

/// Spec §"A blocked stop is not a stop": when the gate blocks, Claude continues
/// the same turn, so nothing is recorded and the turn stays open. The `stop` is
/// recorded on the `stop_hook_active: true` re-fire — exactly once.
#[test]
fn a_blocked_stop_records_nothing_and_leaves_the_turn_open() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".phronesis/outcomes")).unwrap();
    std::fs::write(dir.path().join(".phronesis/confidence.json"), "{}").unwrap();
    std::fs::write(dir.path().join(".phronesis/outcomes/current"), "unit-1").unwrap();
    run_claude_hook(
        dir.path(),
        "UserPromptSubmit",
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"go"}"#,
    );

    let (_, stdout, _) = run_claude_hook(dir.path(), "Stop", r#"{"hook_event_name":"Stop"}"#);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(stdout.trim()).unwrap()["decision"],
        "block",
        "{stdout}"
    );
    assert!(
        !journal_records(dir.path())
            .iter()
            .any(|r| r["kind"] == "stop"),
        "a blocked stop records nothing"
    );
    let turn: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".phronesis/journey/turn")).unwrap(),
    )
    .unwrap();
    assert_eq!(turn["open"], true, "the turn continues");

    // The re-fire prints `{}` and records exactly one stop.
    let (_, stdout, _) = run_claude_hook(
        dir.path(),
        "Stop",
        r#"{"hook_event_name":"Stop","stop_hook_active":true}"#,
    );
    assert_eq!(stdout.trim(), "{}");
    assert_eq!(
        journal_records(dir.path())
            .iter()
            .filter(|r| r["kind"] == "stop")
            .count(),
        1
    );

    // A steer during the continuation is an intervention, not a `fresh` prompt —
    // which is the whole reason the blocked stop must not close the turn.
    let dir2 = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir2.path().join(".phronesis/outcomes")).unwrap();
    std::fs::write(dir2.path().join(".phronesis/confidence.json"), "{}").unwrap();
    std::fs::write(dir2.path().join(".phronesis/outcomes/current"), "unit-1").unwrap();
    run_claude_hook(
        dir2.path(),
        "UserPromptSubmit",
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"go"}"#,
    );
    run_claude_hook(dir2.path(), "Stop", r#"{"hook_event_name":"Stop"}"#);
    run_claude_hook(
        dir2.path(),
        "UserPromptSubmit",
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"no, do this"}"#,
    );
    let recs = journal_records(dir2.path());
    let modes: Vec<&str> = recs
        .iter()
        .filter(|r| r["kind"] == "prompt")
        .map(|r| r["mode"].as_str().unwrap())
        .collect();
    assert_eq!(
        modes,
        vec!["fresh", "mid_turn"],
        "the intervention must not be lost"
    );
}

/// A blocked `SubagentStop` follows the same rule: no record, and the `agents`
/// entry stays, so the real stop still pairs.
#[test]
fn a_blocked_subagent_stop_records_nothing_and_keeps_the_agents_entry() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".phronesis/outcomes")).unwrap();
    std::fs::write(dir.path().join(".phronesis/confidence.json"), "{}").unwrap();
    std::fs::write(dir.path().join(".phronesis/outcomes/current"), "unit-1").unwrap();
    run_claude_hook(
        dir.path(),
        "SubagentStart",
        r#"{"hook_event_name":"SubagentStart","agent_id":"a1","agent_type":"Explore"}"#,
    );
    run_claude_hook(
        dir.path(),
        "SubagentStop",
        r#"{"hook_event_name":"SubagentStop","agent_id":"a1"}"#,
    );
    assert!(
        !journal_records(dir.path())
            .iter()
            .any(|r| r["kind"] == "subagent_stop"),
        "a blocked subagent stop records nothing"
    );
    let (_, stdout, _) = run_claude_hook(
        dir.path(),
        "SubagentStop",
        r#"{"hook_event_name":"SubagentStop","agent_id":"a1","stop_hook_active":true}"#,
    );
    assert_eq!(stdout.trim(), "{}");
    let stop = log_entries(dir.path())
        .into_iter()
        .find(|e| e["event"] == "subagent_stop")
        .expect("the re-fire records it");
    assert_eq!(
        stop["matched_start"], true,
        "the agents entry survived the block"
    );
}

/// A prompt delivered inside a sub-agent is not the human speaking: `fresh`,
/// untagged, and the parent's turn is untouched.
#[test]
fn a_sub_agent_prompt_is_fresh_untagged_and_moves_no_turn_state() {
    let dir = tempfile::tempdir().unwrap();
    run_claude_hook(
        dir.path(),
        "UserPromptSubmit",
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt_id":"p1","prompt":"go"}"#,
    );
    run_claude_hook(
        dir.path(),
        "UserPromptSubmit",
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","agent_id":"sub-1","prompt":"inner task"}"#,
    );
    let recs = journal_records(dir.path());
    let inner = recs
        .iter()
        .find(|r| r["agent"] == "sub-1")
        .expect("the sub-agent's prompt is recorded");
    assert_eq!(inner["mode"], "fresh");
    assert!(
        !inner["tags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t == "lifecycle:intervention"),
        "{inner}"
    );
    // The parent's turn still points at the parent's prompt.
    let turn: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".phronesis/journey/turn")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        turn["turn_id"], "p1",
        "a sub-agent prompt must not move the parent's turn"
    );
}

#[test]
fn gemini_before_agent_records_a_gemini_prompt() {
    let dir = tempfile::tempdir().unwrap();
    run_claude_hook(
        dir.path(),
        "BeforeAgent",
        r#"{"hook_event_name":"BeforeAgent","session_id":"g1","prompt":"hello"}"#,
    );
    let rec = journal_records(dir.path())
        .into_iter()
        .find(|r| r["kind"] == "prompt")
        .unwrap();
    assert_eq!(rec["host"], "gemini");
    assert_eq!(rec["mode"], "fresh");
}

#[test]
fn pre_pushes_inflight_and_post_pops_it() {
    let dir = tempfile::tempdir().unwrap();
    let pre = r#"{"tool_name":"Bash","tool_use_id":"tu-42","tool_input":{"command":"ls"}}"#;
    run_hook_in("pre-check", pre, Some(dir.path()));
    assert_eq!(inflight_keys(dir.path()), vec!["tu-42".to_string()]);
    let post = r#"{"tool_name":"Bash","tool_use_id":"tu-42","tool_input":{"command":"ls"},
        "tool_response":{"exit_code":0,"stdout":""}}"#;
    run_hook_in("post-check", post, Some(dir.path()));
    assert!(inflight_keys(dir.path()).is_empty());
}

/// Moved here from Task 3 on purpose: it needs the `pre_push_inflight` wiring
/// this task adds, so at Task 3 it could not pass. The `inflight` branch of
/// `classify_prompt` is only reachable once the pre-check pushes.
#[test]
fn claude_hook_prompt_after_inflight_records_interrupt_and_correction() {
    let dir = tempfile::tempdir().unwrap();
    run_claude_hook(
        dir.path(),
        "UserPromptSubmit",
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"first"}"#,
    );
    // A tool call is still in flight when the human speaks again.
    run_hook_in(
        "pre-check",
        r#"{"tool_name":"Bash","tool_use_id":"tu-1","tool_input":{"command":"sleep 100"}}"#,
        Some(dir.path()),
    );
    run_claude_hook(
        dir.path(),
        "UserPromptSubmit",
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"stop, do this instead"}"#,
    );
    let recs = journal_records(dir.path());
    let interrupt = recs
        .iter()
        .find(|r| r["kind"] == "interrupt")
        .expect("interrupt record");
    assert!(
        interrupt["tags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t == "lifecycle:interrupt")
    );
    let recs2 = journal_records(dir.path());
    let modes: Vec<&str> = recs2
        .iter()
        .filter(|r| r["kind"] == "prompt")
        .map(|r| r["mode"].as_str().unwrap())
        .collect();
    assert_eq!(modes, vec!["fresh", "correction"]);
    let log = log_entries(dir.path());
    let entry = log.iter().find(|e| e["event"] == "interrupt").unwrap();
    assert_eq!(entry["inferred_from"], "inflight");
}

/// The push happens before the allowlist match, so a tool Phronesis does not
/// govern still makes the next prompt a correction (spec §"Where the writes
/// happen"). Without this test every inflight case uses an allowlisted tool and
/// a regression that pushed after the allowlist would stay green.
#[test]
fn a_non_allowlisted_tool_still_pushes_and_pops_inflight() {
    let dir = tempfile::tempdir().unwrap();
    let pre = r#"{"tool_name":"WebSearch","tool_use_id":"tu-w","tool_input":{"query":"rust"}}"#;
    run_hook_in("pre-check", pre, Some(dir.path()));
    assert_eq!(inflight_keys(dir.path()), vec!["tu-w".to_string()]);
    let post = r#"{"tool_name":"WebSearch","tool_use_id":"tu-w","tool_input":{"query":"rust"},
        "tool_response":{"results":[]}}"#;
    run_hook_in("post-check", post, Some(dir.path()));
    assert!(inflight_keys(dir.path()).is_empty());
}

/// Gemini supplies no `tool_use_id`, so the key is the hash of tool name plus
/// canonical input. Pre and post must agree on it or the entry leaks.
#[test]
fn a_pair_without_tool_use_id_pops_by_hashed_key() {
    let dir = tempfile::tempdir().unwrap();
    let input = r#"{"command":"echo hi"}"#;
    run_hook_in(
        "pre-check",
        &format!(r#"{{"tool_name":"run_shell_command","tool_input":{input}}}"#),
        Some(dir.path()),
    );
    let keys = inflight_keys(dir.path());
    assert_eq!(keys.len(), 1, "{keys:?}");
    assert!(keys[0].starts_with('h'), "hashed key expected: {keys:?}");
    run_hook_in(
        "post-check",
        &format!(
            r#"{{"tool_name":"run_shell_command","tool_input":{input},"tool_response":{{"exit_code":0}}}}"#
        ),
        Some(dir.path()),
    );
    assert!(inflight_keys(dir.path()).is_empty());
}

/// `post-check` pops by key **regardless of age**: the TTL is a classification
/// rule, not a retention rule. A twenty-minute build must still get its commit
/// detected (spec §Correlation state).
#[test]
fn post_check_pops_an_entry_older_than_the_ttl() {
    let dir = tempfile::tempdir().unwrap();
    // Write the entry directly with a timestamp well past the 900 s TTL: driving
    // a real pre-check cannot produce an old entry without sleeping.
    std::fs::create_dir_all(dir.path().join(".phronesis/journey")).unwrap();
    std::fs::write(
        dir.path().join(".phronesis/journey/inflight"),
        "{\"key\":\"tu-old\",\"tool\":\"Bash\",\"ts\":1,\"agent_id\":null,\"head_before\":null}\n",
    )
    .unwrap();
    run_hook_in(
        "post-check",
        r#"{"tool_name":"Bash","tool_use_id":"tu-old","tool_input":{"command":"sleep 2000"},
            "tool_response":{"exit_code":0}}"#,
        Some(dir.path()),
    );
    assert!(
        inflight_keys(dir.path()).is_empty(),
        "a stale entry is still popped by key"
    );
}

/// The pre-filter runs at PRE as well as POST, so a shell call that cannot be a
/// commit spawns no git process at all (spec §"Success signal: commit" step 1).
/// Observable through `head_before`: present only when the filter matched.
#[test]
fn head_before_is_recorded_only_for_commands_that_could_move_head() {
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "t"]);
    git(&["config", "commit.gpgsign", "false"]);
    std::fs::write(dir.path().join("a.txt"), "one").unwrap();
    git(&["add", "a.txt"]);
    git(&["commit", "-qm", "first"]);

    let entry_for = |payload: &str| -> serde_json::Value {
        run_hook_in("pre-check", payload, Some(dir.path()));
        let line = std::fs::read_to_string(dir.path().join(".phronesis/journey/inflight"))
            .unwrap()
            .lines()
            .next_back()
            .unwrap()
            .to_string();
        serde_json::from_str(&line).unwrap()
    };

    let plain = entry_for(
        r#"{"tool_name":"Bash","tool_use_id":"tu-1","tool_input":{"command":"cargo build"}}"#,
    );
    assert!(
        plain["head_before"].is_null(),
        "no git process for a non-commit command: {plain}"
    );
    let committing = entry_for(
        r#"{"tool_name":"Bash","tool_use_id":"tu-2","tool_input":{"command":"git commit -am x"}}"#,
    );
    assert!(committing["head_before"].is_string(), "{committing}");

    // Not a shell tool at all: never.
    let edit = entry_for(
        r#"{"tool_name":"Edit","tool_use_id":"tu-3","tool_input":{"file_path":"a.txt","old_string":"one","new_string":"two"}}"#,
    );
    assert!(edit["head_before"].is_null(), "{edit}");
}

#[test]
fn blocked_pre_check_pops_its_own_inflight_entry() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{"id":"no-rm","phase":"pre","priority":1,
            "when":[{"bash_command_matches":"rm -rf"}],
            "then":{"block":"never"}}]}"#,
    );
    let payload =
        r#"{"tool_name":"Bash","tool_use_id":"tu-block","tool_input":{"command":"rm -rf /"}}"#;
    let (code, stderr) = run_hook_in("pre-check", payload, Some(dir.path()));
    assert_eq!(code, 2, "{stderr}");
    assert!(
        inflight_keys(dir.path()).is_empty(),
        "a block is not an interrupt: {:?}",
        inflight_keys(dir.path())
    );
}

#[test]
fn post_check_records_a_commit_when_head_moved() {
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "t"]);
    // A developer's global `commit.gpgsign = true` would otherwise fail every
    // commit in this test.
    git(&["config", "commit.gpgsign", "false"]);
    std::fs::write(dir.path().join("a.txt"), "one").unwrap();
    git(&["add", "a.txt"]);
    git(&["commit", "-qm", "first"]);

    let pre = r#"{"tool_name":"Bash","tool_use_id":"tu-c","tool_input":{"command":"git commit -am second"}}"#;
    run_hook_in("pre-check", pre, Some(dir.path()));
    // the command actually runs between pre and post
    std::fs::write(dir.path().join("a.txt"), "two").unwrap();
    git(&["commit", "-qam", "second"]);
    let post = r#"{"tool_name":"Bash","tool_use_id":"tu-c","tool_input":{"command":"git commit -am second"},
        "tool_response":{"exit_code":0,"stdout":""}}"#;
    run_hook_in("post-check", post, Some(dir.path()));

    let commit = journal_records(dir.path())
        .into_iter()
        .find(|r| r["kind"] == "commit")
        .expect("commit record");
    assert_eq!(commit["tool"], "__lifecycle");
    let entry = log_entries(dir.path())
        .into_iter()
        .find(|e| e["event"] == "commit")
        .unwrap();
    assert_eq!(entry["tool_use_id"], "tu-c");
    let sha = entry["sha"].as_str().unwrap();
    // 40 for SHA-1, 64 under `--object-format=sha256`; pinning 40 would fail on
    // a host configured for SHA-256.
    assert!(matches!(sha.len(), 40 | 64), "unexpected sha: {sha}");
    assert!(sha.chars().all(|c| c.is_ascii_hexdigit()), "{sha}");
    assert_ne!(entry["sha"], entry["head_before"]);
    assert!(
        entry.get("confidence_band").is_none(),
        "no confidence.json, no band"
    );
}

/// The band is present exactly when confidence scoring is on and a unit is
/// open. The spec promises the field, so the absence case above and this one
/// together pin both halves.
#[test]
fn a_commit_carries_the_confidence_band_when_scoring_is_enabled() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".phronesis/outcomes")).unwrap();
    std::fs::write(dir.path().join(".phronesis/confidence.json"), "{}").unwrap();
    std::fs::write(dir.path().join(".phronesis/outcomes/current"), "unit-1").unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "t"]);
    git(&["config", "commit.gpgsign", "false"]);
    std::fs::write(dir.path().join("a.txt"), "one").unwrap();
    git(&["add", "a.txt"]);
    git(&["commit", "-qm", "first"]);

    let cmd = r#"{"tool_name":"Bash","tool_use_id":"tu-b","tool_input":{"command":"git commit -am second"}"#;
    run_hook_in("pre-check", &format!("{cmd}}}"), Some(dir.path()));
    std::fs::write(dir.path().join("a.txt"), "two").unwrap();
    git(&["commit", "-qam", "second"]);
    run_hook_in(
        "post-check",
        &format!(r#"{cmd},"tool_response":{{"exit_code":0}}}}"#),
        Some(dir.path()),
    );

    let entry = log_entries(dir.path())
        .into_iter()
        .find(|e| e["event"] == "commit")
        .expect("commit entry");
    let band = entry["confidence_band"].as_str().expect("a band");
    assert!(matches!(band, "low" | "medium" | "high"), "{band}");
}

#[test]
fn non_commit_shell_call_records_nothing() {
    let dir = tempfile::tempdir().unwrap();
    run_hook_in(
        "pre-check",
        r#"{"tool_name":"Bash","tool_use_id":"tu-n","tool_input":{"command":"echo hi"}}"#,
        Some(dir.path()),
    );
    run_hook_in(
        "post-check",
        r#"{"tool_name":"Bash","tool_use_id":"tu-n","tool_input":{"command":"echo hi"},
            "tool_response":{"exit_code":0}}"#,
        Some(dir.path()),
    );
    assert!(
        !journal_records(dir.path())
            .iter()
            .any(|r| r["kind"] == "commit")
    );
}

#[test]
fn invoke_agent_derives_a_subagent_pair_before_the_allowlist() {
    let dir = tempfile::tempdir().unwrap();
    let pre =
        r#"{"tool_name":"invoke_agent","tool_input":{"agent_name":"reviewer","prompt":"look"}}"#;
    run_hook_in("pre-check", pre, Some(dir.path()));
    let start = journal_records(dir.path())
        .into_iter()
        .find(|r| r["kind"] == "subagent_start")
        .expect("subagent_start");
    assert_eq!(start["host"], "gemini");
    assert_eq!(start["agent_type"], "reviewer");
    assert!(start["agent"].as_str().unwrap().contains(':'));

    let post = r#"{"tool_name":"invoke_agent","tool_input":{"agent_name":"reviewer","prompt":"look"},
        "tool_response":{"output":"done"}}"#;
    run_hook_in("post-check", post, Some(dir.path()));
    let stop = log_entries(dir.path())
        .into_iter()
        .find(|e| e["event"] == "subagent_stop")
        .expect("subagent_stop");
    assert_eq!(stop["matched_start"], true);
    assert_eq!(stop["agent_type"], "reviewer");

    // The tool record for `invoke_agent` carries the synthetic path, never the
    // sub-agent prompt — which would put content in the journal, and which
    // `journey_distinct` on `path` would then see.
    let tool = journal_records(dir.path())
        .into_iter()
        .find(|r| r["tool"] == "invoke_agent")
        .expect("a tool record for invoke_agent");
    assert_eq!(tool["path"], "<invoke_agent>");
    assert!(
        !std::fs::read_to_string(dir.path().join(".phronesis/journey/events.jsonl"))
            .unwrap()
            .contains("look"),
        "the sub-agent prompt must not reach the journal"
    );
}

/// `agent_name` is model-generated free text and becomes a journal tag, hence a
/// RETE fact. Plan 1 sanitizes it; this pins that the derivation actually goes
/// through that path rather than stamping the raw string.
#[test]
fn a_hostile_invoke_agent_name_is_sanitized_away() {
    let dir = tempfile::tempdir().unwrap();
    run_hook_in(
        "pre-check",
        r#"{"tool_name":"invoke_agent","tool_input":{"agent_name":"kalpa:evil name; rm -rf /","prompt":"x"}}"#,
        Some(dir.path()),
    );
    let start = journal_records(dir.path())
        .into_iter()
        .find(|r| r["kind"] == "subagent_start")
        .expect("subagent_start");
    assert!(
        start.get("agent_type").is_none() || start["agent_type"].is_null(),
        "{start}"
    );
    assert!(
        !start["tags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t.as_str().unwrap().starts_with("lifecycle:agent:")),
        "no tag at all rather than a hostile one: {start}"
    );

    // And a merely differently-cased one is normalized, not dropped.
    run_hook_in(
        "pre-check",
        r#"{"tool_name":"invoke_agent","tool_input":{"agent_name":"Code-Reviewer","prompt":"x"}}"#,
        Some(dir.path()),
    );
    let second = journal_records(dir.path())
        .into_iter()
        .rfind(|r| r["kind"] == "subagent_start")
        .unwrap();
    assert_eq!(second["agent_type"], "code-reviewer");
}

/// A blocked `invoke_agent` pre-check pops the `agents` entry it just pushed and
/// writes no `subagent_start`: a sub-agent that never ran must not leave a
/// dangling entry for the next real stop to pop LIFO (spec §"Where the writes
/// happen").
#[test]
fn a_blocked_invoke_agent_pre_check_pops_its_agents_entry() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{"id":"no-agents","phase":"pre","priority":1,
            "when":[{"change_type":"invoke_agent"}],
            "then":{"block":"no sub-agents here"}}]}"#,
    );
    let (code, stderr) = run_hook_in(
        "pre-check",
        r#"{"tool_name":"invoke_agent","tool_input":{"agent_name":"reviewer","prompt":"look"}}"#,
        Some(dir.path()),
    );
    assert_eq!(code, 2, "{stderr}");
    assert!(
        inflight_keys(dir.path()).is_empty(),
        "the inflight entry is popped too"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join(".phronesis/journey/agents"))
            .unwrap_or_default()
            .trim(),
        "",
        "no dangling open sub-agent"
    );

    // A later real stop must therefore find nothing to pair with, rather than
    // popping the ghost LIFO and reporting a wrong duration.
    run_hook_in(
        "post-check",
        r#"{"tool_name":"invoke_agent","tool_input":{"agent_name":"other","prompt":"x"},
            "tool_response":{"output":"done"}}"#,
        Some(dir.path()),
    );
    let stop = log_entries(dir.path())
        .into_iter()
        .find(|e| e["event"] == "subagent_stop")
        .expect("subagent_stop");
    assert_eq!(stop["matched_start"], false);
}

#[test]
fn tool_records_are_written_at_journal_v2() {
    let dir = tempfile::tempdir().unwrap();
    run_hook_in(
        "post-check",
        r#"{"tool_name":"Edit","tool_input":{"file_path":"src/a.rs","old_string":"a","new_string":"b"}}"#,
        Some(dir.path()),
    );
    let rec = journal_records(dir.path())
        .into_iter()
        .find(|r| r["tool"] == "Edit")
        .expect("tool record");
    assert_eq!(rec["v"], 2);
}

/// A sweep over every event × committed fixture: exit 0 and parseable JSON on
/// stdout, every time. The per-event tests above each pin one shape; this pins
/// that no combination crashes or prints something Gemini would turn into a
/// user-visible `systemMessage`.
///
/// NOTE: the fixtures under `claude/raw/` are currently synthetic (see
/// `raw/README.md`). This sweep validates that the adapter handles them
/// gracefully; the human should replace them with real captures per Task 1.
#[test]
fn every_event_and_fixture_combination_exits_zero_with_json_stdout() {
    let raw_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/payloads/claude/raw");
    let events = [
        "UserPromptSubmit",
        "SessionStart",
        "SessionEnd",
        "SubagentStart",
        "SubagentStop",
        "Stop",
        "BeforeAgent",
        "AfterAgent",
    ];
    let mut fixtures: Vec<(String, String)> = Vec::new();
    for entry in std::fs::read_dir(&raw_dir).expect("captured fixtures") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
            let body = std::fs::read_to_string(&path).expect("fixture");
            fixtures.push((
                path.file_name().unwrap().to_string_lossy().to_string(),
                body,
            ));
        }
    }
    assert!(
        !fixtures.is_empty(),
        "no captured fixtures under {}",
        raw_dir.display()
    );

    for event in events {
        for (name, body) in &fixtures {
            let dir = tempfile::tempdir().unwrap();
            // The argument and the payload's own `hook_event_name` disagree on
            // purpose: the payload wins, and neither path may crash.
            let (code, stdout, stderr) = run_claude_hook(dir.path(), event, body);
            assert_eq!(code, 0, "{event} × {name}: {stderr}");
            let v: serde_json::Value = serde_json::from_str(stdout.trim())
                .unwrap_or_else(|e| panic!("{event} × {name}: {e}: {stdout:?}"));
            assert!(v.is_object(), "{event} × {name}: {stdout:?}");
        }
    }
}

/// `last_assistant_message`, `prompt_response` and transcript paths are read for
/// decisions and dropped at the adapter boundary. Nothing persists them.
#[test]
fn no_log_entry_carries_a_transcript_path_or_an_assistant_message() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = "/home/dev/.claude/projects/p/claude-s-001.jsonl";
    for (event, payload) in [
        (
            "UserPromptSubmit",
            format!(r#"{{"hook_event_name":"UserPromptSubmit","session_id":"s1","transcript_path":"{transcript}","prompt":"go"}}"#),
        ),
        (
            "SubagentStop",
            format!(r#"{{"hook_event_name":"SubagentStop","session_id":"s1","agent_id":"a1","agent_transcript_path":"{transcript}","last_assistant_message":"zzz-assistant-text"}}"#),
        ),
        (
            "AfterAgent",
            r#"{"hook_event_name":"AfterAgent","session_id":"g1","prompt":"hi","prompt_response":"zzz-response-text"}"#.to_string(),
        ),
    ] {
        run_claude_hook(dir.path(), event, &payload);
    }
    let log = std::fs::read_to_string(dir.path().join(".phronesis/log.jsonl")).unwrap();
    assert!(
        !log.contains(".jsonl"),
        "no transcript path in the log: {log}"
    );
    assert!(!log.contains("zzz-assistant-text"), "{log}");
    assert!(!log.contains("zzz-response-text"), "{log}");
    let journal =
        std::fs::read_to_string(dir.path().join(".phronesis/journey/events.jsonl")).unwrap();
    assert!(!journal.contains(".jsonl"), "{journal}");
}

fn run_hook_at(root: &Path, args: &[&str], payload: &str) -> (i32, String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(args)
        .env("PHRONESIS_PROJECT_ROOT", root)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn phr-mcp");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn assert_allow_stdout(stdout: &str) {
    let t = stdout.trim();
    if t.is_empty() {
        return;
    }
    let v: Value = serde_json::from_str(t)
        .unwrap_or_else(|e| panic!("pre/post stdout must be JSON when non-empty: {t:?}: {e}"));
    assert!(v.is_object(), "stdout must be a JSON object: {t}");
}

fn lifecycle_records(root: &Path) -> Vec<Value> {
    let path = root.join(".phronesis/journey/events.jsonl");
    let body = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
    body.lines()
        .map(|l| serde_json::from_str::<Value>(l).expect("journal line"))
        .filter(|r| r["tool"] == "__lifecycle")
        .collect()
}

fn lifecycle_log(root: &Path) -> Vec<Value> {
    let body = std::fs::read_to_string(root.join(".phronesis/log.jsonl")).expect("action log");
    body.lines()
        .map(|l| serde_json::from_str::<Value>(l).expect("log line"))
        .filter(|e| e["kind"] == "lifecycle")
        .collect()
}

#[test]
fn gemini_invoke_agent_pairs_a_subagent_start_and_stop() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let before = r#"{"hook_event_name":"BeforeTool","session_id":"g1","tool_name":"invoke_agent","tool_input":{"agent_name":"codebase-investigator","prompt":"x"}}"#;
    let (code, stdout, stderr) = run_hook_at(root, &["pre-check"], before);
    assert_eq!(code, 0, "pre-check must allow invoke_agent: {stderr}");
    assert_allow_stdout(&stdout);

    let after = r#"{"hook_event_name":"AfterTool","session_id":"g1","tool_name":"invoke_agent","tool_input":{"agent_name":"codebase-investigator","prompt":"x"},"tool_response":{"output":"done"}}"#;
    let (code, stdout, stderr) = run_hook_at(root, &["post-check"], after);
    assert_eq!(code, 0, "post-check must succeed: {stderr}");
    assert_allow_stdout(&stdout);

    let recs = lifecycle_records(root);
    let starts: Vec<&Value> = recs
        .iter()
        .filter(|r| r["kind"] == "subagent_start")
        .collect();
    let stops: Vec<&Value> = recs
        .iter()
        .filter(|r| r["kind"] == "subagent_stop")
        .collect();
    assert_eq!(starts.len(), 1, "exactly one subagent_start: {recs:?}");
    assert_eq!(stops.len(), 1, "exactly one subagent_stop: {recs:?}");
    for r in [starts[0], stops[0]] {
        assert_eq!(r["host"], "gemini");
        assert_eq!(r["path"], "");
        assert_eq!(r["agent_type"], "codebase-investigator");
        let tags = r["tags"].as_array().expect("tags");
        assert!(
            tags.iter()
                .any(|t| t == "lifecycle:agent:codebase-investigator"),
            "agent tag missing: {r}"
        );
    }
    assert_eq!(
        starts[0]["agent"], stops[0]["agent"],
        "the stop must pop the id the start pushed"
    );

    let log = lifecycle_log(root);
    let stop_entry = log
        .iter()
        .find(|e| e["event"] == "subagent_stop")
        .expect("subagent_stop action-log entry");
    assert_eq!(stop_entry["agent_type"], "codebase-investigator");
    assert_eq!(stop_entry["matched_start"], true);
    assert_eq!(stop_entry["host"], "gemini");
    assert_eq!(
        log.iter()
            .filter(|e| e["event"] == "subagent_start")
            .count(),
        1
    );

    let tool_paths: Vec<String> =
        std::fs::read_to_string(root.join(".phronesis/journey/events.jsonl"))
            .unwrap()
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|r| r["tool"] == "invoke_agent")
            .map(|r| r["path"].as_str().unwrap_or_default().to_string())
            .collect();
    assert!(!tool_paths.is_empty(), "invoke_agent is in both allowlists");
    assert!(
        tool_paths.iter().all(|p| p == "<invoke_agent>"),
        "{tool_paths:?}"
    );
}

#[test]
fn a_snake_case_gemini_agent_name_survives_and_a_hostile_one_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let before = r#"{"hook_event_name":"BeforeTool","session_id":"g1","tool_name":"invoke_agent","tool_input":{"agent_name":"codebase_investigator","prompt":"x"}}"#;
    let (code, _, stderr) = run_hook_at(root, &["pre-check"], before);
    assert_eq!(code, 0, "{stderr}");
    let start = lifecycle_records(root)
        .into_iter()
        .find(|r| r["kind"] == "subagent_start")
        .expect("the pair is derived");
    assert_eq!(start["agent_type"], "codebase_investigator", "{start}");
    assert!(
        start["tags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t == "lifecycle:agent:codebase_investigator"),
        "snake_case survives as a tag: {start}"
    );

    let dir2 = tempfile::tempdir().unwrap();
    let root2 = dir2.path();
    let hostile = r#"{"hook_event_name":"BeforeTool","session_id":"g2","tool_name":"invoke_agent","tool_input":{"agent_name":"../../etc/passwd","prompt":"x"}}"#;
    let (code, _, stderr) = run_hook_at(root2, &["pre-check"], hostile);
    assert_eq!(code, 0, "{stderr}");
    let start = lifecycle_records(root2)
        .into_iter()
        .find(|r| r["kind"] == "subagent_start")
        .expect("the pair is still derived");
    assert!(
        start.get("agent_type").is_none() || start["agent_type"].is_null(),
        "an unsanitizable name is stored as absent: {start}"
    );
    assert!(
        !start["tags"].as_array().unwrap().iter().any(|t| {
            t.as_str()
                .unwrap_or_default()
                .starts_with("lifecycle:agent:")
        }),
        "and carries no agent tag: {start}"
    );
}

const G_SESSION_START: &str =
    r#"{"hook_event_name":"SessionStart","session_id":"g1","source":"startup"}"#;
const G_PROMPT_1: &str =
    r#"{"hook_event_name":"BeforeAgent","session_id":"g1","prompt":"add a test"}"#;
const G_PROMPT_2: &str =
    r#"{"hook_event_name":"BeforeAgent","session_id":"g1","prompt":"no, use a temp dir"}"#;
const G_AFTER_AGENT: &str = r#"{"hook_event_name":"AfterAgent","session_id":"g1","prompt":"add a test","prompt_response":"done","stop_hook_active":false}"#;

fn assert_json_object_stdout(stdout: &str) {
    let v: Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("Gemini needs JSON on stdout, got {stdout:?}: {e}"));
    assert!(v.is_object(), "stdout must be a JSON object: {stdout}");
    assert_eq!(v, serde_json::json!({}), "bare project renders no context");
}

#[test]
fn gemini_second_prompt_without_after_agent_is_an_interrupt_and_correction() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    for (args, payload) in [
        (["claude-hook", "SessionStart"], G_SESSION_START),
        (["claude-hook", "BeforeAgent"], G_PROMPT_1),
        (["claude-hook", "BeforeAgent"], G_PROMPT_2),
    ] {
        let (code, stdout, stderr) = run_hook_at(root, &args, payload);
        assert_eq!(code, 0, "{args:?} must exit 0: {stderr}");
        assert_json_object_stdout(&stdout);
    }

    let recs = lifecycle_records(root);
    let kinds: Vec<&str> = recs.iter().filter_map(|r| r["kind"].as_str()).collect();
    assert_eq!(
        kinds,
        ["prompt", "interrupt", "prompt"],
        "a missing AfterAgent means the turn was aborted: {recs:?}"
    );
    assert_eq!(recs[0]["mode"], "fresh");
    assert_eq!(recs[2]["mode"], "correction");
    assert_eq!(recs[2]["host"], "gemini");
    assert!(
        recs[2]["tags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t == "lifecycle:prompt:correction"),
        "correction tag missing: {}",
        recs[2]
    );
    let journal_json = serde_json::to_string(&recs).unwrap();
    assert!(
        !journal_json.contains("add a test"),
        "the journal must never carry prompt text: {recs:?}"
    );
    assert!(
        !journal_json.contains("temp dir"),
        "the journal must never carry prompt text: {recs:?}"
    );

    let log = lifecycle_log(root);
    let interrupt = log
        .iter()
        .find(|e| e["event"] == "interrupt")
        .expect("interrupt entry");
    assert_eq!(interrupt["inferred_from"], "open_turn");
    assert_eq!(interrupt["host"], "gemini");
    let correction = log
        .iter()
        .rfind(|e| e["event"] == "prompt")
        .expect("prompt entry");
    assert_eq!(correction["mode"], "correction");
    assert_eq!(correction["prompt"], "no, use a temp dir");
}

#[test]
fn gemini_after_agent_closes_the_turn_so_the_next_prompt_is_fresh() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    for (args, payload) in [
        (["claude-hook", "SessionStart"], G_SESSION_START),
        (["claude-hook", "BeforeAgent"], G_PROMPT_1),
        (["claude-hook", "AfterAgent"], G_AFTER_AGENT),
        (["claude-hook", "BeforeAgent"], G_PROMPT_2),
    ] {
        let (code, stdout, stderr) = run_hook_at(root, &args, payload);
        assert_eq!(code, 0, "{args:?} must exit 0: {stderr}");
        assert_json_object_stdout(&stdout);
    }

    let recs = lifecycle_records(root);
    let kinds: Vec<&str> = recs.iter().filter_map(|r| r["kind"].as_str()).collect();
    assert_eq!(
        kinds,
        ["prompt", "stop", "prompt"],
        "no interrupt: {recs:?}"
    );
    assert_eq!(recs[0]["mode"], "fresh");
    assert_eq!(recs[2]["mode"], "fresh", "AfterAgent must close the turn");
    assert!(
        lifecycle_log(root)
            .iter()
            .all(|e| e["event"] != "interrupt"),
        "a completed turn must not infer an interrupt"
    );
}

#[test]
fn gemini_session_end_records_a_stop_and_closes_the_turn() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    const G_SESSION_END: &str = r#"{"hook_event_name":"SessionEnd","session_id":"g1"}"#;

    for (args, payload) in [
        (["claude-hook", "SessionStart"], G_SESSION_START),
        (["claude-hook", "BeforeAgent"], G_PROMPT_1),
        (["claude-hook", "SessionEnd"], G_SESSION_END),
        (["claude-hook", "BeforeAgent"], G_PROMPT_2),
    ] {
        let (code, stdout, stderr) = run_hook_at(root, &args, payload);
        assert_eq!(code, 0, "{args:?} must exit 0: {stderr}");
        assert_json_object_stdout(&stdout);
    }

    let recs = lifecycle_records(root);
    let kinds: Vec<&str> = recs.iter().filter_map(|r| r["kind"].as_str()).collect();
    assert_eq!(kinds, ["prompt", "stop", "prompt"], "{recs:?}");
    assert_eq!(recs[1]["host"], "gemini");
    assert_eq!(recs[2]["mode"], "fresh");
    assert!(
        lifecycle_log(root)
            .iter()
            .all(|e| e["event"] != "interrupt"),
        "{recs:?}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join(".phronesis/journey/session"))
            .unwrap()
            .trim(),
        "g1"
    );
}

// --- subject on hook log entries (Task 3) ---

/// Read the last `pre_check` entry from a project's action log.
fn last_pre_check(dir: &Path) -> serde_json::Value {
    std::fs::read_to_string(dir.join(".phronesis/log.jsonl"))
        .expect("action log")
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .rfind(|v| v["event"] == "pre_check")
        .expect("a pre_check entry")
}

/// "`pre_check` / `post_check` action-log entries gain `subject` when a unit is
/// open. Today only journal records carry it, so 'which rules fired for this
/// work item' is not answerable from the log" (spec §"Work items", 2).
#[test]
fn pre_check_log_entry_carries_the_open_work_unit() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{"id":"policy","phase":"pre","priority":1,
            "when":[{"file_path_matches":"vendor/"}],
            "then":{"block":"vendored code is off limits"}}]}"#,
    );
    std::fs::create_dir_all(dir.path().join(".phronesis/outcomes")).unwrap();
    std::fs::write(dir.path().join(".phronesis/outcomes/current"), "item-7").unwrap();

    let payload = r#"{"tool_name":"Edit","tool_input":{"file_path":"src/lib.rs","old_string":"a","new_string":"b"}}"#;
    let (code, stderr) = run_hook_in("pre-check", payload, Some(dir.path()));
    assert_eq!(code, 0, "stderr: {stderr}");

    let entry = last_pre_check(dir.path());
    assert_eq!(entry["subject"], "item-7", "{entry}");
    assert_eq!(entry["exit"], 0);
}

/// No open unit → no `subject` key at all. An empty string or a `null` would
/// make every consumer special-case it; absence is the existing convention for
/// every other optional field on a log entry.
#[test]
fn pre_check_log_entry_omits_subject_when_no_unit_is_open() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{"id":"policy","phase":"pre","priority":1,
            "when":[{"file_path_matches":"vendor/"}],
            "then":{"block":"vendored code is off limits"}}]}"#,
    );
    let payload = r#"{"tool_name":"Edit","tool_input":{"file_path":"src/lib.rs","old_string":"a","new_string":"b"}}"#;
    let (code, stderr) = run_hook_in("pre-check", payload, Some(dir.path()));
    assert_eq!(code, 0, "stderr: {stderr}");
    let entry = last_pre_check(dir.path());
    assert!(entry.get("subject").is_none(), "{entry}");
}

/// The blocked path logs too, and must carry the subject: a block is exactly
/// the kind of rule evaluation the work-item report exists to show.
#[test]
fn a_blocked_pre_check_still_carries_the_subject() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{"id":"policy","phase":"pre","priority":1,
            "when":[{"file_path_matches":"src"}],
            "then":{"block":"project policy"}}]}"#,
    );
    std::fs::create_dir_all(dir.path().join(".phronesis/outcomes")).unwrap();
    std::fs::write(dir.path().join(".phronesis/outcomes/current"), "item-8").unwrap();

    let payload = r#"{"tool_name":"Edit","tool_input":{"file_path":"src/lib.rs","old_string":"a","new_string":"b"}}"#;
    let (code, _stderr) = run_hook_in("pre-check", payload, Some(dir.path()));
    assert_eq!(code, 2);
    let entry = last_pre_check(dir.path());
    assert_eq!(entry["subject"], "item-8", "{entry}");
    assert_eq!(entry["exit"], 2);
    assert_eq!(entry["consequences"][0]["rule_id"], "policy", "{entry}");
}
