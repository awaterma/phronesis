use super::*;

#[test]
fn find_on_path_locates_name_in_one_of_several_dirs() {
    let tmp = std::env::temp_dir().join(format!("phr-doctor-test-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let bin = tmp.join("phr-mcp-test-bin");
    std::fs::write(&bin, b"x").unwrap();
    let path_var = format!("/nonexistent-dir-xyz:{}", tmp.display());
    let found = find_on_path("phr-mcp-test-bin", &path_var);
    assert_eq!(found.as_deref(), Some(bin.as_path()));
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn find_on_path_empty_path_is_none() {
    assert_eq!(find_on_path("phr-mcp", ""), None);
}

#[test]
fn find_on_path_shadowed_by_earlier_nonexistent_dir_still_finds_later_dir() {
    let tmp = std::env::temp_dir().join(format!("phr-doctor-test2-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let bin = tmp.join("shadowed-bin");
    std::fs::write(&bin, b"x").unwrap();
    let path_var = format!("/does/not/exist:{}:/also/does/not/exist", tmp.display());
    assert_eq!(
        find_on_path("shadowed-bin", &path_var).as_deref(),
        Some(bin.as_path())
    );
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn check_governed_reports_fail_for_ungoverned_dir() {
    let tmp = std::env::temp_dir().join(format!("phr-doctor-ungoverned-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let finding = check_governed(&tmp);
    assert_eq!(finding.status, Status::Fail);
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn check_rules_load_reports_fail_for_malformed_json() {
    let tmp = std::env::temp_dir().join(format!("phr-doctor-malformed-{}", std::process::id()));
    std::fs::create_dir_all(tmp.join(".phronesis")).unwrap();
    std::fs::write(tmp.join(".phronesis").join("rules.json"), b"{ not json").unwrap();
    let finding = check_rules_load(&tmp);
    assert_eq!(finding.status, Status::Fail);
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn check_rules_load_not_observed_when_absent() {
    let tmp = std::env::temp_dir().join(format!("phr-doctor-absent-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let finding = check_rules_load(&tmp);
    assert_eq!(finding.status, Status::NotObserved);
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn check_host_hooks_unsupported_host_is_not_observed() {
    let tmp = std::env::temp_dir();
    let finding = check_host_hooks(&tmp, "some-unsupported-host");
    assert_eq!(finding.status, Status::NotObserved);
}

#[test]
fn check_claude_settings_fail_when_missing() {
    let tmp =
        std::env::temp_dir().join(format!("phr-doctor-claude-missing-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let finding = check_claude_settings(&tmp);
    assert_eq!(finding.status, Status::Fail);
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn check_claude_settings_ok_when_both_hooks_wired() {
    let tmp = std::env::temp_dir().join(format!("phr-doctor-claude-ok-{}", std::process::id()));
    std::fs::create_dir_all(tmp.join(".claude")).unwrap();
    let settings = serde_json::json!({
        "hooks": {
            "PreToolUse": [{"matcher": "Edit|Write|MultiEdit|Bash", "hooks": [{"type":"command","command":"phr-mcp pre-check"}]}],
            "PostToolUse": [{"matcher": "Edit|Write|MultiEdit|Bash", "hooks": [{"type":"command","command":"phr-mcp post-check"}]}]
        }
    });
    std::fs::write(
        tmp.join(".claude").join("settings.local.json"),
        serde_json::to_string_pretty(&settings).unwrap(),
    )
    .unwrap();
    let finding = check_claude_settings(&tmp);
    assert_eq!(finding.status, Status::Ok);
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn command_invokes_rejects_substring_match_on_an_unrelated_command() {
    // `echo pre-check` contains the substring "pre-check" but is not an
    // invocation of `phr-mcp pre-check` — must not be treated as wired.
    assert!(!command_invokes("echo pre-check", "pre-check"));
}

#[test]
fn command_invokes_leaves_absolute_binary_paths_unverified() {
    assert!(!command_invokes(
        "/usr/local/bin/phr-mcp pre-check",
        "pre-check"
    ));
}

#[test]
fn command_invokes_rejects_bare_subcommand_with_no_binary() {
    assert!(!command_invokes("pre-check", "pre-check"));
}

#[test]
fn check_claude_settings_warns_unverified_for_a_lookalike_command() {
    let tmp = std::env::temp_dir().join(format!(
        "phr-doctor-claude-lookalike-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(tmp.join(".claude")).unwrap();
    let settings = serde_json::json!({
        "hooks": {
            "PreToolUse": [{"matcher": "Edit|Write|MultiEdit|Bash", "hooks": [{"type":"command","command":"echo pre-check"}]}],
            "PostToolUse": [{"matcher": "Edit|Write|MultiEdit|Bash", "hooks": [{"type":"command","command":"phr-mcp post-check"}]}]
        }
    });
    std::fs::write(
        tmp.join(".claude").join("settings.local.json"),
        serde_json::to_string_pretty(&settings).unwrap(),
    )
    .unwrap();
    let finding = check_claude_settings(&tmp);
    // Must never be Ok on an unverified lookalike command.
    assert_eq!(finding.status, Status::Warn);
    assert!(finding.message.contains("unverified"));
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn command_invokes_rejects_echo_of_the_real_two_token_command() {
    assert!(!command_invokes("echo phr-mcp pre-check", "pre-check"));
}

#[test]
fn command_invokes_rejects_shell_conditional_chain() {
    assert!(!command_invokes("false && phr-mcp pre-check", "pre-check"));
}

#[test]
fn command_invokes_rejects_trailing_shell_command() {
    assert!(!command_invokes(
        "phr-mcp pre-check; rm -rf /tmp/x",
        "pre-check"
    ));
}

#[test]
fn command_invokes_rejects_custom_wrapper_preceding_the_binary() {
    assert!(!command_invokes(
        "env FOO=bar phr-mcp pre-check",
        "pre-check"
    ));
}

#[test]
fn matcher_covers_edit_and_write_rejects_lookalike_tokens() {
    // "NeverEdit"/"NeverWrite" contain the substrings "Edit"/"Write" but
    // are not the exact alternatives — must not count as coverage.
    assert!(!matcher_covers_edit_and_write("NeverEdit|NeverWrite"));
    assert!(matcher_covers_edit_and_write("Edit|Write|MultiEdit|Bash"));
}

#[test]
fn check_claude_settings_fail_when_hooks_disabled_even_with_valid_commands() {
    let tmp =
        std::env::temp_dir().join(format!("phr-doctor-claude-disabled-{}", std::process::id()));
    std::fs::create_dir_all(tmp.join(".claude")).unwrap();
    let settings = serde_json::json!({
        "disableAllHooks": true,
        "hooks": {
            "PreToolUse": [{"matcher": "Edit|Write", "hooks": [{"type":"command","command":"phr-mcp pre-check"}]}],
            "PostToolUse": [{"matcher": "Edit|Write", "hooks": [{"type":"command","command":"phr-mcp post-check"}]}]
        }
    });
    std::fs::write(
        tmp.join(".claude").join("settings.local.json"),
        serde_json::to_string_pretty(&settings).unwrap(),
    )
    .unwrap();
    let finding = check_claude_settings(&tmp);
    assert_eq!(finding.status, Status::Fail);
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn check_claude_settings_warns_on_non_command_hook_type() {
    let tmp =
        std::env::temp_dir().join(format!("phr-doctor-claude-nontype-{}", std::process::id()));
    std::fs::create_dir_all(tmp.join(".claude")).unwrap();
    let settings = serde_json::json!({
        "hooks": {
            "PreToolUse": [{"matcher": "Edit|Write", "hooks": [{"type":"builtin","command":"phr-mcp pre-check"}]}],
            "PostToolUse": [{"matcher": "Edit|Write", "hooks": [{"type":"command","command":"phr-mcp post-check"}]}]
        }
    });
    std::fs::write(
        tmp.join(".claude").join("settings.local.json"),
        serde_json::to_string_pretty(&settings).unwrap(),
    )
    .unwrap();
    let finding = check_claude_settings(&tmp);
    assert_eq!(finding.status, Status::Warn);
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn check_claude_settings_warns_on_async_hook() {
    let tmp = std::env::temp_dir().join(format!("phr-doctor-claude-async-{}", std::process::id()));
    std::fs::create_dir_all(tmp.join(".claude")).unwrap();
    let settings = serde_json::json!({
        "hooks": {
            "PreToolUse": [{"matcher": "Edit|Write", "hooks": [{"type":"command","command":"phr-mcp pre-check","async":true}]}],
            "PostToolUse": [{"matcher": "Edit|Write", "hooks": [{"type":"command","command":"phr-mcp post-check"}]}]
        }
    });
    std::fs::write(
        tmp.join(".claude").join("settings.local.json"),
        serde_json::to_string_pretty(&settings).unwrap(),
    )
    .unwrap();
    let finding = check_claude_settings(&tmp);
    assert_eq!(finding.status, Status::Warn);
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn check_claude_settings_warns_when_matcher_does_not_cover_edit_write() {
    let tmp =
        std::env::temp_dir().join(format!("phr-doctor-claude-matcher-{}", std::process::id()));
    std::fs::create_dir_all(tmp.join(".claude")).unwrap();
    let settings = serde_json::json!({
        "hooks": {
            "PreToolUse": [{"matcher": "Bash", "hooks": [{"type":"command","command":"phr-mcp pre-check"}]}],
            "PostToolUse": [{"matcher": "Bash", "hooks": [{"type":"command","command":"phr-mcp post-check"}]}]
        }
    });
    std::fs::write(
        tmp.join(".claude").join("settings.local.json"),
        serde_json::to_string_pretty(&settings).unwrap(),
    )
    .unwrap();
    let finding = check_claude_settings(&tmp);
    assert_eq!(finding.status, Status::Warn);
    std::fs::remove_dir_all(&tmp).ok();
}

#[cfg(unix)]
#[test]
fn check_binary_on_path_fails_when_found_file_is_not_executable() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = std::env::temp_dir().join(format!("phr-doctor-noexec-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let bin = tmp.join(BIN_NAME);
    std::fs::write(&bin, b"x").unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(!is_executable(&bin));
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn check_rules_load_ok_when_rules_present() {
    let tmp = std::env::temp_dir().join(format!("phr-doctor-rules-present-{}", std::process::id()));
    std::fs::create_dir_all(tmp.join(".phronesis")).unwrap();
    let rules = serde_json::json!({
        "rules": [{
            "id": "demo",
            "phase": "pre",
            "priority": 1,
            "when": [{ "new_content_contains": "x" }],
            "then": { "warn": "demo" }
        }]
    });
    std::fs::write(
        tmp.join(".phronesis").join("rules.json"),
        serde_json::to_string_pretty(&rules).unwrap(),
    )
    .unwrap();
    let finding = check_rules_load(&tmp);
    assert_eq!(finding.status, Status::Ok);
    assert!(finding.message.contains('1'));
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn check_claude_settings_fail_when_one_hook_missing() {
    let tmp =
        std::env::temp_dir().join(format!("phr-doctor-claude-partial-{}", std::process::id()));
    std::fs::create_dir_all(tmp.join(".claude")).unwrap();
    let settings = serde_json::json!({
        "hooks": {
            "PreToolUse": [{"matcher": "Edit|Write|MultiEdit|Bash", "hooks": [{"type":"command","command":"phr-mcp pre-check"}]}]
        }
    });
    std::fs::write(
        tmp.join(".claude").join("settings.local.json"),
        serde_json::to_string_pretty(&settings).unwrap(),
    )
    .unwrap();
    let finding = check_claude_settings(&tmp);
    assert_eq!(finding.status, Status::Fail);
    assert!(finding.message.contains("post-check is"));
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn doctor_report_exit_code_escalates_with_severity() {
    let ok_report = DoctorReport {
        host: "claude".into(),
        path: ".".into(),
        findings: vec![Finding::new("a", Status::Ok, "fine")],
        probe: None,
    };
    assert_eq!(ok_report.exit_code(), 0);

    let warn_report = DoctorReport {
        host: "claude".into(),
        path: ".".into(),
        findings: vec![Finding::new("a", Status::Warn, "meh")],
        probe: None,
    };
    assert_eq!(warn_report.exit_code(), 1);

    let fail_report = DoctorReport {
        host: "claude".into(),
        path: ".".into(),
        findings: vec![
            Finding::new("a", Status::Warn, "meh"),
            Finding::new("b", Status::Fail, "bad"),
        ],
        probe: None,
    };
    assert_eq!(fail_report.exit_code(), 2);
}
