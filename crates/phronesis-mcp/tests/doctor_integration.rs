//! End-to-end tests for `phr-mcp doctor` / `doctor --probe` that exercise
//! the real compiled binary via `Command`, not just the in-process
//! functions. Each test owns its own tempdir as the project root.

use std::path::{Path, PathBuf};
use std::process::Command;

fn unique_tmp(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "phr-mcp-doctor-it-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run_doctor(root: &Path, extra_args: &[&str]) -> (i32, String, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_phr-mcp"));
    cmd.arg("doctor").arg("--path").arg(root);
    for a in extra_args {
        cmd.arg(a);
    }
    let out = cmd.output().expect("spawn phr-mcp doctor");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn write_rules(root: &Path, rules_json: &serde_json::Value) {
    std::fs::create_dir_all(root.join(".phronesis")).unwrap();
    std::fs::write(
        root.join(".phronesis").join("rules.json"),
        serde_json::to_string_pretty(rules_json).unwrap(),
    )
    .unwrap();
}

fn write_claude_settings(root: &Path, settings: &serde_json::Value) {
    std::fs::create_dir_all(root.join(".claude")).unwrap();
    std::fs::write(
        root.join(".claude").join("settings.local.json"),
        serde_json::to_string_pretty(settings).unwrap(),
    )
    .unwrap();
}

#[test]
fn ungoverned_dir_exits_2_and_json_names_fail_findings() {
    let dir = unique_tmp("ungoverned");
    let (exit, stdout, _stderr) = run_doctor(&dir, &["--json"]);
    assert_eq!(exit, 2, "ungoverned dir must exit 2");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    let findings = v["findings"].as_array().expect("findings array");
    let project_governed = findings
        .iter()
        .find(|f| f["id"] == "project_governed")
        .expect("project_governed finding present");
    assert_eq!(project_governed["status"], "fail");
    let host_hooks = findings
        .iter()
        .find(|f| f["id"] == "host_hooks")
        .expect("host_hooks finding present");
    assert_eq!(host_hooks["status"], "fail");
    assert!(v["probe"].is_null());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn malformed_rules_exits_2_with_a_fail_finding() {
    let dir = unique_tmp("malformed");
    std::fs::create_dir_all(dir.join(".phronesis")).unwrap();
    std::fs::write(dir.join(".phronesis").join("rules.json"), b"{ not json").unwrap();
    let (exit, stdout, _stderr) = run_doctor(&dir, &["--json"]);
    assert_eq!(exit, 2);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    let findings = v["findings"].as_array().unwrap();
    assert!(
        findings.iter().any(|f| f["status"] == "fail"),
        "a malformed rules file must produce at least one fail finding: {findings:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn host_trust_and_log_evidence_are_always_not_observed_even_with_a_forged_stale_log() {
    let dir = unique_tmp("forged-log");
    write_rules(
        &dir,
        &serde_json::json!({"rules": [{
            "id": "demo", "phase": "pre", "priority": 1,
            "when": [{"new_content_contains": "x"}],
            "then": {"warn": "demo"}
        }]}),
    );
    write_claude_settings(
        &dir,
        &serde_json::json!({
            "hooks": {
                "PreToolUse": [{"matcher": "Edit|Write|MultiEdit|Bash", "hooks": [{"type":"command","command":"phr-mcp pre-check"}]}],
                "PostToolUse": [{"matcher": "Edit|Write|MultiEdit|Bash", "hooks": [{"type":"command","command":"phr-mcp post-check"}]}]
            }
        }),
    );
    // Forge a stale/confident-looking log record by hand — this must not be
    // allowed to flip any verdict toward confidence.
    std::fs::write(
        dir.join(".phronesis").join("log.jsonl"),
        "{\"ts\":1,\"kind\":\"hook\",\"event\":\"pre_check\",\"exit\":0,\"consequences\":[],\"host_observation\":\"trusted\",\"verified\":true}\n",
    )
    .unwrap();

    let (_exit, stdout, _stderr) = run_doctor(&dir, &["--json"]);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    let findings = v["findings"].as_array().unwrap();
    let host_trust = findings.iter().find(|f| f["id"] == "host_trust").unwrap();
    assert_eq!(host_trust["status"], "not_observed");
    let log_evidence = findings.iter().find(|f| f["id"] == "log_evidence").unwrap();
    assert_eq!(log_evidence["status"], "not_observed");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn healthy_project_has_no_fail_finding_and_probe_matches_a_real_binary() {
    let dir = unique_tmp("healthy");
    write_rules(
        &dir,
        &serde_json::json!({"rules": [{
            "id": "demo", "phase": "pre", "priority": 1,
            "when": [{"new_content_contains": "x"}],
            "then": {"warn": "demo"}
        }]}),
    );
    write_claude_settings(
        &dir,
        &serde_json::json!({
            "hooks": {
                "PreToolUse": [{"matcher": "Edit|Write|MultiEdit|Bash", "hooks": [{"type":"command","command":"phr-mcp pre-check"}]}],
                "PostToolUse": [{"matcher": "Edit|Write|MultiEdit|Bash", "hooks": [{"type":"command","command":"phr-mcp post-check"}]}]
            }
        }),
    );

    let (exit, stdout, _stderr) = run_doctor(&dir, &["--probe", "--json"]);
    // binary_on_path may legitimately warn in this environment (a
    // different `phr-mcp` on PATH), so exit can be 0 or 1 — never 2.
    assert_ne!(exit, 2, "healthy project must not have any fail finding");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    let findings = v["findings"].as_array().unwrap();
    assert!(
        !findings.iter().any(|f| f["status"] == "fail"),
        "no finding should be fail for a fully-wired project: {findings:?}"
    );

    let probe = &v["probe"];
    assert!(!probe.is_null());
    assert_eq!(probe["rule_id"], "doctor-probe-no-unwrap");
    assert_eq!(probe["ok"], true);
    let cases = probe["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 2);
    let blocked = cases
        .iter()
        .find(|c| c["expect_block"] == true)
        .expect("blocked case present");
    assert_eq!(blocked["exit_code"], 2);
    assert_eq!(blocked["decision"], "block");
    assert_eq!(blocked["rule_id"], "doctor-probe-no-unwrap");
    assert_eq!(blocked["matched_expectation"], true);
    let allowed = cases
        .iter()
        .find(|c| c["expect_block"] == false)
        .expect("allowed case present");
    assert_eq!(allowed["exit_code"], 0);
    assert_eq!(allowed["decision"], "allow");
    assert_eq!(allowed["matched_expectation"], true);

    // The probe must never touch the real project's own log/rules.
    let log = dir.join(".phronesis").join("log.jsonl");
    assert!(
        !log.exists() || std::fs::read_to_string(&log).unwrap().is_empty(),
        "plain doctor (even with --probe) must not write to the real project's log"
    );
    let rules_text = std::fs::read_to_string(dir.join(".phronesis").join("rules.json")).unwrap();
    assert!(
        rules_text.contains("\"demo\""),
        "the real project's rules.json must be untouched by --probe"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn unfamiliar_lookalike_command_is_not_reported_as_wired() {
    let dir = unique_tmp("lookalike");
    write_rules(&dir, &serde_json::json!({"rules": []}));
    write_claude_settings(
        &dir,
        &serde_json::json!({
            "hooks": {
                "PreToolUse": [{"matcher": "Edit|Write|MultiEdit|Bash", "hooks": [{"type":"command","command":"echo phr-mcp pre-check"}]}],
                "PostToolUse": [{"matcher": "Edit|Write|MultiEdit|Bash", "hooks": [{"type":"command","command":"phr-mcp post-check"}]}]
            }
        }),
    );
    let (_exit, stdout, _stderr) = run_doctor(&dir, &["--json"]);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    let findings = v["findings"].as_array().unwrap();
    let host_hooks = findings.iter().find(|f| f["id"] == "host_hooks").unwrap();
    assert_ne!(
        host_hooks["status"], "ok",
        "`echo phr-mcp pre-check` must never be reported as confirmed wiring"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn stale_or_invalid_hook_config_never_reports_confirmed_wiring() {
    let temp = tempfile::tempdir().expect("fixture");
    std::fs::create_dir_all(temp.path().join(".phronesis")).expect("rules dir");
    std::fs::create_dir_all(temp.path().join(".claude")).expect("host dir");
    std::fs::write(temp.path().join(".phronesis/rules.json"), r#"{"rules":[]}"#).expect("rules");
    for (binary, matcher, hook_type, asynchronous) in [
        (
            "/does-not-exist/phr-mcp",
            "Edit|Write",
            Some("command"),
            serde_json::Value::Bool(false),
        ),
        (
            "phr-mcp",
            "Edit|Write|[",
            Some("command"),
            serde_json::Value::Bool(false),
        ),
        (
            "phr-mcp",
            "Edit|Write",
            None,
            serde_json::Value::Bool(false),
        ),
        (
            "phr-mcp",
            "Edit|Write",
            Some("command"),
            serde_json::json!("false"),
        ),
    ] {
        let mut hooks = serde_json::Map::new();
        for (event, command) in [("PreToolUse", "pre-check"), ("PostToolUse", "post-check")] {
            let mut hook = serde_json::json!({"command": format!("{binary} {command}"), "async": asynchronous});
            if let Some(kind) = hook_type {
                hook["type"] = serde_json::json!(kind);
            }
            hooks.insert(
                event.into(),
                serde_json::json!([{"matcher": matcher, "hooks": [hook]}]),
            );
        }
        std::fs::write(
            temp.path().join(".claude/settings.local.json"),
            serde_json::to_vec(&serde_json::json!({"hooks": hooks})).expect("settings JSON"),
        )
        .expect("settings");
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
            .args(["doctor", "--json", "--path"])
            .arg(temp.path())
            .output()
            .expect("doctor");
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("report");
        let wiring = report["findings"]
            .as_array()
            .expect("findings")
            .iter()
            .find(|f| f["id"] == "host_hooks")
            .expect("wiring finding");
        assert_ne!(
            wiring["status"], "ok",
            "{binary} {matcher} {hook_type:?}: {report}"
        );
        assert!(!output.status.success(), "invalid wiring needs attention");
    }
}

#[test]
fn shared_settings_are_unobserved_and_caller_log_override_does_not_break_probe() {
    let temp = tempfile::tempdir().expect("fixture");
    std::fs::create_dir_all(temp.path().join(".phronesis")).expect("rules dir");
    std::fs::create_dir_all(temp.path().join(".claude")).expect("host dir");
    std::fs::write(temp.path().join(".phronesis/rules.json"), r#"{"rules":[]}"#).expect("rules");
    std::fs::write(temp.path().join(".claude/settings.json"), r#"{"hooks":{}}"#)
        .expect("shared settings");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(["doctor", "--json", "--probe", "--path"])
        .arg(temp.path())
        .env("PHRONESIS_NO_ACTION_LOG", "1")
        .output()
        .expect("doctor");
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("report");
    let wiring = report["findings"]
        .as_array()
        .expect("findings")
        .iter()
        .find(|f| f["id"] == "host_hooks")
        .expect("wiring");
    assert_eq!(wiring["status"], "not_observed");
    assert_eq!(report["probe"]["ok"], true, "{report}");
    assert!(!temp.path().join(".phronesis/log.jsonl").exists());
}
