//! What happens *after* a rules file fails to load (decision D1).
//!
//! Failing closed at load must not turn into data loss or a lockout:
//!
//! - The MCP server used to autoload nothing when the file did not load, and
//!   its autosave then replaced `rules.json` with only the rules added since
//!   startup — two `add_rule` calls later neither `rules.json` nor its `.bak`
//!   held a single original rule, and the now-valid file lifted the block.
//!   Every rule-writing tool now refuses while the rules on disk do not load.
//! - `pre-check` blocked every edit, including the edit that repairs the
//!   rules file itself. An edit whose target is the file that failed to load
//!   is now allowed with a warning (exit 1); everything else stays blocked.
//! - `session-context` / `interaction-context` printed nothing, so the agent
//!   never learned why every tool call was blocked. They now lead with the
//!   load error.

use std::io::{BufRead, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

/// `keep-a` carries a `description` key, which the loader now rejects;
/// `keep-b` is valid. Both are the user's rules and must survive.
const ORIGINAL: &str = r#"{"rules":[
  {"id":"keep-a","description":"mine","phase":"pre","priority":1,
   "when":[{"new_content_contains":"aaa"}],"then":{"block":"a"}},
  {"id":"keep-b","phase":"pre","priority":1,
   "when":[{"new_content_contains":"bbb"}],"then":{"warn":"b"}}
]}"#;

fn project(rules: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp project");
    std::fs::create_dir_all(dir.path().join(".phronesis")).expect("config dir");
    std::fs::write(rules_path(dir.path()), rules).expect("write rules");
    dir
}

fn rules_path(root: &Path) -> PathBuf {
    root.join(".phronesis/rules.json")
}

fn bak_path(root: &Path) -> PathBuf {
    root.join(".phronesis/rules.json.bak")
}

struct Mcp {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    next_id: u64,
}

/// A tool call's text, tagged with whether the call was refused.
enum Reply {
    Done(String),
    Refused(String),
}

impl Reply {
    fn done(self, what: &str) -> String {
        match self {
            Reply::Done(text) => text,
            Reply::Refused(err) => panic!("{what} failed: {err}"),
        }
    }

    fn refused(self, what: &str) -> String {
        match self {
            Reply::Refused(err) => err,
            Reply::Done(text) => panic!("{what} must be refused; got: {text}"),
        }
    }
}

impl Mcp {
    /// A server with autoload/autosave ENABLED — the user-facing default,
    /// and the configuration in which the data loss happened.
    fn spawn(root: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
            .arg("serve")
            .env("PHRONESIS_PROJECT_ROOT", root)
            .env("PHRONESIS_NO_ACTION_LOG", "1")
            .env_remove("PHRONESIS_NO_AUTOPERSIST")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn server");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        let mut mcp = Self {
            child,
            stdin,
            stdout,
            next_id: 0,
        };
        mcp.call(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "0.1"}
            }),
        );
        let note = json!({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}});
        writeln!(mcp.stdin, "{note}").expect("write");
        mcp
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let msg = json!({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params});
        writeln!(self.stdin, "{msg}").expect("write");
        self.stdin.flush().expect("flush");
        let mut line = String::new();
        self.stdout.read_line(&mut line).expect("read");
        serde_json::from_str(&line).expect("JSON-RPC response")
    }

    fn tool(&mut self, name: &str, args: Value) -> Reply {
        let r = self.call("tools/call", json!({"name": name, "arguments": args}));
        if let Some(msg) = r["error"]["message"].as_str() {
            return Reply::Refused(msg.to_string());
        }
        let text = r["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if r["result"]["isError"] == true {
            return Reply::Refused(text);
        }
        Reply::Done(text)
    }

    fn list_rules(&mut self) -> Value {
        let text = self.tool("list_rules", json!({})).done("list_rules");
        serde_json::from_str(&text).expect("list_rules JSON")
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn new_rule(id: &str) -> Value {
    json!({
        "id": id, "priority": 1,
        "conditions": [{"predicate": "new_content_contains", "args": [id]}],
        "actions": [{"action_type": "log", "params": ["m"]}]
    })
}

#[test]
fn mcp_never_overwrites_a_rules_file_that_does_not_load() {
    let dir = project(ORIGINAL);
    let root = dir.path();
    let mut mcp = Mcp::spawn(root);

    // list_rules surfaces why the network is empty.
    let listed = mcp.list_rules();
    let load_error = listed["load_error"].as_str().unwrap_or_default();
    assert!(
        load_error.contains("keep-a") && load_error.contains("description"),
        "list_rules must report the load error: {listed}"
    );

    // Every rule-writing tool refuses, naming the load error.
    for (tool, args) in [
        ("add_rule", new_rule("new-1")),
        ("add_rule", new_rule("new-2")),
        ("remove_rule", json!({"rule_id": "keep-b"})),
        ("save_rules", json!({})),
        ("save_rules", json!({"merge": false})),
    ] {
        let err = mcp
            .tool(tool, args.clone())
            .refused(&format!("{tool} {args}"));
        assert!(
            err.contains("keep-a") && err.contains("description"),
            "{tool}: refusal must carry the load error: {err}"
        );
    }

    // Nothing was written: the file is byte-identical and no .bak rotated.
    assert_eq!(
        std::fs::read_to_string(rules_path(root)).expect("rules"),
        ORIGINAL
    );
    assert!(!bak_path(root).exists(), "no .bak rotation on refusal");

    // Once the human repairs the file, the same server picks the rules up
    // and the next write keeps them.
    std::fs::write(
        rules_path(root),
        ORIGINAL.replace(r#""description":"mine","#, ""),
    )
    .expect("repair");
    mcp.tool("add_rule", new_rule("new-3"))
        .done("add_rule after repair");
    let on_disk: Value =
        serde_json::from_str(&std::fs::read_to_string(rules_path(root)).expect("rules"))
            .expect("json");
    let ids: Vec<&str> = on_disk["rules"]
        .as_array()
        .expect("rules array")
        .iter()
        .filter_map(|r| r["id"].as_str())
        .collect();
    for id in ["keep-a", "keep-b", "new-3"] {
        assert!(ids.contains(&id), "{id} missing after repair: {ids:?}");
    }
    let listed = mcp.list_rules();
    assert!(listed.get("load_error").is_none(), "{listed}");
}

// ── Hook lockout ──────────────────────────────────────────────────────────

fn spawn_hook(root: &Path, args: &[&str], payload: &Value) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(args)
        .env("PHRONESIS_PROJECT_ROOT", root)
        .env("PHRONESIS_NO_ACTION_LOG", "1")
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn phr-mcp");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(payload.to_string().as_bytes())
        .expect("write payload");
    child.wait_with_output().expect("wait")
}

fn edit(path: &str) -> Value {
    json!({"tool_name": "Edit", "tool_input": {
        "file_path": path, "old_string": "\"description\":\"mine\",", "new_string": ""
    }})
}

#[test]
fn pre_check_lets_the_agent_repair_the_file_that_failed_to_load() {
    let dir = project(ORIGINAL);
    let root = dir.path();
    let absolute = rules_path(root).display().to_string();
    for target in [".phronesis/rules.json", absolute.as_str()] {
        let out = spawn_hook(root, &["pre-check"], &edit(target));
        let err = String::from_utf8_lossy(&out.stderr);
        // Allowed with a warning (exit 1), so the load error stays visible.
        assert_eq!(
            out.status.code(),
            Some(1),
            "editing the failing rules file ({target}) must be allowed: {err}"
        );
        assert!(
            err.contains("keep-a"),
            "the load error is still shown: {err}"
        );
    }
    let write = json!({"tool_name": "Write", "tool_input": {
        "file_path": ".phronesis/rules.json", "content": "{\"rules\":[]}"
    }});
    let out = spawn_hook(root, &["pre-check"], &write);
    assert_eq!(out.status.code(), Some(1));

    // Everything else stays blocked.
    let out = spawn_hook(root, &["pre-check"], &edit("src/lib.rs"));
    assert_eq!(out.status.code(), Some(2));
    let out = spawn_hook(
        root,
        &["pre-check"],
        &json!({"tool_name": "Bash", "tool_input": {"command": "ls"}}),
    );
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn codex_apply_patch_may_repair_the_file_that_failed_to_load() {
    let dir = project(ORIGINAL);
    let root = dir.path();
    let patch = |path: &str| {
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "apply_patch",
            "session_id": "s", "turn_id": "t", "tool_use_id": "u",
            "tool_input": {"command": format!(
                "*** Begin Patch\n*** Update File: {path}\n@@\n-x\n+y\n*** End Patch\n"
            )},
        })
    };
    let out = spawn_hook(
        root,
        &["codex-hook", "PreToolUse"],
        &patch(".phronesis/rules.json"),
    );
    let body: Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_ne!(
        body["hookSpecificOutput"]["permissionDecision"], "deny",
        "repairing the rules file must not be denied: {body}"
    );
    let out = spawn_hook(root, &["codex-hook", "PreToolUse"], &patch("src/lib.rs"));
    let body: Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(body["hookSpecificOutput"]["permissionDecision"], "deny");
}

#[test]
fn context_hooks_lead_with_the_load_error() {
    let dir = project(ORIGINAL);
    for sub in ["session-context", "interaction-context"] {
        let out = spawn_hook(dir.path(), &[sub], &json!({}));
        let stdout = String::from_utf8_lossy(&out.stdout);
        let v: Value = serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("{sub} must print an envelope ({e}): {stdout:?}"));
        let ctx = v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap_or_default();
        assert!(
            ctx.contains("keep-a") && ctx.contains("description"),
            "{sub} must surface the load error: {ctx}"
        );
    }
}
