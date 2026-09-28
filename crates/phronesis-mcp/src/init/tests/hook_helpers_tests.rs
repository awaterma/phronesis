use crate::init::json_helpers::{is_phronesis_hook_command, upsert_hook, upsert_hook_by_command};
use serde_json::{Value, json};

#[test]
fn upsert_hook_replaces_matching_matcher() {
    let mut settings = json!({
        "hooks": {
            "PreToolUse": [
                {"matcher": "Edit|Write|MultiEdit|Bash", "hooks": [{"type":"command","command":"OLD"}]},
                {"matcher": "Read", "hooks": [{"type":"command","command":"keep"}]}
            ]
        }
    });
    upsert_hook(
        &mut settings,
        "PreToolUse",
        json!({
            "matcher": "Edit|Write|MultiEdit|Bash",
            "hooks": [{"type":"command","command":"NEW"}]
        }),
    );
    let arr = settings["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(arr.len(), 2);
    // The Read entry is preserved
    assert!(
        arr.iter()
            .any(|m| m["matcher"] == "Read" && m["hooks"][0]["command"] == "keep")
    );
    // The Edit|... entry is replaced (only one with that matcher)
    let ours: Vec<&Value> = arr
        .iter()
        .filter(|m| m["matcher"] == "Edit|Write|MultiEdit|Bash")
        .collect();
    assert_eq!(ours.len(), 1);
    assert_eq!(ours[0]["hooks"][0]["command"], "NEW");
}

#[test]
fn upsert_hook_by_command_replaces_ours_and_keeps_foreign() {
    let mut settings = json!({"hooks": {"SessionStart": [
        {"matcher": "", "hooks": [{"type": "command", "command": "/usr/local/bin/phr-mcp session-context"}]},
        {"matcher": "", "hooks": [{"type": "command", "command": "my-own-tool --flag"}]}
    ]}});
    upsert_hook_by_command(
        &mut settings,
        "SessionStart",
        json!({"matcher": "", "hooks": [{"type": "command", "command": "phr-mcp claude-hook SessionStart"}]}),
    );
    let arr = settings["hooks"]["SessionStart"].as_array().unwrap();
    assert_eq!(
        arr.len(),
        2,
        "one foreign hook plus exactly one of ours: {arr:?}"
    );
    assert_eq!(arr[0]["hooks"][0]["command"], "my-own-tool --flag");
    assert_eq!(
        arr[1]["hooks"][0]["command"],
        "phr-mcp claude-hook SessionStart"
    );
}

#[test]
fn is_phronesis_hook_command_recognizes_ours_and_only_ours() {
    for ours in [
        "phr-mcp session-context",
        "phr-mcp claude-hook SessionStart",
        "/usr/local/bin/phr-mcp interaction-context",
        "/opt/tools/phr-mcp claude-hook Stop",
        "env FOO=1 phr-mcp codex-hook Interrupt",
        "phr-mcp pre-check",
        "phr-mcp post-check",
    ] {
        assert!(is_phronesis_hook_command(ours), "{ours}");
    }
    for theirs in [
        "my-own-notifier",
        "phr-mcp-notify --all",
        "/usr/bin/phr-mcp-notify --all",
        // The binary with no subcommand of ours is not a hook we registered.
        "phr-mcp --version",
        "phr",
        "",
    ] {
        assert!(!is_phronesis_hook_command(theirs), "{theirs}");
    }
}

#[test]
fn upsert_hook_by_command_creates_missing_event_array() {
    let mut settings = json!({});
    upsert_hook_by_command(
        &mut settings,
        "AfterAgent",
        json!({"matcher": "", "hooks": [{"type": "command", "command": "phr-mcp claude-hook AfterAgent"}]}),
    );
    assert_eq!(settings["hooks"]["AfterAgent"].as_array().unwrap().len(), 1);
}

#[test]
fn upsert_hook_appends_when_no_matching_matcher() {
    let mut settings = json!({"hooks": {"PreToolUse": [
        {"matcher": "Read", "hooks":[{"type":"command","command":"keep"}]}
    ]}});
    upsert_hook(
        &mut settings,
        "PreToolUse",
        json!({"matcher":"Edit|Write|MultiEdit|Bash","hooks":[{"type":"command","command":"NEW"}]}),
    );
    assert_eq!(settings["hooks"]["PreToolUse"].as_array().unwrap().len(), 2);
}
