//! Malformed rule *semantics* fail closed at load, exactly like malformed
//! JSON already does (decision D1).
//!
//! Every rule below was written to block `git push --force`, and before this
//! suite each one loaded silently and allowed the push: a mis-cased verb, a
//! mis-cased phase, an empty `when`, a v1 argument that was not a string, an
//! unknown key, a second v1 action, a v1 action type that is really a v2
//! verb, and a duplicate id. The hooks now refuse to load the file: `pre-check`
//! blocks (exit 2), `post-check` warns (exit 1), `codex-hook PreToolUse`
//! denies, and `phr-mcp audit` / the MCP `load_rules_file` and `add_rule`
//! tools reject the same shapes with the same message.

use std::io::{BufRead, BufReader, Write as _};
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

const PUSH: &str = "git push --force origin main";

fn spawn(root: &Path, args: &[&str], payload: &Value) -> Output {
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
    child.wait_with_output().expect("wait for phr-mcp")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

fn project(rules: &Value) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp project");
    let phr = dir.path().join(".phronesis");
    std::fs::create_dir_all(&phr).expect("config dir");
    std::fs::write(
        phr.join("rules.json"),
        serde_json::to_vec_pretty(rules).expect("rules JSON"),
    )
    .expect("write rules");
    dir
}

fn bash(command: &str) -> Value {
    json!({"tool_name": "Bash", "tool_input": {"command": command}})
}

fn codex_bash(command: &str) -> Value {
    json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "session_id": "rule-validation",
        "turn_id": "rule-validation-turn",
        "tool_use_id": "rule-validation-uid",
        "tool_input": {"command": command},
    })
}

/// A malformed rule set plus the fragments the load error must name: the
/// rule id, the field, the bad value, and (where the field has a closed set)
/// one of the allowed values.
struct Case {
    name: &'static str,
    rules: Value,
    needles: &'static [&'static str],
}

fn v2(id: &str, phase: &str, when: Value, then: Value) -> Value {
    json!({"id": id, "phase": phase, "priority": 10, "when": when, "then": then})
}

fn push_when() -> Value {
    json!([{"new_content_contains": "git push --force"}])
}

fn cases(phase: &str) -> Vec<Case> {
    vec![
        Case {
            name: "(a) mis-cased verb",
            rules: json!({"rules": [v2("no-force-push", phase, push_when(), json!({"Block": "no force push"}))]}),
            needles: &[
                "no-force-push",
                "then",
                "Block",
                "block, warn, log, emit_capsule",
            ],
        },
        Case {
            name: "(b) mis-cased phase",
            rules: json!({"rules": [v2("no-force-push", "Pre", push_when(), json!({"block": "no force push"}))]}),
            needles: &["no-force-push", "phase", "Pre", "pre, post, audit, none"],
        },
        Case {
            name: "(c) empty when",
            rules: json!({"rules": [v2("no-force-push", phase, json!([]), json!({"block": "no force push"}))]}),
            needles: &["no-force-push", "when", "empty"],
        },
        Case {
            name: "(d) v1 non-string arg",
            rules: json!({"rules": [{
                "id": "no-force-push", "phase": phase, "priority": 10,
                "conditions": [{"predicate": "new_content_contains", "args": [{"re": "git push --force"}]}],
                "actions": [{"action_type": "constraint_violation", "params": ["no force push"]}]
            }]}),
            needles: &[
                "no-force-push",
                "args",
                "{\"re\":\"git push --force\"}",
                "string",
            ],
        },
        Case {
            name: "unknown top-level key",
            rules: json!({"rules": [{
                "id": "no-force-push", "Phase": phase, "priority": 10,
                "when": push_when(), "then": {"block": "no force push"}
            }]}),
            needles: &["no-force-push", "Phase", "unknown key", "phase"],
        },
        Case {
            name: "v1 second action silently dropped",
            rules: json!({"rules": [{
                "id": "no-force-push", "phase": phase, "priority": 10,
                "conditions": [{"predicate": "new_content_contains", "args": ["git push --force"]}],
                "actions": [
                    {"action_type": "log", "params": ["seen"]},
                    {"action_type": "constraint_violation", "params": ["no force push"]}
                ]
            }]}),
            needles: &["no-force-push", "actions", "exactly one"],
        },
        Case {
            name: "v1 action_type is a v2 verb",
            rules: json!({"rules": [{
                "id": "no-force-push", "phase": phase, "priority": 10,
                "conditions": [{"predicate": "new_content_contains", "args": ["git push --force"]}],
                "actions": [{"action_type": "block", "params": ["no force push"]}]
            }]}),
            needles: &[
                "no-force-push",
                "action_type",
                "block",
                "constraint_violation",
            ],
        },
        Case {
            name: "duplicate id in one file",
            rules: json!({"rules": [
                v2("no-force-push", phase, push_when(), json!({"block": "no force push"})),
                v2("no-force-push", phase, json!([{"new_content_contains": "zzz-never"}]), json!({"log": "shadow"}))
            ]}),
            needles: &["no-force-push", "duplicate", "id"],
        },
    ]
}

fn assert_names(case: &Case, text: &str) {
    for needle in case.needles {
        assert!(
            text.contains(needle),
            "{}: load error must contain `{needle}`; got: {text}",
            case.name
        );
    }
}

#[test]
fn pre_check_blocks_every_malformed_rule_shape() {
    for case in cases("pre") {
        let dir = project(&case.rules);
        let out = spawn(dir.path(), &["pre-check"], &bash(PUSH));
        let err = stderr(&out);
        assert_eq!(
            out.status.code(),
            Some(2),
            "{}: pre-check must BLOCK a malformed rules file; stderr: {err}",
            case.name
        );
        assert_names(&case, &err);
        // The block is a load error, not a rule firing: an unrelated command
        // is blocked too, so the file cannot silently allow anything.
        let out = spawn(dir.path(), &["pre-check"], &bash("ls"));
        assert_eq!(out.status.code(), Some(2), "{}", case.name);
    }
}

#[test]
fn post_check_warns_on_every_malformed_rule_shape() {
    for case in cases("post") {
        let dir = project(&case.rules);
        let mut payload = bash(PUSH);
        payload["tool_response"] = json!({});
        let out = spawn(dir.path(), &["post-check"], &payload);
        let err = stderr(&out);
        assert_eq!(
            out.status.code(),
            Some(1),
            "{}: post-check must WARN; stderr: {err}",
            case.name
        );
        assert_names(&case, &err);
    }
}

#[test]
fn codex_pre_tool_use_denies_every_malformed_rule_shape() {
    for case in cases("pre") {
        let dir = project(&case.rules);
        let out = spawn(dir.path(), &["codex-hook", "PreToolUse"], &codex_bash(PUSH));
        let body: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
            panic!(
                "{}: codex-hook stdout must be JSON ({e}): {}",
                case.name,
                String::from_utf8_lossy(&out.stdout)
            )
        });
        let hso = &body["hookSpecificOutput"];
        assert_eq!(
            hso["permissionDecision"],
            "deny",
            "{}: Codex must deny; body: {body}; stderr: {}",
            case.name,
            stderr(&out)
        );
        let reason = hso["permissionDecisionReason"].as_str().unwrap_or_default();
        assert_names(&case, reason);
    }
}

#[test]
fn audit_rejects_every_malformed_rule_shape() {
    for case in cases("pre") {
        let dir = project(&case.rules);
        std::fs::write(dir.path().join("a.sh"), "git push --force\n").expect("file");
        let out = spawn(dir.path(), &["audit"], &json!({}));
        let err = stderr(&out);
        assert_ne!(
            out.status.code(),
            Some(0),
            "{}: audit must fail on a malformed rules file; stderr: {err}",
            case.name
        );
        assert_names(&case, &err);
    }
}

// ── MCP: load_rules_file and add_rule ─────────────────────────────────────

struct Mcp {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    next_id: u64,
}

impl Mcp {
    fn spawn(root: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
            .arg("serve")
            .env("PHRONESIS_PROJECT_ROOT", root)
            .env("PHRONESIS_NO_AUTOPERSIST", "1")
            .env("PHRONESIS_NO_ACTION_LOG", "1")
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

    /// The error text of a failed tool call; panics when the call succeeded.
    fn tool_error(&mut self, name: &str, args: Value) -> String {
        let r = self.call("tools/call", json!({"name": name, "arguments": args}));
        if let Some(msg) = r["error"]["message"].as_str() {
            return msg.to_string();
        }
        assert_eq!(
            r["result"]["isError"], true,
            "{name} must reject the rule; response: {r}"
        );
        r["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn mcp_load_rules_file_rejects_every_malformed_rule_shape() {
    for case in cases("pre") {
        let dir = project(&case.rules);
        let mut mcp = Mcp::spawn(dir.path());
        let err = mcp.tool_error("load_rules_file", json!({}));
        assert_names(&case, &err);
    }
}

#[test]
fn mcp_add_rule_rejects_unknown_action_type_and_empty_conditions() {
    let dir = tempfile::tempdir().expect("temp project");
    let mut mcp = Mcp::spawn(dir.path());
    let err = mcp.tool_error(
        "add_rule",
        json!({
            "id": "no-force-push", "priority": 10,
            "conditions": [{"predicate": "new_content_contains", "args": ["git push --force"]}],
            "actions": [{"action_type": "Block", "params": ["no force push"]}]
        }),
    );
    for needle in [
        "no-force-push",
        "action_type",
        "Block",
        "constraint_violation",
    ] {
        assert!(err.contains(needle), "missing `{needle}`: {err}");
    }
    let err = mcp.tool_error(
        "add_rule",
        json!({
            "id": "no-force-push", "priority": 10,
            "conditions": [],
            "actions": [{"action_type": "constraint_violation", "params": ["no force push"]}]
        }),
    );
    for needle in ["no-force-push", "conditions", "empty"] {
        assert!(err.contains(needle), "missing `{needle}`: {err}");
    }
    let err = mcp.tool_error(
        "add_rule",
        json!({
            "id": "no-force-push", "priority": 10,
            "conditions": [{"predicate": "new_content_contains", "args": ["git push --force"]}],
            "actions": []
        }),
    );
    for needle in ["no-force-push", "actions", "exactly one"] {
        assert!(err.contains(needle), "missing `{needle}`: {err}");
    }
}

/// The accepted spellings still load: v2 verbs, the internal action names as
/// verbs (existing fixtures use `constraint_violation`), every phase, and a
/// missing phase (defaults to `pre`).
#[test]
fn well_formed_rules_still_block_the_push() {
    let rules = json!({"rules": [
        {"id": "no-force-push", "priority": 10, "when": push_when(), "then": {"block": "no force push"}},
        v2("internal-verb", "post", push_when(), json!({"constraint_violation": "x"})),
        v2("audit-only", "audit", push_when(), json!({"warn": "x"})),
        v2("disabled", "none", push_when(), json!({"log": "x"})),
        {"id": "v1", "phase": "pre", "priority": 1,
         "conditions": [{"predicate": "new_content_contains", "args": ["zzz-never"]}],
         "actions": [{"action_type": "constraint_warning", "params": ["x"]}]}
    ]});
    let dir = project(&rules);
    let out = spawn(dir.path(), &["pre-check"], &bash(PUSH));
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(2), "{err}");
    assert!(err.contains("no force push"), "{err}");
    let out = spawn(dir.path(), &["pre-check"], &bash("ls"));
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
}
