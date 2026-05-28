//! End-to-end tests for `save_rules` and `load_rules_file` driven through the
//! MCP stdio server in a subprocess. Each test owns its own temp directory and
//! passes it as the project root via `PHRONESIS_PROJECT_ROOT`, isolating the
//! environment from other tests.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

struct McpClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl McpClient {
    /// Spawn a server with autoload/autosave DISABLED. Used by tests that
    /// exercise the explicit `save_rules` / `load_rules_file` tools in
    /// isolation and don't want auto-persistence side effects.
    fn spawn(project_root: &Path) -> Self {
        Self::spawn_with_env(project_root, &[("PHRONESIS_NO_AUTOPERSIST", "1")])
    }

    /// Spawn a server with autoload/autosave ENABLED (the default user-facing
    /// behavior). Used by tests that verify the auto-persistence contract.
    fn spawn_with_autopersist(project_root: &Path) -> Self {
        Self::spawn_with_env(project_root, &[])
    }

    fn spawn_with_env(project_root: &Path, extra_env: &[(&str, &str)]) -> Self {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_phr-mcp"));
        cmd.arg("serve")
            .env("PHRONESIS_PROJECT_ROOT", project_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("spawn server");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut client = Self {
            child,
            stdin,
            stdout,
            next_id: 0,
        };
        client.call(
            "initialize",
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name":"test","version":"0.1"}
            }),
        );
        client.notify("notifications/initialized", serde_json::json!({}));
        client
    }

    fn call(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        self.next_id += 1;
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": self.next_id,
            "method": method,
            "params": params,
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
            serde_json::json!({"name": name, "arguments": args}),
        );
        let text = r["result"]["content"][0]["text"].as_str().unwrap();
        serde_json::from_str(text).unwrap_or_else(|_| serde_json::Value::String(text.to_string()))
    }

    fn tool_text(&mut self, name: &str, args: serde_json::Value) -> String {
        let r = self.call(
            "tools/call",
            serde_json::json!({"name": name, "arguments": args}),
        );
        r["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string()
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn rules_path(root: &Path) -> PathBuf {
    root.join(".phronesis").join("rules.json")
}

fn read_rules(root: &Path) -> serde_json::Value {
    let content = std::fs::read_to_string(rules_path(root)).unwrap();
    serde_json::from_str(&content).unwrap()
}

fn simple_rule(id: &str, predicate: &str, arg: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "priority": 5,
        "conditions": [{"predicate": predicate, "args": [arg]}],
        "actions": [{"action_type":"log","params":["matched"]}]
    })
}

// ─────────────────────────────────────────────────────────────────────
// save_rules
// ─────────────────────────────────────────────────────────────────────

#[test]
fn save_rules_writes_added_rules_to_disk() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = McpClient::spawn(dir.path());

    c.tool("add_rule", simple_rule("r1", "p", "x"));
    c.tool("add_rule", simple_rule("r2", "p", "y"));
    let summary = c.tool("save_rules", serde_json::json!({}));

    assert_eq!(summary["added"], 2);
    assert_eq!(summary["updated"], 0);
    assert_eq!(summary["total"], 2);

    let on_disk = read_rules(dir.path());
    let rules = on_disk["rules"].as_array().unwrap();
    assert_eq!(rules.len(), 2);
    assert!(rules.iter().all(|r| r["phase"] == "pre"));
}

#[test]
fn save_rules_honors_explicit_phase() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = McpClient::spawn(dir.path());

    c.tool("add_rule", simple_rule("r1", "p", "x"));
    c.tool("save_rules", serde_json::json!({"phase":"post"}));

    let on_disk = read_rules(dir.path());
    assert_eq!(on_disk["rules"][0]["phase"], "post");
}

#[test]
fn save_rules_records_per_rule_phase_via_add_rule() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = McpClient::spawn(dir.path());

    let mut r1 = simple_rule("r1", "p", "x");
    r1["phase"] = serde_json::Value::String("post".into());
    c.tool("add_rule", r1);
    c.tool("add_rule", simple_rule("r2", "p", "y")); // default pre
    c.tool("save_rules", serde_json::json!({}));

    let on_disk = read_rules(dir.path());
    let by_id: std::collections::HashMap<String, String> = on_disk["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["id"].as_str().unwrap().to_string(),
                r["phase"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(by_id["r1"], "post");
    assert_eq!(by_id["r2"], "pre");
}

#[test]
fn save_rules_merge_preserves_disk_only_rules() {
    let dir = tempfile::tempdir().unwrap();
    // Pre-seed the rules file
    std::fs::create_dir_all(dir.path().join(".phronesis")).unwrap();
    std::fs::write(
        rules_path(dir.path()),
        r#"{"rules":[
            {"id":"on-disk","phase":"pre","priority":1,
             "when":[{"p":"a"}],
             "then":{"log":"m"}}
        ]}"#,
    )
    .unwrap();

    let mut c = McpClient::spawn(dir.path());
    c.tool("add_rule", simple_rule("in-memory", "p", "b"));
    let summary = c.tool("save_rules", serde_json::json!({}));

    assert_eq!(summary["added"], 1);
    assert_eq!(summary["preserved"], 1);
    assert_eq!(summary["total"], 2);

    let on_disk = read_rules(dir.path());
    let ids: Vec<String> = on_disk["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect();
    assert!(ids.contains(&"on-disk".to_string()));
    assert!(ids.contains(&"in-memory".to_string()));
}

#[test]
fn save_rules_dry_run_does_not_write() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = McpClient::spawn(dir.path());
    c.tool("add_rule", simple_rule("r1", "p", "x"));

    let summary = c.tool("save_rules", serde_json::json!({"dry_run": true}));
    assert_eq!(summary["added"], 1);
    assert_eq!(summary["dry_run"], true);

    assert!(
        !rules_path(dir.path()).exists(),
        "dry_run should not touch disk"
    );
}

#[test]
fn save_rules_replace_mode_discards_disk_only() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".phronesis")).unwrap();
    std::fs::write(
        rules_path(dir.path()),
        r#"{"rules":[
            {"id":"orphan","phase":"pre","priority":1,
             "when":[{"p":"a"}],
             "then":{"log":"m"}}
        ]}"#,
    )
    .unwrap();

    let mut c = McpClient::spawn(dir.path());
    c.tool("add_rule", simple_rule("in-memory", "p", "b"));
    c.tool("save_rules", serde_json::json!({"merge": false}));

    let on_disk = read_rules(dir.path());
    let rules = on_disk["rules"].as_array().unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0]["id"], "in-memory");
}

#[test]
fn save_rules_creates_backup_on_second_write() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = McpClient::spawn(dir.path());

    c.tool("add_rule", simple_rule("v1", "p", "x"));
    c.tool("save_rules", serde_json::json!({}));
    c.tool("add_rule", simple_rule("v2", "p", "y"));
    c.tool("save_rules", serde_json::json!({}));

    let bak = dir.path().join(".phronesis").join("rules.json.bak");
    assert!(bak.exists(), "backup should be created on second save");
    let bak_content: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&bak).unwrap()).unwrap();
    assert_eq!(bak_content["rules"].as_array().unwrap().len(), 1);
    assert_eq!(bak_content["rules"][0]["id"], "v1");
}

#[test]
fn save_rules_rejects_invalid_phase() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = McpClient::spawn(dir.path());

    let result = c.call(
        "tools/call",
        serde_json::json!({
            "name": "save_rules",
            "arguments": {"phase": "invalid"}
        }),
    );
    assert!(result.get("error").is_some(), "should return error");
}

// ─────────────────────────────────────────────────────────────────────
// load_rules_file
// ─────────────────────────────────────────────────────────────────────

#[test]
fn load_rules_file_hydrates_from_disk() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".phronesis")).unwrap();
    std::fs::write(
        rules_path(dir.path()),
        r#"{"rules":[
            {"id":"a","phase":"pre","priority":1,
             "when":[{"p":"x"}],
             "then":{"log":"m"}},
            {"id":"b","phase":"post","priority":2,
             "when":[{"p":"y"}],
             "then":{"log":"m"}}
        ]}"#,
    )
    .unwrap();

    let mut c = McpClient::spawn(dir.path());
    let summary = c.tool("load_rules_file", serde_json::json!({}));
    assert_eq!(summary["loaded"], 2);
    assert_eq!(summary["skipped_duplicate_ids"], 0);

    let rules: Vec<serde_json::Value> =
        serde_json::from_str(&c.tool_text("list_rules", serde_json::json!({}))).unwrap();
    assert_eq!(rules.len(), 2);
}

#[test]
fn load_rules_file_skips_existing_ids() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".phronesis")).unwrap();
    std::fs::write(
        rules_path(dir.path()),
        r#"{"rules":[
            {"id":"a","phase":"pre","priority":1,
             "when":[{"p":"x"}],
             "then":{"log":"m"}}
        ]}"#,
    )
    .unwrap();

    let mut c = McpClient::spawn(dir.path());
    c.tool("add_rule", simple_rule("a", "p", "x"));
    let summary = c.tool("load_rules_file", serde_json::json!({}));
    assert_eq!(summary["loaded"], 0);
    assert_eq!(summary["skipped_duplicate_ids"], 1);
}

#[test]
fn load_rules_file_missing_returns_zero() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = McpClient::spawn(dir.path());
    let summary = c.tool("load_rules_file", serde_json::json!({}));
    assert_eq!(summary["loaded"], 0);
}

// ─────────────────────────────────────────────────────────────────────
// Round-trip
// ─────────────────────────────────────────────────────────────────────

#[test]
fn round_trip_add_save_reload_preserves_phases() {
    let dir = tempfile::tempdir().unwrap();

    {
        let mut c = McpClient::spawn(dir.path());
        let mut r1 = simple_rule("pre-rule", "p", "x");
        r1["phase"] = serde_json::Value::String("pre".into());
        let mut r2 = simple_rule("post-rule", "p", "y");
        r2["phase"] = serde_json::Value::String("post".into());
        c.tool("add_rule", r1);
        c.tool("add_rule", r2);
        c.tool("save_rules", serde_json::json!({}));
    }

    // Fresh server, load from disk
    let mut c = McpClient::spawn(dir.path());
    let summary = c.tool("load_rules_file", serde_json::json!({}));
    assert_eq!(summary["loaded"], 2);

    // Re-save: phases should round-trip exactly (no rule changes its phase)
    c.tool("save_rules", serde_json::json!({}));

    let on_disk = read_rules(dir.path());
    let by_id: std::collections::HashMap<String, String> = on_disk["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["id"].as_str().unwrap().to_string(),
                r["phase"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(by_id["pre-rule"], "pre");
    assert_eq!(by_id["post-rule"], "post");
}

// ─────────────────────────────────────────────────────────────────────
// Auto-persist behavior (default user-facing)
// ─────────────────────────────────────────────────────────────────────

#[test]
fn autosave_writes_rules_file_after_add_rule() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = McpClient::spawn_with_autopersist(dir.path());

    // No explicit save_rules call — autosave should have written the file.
    c.tool("add_rule", simple_rule("auto-r1", "p", "x"));

    assert!(
        rules_path(dir.path()).exists(),
        "autosave must write .phronesis/rules.json"
    );
    let on_disk = read_rules(dir.path());
    let rules = on_disk["rules"].as_array().unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0]["id"], "auto-r1");
    assert_eq!(rules[0]["phase"], "pre");
}

#[test]
fn autosave_persists_across_multiple_mutations() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = McpClient::spawn_with_autopersist(dir.path());

    c.tool("add_rule", simple_rule("a", "p", "x"));
    c.tool("add_rule", simple_rule("b", "p", "y"));
    c.tool("add_rule", simple_rule("c", "p", "z"));

    let on_disk = read_rules(dir.path());
    let ids: Vec<String> = on_disk["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids.len(), 3);
    assert!(ids.contains(&"a".to_string()));
    assert!(ids.contains(&"b".to_string()));
    assert!(ids.contains(&"c".to_string()));
}

#[test]
fn autosave_persists_remove_rule() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = McpClient::spawn_with_autopersist(dir.path());

    c.tool("add_rule", simple_rule("keep", "p", "x"));
    c.tool("add_rule", simple_rule("drop", "p", "y"));
    c.tool("remove_rule", serde_json::json!({"rule_id": "drop"}));

    let on_disk = read_rules(dir.path());
    let ids: Vec<String> = on_disk["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids, vec!["keep".to_string()]);
}

#[test]
fn autoload_hydrates_in_memory_state_at_startup() {
    let dir = tempfile::tempdir().unwrap();
    // Pre-seed rules file BEFORE starting the server.
    std::fs::create_dir_all(dir.path().join(".phronesis")).unwrap();
    std::fs::write(
        rules_path(dir.path()),
        r#"{"rules":[
            {"id":"pre-loaded","phase":"pre","priority":7,
             "when":[{"p":"x"}],
             "then":{"log":"m"}}
        ]}"#,
    )
    .unwrap();

    let mut c = McpClient::spawn_with_autopersist(dir.path());
    // No explicit load_rules_file — autoload should have hydrated it.
    let rules_text = c.tool_text("list_rules", serde_json::json!({}));
    let rules: Vec<serde_json::Value> = serde_json::from_str(&rules_text).unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0]["id"], "pre-loaded");
}

#[test]
fn autoload_plus_autosave_round_trips_added_rules() {
    let dir = tempfile::tempdir().unwrap();

    // Session 1: add a rule, no explicit save_rules.
    {
        let mut c = McpClient::spawn_with_autopersist(dir.path());
        c.tool("add_rule", simple_rule("session1-rule", "p", "x"));
    }

    // Session 2: fresh server, autoload should pick up session 1's rule.
    let mut c = McpClient::spawn_with_autopersist(dir.path());
    let rules_text = c.tool_text("list_rules", serde_json::json!({}));
    let rules: Vec<serde_json::Value> = serde_json::from_str(&rules_text).unwrap();
    assert_eq!(
        rules.len(),
        1,
        "session 1's rule must survive across restarts"
    );
    assert_eq!(rules[0]["id"], "session1-rule");
}

#[test]
fn no_autopersist_env_var_disables_both_directions() {
    let dir = tempfile::tempdir().unwrap();
    // Pre-seed a rules file
    std::fs::create_dir_all(dir.path().join(".phronesis")).unwrap();
    std::fs::write(
        rules_path(dir.path()),
        r#"{"rules":[
            {"id":"on-disk","phase":"pre","priority":1,
             "when":[{"p":"x"}],
             "then":{"log":"m"}}
        ]}"#,
    )
    .unwrap();

    // Spawn with the opt-out — no autoload.
    let mut c = McpClient::spawn(dir.path());
    let rules_text = c.tool_text("list_rules", serde_json::json!({}));
    let rules: Vec<serde_json::Value> = serde_json::from_str(&rules_text).unwrap();
    assert_eq!(
        rules.len(),
        0,
        "PHRONESIS_NO_AUTOPERSIST must disable autoload"
    );

    // Add a rule — no autosave either. File still contains only "on-disk".
    c.tool("add_rule", simple_rule("in-memory-only", "p", "y"));
    let on_disk = read_rules(dir.path());
    let ids: Vec<String> = on_disk["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids, vec!["on-disk".to_string()]);
}
