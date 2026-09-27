//! Claude/Codex parity for configuration errors at hook time.
//!
//! `.phronesis/wiki/decisions/2026-06-23-undefined-selector-rejection.md`
//! makes the pre-hook fail closed on configuration errors: a rule that can
//! never evaluate correctly must block, not silently allow. `pre-check`
//! (Claude Code / Gemini) has done so since that decision; these tests drive
//! the built binary through both `pre-check` and `codex-hook PreToolUse`
//! against the same project so the Codex adapter cannot drift back to
//! failing open. Post-phase counterparts check that both hosts surface the
//! same error as an advisory (the tool already ran).

use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

fn spawn(root: &Path, args: &[&str], payload: &Value) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(args)
        .env("PHRONESIS_PROJECT_ROOT", root)
        .env("PHRONESIS_NO_ACTION_LOG", "1")
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

fn codex_body(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "codex-hook stdout must be JSON ({e}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn setup(rules: &Value, journey: Option<&str>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp project");
    let phr = dir.path().join(".phronesis");
    std::fs::create_dir_all(phr.join("journey")).expect("config dir");
    std::fs::write(
        phr.join("rules.json"),
        serde_json::to_vec_pretty(rules).expect("rules JSON"),
    )
    .expect("write rules");
    if let Some(j) = journey {
        std::fs::write(phr.join("journey.json"), j).expect("write journey");
    }
    // Pin the session id so the journey window never depends on the clock.
    std::fs::write(phr.join("journey/session"), "s-parity").expect("write session");
    dir
}

/// A rule whose `auth` selector is not defined by any journey tagger.
fn selector_rule(phase: &str) -> Value {
    json!({"rules": [{
        "id": "bogus-auth-rule",
        "phase": phase,
        "priority": 1,
        "when": [{"__script__": "facts_count('journey_occurrence', ['auth','s']) >= 3"}],
        "then": {"warn": "should never load"}
    }]})
}

/// A well-formed journey config that defines `build` but not `auth`.
const JOURNEY_WITHOUT_AUTH: &str = r#"{
    "version":1,
    "taggers":[{"tag":"build","when":[{"bash_command_matches":"cargo build"}]}],
    "modules":[]
}"#;

fn claude_bash(command: &str) -> Value {
    json!({"tool_name": "Bash", "tool_input": {"command": command}})
}

fn codex_bash(event: &str, command: &str) -> Value {
    json!({
        "hook_event_name": event,
        "tool_name": "Bash",
        "session_id": "codex-parity",
        "turn_id": "codex-parity-turn",
        "tool_use_id": "codex-parity-uid",
        "tool_input": {"command": command},
    })
}

fn codex_patch(path: &str) -> Value {
    json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "apply_patch",
        "session_id": "codex-parity",
        "turn_id": "codex-parity-turn",
        "tool_use_id": "codex-parity-uid",
        "tool_input": {"command": format!(
            "*** Begin Patch\n*** Add File: {path}\n+pub fn a() {{}}\n*** End Patch\n"
        )},
    })
}

/// `pre-check` blocks (exit 2) and names the rule and selector.
fn assert_claude_blocks(root: &Path) {
    let out = spawn(root, &["pre-check"], &claude_bash("ls"));
    let err = stderr(&out);
    assert_eq!(
        out.status.code(),
        Some(2),
        "Claude must block; stderr: {err}"
    );
    assert!(err.contains("BLOCKED"), "{err}");
    assert!(
        err.contains("bogus-auth-rule") && err.contains("auth"),
        "{err}"
    );
}

/// `codex-hook PreToolUse` denies through the structured-JSON contract
/// (process exit stays 0) with a reason naming the rule and selector.
fn assert_codex_denies(root: &Path, payload: &Value) {
    let out = spawn(root, &["codex-hook", "PreToolUse"], payload);
    let body = codex_body(&out);
    assert_eq!(
        out.status.code(),
        Some(0),
        "Codex reads the JSON decision, not the exit code: {body}"
    );
    let hso = &body["hookSpecificOutput"];
    assert_eq!(
        hso["permissionDecision"],
        "deny",
        "Codex must deny like Claude blocks; body: {body}; stderr: {}",
        stderr(&out)
    );
    let reason = hso["permissionDecisionReason"].as_str().unwrap_or_default();
    assert!(
        reason.contains("bogus-auth-rule") && reason.contains("auth"),
        "deny reason must name the rule and the selector: {reason}"
    );
}

#[test]
fn undefined_selector_blocks_on_both_hosts() {
    let dir = setup(&selector_rule("pre"), Some(JOURNEY_WITHOUT_AUTH));
    assert_claude_blocks(dir.path());
    assert_codex_denies(dir.path(), &codex_bash("PreToolUse", "ls"));
    assert_codex_denies(dir.path(), &codex_patch("src/a.rs"));
}

/// A malformed `journey.json` falls back to the default (tagger-less)
/// config, so a rule naming a journey selector is an undefined-selector
/// configuration error on both hosts — Codex used to skip journey
/// derivation entirely and allow.
#[test]
fn malformed_journey_json_with_selector_rule_blocks_on_both_hosts() {
    let dir = setup(&selector_rule("pre"), Some("{not json"));
    assert_claude_blocks(dir.path());
    assert_codex_denies(dir.path(), &codex_bash("PreToolUse", "ls"));
    assert_codex_denies(dir.path(), &codex_patch("src/a.rs"));
}

/// The fail-open half of the same policy: a malformed `journey.json` that no
/// rule depends on must not block either host.
#[test]
fn malformed_journey_json_without_selector_rules_allows_on_both_hosts() {
    let rules = json!({"rules": [{
        "id": "never-fires", "phase": "pre", "priority": 1,
        "when": [{"new_content_contains": "zzz-never-present"}],
        "then": {"block": "never"}
    }]});
    let dir = setup(&rules, Some("{not json"));
    let out = spawn(dir.path(), &["pre-check"], &claude_bash("ls"));
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let out = spawn(
        dir.path(),
        &["codex-hook", "PreToolUse"],
        &codex_bash("PreToolUse", "ls"),
    );
    assert_eq!(codex_body(&out), json!({}), "{}", stderr(&out));
}

/// Post phase: the tool already ran, so both hosts surface the configuration
/// error as an advisory — Claude exits 1 with a WARNING, Codex returns a
/// `systemMessage` without stopping the session.
#[test]
fn undefined_selector_warns_after_the_fact_on_both_hosts() {
    let dir = setup(&selector_rule("post"), Some(JOURNEY_WITHOUT_AUTH));
    let mut claude = claude_bash("ls");
    claude["tool_response"] = json!({});
    let out = spawn(dir.path(), &["post-check"], &claude);
    let err = stderr(&out);
    assert_eq!(
        out.status.code(),
        Some(1),
        "Claude must warn; stderr: {err}"
    );
    assert!(
        err.contains("WARNING") && err.contains("bogus-auth-rule"),
        "{err}"
    );

    let out = spawn(
        dir.path(),
        &["codex-hook", "PostToolUse"],
        &codex_bash("PostToolUse", "ls"),
    );
    let body = codex_body(&out);
    assert_eq!(out.status.code(), Some(0), "{body}");
    let message = body["systemMessage"].as_str().unwrap_or_default();
    assert!(
        message.contains("bogus-auth-rule") && message.contains("auth"),
        "Codex must surface the configuration error as an advisory: {body}"
    );
    assert!(body.get("continue").is_none(), "advisory only: {body}");
}

/// An `apply_patch` `Update File` block with no `+` hunk lines forces
/// `evaluate_patch_file` down its disk-read fallback (the patch adds nothing
/// itself). When that file exists on disk but cannot be read (permission
/// denied), the call must deny rather than silently evaluate content rules
/// against empty text — a real-binary regression test for the fix to
/// `codex_hook::evaluate_patch_file`, which used to swallow any read error
/// via `unwrap_or_default()`.
#[cfg(unix)]
#[test]
fn apply_patch_denies_when_an_existing_target_file_is_unreadable() {
    use std::os::unix::fs::PermissionsExt;

    let rules = json!({"rules": [{
        "id": "never-fires", "phase": "pre", "priority": 1,
        "when": [{"new_content_contains": "zzz-never-present"}],
        "then": {"block": "never"}
    }]});
    let dir = setup(&rules, None);
    let rel_path = "src/unreadable.rs";
    let abs_path = dir.path().join(rel_path);
    std::fs::create_dir_all(abs_path.parent().unwrap()).expect("mkdir");
    std::fs::write(&abs_path, "fn existing() {}\n").expect("seed file");
    std::fs::set_permissions(&abs_path, std::fs::Permissions::from_mode(0o000)).expect("chmod 000");
    if std::fs::read(&abs_path).is_ok() {
        // Running as root: the mode bits do not apply, so there is nothing
        // to test here.
        return;
    }

    let payload = json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "apply_patch",
        "session_id": "codex-parity",
        "turn_id": "codex-parity-turn",
        "tool_use_id": "codex-parity-uid",
        "tool_input": {"command": format!(
            "*** Begin Patch\n*** Update File: {rel_path}\n@@ -1,1 +1,1 @@\n*** End Patch\n"
        )},
    });
    let out = spawn(dir.path(), &["codex-hook", "PreToolUse"], &payload);

    // Restore permissions so the tempdir can be cleaned up regardless of the
    // assertion outcome below.
    let _ = std::fs::set_permissions(&abs_path, std::fs::Permissions::from_mode(0o644));

    let body = codex_body(&out);
    assert_eq!(
        out.status.code(),
        Some(0),
        "Codex reads the JSON decision, not the exit code: {body}"
    );
    let hso = &body["hookSpecificOutput"];
    assert_eq!(
        hso["permissionDecision"],
        "deny",
        "an unreadable existing patch target must fail closed, not be treated as empty \
         content; body: {body}; stderr: {}",
        stderr(&out)
    );
    let reason = hso["permissionDecisionReason"].as_str().unwrap_or_default();
    assert!(
        reason.contains(rel_path),
        "deny reason should name the unreadable file: {reason}"
    );
}
