use crate::init::global_install::{McpTarget, install_one_target};
use crate::init::json_helpers::{upsert_codex_hook, with_extension};
use crate::init::writers_hooks::{write_gemini_settings, write_mcp_json};
use crate::init::*;
use serde_json::{Value, json};

fn opts(root: &Path, dry_run: bool, force: bool) -> InitOpts {
    InitOpts {
        project_root: root.to_path_buf(),
        packs: vec![Pack::Llm],
        force,
        dry_run,
        rules_only: false,
        hooks_only: false,
    }
}

fn read_value(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("read")).expect("json")
}

fn phronesis_entry() -> Value {
    json!({"command": "phr-mcp", "args": ["serve"]})
}

// ── install_one_target ────────────────────────────────────────────

#[test]
fn install_one_target_creates_missing_file_and_parent() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("nested/dir/.claude.json");
    let target = McpTarget {
        path: path.clone(),
        label: "~/.claude.json",
    };
    let mut report = InitReport::default();
    install_one_target(&target, false, &mut report).unwrap();
    let v = read_value(&path);
    assert_eq!(v["mcpServers"]["phronesis"], phronesis_entry());
    assert!(
        !with_extension(&path, "bak").exists(),
        "no backup for new file"
    );
    assert!(report.steps.iter().any(|s| s.starts_with("+ registered")));
}

#[test]
fn install_one_target_preserves_other_keys_and_backs_up() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join(".claude.json");
    std::fs::write(
        &path,
        r#"{"theme":"dark","mcpServers":{"other":{"command":"x"}}}"#,
    )
    .unwrap();
    let target = McpTarget {
        path: path.clone(),
        label: "~/.claude.json",
    };
    let mut report = InitReport::default();
    install_one_target(&target, false, &mut report).unwrap();
    let v = read_value(&path);
    assert_eq!(v["theme"], "dark");
    assert_eq!(v["mcpServers"]["other"]["command"], "x");
    assert_eq!(v["mcpServers"]["phronesis"], phronesis_entry());
    let bak = with_extension(&path, "bak");
    assert!(bak.exists());
    assert!(std::fs::read_to_string(bak).unwrap().contains("\"dark\""));
}

#[test]
fn install_one_target_is_idempotent() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join(".claude.json");
    let target = McpTarget {
        path: path.clone(),
        label: "~/.claude.json",
    };
    let mut report = InitReport::default();
    install_one_target(&target, false, &mut report).unwrap();
    let first = std::fs::read_to_string(&path).unwrap();
    let mut report = InitReport::default();
    install_one_target(&target, false, &mut report).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), first);
    assert!(report.steps.iter().any(|s| s.contains("already registers")));
    assert!(
        !with_extension(&path, "bak").exists(),
        "no write → no backup"
    );
}

#[test]
fn install_one_target_dry_run_writes_nothing() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join(".claude.json");
    let target = McpTarget {
        path: path.clone(),
        label: "~/.claude.json",
    };
    let mut report = InitReport::default();
    install_one_target(&target, true, &mut report).unwrap();
    assert!(!path.exists());
    assert!(report.steps.iter().any(|s| s.contains("would register")));
}

#[test]
fn install_one_target_replaces_non_object_shapes() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join(".claude.json");
    // Top-level array and a non-object mcpServers both get normalized.
    std::fs::write(&path, "[1,2,3]").unwrap();
    let target = McpTarget {
        path: path.clone(),
        label: "~/.claude.json",
    };
    install_one_target(&target, false, &mut InitReport::default()).unwrap();
    assert_eq!(
        read_value(&path)["mcpServers"]["phronesis"],
        phronesis_entry()
    );

    std::fs::write(&path, r#"{"mcpServers":"oops"}"#).unwrap();
    install_one_target(&target, false, &mut InitReport::default()).unwrap();
    assert_eq!(
        read_value(&path)["mcpServers"]["phronesis"],
        phronesis_entry()
    );
}

#[test]
fn install_one_target_stale_entry_is_upgraded() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join(".claude.json");
    std::fs::write(&path, r#"{"mcpServers":{"phronesis":{"command":"old"}}}"#).unwrap();
    let target = McpTarget {
        path: path.clone(),
        label: "~/.claude.json",
    };
    install_one_target(&target, false, &mut InitReport::default()).unwrap();
    assert_eq!(
        read_value(&path)["mcpServers"]["phronesis"],
        phronesis_entry()
    );
}

#[test]
fn install_one_target_empty_file_is_treated_as_absent() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join(".claude.json");
    std::fs::write(&path, "  \n").unwrap();
    let target = McpTarget {
        path: path.clone(),
        label: "~/.claude.json",
    };
    install_one_target(&target, false, &mut InitReport::default()).unwrap();
    assert_eq!(
        read_value(&path)["mcpServers"]["phronesis"],
        phronesis_entry()
    );
}

#[test]
fn install_one_target_malformed_json_errors_without_clobbering() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join(".claude.json");
    std::fs::write(&path, "{not json").unwrap();
    let target = McpTarget {
        path: path.clone(),
        label: "~/.claude.json",
    };
    let err = install_one_target(&target, false, &mut InitReport::default()).unwrap_err();
    assert!(matches!(err, InitError::Json(_)), "{err}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{not json");
}

// ── write_mcp_json ────────────────────────────────────────────────

#[test]
fn write_mcp_json_creates_file() {
    let dir = tempfile::tempdir().unwrap();
    let o = opts(dir.path(), false, false);
    let mut report = InitReport::default();
    write_mcp_json(dir.path(), &o, &mut report).unwrap();
    let v = read_value(&dir.path().join(".mcp.json"));
    assert_eq!(v["mcpServers"]["phronesis"], phronesis_entry());
    assert!(report.steps.iter().any(|s| s == "+ wrote .mcp.json"));
}

#[test]
fn write_mcp_json_merges_existing_servers_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".mcp.json");
    std::fs::write(&path, r#"{"mcpServers":{"other":{"command":"o"}},"x":1}"#).unwrap();
    let o = opts(dir.path(), false, false);
    write_mcp_json(dir.path(), &o, &mut InitReport::default()).unwrap();
    let v = read_value(&path);
    assert_eq!(v["x"], 1);
    assert_eq!(v["mcpServers"]["other"]["command"], "o");
    assert_eq!(v["mcpServers"]["phronesis"], phronesis_entry());

    let mut report = InitReport::default();
    write_mcp_json(dir.path(), &o, &mut report).unwrap();
    assert!(report.steps.iter().any(|s| s.contains("unchanged")));
}

#[test]
fn write_mcp_json_dry_run_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let o = opts(dir.path(), true, false);
    let mut report = InitReport::default();
    write_mcp_json(dir.path(), &o, &mut report).unwrap();
    assert!(!dir.path().join(".mcp.json").exists());
    assert!(report.steps.iter().any(|s| s.contains("would write")));
}

#[test]
fn write_mcp_json_normalizes_non_object_and_rejects_malformed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".mcp.json");
    let o = opts(dir.path(), false, false);
    std::fs::write(&path, "42").unwrap();
    write_mcp_json(dir.path(), &o, &mut InitReport::default()).unwrap();
    assert_eq!(
        read_value(&path)["mcpServers"]["phronesis"],
        phronesis_entry()
    );

    std::fs::write(&path, r#"{"mcpServers": []}"#).unwrap();
    write_mcp_json(dir.path(), &o, &mut InitReport::default()).unwrap();
    assert_eq!(
        read_value(&path)["mcpServers"]["phronesis"],
        phronesis_entry()
    );

    std::fs::write(&path, "{").unwrap();
    let err = write_mcp_json(dir.path(), &o, &mut InitReport::default()).unwrap_err();
    assert!(matches!(err, InitError::Json(_)));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{");
}

#[test]
fn write_mcp_json_force_backs_up_changed_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".mcp.json");
    std::fs::write(&path, r#"{"mcpServers":{}}"#).unwrap();
    let o = opts(dir.path(), false, true);
    write_mcp_json(dir.path(), &o, &mut InitReport::default()).unwrap();
    assert!(with_extension(&path, "bak").exists());
}

// ── write_gemini_settings ─────────────────────────────────────────

#[test]
fn write_gemini_settings_creates_full_wiring() {
    let dir = tempfile::tempdir().unwrap();
    let o = opts(dir.path(), false, false);
    write_gemini_settings(dir.path(), &o, &mut InitReport::default()).unwrap();
    let v = read_value(&dir.path().join(".gemini/settings.json"));
    assert_eq!(v["mcpServers"]["phronesis"], phronesis_entry());
    for (event, cmd) in [
        ("BeforeTool", "phr-mcp pre-check"),
        ("AfterTool", "phr-mcp post-check"),
        ("SessionStart", "phr-mcp claude-hook SessionStart"),
        ("SessionEnd", "phr-mcp claude-hook SessionEnd"),
        ("BeforeAgent", "phr-mcp claude-hook BeforeAgent"),
        ("AfterAgent", "phr-mcp claude-hook AfterAgent"),
    ] {
        let arr = v["hooks"][event]
            .as_array()
            .unwrap_or_else(|| panic!("{event}"));
        assert_eq!(arr.len(), 1, "{event}");
        assert_eq!(arr[0]["hooks"][0]["command"], cmd, "{event}");
    }
    assert_eq!(
        v["hooks"]["BeforeTool"][0]["matcher"],
        "^(replace|write_file|run_shell_command|invoke_agent)$"
    );
    assert_eq!(v["hooks"]["SessionStart"][0]["matcher"], "");
}

#[test]
fn write_gemini_settings_preserves_user_hooks_and_drops_legacy_event() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".gemini/settings.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
            &path,
            json!({
                "theme": "x",
                "mcpServers": {"other": {"command": "o"}},
                "hooks": {
                    "BeforeTool": [
                        {"matcher": "custom", "hooks": [{"type": "command", "command": "mine"}]},
                        {"matcher": "replace|write_file|run_shell_command", "hooks": [{"type": "command", "command": "stale"}]}
                    ],
                    "BeforeModelRequest": [{"matcher": "", "hooks": []}]
                }
            })
            .to_string(),
        )
        .unwrap();
    let o = opts(dir.path(), false, false);
    write_gemini_settings(dir.path(), &o, &mut InitReport::default()).unwrap();
    let v = read_value(&path);
    assert_eq!(v["theme"], "x");
    assert_eq!(v["mcpServers"]["other"]["command"], "o");
    assert!(
        v["hooks"].get("BeforeModelRequest").is_none(),
        "legacy hook removed"
    );
    let before = v["hooks"]["BeforeTool"].as_array().unwrap();
    // command-keyed replacement: "stale" is foreign (not `phr-mcp `), so
    // it survives alongside "mine"; the new `phr-mcp pre-check` entry is
    // appended.
    assert_eq!(before.len(), 3);
    assert!(before.iter().any(|e| e["hooks"][0]["command"] == "mine"));
    assert!(
        before
            .iter()
            .any(|e| e["hooks"][0]["command"] == "phr-mcp pre-check")
    );
    assert!(before.iter().any(|e| e["hooks"][0]["command"] == "stale"));
    assert_eq!(
        before
            .iter()
            .find(|e| e["hooks"][0]["command"] == "phr-mcp pre-check")
            .unwrap()["matcher"],
        "^(replace|write_file|run_shell_command|invoke_agent)$"
    );
}

#[test]
fn write_gemini_settings_is_idempotent_and_dry_run_safe() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".gemini/settings.json");
    let dry = opts(dir.path(), true, false);
    let mut report = InitReport::default();
    write_gemini_settings(dir.path(), &dry, &mut report).unwrap();
    assert!(!path.exists());
    assert!(report.steps.iter().any(|s| s.contains("would write")));

    let o = opts(dir.path(), false, false);
    write_gemini_settings(dir.path(), &o, &mut InitReport::default()).unwrap();
    let first = std::fs::read_to_string(&path).unwrap();
    let mut report = InitReport::default();
    write_gemini_settings(dir.path(), &o, &mut report).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), first);
    assert!(report.steps.iter().any(|s| s.contains("unchanged")));
}

#[test]
fn write_gemini_settings_normalizes_bad_shapes_and_rejects_malformed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".gemini/settings.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let o = opts(dir.path(), false, false);

    std::fs::write(
        &path,
        r#"{"mcpServers": 5, "hooks": {"BeforeTool": "nope"}}"#,
    )
    .unwrap();
    write_gemini_settings(dir.path(), &o, &mut InitReport::default()).unwrap();
    let v = read_value(&path);
    assert_eq!(v["mcpServers"]["phronesis"], phronesis_entry());
    assert_eq!(v["hooks"]["BeforeTool"].as_array().unwrap().len(), 1);

    std::fs::write(&path, "null").unwrap();
    write_gemini_settings(dir.path(), &o, &mut InitReport::default()).unwrap();
    assert_eq!(
        read_value(&path)["mcpServers"]["phronesis"],
        phronesis_entry()
    );

    std::fs::write(&path, "{\"a\":").unwrap();
    let err = write_gemini_settings(dir.path(), &o, &mut InitReport::default()).unwrap_err();
    assert!(matches!(err, InitError::Json(_)));
}

// ── upsert_codex_hook ─────────────────────────────────────────────

fn codex_entry(event: &str, matcher: &str) -> Value {
    json!({
        "matcher": matcher,
        "hooks": [{"type": "command", "command": format!("phr-mcp codex-hook {event}")}]
    })
}

#[test]
fn upsert_codex_hook_creates_hooks_and_event_arrays() {
    let mut settings = json!({});
    upsert_codex_hook(
        &mut settings,
        "PreToolUse",
        codex_entry("PreToolUse", "Bash"),
    );
    let arr = settings["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["matcher"], "Bash");
}

#[test]
fn upsert_codex_hook_replaces_our_entry_regardless_of_matcher() {
    let mut settings = json!({"hooks": {"PreToolUse": [
        codex_entry("PreToolUse", "old-matcher"),
        {"matcher": "old-matcher", "hooks": [{"type": "command", "command": "user-cmd"}]}
    ]}});
    upsert_codex_hook(
        &mut settings,
        "PreToolUse",
        codex_entry("PreToolUse", "new"),
    );
    let arr = settings["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(arr.len(), 2, "{arr:?}");
    assert!(
        arr.iter().any(|e| e["hooks"][0]["command"] == "user-cmd"),
        "user hook kept"
    );
    let ours: Vec<_> = arr
        .iter()
        .filter(|e| e["hooks"][0]["command"] == "phr-mcp codex-hook PreToolUse")
        .collect();
    assert_eq!(ours.len(), 1);
    assert_eq!(ours[0]["matcher"], "new");
}

#[test]
fn upsert_codex_hook_is_idempotent_and_scoped_to_event() {
    let mut settings = json!({});
    upsert_codex_hook(&mut settings, "PreToolUse", codex_entry("PreToolUse", "m"));
    upsert_codex_hook(&mut settings, "PreToolUse", codex_entry("PreToolUse", "m"));
    upsert_codex_hook(
        &mut settings,
        "PostToolUse",
        codex_entry("PostToolUse", "m"),
    );
    assert_eq!(settings["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
    assert_eq!(
        settings["hooks"]["PostToolUse"].as_array().unwrap().len(),
        1
    );
}

#[test]
fn upsert_codex_hook_repairs_non_array_event_and_tolerates_odd_entries() {
    let mut settings = json!({"hooks": {"PreToolUse": {"bad": true}}});
    upsert_codex_hook(&mut settings, "PreToolUse", codex_entry("PreToolUse", "m"));
    assert_eq!(settings["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);

    // Entries lacking a hooks array are not ours and must survive.
    let mut settings = json!({"hooks": {"PreToolUse": [5, {"matcher": "x"}]}});
    upsert_codex_hook(&mut settings, "PreToolUse", codex_entry("PreToolUse", "m"));
    assert_eq!(settings["hooks"]["PreToolUse"].as_array().unwrap().len(), 3);
}

#[test]
fn upsert_codex_hook_no_ops_when_settings_or_hooks_not_objects() {
    let mut settings = json!([]);
    upsert_codex_hook(&mut settings, "PreToolUse", codex_entry("PreToolUse", "m"));
    assert_eq!(settings, json!([]));

    let mut settings = json!({"hooks": "string"});
    upsert_codex_hook(&mut settings, "PreToolUse", codex_entry("PreToolUse", "m"));
    assert_eq!(settings, json!({"hooks": "string"}));
}
