//! Governance characterization at baseline efc55f4, written before the
//! planned parser extraction. Each test pins *current* observable behavior
//! through public surfaces only: `phr-mcp` subprocesses (hooks, codex adapter,
//! CLI) and the MCP stdio server. If extraction changes any assertion here,
//! that is a deliberate behavior change and must be reviewed, not silently
//! migrated.
//!
//! Covered: strict v1/v2 load errors (fail closed), valid phases/actions, OR
//! clauses, emit_capsule, rule-layer overrides over absolute temp paths,
//! pre/post/Codex decisions, and persistent server + persistence behavior.

use std::io::{BufRead, BufReader, Write as _};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Output, Stdio};

use serde_json::{Value, json};

const FORCE_PUSH: &str = "git push --force origin main";
const OVERRIDE_MSG: &str = "overridden: force push is logged by the override layer";

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/governance")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read fixture {name}: {e}"))
}

fn spawn(root: &Path, args: &[&str], payload: &Value) -> Output {
    spawn_env(root, args, payload, &[])
}

fn spawn_env(root: &Path, args: &[&str], payload: &Value, extra_env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_phr-mcp"));
    cmd.args(args)
        .env("PHRONESIS_PROJECT_ROOT", root)
        .env("PHRONESIS_NO_ACTION_LOG", "1")
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in extra_env {
        if v.is_empty() {
            cmd.env_remove(k);
        } else {
            cmd.env(k, v);
        }
    }
    let mut child = cmd.spawn().expect("spawn phr-mcp");
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

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn bash(command: &str) -> Value {
    json!({"tool_name": "Bash", "tool_input": {"command": command}})
}

fn post_bash(command: &str) -> Value {
    let mut payload = bash(command);
    payload["tool_response"] = json!({});
    payload
}

fn codex_bash(command: &str) -> Value {
    json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "session_id": "t4-characterization",
        "turn_id": "t4-turn",
        "tool_use_id": "t4-uid",
        "tool_input": {"command": command},
    })
}

fn governed_project(rules: &Value) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp project");
    write_rules(dir.path(), rules.clone());
    dir
}

fn write_rules(root: &Path, rules: Value) {
    let phr = root.join(".phronesis");
    std::fs::create_dir_all(&phr).expect("config dir");
    std::fs::write(
        phr.join("rules.json"),
        serde_json::to_vec_pretty(&rules).expect("rules JSON"),
    )
    .expect("write rules");
}

fn v2(id: &str, phase: &str, when: Value, then: Value) -> Value {
    json!({"id": id, "phase": phase, "priority": 10, "when": when, "then": then})
}

// ── Strict v1/v2 parser errors (fail closed at load) ───────────────────────

struct BadCase {
    name: &'static str,
    rules: Value,
    needles: &'static [&'static str],
}

fn bad_cases() -> Vec<BadCase> {
    let push_when = || json!([{"new_content_contains": "git push --force"}]);
    vec![
        BadCase {
            name: "mis-cased v2 verb",
            rules: json!({"rules": [v2("no-force-push", "pre", push_when(), json!({"Block": "no force push"}))]}),
            needles: &[
                "no-force-push",
                "then",
                "Block",
                "block, warn, log, emit_capsule",
            ],
        },
        BadCase {
            name: "mis-cased phase",
            rules: json!({"rules": [v2("no-force-push", "Pre", push_when(), json!({"block": "no force push"}))]}),
            needles: &["no-force-push", "phase", "Pre", "pre, post, audit, none"],
        },
        BadCase {
            name: "empty when",
            rules: json!({"rules": [v2("no-force-push", "pre", json!([]), json!({"block": "no force push"}))]}),
            needles: &["no-force-push", "when", "empty"],
        },
        BadCase {
            name: "v1 non-string arg",
            rules: json!({"rules": [{
                "id": "no-force-push", "phase": "pre", "priority": 10,
                "conditions": [{"predicate": "new_content_contains", "args": [{"re": "git push --force"}]}],
                "actions": [{"action_type": "constraint_violation", "params": ["no force push"]}]
            }]}),
            needles: &["no-force-push", "args", "string"],
        },
        BadCase {
            name: "v1 action_type spelled as a v2 verb",
            rules: json!({"rules": [{
                "id": "no-force-push", "phase": "pre", "priority": 10,
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
        BadCase {
            name: "duplicate id in one file",
            rules: json!({"rules": [
                v2("no-force-push", "pre", push_when(), json!({"block": "no force push"})),
                v2("no-force-push", "pre", json!([{"new_content_contains": "zzz-never"}]), json!({"log": "shadow"}))
            ]}),
            needles: &["no-force-push", "duplicate", "id"],
        },
        BadCase {
            name: "invalid emit_capsule contract",
            rules: json!({"rules": [v2("cap-bad", "post", push_when(), json!({"emit_capsule": {
                "id": "cap-bad", "body": "x", "lifecycle": "forever"
            }}))]}),
            needles: &["cap-bad", "Invalid lifecycle: forever"],
        },
    ]
}

fn assert_needles(name: &str, needles: &[&str], text: &str) {
    for needle in needles {
        assert!(
            text.contains(needle),
            "{name}: error must contain `{needle}`; got: {text}"
        );
    }
}

#[test]
fn strict_parser_errors_block_pre_check_and_name_the_offense() {
    for case in bad_cases() {
        let dir = governed_project(&case.rules);
        let out = spawn(dir.path(), &["pre-check"], &bash(FORCE_PUSH));
        let err = stderr(&out);
        assert_eq!(
            out.status.code(),
            Some(2),
            "{}: pre-check must block a malformed rules file; stderr: {err}",
            case.name
        );
        assert_needles(case.name, case.needles, &err);
        // Fail closed: an unrelated command is blocked by the same load error.
        let out = spawn(dir.path(), &["pre-check"], &bash("ls"));
        assert_eq!(out.status.code(), Some(2), "{}", case.name);
    }
}

#[test]
fn strict_parser_errors_warn_post_check_and_deny_codex() {
    let case = &bad_cases()[0];
    let dir = governed_project(&case.rules);

    let out = spawn(dir.path(), &["post-check"], &post_bash(FORCE_PUSH));
    let err = stderr(&out);
    assert_eq!(
        out.status.code(),
        Some(1),
        "post-check must warn on a malformed rules file; stderr: {err}"
    );
    assert_needles(case.name, case.needles, &err);

    let out = spawn(
        dir.path(),
        &["codex-hook", "PreToolUse"],
        &codex_bash(FORCE_PUSH),
    );
    assert_eq!(out.status.code(), Some(0), "Codex process exit stays zero");
    let body: Value = serde_json::from_slice(&out.stdout).expect("codex stdout JSON");
    let hso = &body["hookSpecificOutput"];
    assert_eq!(hso["permissionDecision"], "deny", "body: {body}");
    assert_needles(
        case.name,
        case.needles,
        hso["permissionDecisionReason"].as_str().unwrap_or_default(),
    );
}

// ── Valid phases, actions, OR clauses, capsules ────────────────────────────

#[test]
fn valid_phases_drive_pre_post_audit_and_parked_rules() {
    let when = || json!([{"new_content_contains": "git push --force"}]);

    // pre + block: blocks violating, allows clean.
    let dir = governed_project(
        &json!({"rules": [v2("r-pre", "pre", when(), json!({"block": "no force push"}))]}),
    );
    let out = spawn(dir.path(), &["pre-check"], &bash(FORCE_PUSH));
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("no force push"), "{}", stderr(&out));
    let out = spawn(dir.path(), &["pre-check"], &bash("ls"));
    assert_eq!(out.status.code(), Some(0));

    // post + warn: warns violating (exit 1), allows clean (exit 0). Post
    // command rules match the command through the bash_command regex
    // predicates; `new_content_contains` is a pre-phase surface for Bash.
    let dir = governed_project(
        &json!({"rules": [v2("r-post", "post", json!([{"bash_command_matches": "git push --force"}]), json!({"warn": "force push seen"}))]}),
    );
    let out = spawn(dir.path(), &["post-check"], &post_bash(FORCE_PUSH));
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("force push seen"), "{}", stderr(&out));
    let out = spawn(dir.path(), &["post-check"], &post_bash("ls"));
    assert_eq!(out.status.code(), Some(0));

    // audit phase: hooks skip it; the whole-tree audit reports it — but only
    // when the rule explicitly opts in with `"audit": true` (phase alone is
    // not enough: rules without the flag are skipped by the audit engine).
    let dir = tempfile::tempdir().unwrap();
    write_rules(
        dir.path(),
        json!({"rules": [{
            "id": "r-audit", "phase": "audit", "priority": 10, "audit": true,
            "when": when(), "then": {"warn": "audit-only finding"}
        }]}),
    );
    std::fs::write(dir.path().join("push.sh"), "git push --force\n").expect("fixture file");
    let out = spawn(dir.path(), &["pre-check"], &bash(FORCE_PUSH));
    assert_eq!(out.status.code(), Some(0), "audit rules do not block hooks");
    let out = spawn(dir.path(), &["audit", "--json"], &json!({}));
    assert_eq!(out.status.code(), Some(0));
    let report: Value = serde_json::from_str(&stdout(&out)).expect("audit --json");
    let finding = report["rules"]
        .as_array()
        .expect("rules array")
        .iter()
        .find(|r| r["rule_id"] == "r-audit")
        .unwrap_or_else(|| panic!("audit must report r-audit: {}", stdout(&out)));
    assert!(finding["hits"].as_u64().expect("hits") >= 1);

    // none phase: parked, never fires anywhere.
    let dir = governed_project(
        &json!({"rules": [v2("r-none", "none", when(), json!({"block": "parked"}))]}),
    );
    let out = spawn(dir.path(), &["pre-check"], &bash(FORCE_PUSH));
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn or_clause_fires_when_any_alternative_matches() {
    let rules = json!({"rules": [v2(
        "r-or",
        "post",
        json!([{"or": [
            {"bash_command_matches": "rm -rf /"},
            {"bash_command_matches": "DROP TABLE"}
        ]}]),
        json!({"warn": "destructive command"}),
    )]});
    let dir = governed_project(&rules);

    let out = spawn(
        dir.path(),
        &["post-check"],
        &post_bash("echo DROP TABLE users"),
    );
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("destructive command"));

    let out = spawn(dir.path(), &["post-check"], &post_bash("rm -rf /tmp/x"));
    assert_eq!(
        out.status.code(),
        Some(1),
        "second alternative also matches"
    );

    let out = spawn(dir.path(), &["post-check"], &post_bash("ls -la"));
    assert_eq!(out.status.code(), Some(0), "no alternative matches");
}

#[test]
fn emit_capsule_rule_records_a_capsule_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let phr = dir.path().join(".phronesis");
    std::fs::create_dir_all(&phr).expect("config dir");
    std::fs::write(phr.join("rules.json"), fixture("rules-capsule-or.json")).expect("write rules");

    let out = spawn(
        dir.path(),
        &["post-check"],
        &post_bash("echo DROP TABLE users"),
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "emit_capsule neither blocks nor warns"
    );
    assert!(stderr(&out).is_empty(), "stderr: {}", stderr(&out));

    // The capsule is durable on disk and visible through the context CLI.
    let store = dir.path().join(".phronesis/emitted-capsules.json");
    assert!(store.exists(), "capsule store written at hook time");
    let child = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(["context", "list", "--json", "--path"])
        .arg(dir.path())
        .env("PHRONESIS_NO_ACTION_LOG", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn context list");
    let out = child.wait_with_output().expect("context list");
    let listed: Value = serde_json::from_str(&stdout(&out)).expect("context list JSON");
    let capsule = listed["capsules"]
        .as_array()
        .expect("capsules array")
        .iter()
        .find(|c| c["id"] == "cap-danger")
        .unwrap_or_else(|| panic!("cap-danger listed: {}", stdout(&out)));
    assert_eq!(capsule["lifecycle"], "session");
    // The OR clause unfolds into per-branch disk rules with suffixed ids
    // (`cap-danger#or1`), which is what the provenance records as the emitter.
    assert_eq!(capsule["emitted_by"], "cap-danger#or1");
    assert_eq!(capsule["priority"], 60);
}

// ── Rule-layer overrides over absolute temp paths ──────────────────────────

/// Writes the fixture layer files into the project root at opaque absolute
/// temp paths (inside the tempdir, so the paths are known to the test but
/// unique per run) and renders `loader.json` from the template.
fn layered_project(extra_layer: Option<(&str, &str)>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let phr = dir.path().join(".phronesis");
    std::fs::create_dir_all(&phr).expect("config dir");

    let base = dir.path().join("base-rules-layer.json");
    std::fs::write(&base, fixture("loader-base-rules.json")).expect("base layer");
    let override_path = dir.path().join("override-rules-layer.json");
    std::fs::write(&override_path, fixture("loader-override-rules.json")).expect("override layer");

    let mut template = fixture("loader-template.json");
    template = template.replace("__LAYER_BASE__", &base.display().to_string());
    template = template.replace("__LAYER_OVERRIDE__", &override_path.display().to_string());
    if let Some((name, path)) = extra_layer {
        let config: Value = serde_json::from_str(&template).expect("template JSON");
        let mut layers = config["layers"].as_array().expect("layers").clone();
        layers.push(json!({"name": name, "path": path}));
        template = serde_json::to_string_pretty(&json!({"version": 1, "layers": layers})).unwrap();
    }
    std::fs::write(phr.join("loader.json"), template).expect("write loader.json");
    dir
}

#[test]
fn loader_layers_override_a_rule_from_a_later_absolute_temp_path() {
    // Baseline without layers: the base rule alone blocks.
    let base: Value =
        serde_json::from_str(&fixture("loader-base-rules.json")).expect("fixture JSON");
    let dir = governed_project(&base);
    let out = spawn(dir.path(), &["pre-check"], &bash(FORCE_PUSH));
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("base layer"), "{}", stderr(&out));

    // The override's `log` action is silent on the hook surface but is
    // recorded as a log consequence in the action log.
    let dir = layered_project(None);
    let out = spawn_env(
        dir.path(),
        &["pre-check"],
        &bash(FORCE_PUSH),
        &[("PHRONESIS_NO_ACTION_LOG", "")],
    );
    let err = stderr(&out);
    assert_eq!(
        out.status.code(),
        Some(0),
        "override layer replaces the blocking rule; stderr: {err}"
    );
    assert!(
        !err.contains("base layer"),
        "blocked message must be gone: {err}"
    );
    assert!(err.is_empty(), "log action is silent on stderr: {err}");
    let log = std::fs::read_to_string(dir.path().join(".phronesis/log.jsonl")).expect("log.jsonl");
    assert!(
        log.contains(OVERRIDE_MSG) && log.contains("\"action_type\":\"log\""),
        "log consequence recorded: {log}"
    );
}

#[test]
fn loader_config_errors_fail_closed_with_a_diagnosis() {
    // Required layer missing -> fail closed naming the layer.
    let dir = layered_project(Some(("ghost", "/nonexistent/t4/ghost-rules.json")));
    let out = spawn(dir.path(), &["pre-check"], &bash("ls"));
    let err = stderr(&out);
    assert_eq!(
        out.status.code(),
        Some(2),
        "missing required layer must block; stderr: {err}"
    );
    assert!(
        err.contains("ghost") && err.contains("does not exist"),
        "{err}"
    );

    // Unsupported version -> fail closed naming the version.
    let dir = tempfile::tempdir().unwrap();
    let phr = dir.path().join(".phronesis");
    std::fs::create_dir_all(&phr).expect("config dir");
    std::fs::write(
        phr.join("loader.json"),
        r#"{"version": 2, "layers": [{"name": "base", "path": "rules.json"}]}"#,
    )
    .expect("write loader.json");
    let out = spawn(dir.path(), &["pre-check"], &bash("ls"));
    let err = stderr(&out);
    assert_eq!(
        out.status.code(),
        Some(2),
        "unsupported loader version must block; stderr: {err}"
    );
    assert!(
        err.contains("version 2") && err.contains("expected 1"),
        "{err}"
    );
}

#[test]
fn loader_optional_layer_may_be_absent() {
    let dir = layered_project(None);
    // Rewrite loader.json marking the override layer optional, then delete it.
    let phr = dir.path().join(".phronesis");
    let config: Value = serde_json::from_str(
        &std::fs::read_to_string(phr.join("loader.json")).expect("loader.json"),
    )
    .expect("loader JSON");
    let mut layers = config["layers"].as_array().expect("layers").clone();
    layers[1]["optional"] = json!(true);
    std::fs::write(
        phr.join("loader.json"),
        serde_json::to_string_pretty(&json!({"version": 1, "layers": layers})).unwrap(),
    )
    .expect("rewrite loader.json");
    std::fs::remove_file(dir.path().join("override-rules-layer.json")).expect("remove layer");

    // Base rule is still effective: force push is blocked again.
    let out = spawn(dir.path(), &["pre-check"], &bash(FORCE_PUSH));
    let err = stderr(&out);
    assert_eq!(
        out.status.code(),
        Some(2),
        "optional missing layer is skipped; stderr: {err}"
    );
    assert!(err.contains("base layer"), "{err}");
}

// ── Codex hook decisions ───────────────────────────────────────────────────

#[test]
fn codex_hook_decisions_are_structured_json_with_zero_exit() {
    let dir = governed_project(&json!({"rules": [
        v2("r-block", "pre", json!([{"new_content_contains": "git push --force"}]), json!({"block": "no force push"})),
        v2("r-warn", "pre", json!([{"new_content_contains": "deploy to prod"}]), json!({"warn": "deploy needs review"})),
    ]}));

    // Clean: an empty JSON object — no allow envelope, no decision field.
    let out = spawn(dir.path(), &["codex-hook", "PreToolUse"], &codex_bash("ls"));
    assert_eq!(out.status.code(), Some(0));
    let body: Value = serde_json::from_slice(&out.stdout).expect("codex stdout JSON");
    assert_eq!(body, json!({}), "clean PreToolUse renders an empty object");

    // Warn: allow with the message as additionalContext (no permissionDecision).
    let out = spawn(
        dir.path(),
        &["codex-hook", "PreToolUse"],
        &codex_bash("deploy to prod"),
    );
    assert_eq!(out.status.code(), Some(0));
    let body: Value = serde_json::from_slice(&out.stdout).expect("codex stdout JSON");
    let hso = &body["hookSpecificOutput"];
    assert_eq!(
        hso["additionalContext"], "deploy needs review",
        "body: {body}"
    );
    assert_eq!(hso["hookEventName"], "PreToolUse");
    assert!(
        hso.get("permissionDecision").is_none(),
        "warn carries no permissionDecision: {body}"
    );

    // Deny: structured permissionDecision, process exit stays zero.
    let out = spawn(
        dir.path(),
        &["codex-hook", "PreToolUse"],
        &codex_bash(FORCE_PUSH),
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "deny is structured JSON, exit stays 0"
    );
    let body: Value = serde_json::from_slice(&out.stdout).expect("codex stdout JSON");
    let hso = &body["hookSpecificOutput"];
    assert_eq!(hso["permissionDecision"], "deny");
    assert!(
        hso["permissionDecisionReason"]
            .as_str()
            .unwrap_or_default()
            .contains("no force push")
    );

    // Non-tool lifecycle events fail open with an empty JSON object.
    let out = spawn(
        dir.path(),
        &["codex-hook", "UserPromptSubmit"],
        &json!({"hook_event_name": "UserPromptSubmit", "prompt": "hello"}),
    );
    assert_eq!(out.status.code(), Some(0));
    let body: Value = serde_json::from_slice(&out.stdout).expect("lifecycle JSON object");
    assert_eq!(body, json!({}));
}

// ── Persistent MCP server + persistence ────────────────────────────────────

struct McpClient {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl McpClient {
    fn spawn(project_root: &Path, extra_env: &[(&str, &str)]) -> Self {
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
            stdin: Some(stdin),
            stdout,
            next_id: 0,
        };
        client.call(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name":"t4","version":"0.1"}
            }),
        );
        client.notify("notifications/initialized", json!({}));
        client
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(self.stdin.as_mut().expect("server stdin is open"), "{msg}").unwrap();
        self.stdin
            .as_mut()
            .expect("server stdin is open")
            .flush()
            .unwrap();
        // The server may complete requests out of order (autosave runs off the
        // response path), so pair responses by id, skipping notifications.
        loop {
            let mut line = String::new();
            let n = self
                .stdout
                .read_line(&mut line)
                .expect("read server response");
            assert!(n > 0, "server closed stdout before answering id {id}");
            let value: Value = serde_json::from_str(&line).expect("JSON-RPC line");
            if value.get("id").and_then(Value::as_u64) == Some(id) {
                return value;
            }
        }
    }

    fn notify(&mut self, method: &str, params: Value) {
        let msg = json!({"jsonrpc":"2.0","method":method,"params":params});
        writeln!(self.stdin.as_mut().expect("server stdin is open"), "{msg}").unwrap();
        self.stdin
            .as_mut()
            .expect("server stdin is open")
            .flush()
            .unwrap();
    }

    fn tool(&mut self, name: &str, args: Value) -> Value {
        let r = self.call("tools/call", json!({"name": name, "arguments": args}));
        let result = &r["result"];
        assert_eq!(result["isError"], false, "tool call {name} failed: {r}");
        let text = result["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("tool {name} returned no text: {r}"));
        serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string()))
    }

    fn tool_error(&mut self, name: &str, args: Value) -> String {
        let r = self.call("tools/call", json!({"name": name, "arguments": args}));
        r["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn add_rule_args() -> Value {
    json!({
        "id": "persisted-rule", "phase": "post", "priority": 10,
        "conditions": [{"predicate": "bash_command_matches", "args": ["git push --force"]}],
        "actions": [{"action_type": "constraint_warning", "params": ["force push seen"]}]
    })
}

#[test]
fn mcp_add_rule_takes_v1_shape_and_rejects_v2_disk_form() {
    let dir = tempfile::tempdir().unwrap();
    let mut client = McpClient::spawn(dir.path(), &[]);

    // The wire form is the v1 internal shape (conditions/actions).
    client.tool("add_rule", add_rule_args());
    let listed = client.tool("list_rules", json!({}));
    let rule = &listed["rules"][0];
    assert_eq!(rule["id"], "persisted-rule");

    // A v2 disk-form rule object (`when`/`then`) is refused with the schema
    // error, mirroring the fail-closed loader.
    let err = client.tool_error(
        "add_rule",
        json!({
            "id": "v2-shaped", "phase": "post", "priority": 10,
            "when": [{"new_content_contains": "x"}],
            "then": {"warn": "nope"}
        }),
    );
    assert!(
        err.contains("missing field `conditions`"),
        "add_rule error: {err}"
    );
}

#[test]
fn mcp_autopersist_writes_rules_a_new_server_autoloads() {
    let dir = tempfile::tempdir().unwrap();

    // Default server: autosave on every add_rule.
    let mut client = McpClient::spawn(dir.path(), &[]);
    client.tool("add_rule", add_rule_args());
    let on_disk =
        std::fs::read_to_string(dir.path().join(".phronesis/rules.json")).expect("rules.json");
    assert!(
        on_disk.contains("persisted-rule"),
        "autosave wrote disk: {on_disk}"
    );
    drop(client);

    // A fresh server hydrates from disk (autoload).
    let mut client = McpClient::spawn(dir.path(), &[]);
    let listed = client.tool("list_rules", json!({}));
    let ids: Vec<&str> = listed["rules"]
        .as_array()
        .expect("rules array")
        .iter()
        .filter_map(|r| r["id"].as_str())
        .collect();
    assert!(
        ids.contains(&"persisted-rule"),
        "autoload after restart: {listed}"
    );

    // The hydrated rule governs a fresh hook process in the same project.
    let out = spawn(dir.path(), &["post-check"], &post_bash(FORCE_PUSH));
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("force push seen"));
}

#[test]
fn mcp_no_autopersist_requires_explicit_save_rules() {
    let dir = tempfile::tempdir().unwrap();
    let mut client = McpClient::spawn(dir.path(), &[("PHRONESIS_NO_AUTOPERSIST", "1")]);
    client.tool("add_rule", add_rule_args());
    let on_disk =
        std::fs::read_to_string(dir.path().join(".phronesis/rules.json")).unwrap_or_default();
    assert!(
        !on_disk.contains("persisted-rule"),
        "autosave disabled: {on_disk}"
    );

    client.tool("save_rules", json!({"dry_run": true}));
    let on_disk =
        std::fs::read_to_string(dir.path().join(".phronesis/rules.json")).unwrap_or_default();
    assert!(
        !on_disk.contains("persisted-rule"),
        "dry run must not write: {on_disk}"
    );

    client.tool("save_rules", json!({}));
    let on_disk =
        std::fs::read_to_string(dir.path().join(".phronesis/rules.json")).expect("rules.json");
    assert!(
        on_disk.contains("persisted-rule"),
        "explicit save wrote disk: {on_disk}"
    );
}

#[test]
fn mcp_capsule_emission_persists_across_server_restart() {
    let dir = tempfile::tempdir().unwrap();
    // Rule mutation + fact + firing run without autopersist: the autosave
    // path rebuilds the network from disk in the background and can race an
    // immediately-following fact assertion out of working memory (observed
    // while characterizing; the existing suite also runs this flow with
    // autosave disabled).
    let mut client = McpClient::spawn(dir.path(), &[("PHRONESIS_NO_AUTOPERSIST", "1")]);
    client.tool(
        "add_rule",
        json!({
            "id": "mcp-capsule", "priority": 5,
            "conditions": [{"predicate": "status", "args": ["?state"]}],
            "actions": [{"action_type": "emit_capsule", "params": [], "data": {
                "id": "cap-from-mcp", "body": "state=?state", "lifecycle": "persistent", "priority": 50
            }}]
        }),
    );
    client.tool(
        "assert_fact",
        json!({"id": "status-ready", "predicate": "status", "args": ["ready"]}),
    );
    let fired = client.tool("fire_rules", json!({}));
    assert_eq!(fired["actions_fired"], 1);
    assert_eq!(
        fired["consequences"][0]["provenance"]["rule_id"],
        "mcp-capsule"
    );
    assert_eq!(
        fired["consequences"][0]["payload"]["data"]["body"],
        "state=ready"
    );
    let listed = client.tool("list_emitted_capsules", json!({}));
    assert_eq!(listed["capsules"][0]["id"], "cap-from-mcp");
    drop(client);

    // A fresh server sees the persisted capsule store.
    let mut client = McpClient::spawn(dir.path(), &[]);
    let listed = client.tool("list_emitted_capsules", json!({}));
    assert_eq!(
        listed["capsules"][0]["id"], "cap-from-mcp",
        "capsule store persists: {listed}"
    );

    // Retraction is idempotent.
    let first = client.tool("retract_emitted_capsule", json!({"id": "cap-from-mcp"}));
    let second = client.tool("retract_emitted_capsule", json!({"id": "cap-from-mcp"}));
    assert_eq!(first["removed"], true);
    assert_eq!(second["removed"], false);
    assert_eq!(second["success"], true);
}
