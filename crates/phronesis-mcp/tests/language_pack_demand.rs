//! Hook and audit demand must not activate unrelated structural parsers.
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
};

fn project(rules: Value) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join(".phronesis")).unwrap();
    fs::write(
        root.path().join(".phronesis/rules.json"),
        json!({"rules":rules}).to_string(),
    )
    .unwrap();
    root
}
fn run(root: &Path, args: &[&str], payload: Option<Value>, cap: Option<&str>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_phr-mcp"));
    cmd.args(args)
        .current_dir(root)
        .env("PHRONESIS_PROJECT_ROOT", root)
        .env_remove("PHRONESIS_MAX_FILE_BYTES")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cap) = cap {
        cmd.env("PHRONESIS_MAX_FILE_BYTES", cap);
    }
    let mut child = cmd.spawn().unwrap();
    if let Some(payload) = payload {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.to_string().as_bytes())
            .unwrap();
    }
    child.wait_with_output().unwrap()
}
fn hook(root: &Path, phase: &str, source: &str) -> Output {
    run(
        root,
        &[phase],
        Some(
            json!({"tool_name":"Write","tool_input":{"file_path":root.join("main.go"),"content":source}}),
        ),
        None,
    )
}
fn policy(predicate: &str, phase: &str) -> Value {
    json!({"id":"probe", "phase":phase,"audit":true,"when":[{predicate:["?file","?path"]}],"then":{"block":"structural policy"}})
}
fn bad_sources() -> Vec<String> {
    vec![
        "package main\nfunc main( {\n".into(),
        format!(
            "package main\nfunc F0() {{}}\n{}",
            (1..1000)
                .map(|i| format!("func F{i}() {{ F0() }}\n"))
                .collect::<String>()
        ),
    ]
}
#[test]
fn absent_lexical_and_other_pack_policies_do_not_parse_go() {
    for rules in [
        json!([]),
        json!([{"id":"lexical","phase":"pre","when":[{"new_content_contains":"REJECT_TOKEN"}],"then":{"block":"lexical policy"}}]),
        json!([policy("pack_other_fact", "pre")]),
    ] {
        let root = project(rules);
        for source in bad_sources() {
            let out = hook(root.path(), "pre-check", &source);
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
    let root = project(
        json!([{"id":"lexical","phase":"pre","when":[{"new_content_contains":"REJECT_TOKEN"}],"then":{"block":"lexical policy"}}]),
    );
    assert_eq!(
        hook(root.path(), "pre-check", "package main\n// REJECT_TOKEN\n")
            .status
            .code(),
        Some(2)
    );
}
#[test]
fn go_edits_without_demand_work_in_every_feature_build() {
    let root = project(json!([]));
    let out = hook(root.path(), "pre-check", "package main\nfunc main() {}\n");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = run(root.path(), &["pack", "list", "--json"], None, None);
    let inventory: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        inventory["packs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == "go")
    );
}
#[test]
fn demanded_go_policy_still_fails_closed() {
    let root = project(json!([policy("pack_go_dot_import", "pre")]));
    for source in [
        bad_sources().remove(0),
        "package main\nimport . \"errors\"\nfunc F() { New(\"x\") }\n".into(),
    ] {
        let out = hook(root.path(), "pre-check", &source);
        assert_eq!(
            out.status.code(),
            Some(2),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
#[test]
fn post_and_codex_skip_unrelated_go_extraction() {
    let root = project(json!([
        policy("pack_other_fact", "post"),
        policy("pack_other_fact", "pre")
    ]));
    // Unique ids are required even for different phases.
    let mut rules = json!([
        policy("pack_other_fact", "post"),
        policy("pack_other_fact", "pre")
    ]);
    rules[0]["id"] = json!("post");
    fs::write(
        root.path().join(".phronesis/rules.json"),
        json!({"rules":rules}).to_string(),
    )
    .unwrap();
    let source = bad_sources().remove(0);
    fs::write(root.path().join("main.go"), &source).unwrap();
    let out = hook(root.path(), "post-check", &source);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    fs::remove_file(root.path().join("main.go")).unwrap();
    let out = run(
        root.path(),
        &["codex-hook", "PreToolUse"],
        Some(
            json!({"hook_event_name":"PreToolUse","tool_name":"apply_patch","tool_input":{"command":"*** Begin Patch\n*** Add File: main.go\n+package main\n+func main( {\n*** End Patch\n"}}),
        ),
        None,
    );
    let response: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_ne!(
        response["hookSpecificOutput"]["permissionDecision"], "deny",
        "{response}"
    );
}
#[test]
fn lexical_audit_uses_configured_cap_and_reports_excluded_go() {
    let root = project(
        json!([{"id":"lexical","phase":"audit","audit":true,"when":[{"new_content_contains":"TAIL_PROBE"}],"then":{"warn":"tail found"}}]),
    );
    fs::write(
        root.path().join("generated.go"),
        format!(
            "package main\n//{}\n// TAIL_PROBE\n",
            "x".repeat(1536 * 1024)
        ),
    )
    .unwrap();
    let out = run(root.path(), &["audit", "--json"], None, None);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        report["lexical_excluded"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p.as_str() == Some("generated.go")),
        "{report}"
    );
    let out = run(root.path(), &["audit", "--json"], None, Some("2097152"));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        report["rules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["rule_id"] == "lexical" && r["hits"].as_u64().unwrap_or(0) > 0),
        "{report}"
    );
}
#[test]
fn oversized_go_with_demanded_structural_audit_is_incomplete() {
    let root = project(json!([policy("pack_go_dot_import", "audit")]));
    fs::write(
        root.path().join("generated.go"),
        format!("package main\n//{}", "x".repeat(1536 * 1024)),
    )
    .unwrap();
    let out = run(root.path(), &["audit", "--json"], None, Some("2097152"));
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("incomplete"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn native_diagnostics_remain_active_through_hook_demand_gate() {
    let root = project(
        json!([{"id":"lua","phase":"pre","when":[{"lua_dynamic_code_load":["?file","?module","?loader"]}],"then":{"block":"dynamic load"}}]),
    );
    phronesis_mcp::graph::sync::rebuild(root.path()).unwrap();
    let out = run(
        root.path(),
        &["pre-check"],
        Some(
            json!({"tool_name":"Write","tool_input":{"file_path":root.path().join("main.lua"),"content":"load(payload)"}}),
        ),
        None,
    );
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("dynamic load"));
}
#[test]
fn codex_patch_with_go_demand_still_denies_unavailable_or_invalid_evidence() {
    let root = project(json!([policy("pack_go_dot_import", "pre")]));
    let out = run(
        root.path(),
        &["codex-hook", "PreToolUse"],
        Some(
            json!({"hook_event_name":"PreToolUse","tool_name":"apply_patch","tool_input":{"command":"*** Begin Patch\n*** Add File: main.go\n+package main\n+func main( {\n*** End Patch\n"}}),
        ),
        None,
    );
    let response: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        response["hookSpecificOutput"]["permissionDecision"], "deny",
        "{response}"
    );
}

#[test]
fn script_guards_that_read_pack_facts_demand_structural_evidence() {
    let root = project(json!([{"id":"script-pack","phase":"pre","when":[
        {"file_extension_is":"go"},
        {"__script__":"facts_contain('pack_go_dot_import', ['*', '*'])"}
    ],"then":{"block":"script requested Go facts"}}]));
    let out = hook(
        root.path(),
        "pre-check",
        "package main\nimport . \"errors\"\nfunc F() { New(\"x\") }\n",
    );
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
