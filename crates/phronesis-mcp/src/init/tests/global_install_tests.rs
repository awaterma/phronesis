use crate::init::*;
use serde_json::Value;

#[test]
fn install_globally_with_home_writes_gemini_settings() {
    let home = tempfile::tempdir().unwrap();
    let report = install_globally_with_home(home.path(), false).unwrap();

    let gemini_path = home.path().join(".gemini").join("settings.json");
    assert!(
        gemini_path.exists(),
        "~/.gemini/settings.json should be created"
    );

    let content: Value =
        serde_json::from_str(&std::fs::read_to_string(&gemini_path).unwrap()).unwrap();
    let entry = &content["mcpServers"]["phronesis"];
    assert_eq!(entry["command"], "phr-mcp");
    assert_eq!(entry["args"][0], "serve");

    assert!(
        report.steps.iter().any(|s| s.contains("gemini")),
        "report should mention gemini: {:?}",
        report.steps
    );
}

#[test]
fn install_globally_with_home_writes_claude_json() {
    let home = tempfile::tempdir().unwrap();
    let report = install_globally_with_home(home.path(), false).unwrap();

    let claude_path = home.path().join(".claude.json");
    assert!(claude_path.exists(), "~/.claude.json should be created");

    let content: Value =
        serde_json::from_str(&std::fs::read_to_string(&claude_path).unwrap()).unwrap();
    let entry = &content["mcpServers"]["phronesis"];
    assert_eq!(entry["command"], "phr-mcp");
    assert_eq!(entry["args"][0], "serve");

    assert!(
        report.steps.iter().any(|s| s.contains("claude")),
        "report should mention claude: {:?}",
        report.steps
    );
}

#[test]
fn install_globally_with_home_preserves_other_gemini_settings() {
    let home = tempfile::tempdir().unwrap();
    let gemini_dir = home.path().join(".gemini");
    std::fs::create_dir_all(&gemini_dir).unwrap();
    let gemini_path = gemini_dir.join("settings.json");

    // Pre-existing mcpServers entry that should survive
    std::fs::write(
        &gemini_path,
        r#"{"mcpServers":{"other-tool":{"command":"other","args":[]}},"theme":"dark"}"#,
    )
    .unwrap();

    install_globally_with_home(home.path(), false).unwrap();

    let content: Value =
        serde_json::from_str(&std::fs::read_to_string(&gemini_path).unwrap()).unwrap();
    // Our entry is present
    assert_eq!(content["mcpServers"]["phronesis"]["command"], "phr-mcp");
    // Pre-existing entry is preserved
    assert_eq!(content["mcpServers"]["other-tool"]["command"], "other");
    // Top-level key preserved
    assert_eq!(content["theme"], "dark");
}

#[test]
fn install_globally_with_home_idempotent() {
    let home = tempfile::tempdir().unwrap();
    install_globally_with_home(home.path(), false).unwrap();
    let report2 = install_globally_with_home(home.path(), false).unwrap();

    // Second run: both files should report no-change
    assert!(
        report2
            .steps
            .iter()
            .any(|s| s.contains("already registers") && s.contains("claude")),
        "second run should report claude already registered: {:?}",
        report2.steps
    );
    assert!(
        report2
            .steps
            .iter()
            .any(|s| s.contains("already registers") && s.contains("gemini")),
        "second run should report gemini already registered: {:?}",
        report2.steps
    );
}

#[test]
fn uninstall_globally_with_home_removes_from_gemini() {
    let home = tempfile::tempdir().unwrap();
    // First install
    install_globally_with_home(home.path(), false).unwrap();

    let gemini_path = home.path().join(".gemini").join("settings.json");
    assert!(gemini_path.exists());

    // Now uninstall
    let report = uninstall_globally_with_home(home.path(), false).unwrap();

    let content: Value =
        serde_json::from_str(&std::fs::read_to_string(&gemini_path).unwrap()).unwrap();
    assert!(
        content["mcpServers"].get("phronesis").is_none()
            || content["mcpServers"]["phronesis"].is_null(),
        "phronesis entry should be removed from gemini settings"
    );

    assert!(
        report.steps.iter().any(|s| s.contains("gemini")),
        "report should mention gemini removal: {:?}",
        report.steps
    );
}

#[test]
fn uninstall_globally_with_home_removes_from_claude() {
    let home = tempfile::tempdir().unwrap();
    install_globally_with_home(home.path(), false).unwrap();

    let claude_path = home.path().join(".claude.json");
    assert!(claude_path.exists());

    let report = uninstall_globally_with_home(home.path(), false).unwrap();

    let content: Value =
        serde_json::from_str(&std::fs::read_to_string(&claude_path).unwrap()).unwrap();
    assert!(
        content["mcpServers"].get("phronesis").is_none()
            || content["mcpServers"]["phronesis"].is_null(),
        "phronesis entry should be removed from claude settings"
    );

    assert!(
        report.steps.iter().any(|s| s.contains("claude")),
        "report should mention claude removal: {:?}",
        report.steps
    );
}

#[test]
fn uninstall_globally_with_home_idempotent_when_nothing_installed() {
    let home = tempfile::tempdir().unwrap();
    // No install — uninstall should be a no-op without error
    let report = uninstall_globally_with_home(home.path(), false).unwrap();
    assert!(
        report
            .steps
            .iter()
            .any(|s| s.contains("doesn't exist") || s.contains("nothing")),
        "should report nothing to uninstall: {:?}",
        report.steps
    );
}

#[test]
fn install_globally_with_home_dry_run_writes_nothing() {
    let home = tempfile::tempdir().unwrap();
    let report = install_globally_with_home(home.path(), true).unwrap();

    assert!(
        !home.path().join(".claude.json").exists(),
        "dry run must not write ~/.claude.json"
    );
    assert!(
        !home.path().join(".gemini").join("settings.json").exists(),
        "dry run must not write ~/.gemini/settings.json"
    );
    assert!(
        !report.steps.is_empty(),
        "dry run should still report planned steps"
    );
}
