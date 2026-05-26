//! End-to-end tests for `.phronesis/log.jsonl` written by the hook and the
//! MCP server. Each test owns its own tempdir as the project root, so the
//! action log it writes is fully isolated.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

// ─────────────────────────────────────────────────────────────────────
// Hook-side logging
// ─────────────────────────────────────────────────────────────────────

fn run_pre_check(payload: &str, root: &Path) -> (i32, String) {
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
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn log_path(root: &Path) -> PathBuf {
    root.join(".phronesis").join("log.jsonl")
}

fn read_log_entries(root: &Path) -> Vec<serde_json::Value> {
    let content = std::fs::read_to_string(log_path(root)).unwrap_or_default();
    content
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn write_rules(root: &Path, rules_json: &str) {
    let ep = root.join(".phronesis");
    std::fs::create_dir_all(&ep).unwrap();
    std::fs::write(ep.join("rules.json"), rules_json).unwrap();
}

#[test]
fn pre_check_block_appends_log_entry() {
    let dir = tempfile::tempdir().unwrap();
    write_rules(
        dir.path(),
        r#"{"rules":[{
            "id":"no-unwrap","phase":"pre","priority":10,
            "conditions":[
                {"predicate":"new_content_contains","args":[".unwrap()"]},
                {"predicate":"file_path_matches","args":["src"]}
            ],
            "actions":[{"action_type":"constraint_violation","params":["bad"]}]
        }]}"#,
    );
    let payload = r#"{"tool_name":"Edit","tool_input":{
        "file_path":"src/x.rs","old_string":"a","new_string":"foo.unwrap()"
    }}"#;
    let (code, _) = run_pre_check(payload, dir.path());
    assert_eq!(code, 2);

    let entries = read_log_entries(dir.path());
    assert_eq!(entries.len(), 1, "exactly one log entry expected");
    let e = &entries[0];
    assert_eq!(e["kind"], "hook");
    assert_eq!(e["event"], "pre_check");
    assert_eq!(e["phase"], "pre");
    assert_eq!(e["tool"], "Edit");
    assert_eq!(e["file"], "src/x.rs");
    assert_eq!(e["exit"], 2);
    assert!(
        e["consequences"]
            .as_array()
            .unwrap()
            .iter()
            .any(
                |c| c["action_type"].as_str() == Some("constraint_violation")
                    && c["message"].as_str() == Some("bad")
            )
    );
    assert!(e["ts"].as_u64().unwrap() > 0);
}

#[test]
fn pre_check_allow_appends_log_entry_with_exit_zero() {
    let dir = tempfile::tempdir().unwrap();
    write_rules(
        dir.path(),
        r#"{"rules":[{
            "id":"no-unwrap","phase":"pre","priority":10,
            "conditions":[
                {"predicate":"new_content_contains","args":[".unwrap()"]},
                {"predicate":"file_path_matches","args":["src"]}
            ],
            "actions":[{"action_type":"constraint_violation","params":["bad"]}]
        }]}"#,
    );
    let payload = r#"{"tool_name":"Edit","tool_input":{
        "file_path":"src/x.rs","old_string":"a","new_string":"foo.map_err(|e|e)?"
    }}"#;
    let (code, _) = run_pre_check(payload, dir.path());
    assert_eq!(code, 0);

    let entries = read_log_entries(dir.path());
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["exit"], 0);
    assert!(entries[0]["consequences"].as_array().unwrap().is_empty());
}

#[test]
fn no_action_log_env_var_suppresses_writes() {
    let dir = tempfile::tempdir().unwrap();
    write_rules(dir.path(), r#"{"rules":[]}"#);
    let payload = r#"{"tool_name":"Edit","tool_input":{
        "file_path":"src/x.rs","old_string":"a","new_string":"b"
    }}"#;

    // Run with the env var set
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_phr-mcp"));
    cmd.arg("pre-check")
        .env("PHRONESIS_PROJECT_ROOT", dir.path())
        .env("PHRONESIS_NO_ACTION_LOG", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = cmd.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let _ = stdin.write_all(payload.as_bytes());
    drop(stdin);
    let _ = child.wait_with_output().unwrap();

    assert!(
        !log_path(dir.path()).exists(),
        "no_action_log env var must suppress log writes"
    );
}

// ─────────────────────────────────────────────────────────────────────
// MCP-side logging via subprocess
// ─────────────────────────────────────────────────────────────────────

struct McpClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl McpClient {
    fn spawn(project_root: &Path) -> Self {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_phr-mcp"));
        cmd.arg("serve")
            .env("PHRONESIS_PROJECT_ROOT", project_root)
            // Disable autopersist so rules don't interfere with log-only assertions.
            // The action log itself is independent of autopersist.
            .env("PHRONESIS_NO_AUTOPERSIST", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = cmd.spawn().expect("spawn server");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut c = Self {
            child,
            stdin,
            stdout,
            next_id: 0,
        };
        c.call(
            "initialize",
            serde_json::json!({
                "protocolVersion":"2024-11-05","capabilities":{},
                "clientInfo":{"name":"test","version":"0.1"}
            }),
        );
        c.notify("notifications/initialized", serde_json::json!({}));
        c
    }
    fn call(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        self.next_id += 1;
        let msg = serde_json::json!({
            "jsonrpc":"2.0","id":self.next_id,"method":method,"params":params
        });
        writeln!(self.stdin, "{}", msg).unwrap();
        self.stdin.flush().unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }
    fn notify(&mut self, method: &str, params: serde_json::Value) {
        let msg = serde_json::json!({"jsonrpc":"2.0","method":method,"params":params});
        writeln!(self.stdin, "{}", msg).unwrap();
        self.stdin.flush().unwrap();
    }
    fn tool(&mut self, name: &str, args: serde_json::Value) -> serde_json::Value {
        let r = self.call(
            "tools/call",
            serde_json::json!({"name":name,"arguments":args}),
        );
        let text = r["result"]["content"][0]["text"].as_str().unwrap();
        serde_json::from_str(text).unwrap_or_else(|_| serde_json::Value::String(text.into()))
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn simple_rule(id: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id, "priority": 5,
        "conditions": [{"predicate":"p","args":["x"]}],
        "actions": [{"action_type":"log","params":["m"]}]
    })
}

#[test]
fn add_rule_logs_mcp_event() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = McpClient::spawn(dir.path());
    c.tool("add_rule", simple_rule("r1"));
    drop(c); // ensure server flushes and exits

    let entries = read_log_entries(dir.path());
    let add = entries
        .iter()
        .find(|e| e["event"] == "add_rule")
        .expect("add_rule event should be logged");
    assert_eq!(add["kind"], "mcp");
    assert_eq!(add["rule_id"], "r1");
}

#[test]
fn fire_rules_logs_action_counts() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = McpClient::spawn(dir.path());
    c.tool(
        "add_rule",
        serde_json::json!({
            "id":"r","priority":5,
            "conditions":[{"predicate":"p","args":["x"]}],
            "actions":[{"action_type":"log","params":["m"]}]
        }),
    );
    c.tool(
        "assert_fact",
        serde_json::json!({"id":"f","predicate":"p","args":["x"]}),
    );
    c.tool("fire_rules", serde_json::json!({}));
    drop(c);

    let entries = read_log_entries(dir.path());
    let fire = entries
        .iter()
        .find(|e| e["event"] == "fire_rules")
        .expect("fire_rules event should be logged");
    assert_eq!(fire["actions_fired"].as_u64().unwrap(), 1);
    assert!(
        fire["action_types"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str() == Some("log"))
    );
}

#[test]
fn set_section_context_logs_event() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = McpClient::spawn(dir.path());
    c.tool(
        "set_section_context",
        serde_json::json!({"file":"docs/X.md","section":"Idioms"}),
    );
    drop(c);

    let entries = read_log_entries(dir.path());
    let evt = entries
        .iter()
        .find(|e| e["event"] == "set_section_context")
        .expect("set_section_context event should be logged");
    assert_eq!(evt["file"], "docs/X.md");
    assert_eq!(evt["section"], "Idioms");
}

#[test]
fn remove_rule_logs_event() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = McpClient::spawn(dir.path());
    c.tool("add_rule", simple_rule("doomed"));
    c.tool("remove_rule", serde_json::json!({"rule_id":"doomed"}));
    drop(c);

    let entries = read_log_entries(dir.path());
    assert!(
        entries
            .iter()
            .any(|e| e["event"] == "remove_rule" && e["rule_id"] == "doomed"),
        "remove_rule event missing: {:?}",
        entries
    );
}

// ─────────────────────────────────────────────────────────────────────
// get_action_log tool
// ─────────────────────────────────────────────────────────────────────

#[test]
fn get_action_log_returns_recent_entries() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = McpClient::spawn(dir.path());
    c.tool("add_rule", simple_rule("a"));
    c.tool("add_rule", simple_rule("b"));
    c.tool("add_rule", simple_rule("c"));

    let result = c.tool("get_action_log", serde_json::json!({}));
    let entries = result.as_array().expect("log result should be array");
    let add_rule_events: Vec<&serde_json::Value> = entries
        .iter()
        .filter(|e| e["event"] == "add_rule")
        .collect();
    assert_eq!(add_rule_events.len(), 3);
}

#[test]
fn get_action_log_filters_by_kind() {
    let dir = tempfile::tempdir().unwrap();
    write_rules(dir.path(), r#"{"rules":[]}"#);
    // Hook entry from a subprocess invocation
    let payload = r#"{"tool_name":"Edit","tool_input":{
        "file_path":"src/x.rs","old_string":"a","new_string":"b"
    }}"#;
    run_pre_check(payload, dir.path());

    // MCP entry from server
    let mut c = McpClient::spawn(dir.path());
    c.tool("add_rule", simple_rule("r"));

    let mcp_only = c.tool("get_action_log", serde_json::json!({"kind":"mcp"}));
    for e in mcp_only.as_array().unwrap() {
        assert_eq!(e["kind"], "mcp");
    }
    let hook_only = c.tool("get_action_log", serde_json::json!({"kind":"hook"}));
    for e in hook_only.as_array().unwrap() {
        assert_eq!(e["kind"], "hook");
    }
}

#[test]
fn get_action_log_filters_only_nonzero_exit() {
    let dir = tempfile::tempdir().unwrap();
    write_rules(
        dir.path(),
        r#"{"rules":[{
            "id":"no-unwrap","phase":"pre","priority":10,
            "conditions":[
                {"predicate":"new_content_contains","args":[".unwrap()"]},
                {"predicate":"file_path_matches","args":["src"]}
            ],
            "actions":[{"action_type":"constraint_violation","params":["bad"]}]
        }]}"#,
    );
    // One allow (exit 0)
    run_pre_check(
        r#"{"tool_name":"Edit","tool_input":{
            "file_path":"src/x.rs","old_string":"a","new_string":"safe"
        }}"#,
        dir.path(),
    );
    // One block (exit 2)
    run_pre_check(
        r#"{"tool_name":"Edit","tool_input":{
            "file_path":"src/x.rs","old_string":"a","new_string":"foo.unwrap()"
        }}"#,
        dir.path(),
    );

    let mut c = McpClient::spawn(dir.path());
    let blocks = c.tool(
        "get_action_log",
        serde_json::json!({"only_nonzero_exit": true}),
    );
    let arr = blocks.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["exit"], 2);
}

#[test]
fn get_action_log_returns_empty_when_no_log_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = McpClient::spawn(dir.path());
    let result = c.tool("get_action_log", serde_json::json!({}));
    let arr = result.as_array().unwrap();
    assert!(arr.is_empty());
}

// ─────────────────────────────────────────────────────────────────────
// Rotation through get_action_log
// ─────────────────────────────────────────────────────────────────────

#[test]
fn get_action_log_reads_across_rotation_boundary() {
    let dir = tempfile::tempdir().unwrap();

    // Start with a low threshold so rotation triggers after very few writes.
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_phr-mcp"));
    cmd.arg("serve")
        .env("PHRONESIS_PROJECT_ROOT", dir.path())
        .env("PHRONESIS_NO_AUTOPERSIST", "1")
        // 500 bytes fits ~6 add_rule entries before triggering rotation.
        // With 10 entries total, all 10 stay readable: older ones in .1,
        // newer ones in the current file.
        .env("PHRONESIS_LOG_MAX_BYTES", "500")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = cmd.spawn().expect("spawn server");
    let stdin = child.stdin.take().unwrap();
    let stdout = BufReader::new(child.stdout.take().unwrap());
    let mut c = McpClient {
        child,
        stdin,
        stdout,
        next_id: 0,
    };
    c.call(
        "initialize",
        serde_json::json!({
            "protocolVersion":"2024-11-05","capabilities":{},
            "clientInfo":{"name":"test","version":"0.1"}
        }),
    );
    c.notify("notifications/initialized", serde_json::json!({}));

    // Add enough rules to overflow the small threshold and rotate at least once.
    for i in 0..10 {
        c.tool("add_rule", simple_rule(&format!("r{}", i)));
    }

    // Rotation should have created .1
    let rotated = dir.path().join(".phronesis").join("log.jsonl.1");
    assert!(
        rotated.exists(),
        "rotation must occur once log exceeds 200 bytes"
    );

    // get_action_log should still see entries from both files
    let entries = c.tool("get_action_log", serde_json::json!({"limit": 100}));
    let add_rule_events: Vec<&serde_json::Value> = entries
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["event"] == "add_rule")
        .collect();
    assert_eq!(
        add_rule_events.len(),
        10,
        "all 10 add_rule events must be visible across rotation"
    );
}
