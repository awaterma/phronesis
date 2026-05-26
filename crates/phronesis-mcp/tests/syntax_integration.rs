//! End-to-end: a rule using `function_returns_result_string` fires when the
//! hook sees an edit that adds such a function.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

fn write_rules_file(dir: &Path, contents: &str) {
    let ep = dir.join(".phronesis");
    std::fs::create_dir_all(&ep).unwrap();
    std::fs::write(ep.join("rules.json"), contents).unwrap();
}

fn run_hook_with_root(payload: &str, root: &Path) -> (i32, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_phr-mcp"));
    cmd.arg("pre-check")
        .env("PHRONESIS_PROJECT_ROOT", root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let _ = stdin.write_all(payload.as_bytes());
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    (
        out.valueus.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn result_string_rule_blocks_offending_function() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "fn existing() -> u32 { 0 }\n",
    )
    .unwrap();

    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"no-string-error","phase":"pre","priority":10,
            "conditions":[
                {"predicate":"function_returns_result_string","args":["?file","?fn"]}
            ],
            "actions":[{
                "action_type":"constraint_violation",
                "params":["Function `?fn` in ?file uses Result<_, String>. Define a thiserror enum."]
            }]
        }]}"#,
    );

    let payload = r#"{
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/lib.rs",
            "old_string": "fn existing() -> u32 { 0 }",
            "new_string": "fn existing() -> u32 { 0 }\nfn bad() -> Result<u32, String> { Ok(0) }"
        }
    }"#;
    let (code, stderr) = run_hook_with_root(payload, dir.path());
    assert_eq!(code, 2, "should block: {}", stderr);
    assert!(stderr.contains("bad"), "stderr: {}", stderr);
    assert!(stderr.contains("thiserror"), "stderr: {}", stderr);
}

#[test]
fn result_string_rule_allows_proper_error_type() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "fn existing() -> u32 { 0 }\n",
    )
    .unwrap();

    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"no-string-error","phase":"pre","priority":10,
            "conditions":[
                {"predicate":"function_returns_result_string","args":["?file","?fn"]}
            ],
            "actions":[{
                "action_type":"constraint_violation",
                "params":["bad: ?fn"]
            }]
        }]}"#,
    );

    let payload = r#"{
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/lib.rs",
            "old_string": "fn existing() -> u32 { 0 }",
            "new_string": "fn existing() -> u32 { 0 }\nfn ok() -> Result<u32, MyError> { Ok(0) }"
        }
    }"#;
    let (code, stderr) = run_hook_with_root(payload, dir.path());
    assert_eq!(code, 0, "should allow: {}", stderr);
}

#[test]
fn result_string_rule_ignores_test_blocks() {
    // Regression test: a Result<_, String> function inside #[cfg(test)] mod
    // tests must NOT trigger the production rule. The hook strips test blocks
    // before running the values analyzer.
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "fn existing() -> u32 { 0 }\n",
    )
    .unwrap();

    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"no-string-error","phase":"pre","priority":10,
            "conditions":[
                {"predicate":"function_returns_result_string","args":["?file","?fn"]}
            ],
            "actions":[{
                "action_type":"constraint_violation",
                "params":["bad: ?fn"]
            }]
        }]}"#,
    );

    let payload = r#"{
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/lib.rs",
            "old_string": "fn existing() -> u32 { 0 }",
            "new_string": "fn existing() -> u32 { 0 }\n#[cfg(test)]\nmod tests {\n    fn helper() -> Result<u32, String> { Ok(0) }\n}\n"
        }
    }"#;
    let (code, stderr) = run_hook_with_root(payload, dir.path());
    assert_eq!(
        code, 0,
        "test-scoped Result<_, String> must not block; stderr: {}",
        stderr
    );
}

fn run_post_hook_with_root(payload: &str, root: &Path) -> (i32, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_phr-mcp"));
    cmd.arg("post-check")
        .env("PHRONESIS_PROJECT_ROOT", root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let _ = stdin.write_all(payload.as_bytes());
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    (
        out.valueus.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

// NOTE on action message phrasing in these tests:
// There is a pre-existing bug in the upstream `phronesis` crate where mixed
// action strings (e.g. `"Public ?fn takes ?param: &String"`) do not have
// their `?var` placeholders substituted with bound values — variables remain
// literal in the rendered message. To keep these integration tests robust to
// that bug (and to its eventual fix), action params are phrased as plain
// English without `?var` interpolation, and assertions check for literal
// substrings of the action text rather than substituted variable bindings.

#[test]
fn public_fn_with_string_ref_warns() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    // Post-check reads file content from disk (not the payload), so the
    // on-disk content must reflect the post-edit state.
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "pub fn greet(name: &String) {}\n",
    )
    .unwrap();

    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"warn-pub-str-ref","phase":"post","priority":10,
            "conditions":[
                {"predicate":"function_is_public","args":["?file","?fn"]},
                {"predicate":"function_param_type","args":["?file","?fn","?param","&String"]}
            ],
            "actions":[{
                "action_type":"constraint_warning",
                "params":["Public fn takes &String — prefer &str"]
            }]
        }]}"#,
    );

    let payload = r#"{
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/lib.rs",
            "old_string": "",
            "new_string": "pub fn greet(name: &String) {}"
        }
    }"#;
    let (code, stderr) = run_post_hook_with_root(payload, dir.path());
    assert_eq!(code, 1, "warning should exit 1: {}", stderr);
    assert!(stderr.contains("&str"), "stderr: {}", stderr);
}

#[test]
fn public_fn_with_vec_ref_warns() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "pub fn process(items: &Vec<u8>) {}\n",
    )
    .unwrap();

    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"warn-pub-vec-ref","phase":"post","priority":10,
            "conditions":[
                {"predicate":"function_is_public","args":["?file","?fn"]},
                {"predicate":"function_param_is_vec_ref","args":["?file","?fn","?param"]}
            ],
            "actions":[{
                "action_type":"constraint_warning",
                "params":["Public fn takes &Vec<T> — prefer &[T]"]
            }]
        }]}"#,
    );

    let payload = r#"{
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/lib.rs",
            "old_string": "",
            "new_string": "pub fn process(items: &Vec<u8>) {}"
        }
    }"#;
    let (code, stderr) = run_post_hook_with_root(payload, dir.path());
    assert_eq!(code, 1, "warning should exit 1: {}", stderr);
    assert!(stderr.contains("&[T]"), "stderr: {}", stderr);
}

#[test]
fn clone_count_warning_fires() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "fn copy_heavy(a: &String, b: &String) { let _x = a.clone(); let _y = b.clone(); }\n",
    )
    .unwrap();

    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"warn-clone-count","phase":"post","priority":5,
            "conditions":[
                {"predicate":"function_clone_count","args":["?file","?fn","?count"]}
            ],
            "actions":[{
                "action_type":"constraint_warning",
                "params":["clone usage detected — review for borrows"]
            }]
        }]}"#,
    );

    let payload = r#"{
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/lib.rs",
            "old_string": "",
            "new_string": "fn copy_heavy(a: &String, b: &String) { let _x = a.clone(); let _y = b.clone(); }"
        }
    }"#;
    let (code, stderr) = run_post_hook_with_root(payload, dir.path());
    assert_eq!(code, 1, "warning should exit 1: {}", stderr);
    assert!(
        stderr.contains("clone usage detected"),
        "stderr: {}",
        stderr
    );
}

#[test]
fn struct_derives_warning_fires() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "#[derive(Clone)]\npub struct Foo { x: u32 }\n",
    )
    .unwrap();

    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"warn-clone-without-debug","phase":"post","priority":5,
            "conditions":[
                {"predicate":"struct_derives","args":["?file","?struct","Clone"]}
            ],
            "actions":[{
                "action_type":"constraint_warning",
                "params":["Cloneable struct — consider Debug too"]
            }]
        }]}"#,
    );

    let payload = r##"{
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/lib.rs",
            "old_string": "",
            "new_string": "#[derive(Clone)]\npub struct Foo { x: u32 }"
        }
    }"##;
    let (code, stderr) = run_post_hook_with_root(payload, dir.path());
    assert_eq!(code, 1, "warning should exit 1: {}", stderr);
    assert!(stderr.contains("Cloneable struct"), "stderr: {}", stderr);
}

#[test]
fn swift_force_unwrap_warning_fires() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("Sources")).unwrap();
    std::fs::write(
        dir.path().join("Sources/A.swift"),
        "func grab(x: Int?) -> Int { return x! }\n",
    )
    .unwrap();

    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"warn-force-unwrap","phase":"post","priority":10,
            "conditions":[
                {"predicate":"function_uses_force_unwrap","args":["?file","?fn","?count"]}
            ],
            "actions":[{
                "action_type":"constraint_warning",
                "params":["force-unwrap detected — prefer guard let"]
            }]
        }]}"#,
    );

    let payload = r#"{
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "Sources/A.swift",
            "old_string": "",
            "new_string": "func grab(x: Int?) -> Int { return x! }"
        }
    }"#;
    let (code, stderr) = run_post_hook_with_root(payload, dir.path());
    assert_eq!(code, 1, "warning should exit 1: {}", stderr);
    assert!(
        stderr.contains("force-unwrap detected"),
        "stderr: {}",
        stderr
    );
}

#[test]
fn swift_throws_predicate_fires() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("Sources")).unwrap();
    std::fs::write(
        dir.path().join("Sources/A.swift"),
        "func fetch() throws { }\n",
    )
    .unwrap();

    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"warn-throws","phase":"post","priority":5,
            "conditions":[
                {"predicate":"function_throws","args":["?file","?fn"]}
            ],
            "actions":[{
                "action_type":"constraint_warning",
                "params":["throwing function added"]
            }]
        }]}"#,
    );

    let payload = r#"{
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "Sources/A.swift",
            "old_string": "",
            "new_string": "func fetch() throws { }"
        }
    }"#;
    let (code, stderr) = run_post_hook_with_root(payload, dir.path());
    assert_eq!(code, 1, "warning should exit 1: {}", stderr);
    assert!(
        stderr.contains("throwing function added"),
        "stderr: {}",
        stderr
    );
}

#[test]
fn log_entry_records_rule_id_and_bindings_per_consequence() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "fn existing() -> u32 { 0 }\n",
    )
    .unwrap();

    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"rust-error-thiserror-for-libraries","phase":"pre","priority":10,
            "conditions":[
                {"predicate":"function_returns_result_string","args":["?file","?fn"]}
            ],
            "actions":[{
                "action_type":"constraint_violation",
                "params":["`?fn` in ?file returns Result<_, String>"]
            }]
        }]}"#,
    );

    let payload = r#"{
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/lib.rs",
            "old_string": "fn existing() -> u32 { 0 }",
            "new_string": "fn existing() -> u32 { 0 }\nfn bad() -> Result<u32, String> { Ok(0) }"
        }
    }"#;
    let (code, _stderr) = run_hook_with_root(payload, dir.path());
    assert_eq!(code, 2, "rule must block");

    let log_path = dir.path().join(".phronesis/log.jsonl");
    let contents = std::fs::read_to_string(&log_path).expect("log written");
    let last_line = contents.lines().last().expect("at least one log line");
    let entry: serde_json::Value = serde_json::from_str(last_line).expect("log line is valid JSON");

    let consequences = entry
        .get("consequences")
        .and_then(|v| v.as_array())
        .expect("entry has consequences array");
    assert_eq!(consequences.len(), 1, "exactly one consequence fired");

    let c = &consequences[0];
    assert_eq!(c["rule_id"], "rust-error-thiserror-for-libraries");
    assert_eq!(c["action_type"], "constraint_violation");
    // Substitution should now actually work — message contains "bad", not "?fn".
    assert!(
        c["message"].as_str().unwrap().contains("bad"),
        "message should contain substituted function name: {}",
        c["message"]
    );
    assert!(
        c["message"].as_str().unwrap().contains("src/lib.rs"),
        "message should contain substituted file path: {}",
        c["message"]
    );
    // Bindings preserved as a queryable map.
    assert_eq!(c["bindings"]["?fn"], "bad");
    assert_eq!(c["bindings"]["?file"], "src/lib.rs");
}

#[test]
fn warn_cargo_build_without_workspace_fires_on_bash() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"warn-cargo-build-without-workspace","phase":"pre","priority":3,
            "conditions":[{"predicate":"cargo_command_lacks_workspace","args":["?cmd"]}],
            "actions":[{"action_type":"constraint_warning","params":["use `--workspace`"]}]
        }]}"#,
    );
    let payload = r#"{"tool_name":"Bash","tool_input":{"command":"cargo build"}}"#;
    let (code, stderr) = run_hook_with_root(payload, dir.path());
    assert_eq!(code, 1, "pre-check warning must exit 1");
    assert!(stderr.contains("--workspace"), "stderr: {stderr}");
}

#[test]
fn block_await_on_sync_execute_all_agenda_items_blocks_edit() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), "").unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"block-await-on-sync-execute-all-agenda-items","phase":"pre","priority":10,
            "conditions":[
                {"predicate":"new_content_contains","args":["execute_all_agenda_items().await"]},
                {"predicate":"file_extension_is","args":["rs"]}
            ],
            "actions":[{"action_type":"constraint_violation","params":["execute_all_agenda_items is sync"]}]
        }]}"#,
    );
    let payload = r#"{"tool_name":"Edit","tool_input":{"file_path":"src/lib.rs","old_string":"","new_string":"let _ = network.execute_all_agenda_items().await;"}}"#;
    let (code, stderr) = run_hook_with_root(payload, dir.path());
    assert_eq!(code, 2, "block must exit 2");
    assert!(stderr.contains("is sync"), "stderr: {stderr}");
}

#[test]
fn block_await_on_sync_fire_all_consequences_blocks_edit() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), "").unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"block-await-on-sync-fire-all-consequences","phase":"pre","priority":10,
            "conditions":[
                {"predicate":"new_content_contains","args":["fire_all_consequences().await"]},
                {"predicate":"file_extension_is","args":["rs"]}
            ],
            "actions":[{"action_type":"constraint_violation","params":["fire_all_consequences is sync"]}]
        }]}"#,
    );
    let payload = r#"{"tool_name":"Edit","tool_input":{"file_path":"src/lib.rs","old_string":"","new_string":"let _ = network.fire_all_consequences().await;"}}"#;
    let (code, stderr) = run_hook_with_root(payload, dir.path());
    assert_eq!(code, 2);
    assert!(stderr.contains("is sync"));
}

#[test]
fn warn_clone_heavy_fires_at_threshold_3() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    // Post-check reads file content from disk, not the payload.
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "fn foo() { let _ = x.clone(); let _ = y.clone(); let _ = z.clone(); }\n",
    )
    .unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"warn-clone-heavy","phase":"post","priority":5,
            "conditions":[{"predicate":"function_clone_count_high","args":["?file","?fn","?count"]}],
            "actions":[{"action_type":"constraint_warning","params":["clone-heavy"]}]
        }]}"#,
    );
    // 3 clones triggers the rule
    let payload = r#"{"tool_name":"Write","tool_input":{"file_path":"src/lib.rs","content":"fn foo() { let _ = x.clone(); let _ = y.clone(); let _ = z.clone(); }"}}"#;
    let (code, stderr) = run_post_hook_with_root(payload, dir.path());
    assert_eq!(code, 1);
    assert!(stderr.contains("clone-heavy"));
}

#[test]
fn warn_clone_heavy_does_not_fire_below_threshold() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "fn foo() { let _ = x.clone(); let _ = y.clone(); }\n",
    )
    .unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"warn-clone-heavy","phase":"post","priority":5,
            "conditions":[{"predicate":"function_clone_count_high","args":["?file","?fn","?count"]}],
            "actions":[{"action_type":"constraint_warning","params":["clone-heavy"]}]
        }]}"#,
    );
    // 2 clones — below threshold
    let payload = r#"{"tool_name":"Write","tool_input":{"file_path":"src/lib.rs","content":"fn foo() { let _ = x.clone(); let _ = y.clone(); }"}}"#;
    let (code, _stderr) = run_post_hook_with_root(payload, dir.path());
    assert_eq!(code, 0, "2 clones must NOT trigger warn-clone-heavy");
}

#[test]
fn warn_pub_fn_missing_doc_fires_on_naked_pub_fn() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), "pub fn naked() {}\n").unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"warn-pub-fn-missing-doc","phase":"post","priority":3,
            "conditions":[{"predicate":"pub_fn_without_doc_comment","args":["?file","?fn"]}],
            "actions":[{"action_type":"constraint_warning","params":["needs doc"]}]
        }]}"#,
    );
    let payload = r#"{"tool_name":"Write","tool_input":{"file_path":"src/lib.rs","content":"pub fn naked() {}"}}"#;
    let (code, stderr) = run_post_hook_with_root(payload, dir.path());
    assert_eq!(code, 1);
    assert!(stderr.contains("needs doc"));
}

#[test]
fn warn_empty_test_fires_on_test_with_no_assertions() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    // `assert_values_facts` dual-extracts so test-quality predicates see the
    // unstripped content. The canonical multi-line `#[test]\nfn ...` form
    // survives this path and is what production code actually looks like.
    std::fs::write(dir.path().join("src/lib.rs"), "#[test]\nfn empty() {\n}\n").unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"warn-empty-test","phase":"post","priority":5,
            "conditions":[{"predicate":"test_without_assertion","args":["?file","?fn"]}],
            "actions":[{"action_type":"constraint_warning","params":["empty test"]}]
        }]}"#,
    );
    let payload = r##"{"tool_name":"Write","tool_input":{"file_path":"src/lib.rs","content":"#[test]\nfn empty() {\n}"}}"##;
    let (code, stderr) = run_post_hook_with_root(payload, dir.path());
    assert_eq!(code, 1);
    assert!(stderr.contains("empty test"));
}

#[test]
fn block_rhai_inline_eval_string_blocks_edit() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), "").unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"block-rhai-inline-eval-string","phase":"pre","priority":10,
            "conditions":[
                {"predicate":"engine_eval_string_literal","args":["?file","?fn"]},
                {"predicate":"file_extension_is","args":["rs"]}
            ],
            "actions":[{"action_type":"constraint_violation","params":["use precompiled AST"]}]
        }]}"#,
    );
    let payload = r#"{"tool_name":"Edit","tool_input":{"file_path":"src/lib.rs","old_string":"","new_string":"fn host() { let engine = rhai::Engine::new(); let _: i64 = engine.eval(\"40+2\").unwrap(); }"}}"#;
    let (code, stderr) = run_hook_with_root(payload, dir.path());
    assert_eq!(code, 2);
    assert!(stderr.contains("precompiled"));
}

#[test]
fn block_rhai_print_in_script_blocks_edit() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("scripts/example")).unwrap();
    std::fs::write(
        dir.path().join("scripts/example/test.rhai"),
        "print(\"hello\")\n",
    )
    .unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"block-rhai-print-in-script","phase":"pre","priority":10,
            "conditions":[
                {"predicate":"new_content_contains","args":["print("]},
                {"predicate":"file_extension_is","args":["rhai"]}
            ],
            "actions":[{"action_type":"constraint_violation","params":["use response_append instead of print"]}]
        }]}"#,
    );
    let payload = r#"{"tool_name":"Write","tool_input":{"file_path":"scripts/example/test.rhai","content":"print(\"hello\")"}}"#;
    let (code, stderr) = run_hook_with_root(payload, dir.path());
    assert_eq!(code, 2, "print( in .rhai is now a hard block");
    assert!(stderr.contains("response_append"));
}

#[test]
fn warn_cargo_with_p_flag_does_not_fire() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"warn-cargo-build-without-workspace","phase":"pre","priority":3,
            "conditions":[{"predicate":"cargo_command_lacks_workspace","args":["?cmd"]}],
            "actions":[{"action_type":"constraint_warning","params":["use workspace"]}]
        }]}"#,
    );
    let payload = r#"{"tool_name":"Bash","tool_input":{"command":"cargo build -p mycrate"}}"#;
    let (code, _stderr) = run_hook_with_root(payload, dir.path());
    assert_eq!(code, 0, "-p flag suppresses the warning");
}

#[test]
fn warn_cargo_with_bin_flag_does_not_fire() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"warn-cargo-build-without-workspace","phase":"pre","priority":3,
            "conditions":[{"predicate":"cargo_command_lacks_workspace","args":["?cmd"]}],
            "actions":[{"action_type":"constraint_warning","params":["use workspace"]}]
        }]}"#,
    );
    let payload = r#"{"tool_name":"Bash","tool_input":{"command":"cargo test --bin server"}}"#;
    let (code, _stderr) = run_hook_with_root(payload, dir.path());
    assert_eq!(code, 0, "--bin flag suppresses the warning");
}

#[test]
fn warn_clone_heavy_suppresses_unchanged_function() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    // Pre-existing file content WITH the heavy-clone function. The next
    // edit will leave this function unchanged but touch something else.
    let prior = "fn heavy(x: &String) {
    let _a = x.clone();
    let _b = x.clone();
    let _c = x.clone();
    let _d = x.clone();
}
fn unrelated() {}
";
    std::fs::write(dir.path().join("src/lib.rs"), prior).unwrap();

    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"warn-clone-heavy","phase":"pre","priority":5,
            "conditions":[{"predicate":"function_clone_count_high","args":["?file","?fn","?count"]}],
            "actions":[{"action_type":"constraint_warning","params":["clone-heavy"]}]
        }]}"#,
    );

    // Edit replaces `unrelated` (not the heavy function). Heavy fn stays unchanged.
    let new_content = "fn heavy(x: &String) {
    let _a = x.clone();
    let _b = x.clone();
    let _c = x.clone();
    let _d = x.clone();
}
fn changed() { let _ = 42; }
";
    let payload = format!(
        r#"{{"tool_name":"Edit","tool_input":{{"file_path":"src/lib.rs","old_string":{prior_json},"new_string":{new_json}}}}}"#,
        prior_json = serde_json::to_string(prior).unwrap(),
        new_json = serde_json::to_string(new_content).unwrap(),
    );
    let (code, _stderr) = run_hook_with_root(&payload, dir.path());
    assert_eq!(
        code, 0,
        "heavy-clone fn count did not change; rule should not fire"
    );
}

#[test]
fn warn_clone_heavy_fires_when_count_increases() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    // Prior file: heavy function with 3 clones (at the threshold).
    let prior = "fn heavy(x: &String) {
    let _a = x.clone();
    let _b = x.clone();
    let _c = x.clone();
}
";
    std::fs::write(dir.path().join("src/lib.rs"), prior).unwrap();

    write_rules_file(
        dir.path(),
        r#"{"rules":[{
            "id":"warn-clone-heavy","phase":"pre","priority":5,
            "conditions":[{"predicate":"function_clone_count_high","args":["?file","?fn","?count"]}],
            "actions":[{"action_type":"constraint_warning","params":["clone-heavy"]}]
        }]}"#,
    );

    // New content: same fn now with 4 clones (one more added).
    let new_content = "fn heavy(x: &String) {
    let _a = x.clone();
    let _b = x.clone();
    let _c = x.clone();
    let _d = x.clone();
}
";
    let payload = format!(
        r#"{{"tool_name":"Edit","tool_input":{{"file_path":"src/lib.rs","old_string":{prior_json},"new_string":{new_json}}}}}"#,
        prior_json = serde_json::to_string(prior).unwrap(),
        new_json = serde_json::to_string(new_content).unwrap(),
    );
    let (code, stderr) = run_hook_with_root(&payload, dir.path());
    assert_eq!(code, 1, "increased clone count must fire the warning");
    assert!(stderr.contains("clone-heavy"));
}
