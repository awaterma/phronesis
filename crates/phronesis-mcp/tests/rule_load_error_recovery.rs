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
    /// `None` once `Drop` has closed it to shut the server down.
    stdin: Option<std::process::ChildStdin>,
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
        Self::spawn_with(root, true)
    }

    fn spawn_with(root: &Path, autopersist: bool) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_phr-mcp"));
        command
            .arg("serve")
            .env("PHRONESIS_PROJECT_ROOT", root)
            .env("PHRONESIS_NO_ACTION_LOG", "1")
            .env_remove("PHRONESIS_NO_AUTOPERSIST");
        if !autopersist {
            command.env("PHRONESIS_NO_AUTOPERSIST", "1");
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn server");
        let stdin = Some(child.stdin.take().expect("stdin"));
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
        writeln!(mcp.stdin(), "{note}").expect("write");
        mcp
    }

    fn stdin(&mut self) -> &mut std::process::ChildStdin {
        self.stdin.as_mut().expect("server stdin is open")
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let msg = json!({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params});
        writeln!(self.stdin(), "{msg}").expect("write");
        self.stdin().flush().expect("flush");
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

/// Close stdin so the server exits on its own. A clean exit is what writes
/// the child's coverage profile; `kill` (SIGKILL) left every server-side line
/// these tests drive reported as never executed. Kill only as a fallback.
impl Drop for Mcp {
    fn drop(&mut self) {
        drop(self.stdin.take());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
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

// ── Duplicate ids written by the MCP tools ────────────────────────────────

fn disk_ids(root: &Path) -> Vec<String> {
    let on_disk: Value =
        serde_json::from_str(&std::fs::read_to_string(rules_path(root)).expect("rules"))
            .expect("json");
    on_disk["rules"]
        .as_array()
        .expect("rules array")
        .iter()
        .filter_map(|r| r["id"].as_str().map(String::from))
        .collect()
}

fn pre_check_ls(root: &Path) -> Output {
    spawn_hook(
        root,
        &["pre-check"],
        &json!({"tool_name": "Bash", "tool_input": {"command": "ls"}}),
    )
}

/// `add_rule` with an id already on disk is an update. It used to append a
/// second rule with that id, and the loader (which rejects duplicate ids)
/// then blocked every tool call.
#[test]
fn add_rule_with_an_existing_id_replaces_it() {
    let dir = project(
        r#"{"rules":[{"id":"A","phase":"pre","priority":1,
            "when":[{"new_content_contains":"old"}],"then":{"log":"old"}}]}"#,
    );
    let root = dir.path();
    let mut mcp = Mcp::spawn(root);
    let text = mcp.tool("add_rule", new_rule("A")).done("add_rule A");
    assert!(text.contains("replaced"), "{text}");
    assert_eq!(disk_ids(root), vec!["A"]);
    let listed = mcp.list_rules();
    assert_eq!(
        listed["rules"].as_array().map(Vec::len),
        Some(1),
        "{listed}"
    );
    assert_eq!(pre_check_ls(root).status.code(), Some(0));
}

/// The documented workflow extracts a guide once per session; the next
/// session autoloads those rules and extracts again. Re-extraction replaces.
#[test]
fn extracting_the_same_guide_twice_does_not_duplicate_rules() {
    let dir = project(r#"{"rules":[]}"#);
    let root = dir.path();
    std::fs::write(
        root.join("GUIDE.md"),
        "# Guide\n\n## Errors\n\n- Never use unwrap in production code.\n- Always propagate errors with ?.\n",
    )
    .expect("guide");
    {
        let mut mcp = Mcp::spawn(root);
        mcp.tool("extract_rules", json!({"file_path": "GUIDE.md"}))
            .done("extract 1");
        mcp.tool("extract_rules", json!({"file_path": "GUIDE.md"}))
            .done("extract 2 (same session)");
    }
    let first = disk_ids(root);
    assert!(!first.is_empty(), "the guide yields rules");
    let mut mcp = Mcp::spawn(root); // session 2 autoloads, then extracts again
    mcp.tool("extract_rules", json!({"file_path": "GUIDE.md"}))
        .done("extract 3 (next session)");
    let ids = disk_ids(root);
    let unique: std::collections::BTreeSet<_> = ids.iter().collect();
    assert_eq!(unique.len(), ids.len(), "duplicate ids on disk: {ids:?}");
    assert_eq!(ids, first);
    assert_eq!(pre_check_ls(root).status.code(), Some(0));
}

/// Older servers could write the same rule twice. Byte-for-byte identical
/// copies are unambiguous, so they load (one copy, with a warning); copies
/// that differ are rejected, because which one wins is a guess.
#[test]
fn identical_duplicate_rules_load_differing_ones_do_not() {
    let rule = r#"{"id":"A","phase":"pre","priority":1,"when":[{"new_content_contains":"zzz"}],"then":{"block":"no"}}"#;
    let dir = project(&format!(r#"{{"rules":[{rule},{rule}]}}"#));
    let out = pre_check_ls(dir.path());
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert!(err.contains("`A`") && err.contains("identical"), "{err}");

    let other = rule.replace("\"no\"", "\"different\"");
    let dir = project(&format!(r#"{{"rules":[{rule},{other}]}}"#));
    let out = pre_check_ls(dir.path());
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("duplicate rule id `A`"));
}

// ── Repair allowance must not be steerable ────────────────────────────────

/// A `loader.json` layer pointing at a source file must not make that source
/// file the "repair target": only `.phronesis/rules.json` and
/// `.phronesis/loader.json` are ever editable under a load error.
#[test]
fn repair_allowance_is_limited_to_the_phronesis_rules_files() {
    let dir = project(r#"{"rules":[]}"#);
    let root = dir.path();
    std::fs::create_dir_all(root.join("src")).expect("src");
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("main");
    std::fs::write(
        root.join(".phronesis/loader.json"),
        r#"{"version":1,"layers":[{"name":"x","path":"src/main.rs"}]}"#,
    )
    .expect("loader");
    let out = spawn_hook(root, &["pre-check"], &edit("src/main.rs"));
    assert_eq!(
        out.status.code(),
        Some(2),
        "a layer path must not open a source file to edits: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn codex_patch_payload(body: &str) -> Value {
    json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "apply_patch",
        "session_id": "s", "turn_id": "t", "tool_use_id": "u",
        "tool_input": {"command": format!("*** Begin Patch\n{body}*** End Patch\n")},
    })
}

#[test]
fn codex_repair_allowance_rejects_moves_and_deletes() {
    let dir = project(ORIGINAL);
    let root = dir.path();
    for body in [
        "*** Update File: .phronesis/rules.json\n*** Move to: src/evil.rs\n@@\n-x\n+y\n",
        "*** Delete File: .phronesis/rules.json\n",
    ] {
        let out = spawn_hook(
            root,
            &["codex-hook", "PreToolUse"],
            &codex_patch_payload(body),
        );
        let body_json: Value = serde_json::from_slice(&out.stdout).expect("json");
        assert_eq!(
            body_json["hookSpecificOutput"]["permissionDecision"], "deny",
            "{body:?} is not a repair: {body_json}"
        );
    }
}

#[test]
fn codex_context_events_lead_with_the_load_error() {
    let dir = project(ORIGINAL);
    for event in ["SessionStart", "UserPromptSubmit"] {
        let payload = json!({
            "hook_event_name": event, "session_id": "s", "turn_id": "t",
            "source": "startup", "prompt": "hello",
        });
        let out = spawn_hook(dir.path(), &["codex-hook", event], &payload);
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        assert!(
            text.contains("keep-a") && text.contains("description"),
            "codex {event} must surface the load error: {text}"
        );
    }
}

/// While the file was broken the server kept its startup copy. When the
/// file is repaired, the repaired version wins: rules changed on disk are
/// taken from disk and rules deleted from disk stay deleted.
#[test]
fn recovery_takes_the_repaired_file_not_the_servers_stale_copy() {
    let dir = project(
        r#"{"rules":[
          {"id":"A","phase":"pre","priority":1,"when":[{"new_content_contains":"a"}],"then":{"log":"old"}},
          {"id":"B","phase":"pre","priority":1,"when":[{"new_content_contains":"b"}],"then":{"log":"b"}}
        ]}"#,
    );
    let root = dir.path();
    let mut mcp = Mcp::spawn(root);
    std::fs::write(rules_path(root), ORIGINAL).expect("break");
    mcp.tool("add_rule", new_rule("C"))
        .refused("add_rule while broken");
    std::fs::write(
        rules_path(root),
        r#"{"rules":[{"id":"A","phase":"pre","priority":1,"when":[{"new_content_contains":"a"}],"then":{"log":"NEW"}}]}"#,
    )
    .expect("repair");
    mcp.tool("add_rule", new_rule("D"))
        .done("add_rule after repair");
    let text = std::fs::read_to_string(rules_path(root)).expect("rules");
    assert_eq!(disk_ids(root), vec!["A", "D"], "{text}");
    assert!(text.contains("NEW") && !text.contains("old"), "{text}");
}

#[test]
fn list_rules_reports_the_load_error_without_autopersist() {
    let dir = project(ORIGINAL);
    let mut mcp = Mcp::spawn_with(dir.path(), false);
    let listed = mcp.list_rules();
    assert!(
        listed["load_error"]
            .as_str()
            .is_some_and(|e| e.contains("keep-a")),
        "{listed}"
    );
}

// ── add_rule applies the loader's shape checks (D1) ───────────────────────

const ONE_GOOD_RULE: &str = r#"{"rules":[
  {"id":"keep","phase":"pre","priority":1,
   "when":[{"new_content_contains":"k"}],"then":{"warn":"k"}}
]}"#;

/// A rule the hook would refuse to load must be refused by `add_rule` too,
/// with a message naming the rule, the field, the bad value and what is
/// allowed — and it must never reach disk, where it would block every hook.
#[test]
fn add_rule_refuses_the_shapes_the_loader_rejects() {
    let dir = project(ONE_GOOD_RULE);
    let root = dir.path();
    let mut mcp = Mcp::spawn(root);

    let mut bad_verb = new_rule("bad-verb");
    bad_verb["actions"][0]["action_type"] = json!("Block");
    let err = mcp.tool("add_rule", bad_verb).refused("unknown verb");
    for part in [
        "rule `bad-verb`",
        "field `action_type`",
        "`Block`",
        "allowed:",
        "constraint_violation",
    ] {
        assert!(err.contains(part), "unknown verb: missing {part:?}: {err}");
    }

    let mut no_when = new_rule("no-when");
    no_when["conditions"] = json!([]);
    let err = mcp.tool("add_rule", no_when).refused("empty conditions");
    for part in ["rule `no-when`", "field `conditions`", "is empty"] {
        assert!(err.contains(part), "empty when: missing {part:?}: {err}");
    }

    let mut two_actions = new_rule("two-actions");
    two_actions["actions"] = json!([
        {"action_type": "log", "params": ["a"]},
        {"action_type": "log", "params": ["b"]}
    ]);
    let err = mcp.tool("add_rule", two_actions).refused("two actions");
    for part in ["rule `two-actions`", "field `actions`", "got 2"] {
        assert!(err.contains(part), "two actions: missing {part:?}: {err}");
    }

    let mut bad_phase = new_rule("bad-phase");
    bad_phase["phase"] = json!("Pre");
    let err = mcp.tool("add_rule", bad_phase).refused("phase typo");
    for part in [
        "rule `bad-phase`",
        "field `phase`",
        "`Pre`",
        "allowed:",
        "pre",
        "post",
    ] {
        assert!(err.contains(part), "phase typo: missing {part:?}: {err}");
    }

    // Nothing reached disk or the network.
    assert_eq!(
        std::fs::read_to_string(rules_path(root)).expect("rules"),
        ONE_GOOD_RULE
    );
    assert!(
        !bak_path(root).exists(),
        "a refused rule must not rotate .bak"
    );
    let ids: Vec<String> = mcp.list_rules()["rules"]
        .as_array()
        .expect("rules")
        .iter()
        .filter_map(|r| r["id"].as_str().map(str::to_string))
        .collect();
    assert_eq!(ids, vec!["keep"]);

    // The refusals left the server usable: a well-formed rule still saves.
    mcp.tool("add_rule", new_rule("good"))
        .done("valid add_rule");
    assert_eq!(disk_ids(root), vec!["keep", "good"]);
}

// ── Load errors without autopersist ───────────────────────────────────────

/// With `PHRONESIS_NO_AUTOPERSIST` the server writes only through
/// `save_rules`. `add_rule` then never touches disk and may proceed, but
/// `save_rules` must refuse while the file does not load, and work again once
/// it is repaired — keeping the repaired file's rules.
#[test]
fn without_autopersist_save_rules_refuses_until_the_file_loads() {
    let dir = project(ORIGINAL);
    let root = dir.path();
    let mut mcp = Mcp::spawn_with(root, false);

    mcp.tool("add_rule", new_rule("in-memory"))
        .done("add_rule never writes without autopersist");
    let err = mcp.tool("save_rules", json!({})).refused("save_rules");
    assert!(
        err.contains("refusing to change rules")
            && err.contains("keep-a")
            && err.contains("description")
            && err.contains("rules.json"),
        "save_rules must name the failing file and the load error: {err}"
    );
    assert_eq!(
        std::fs::read_to_string(rules_path(root)).expect("rules"),
        ORIGINAL
    );
    assert!(!bak_path(root).exists(), "no .bak rotation on refusal");

    std::fs::write(
        rules_path(root),
        ORIGINAL.replace(r#""description":"mine","#, ""),
    )
    .expect("repair");
    mcp.tool("save_rules", json!({}))
        .done("save_rules after repair");
    assert_eq!(disk_ids(root), vec!["keep-a", "keep-b", "in-memory"]);
    let listed = mcp.list_rules();
    assert!(listed.get("load_error").is_none(), "{listed}");
}

// ── Layered rules: refusal names the failing layer; recovery reloads all ──

fn layered_project(project_rules: &str, personal_rules: &str) -> tempfile::TempDir {
    let dir = project(project_rules);
    std::fs::write(dir.path().join("personal.json"), personal_rules).expect("personal");
    std::fs::write(
        dir.path().join(".phronesis/loader.json"),
        r#"{"layers":[
            {"name":"project","path":".phronesis/rules.json"},
            {"name":"personal","path":"personal.json","decision":"ADR-personal"}
        ]}"#,
    )
    .expect("loader");
    dir
}

const LAYER_PROJECT: &str = r#"{"rules":[
  {"id":"shared","phase":"pre","priority":1,"when":[{"p":"project"}],"then":{"log":"project"}},
  {"id":"mine","phase":"pre","priority":1,"when":[{"p":"mine"}],"then":{"log":"mine"}}
]}"#;

const LAYER_PERSONAL: &str = r#"{"rules":[
  {"id":"shared","phase":"pre","priority":9,"when":[{"p":"personal"}],"then":{"log":"personal"}}
]}"#;

fn override_facts(mcp: &mut Mcp) -> Vec<Value> {
    let text = mcp.tool("list_facts", json!({})).done("list_facts");
    let facts: Value = serde_json::from_str(&text).expect("list_facts JSON");
    facts["facts"]
        .as_array()
        .expect("facts")
        .iter()
        .filter(|f| f["predicate"] == "rule_overridden")
        .cloned()
        .collect()
}

/// A broken *layer* (not the project file) also blocks writes, and the
/// refusal names that layer's file. After the repair the server reloads every
/// layer: the project's shadowed definition stays on disk untouched, the
/// personal layer's rule is never written into the project file, and the
/// override provenance is re-derived once rather than duplicated.
#[test]
fn a_broken_layer_blocks_writes_and_recovery_reloads_every_layer() {
    let dir = layered_project(LAYER_PROJECT, LAYER_PERSONAL);
    let root = dir.path();
    let mut mcp = Mcp::spawn(root);
    assert_eq!(override_facts(&mut mcp).len(), 1);

    let broken_personal = LAYER_PERSONAL.replace(r#""log""#, r#""Log""#);
    std::fs::write(root.join("personal.json"), &broken_personal).expect("break layer");
    let err = mcp
        .tool("add_rule", new_rule("while-broken"))
        .refused("add_rule while a layer is broken");
    assert!(
        err.contains("personal.json") && err.contains("`Log`"),
        "refusal must name the failing layer file and the bad verb: {err}"
    );
    let listed = mcp.list_rules();
    assert!(
        listed["load_error"]
            .as_str()
            .is_some_and(|e| e.contains("`Log`")),
        "a failed write records the load error for list_rules: {listed}"
    );
    assert_eq!(
        std::fs::read_to_string(rules_path(root)).expect("rules"),
        LAYER_PROJECT,
        "the project file is not rewritten while a layer is broken"
    );

    std::fs::write(root.join("personal.json"), LAYER_PERSONAL).expect("repair layer");
    mcp.tool("add_rule", new_rule("after-repair"))
        .done("add_rule after repair");

    assert_eq!(disk_ids(root), vec!["shared", "mine", "after-repair"]);
    let disk: Value =
        serde_json::from_str(&std::fs::read_to_string(rules_path(root)).expect("rules"))
            .expect("json");
    let shared = disk["rules"]
        .as_array()
        .expect("rules")
        .iter()
        .find(|r| r["id"] == "shared")
        .expect("shared")
        .clone();
    assert_eq!(
        shared["priority"], 1,
        "the shadowed project definition must stay as written: {shared}"
    );
    let overrides = override_facts(&mut mcp);
    assert_eq!(overrides.len(), 1, "override facts: {overrides:?}");
    assert_eq!(overrides[0]["args"][0], "shared");
    let listed = mcp.list_rules();
    assert!(listed.get("load_error").is_none(), "{listed}");
}

// ── load_rules_file on a file that does not load ──────────────────────────

/// The explicit reload tool reports the load error instead of loading a
/// partial set, and loads the file once it is repaired.
#[test]
fn load_rules_file_reports_the_error_then_loads_the_repaired_file() {
    let dir = project(ORIGINAL);
    let root = dir.path();
    let mut mcp = Mcp::spawn_with(root, false);

    let err = mcp
        .tool("load_rules_file", json!({}))
        .refused("load_rules_file on a broken file");
    assert!(
        err.contains("keep-a") && err.contains("description"),
        "{err}"
    );
    assert!(
        mcp.list_rules()["rules"]
            .as_array()
            .expect("rules")
            .is_empty()
    );

    std::fs::write(
        rules_path(root),
        ORIGINAL.replace(r#""description":"mine","#, ""),
    )
    .expect("repair");
    let text = mcp
        .tool("load_rules_file", json!({}))
        .done("load_rules_file after repair");
    let summary: Value = serde_json::from_str(&text).expect("summary JSON");
    assert_eq!(summary["loaded"], 2, "{summary}");
    let listed = mcp.list_rules();
    assert!(
        listed.get("load_error").is_none(),
        "a file that loads again must not still be reported as failing: {listed}"
    );
}

/// With autopersist on, a file that broke after startup and was then
/// repaired is reloaded wholesale by `load_rules_file`, as a write would:
/// the repaired definitions win over the server's startup copy, rules
/// deleted from the file stay deleted, and the load error is cleared.
#[test]
fn load_rules_file_after_repair_takes_the_repaired_file() {
    let dir = project(
        r#"{"rules":[
          {"id":"A","phase":"pre","priority":1,"when":[{"new_content_contains":"a"}],"then":{"log":"old"}},
          {"id":"B","phase":"pre","priority":1,"when":[{"new_content_contains":"b"}],"then":{"log":"b"}}
        ]}"#,
    );
    let root = dir.path();
    let mut mcp = Mcp::spawn(root);
    std::fs::write(rules_path(root), ORIGINAL).expect("break");
    mcp.tool("add_rule", new_rule("C"))
        .refused("add_rule while broken");
    let repaired = r#"{"rules":[{"id":"A","phase":"pre","priority":1,"when":[{"new_content_contains":"a"}],"then":{"log":"NEW"}}]}"#;
    std::fs::write(rules_path(root), repaired).expect("repair");

    let summary: Value = serde_json::from_str(
        &mcp.tool("load_rules_file", json!({}))
            .done("load_rules_file after repair"),
    )
    .expect("summary json");
    assert_eq!(
        summary["loaded"], 1,
        "the repaired file's rule is loaded: {summary}"
    );
    assert_eq!(summary["skipped_duplicate_ids"], 0, "{summary}");
    let listed = mcp.list_rules();
    assert!(listed.get("load_error").is_none(), "{listed}");
    let rules = listed["rules"].as_array().expect("rules");
    assert_eq!(rules.len(), 1, "B was deleted from the file: {listed}");
    assert_eq!(rules[0]["actions"][0]["params"][0], "NEW", "{listed}");
    assert_eq!(
        std::fs::read_to_string(rules_path(root)).expect("rules"),
        repaired,
        "loading must not rewrite the file"
    );
}
