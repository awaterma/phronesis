//! Hooks fired from a directory no phronesis project governs must leave no
//! trace. Before this was enforced, the lifecycle writers created
//! `<cwd>/.phronesis/journey/` from any ungoverned cwd, and those stray dirs
//! then stopped the project-root walk and ungoverned their whole subtree.

use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

fn run(cwd: &Path, args: &[&str], stdin: &str) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(args)
        .current_dir(cwd)
        .env_remove("PHRONESIS_PROJECT_ROOT")
        .env_remove("PHRONESIS_CAPTURE_DIR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn phr-mcp");
    let mut pipe = child.stdin.take().expect("stdin");
    // A hook that exits before reading stdin closes the pipe; that is fine.
    let _ = pipe.write_all(stdin.as_bytes());
    drop(pipe);
    child.wait_with_output().expect("wait phr-mcp")
}

/// Every `.phronesis` directory at or below `dir`.
fn phronesis_dirs(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == ".phronesis") {
                    found.push(path.clone());
                }
                stack.push(path);
            }
        }
    }
    found
}

const EDIT: &str = r#"{"session_id":"s1","tool_name":"Edit","tool_use_id":"t1","tool_input":{"file_path":"src/lib.rs","old_string":"a","new_string":"b"}}"#;
const BASH: &str = r#"{"session_id":"s1","tool_name":"Bash","tool_use_id":"t2","tool_input":{"command":"git commit -m x"},"tool_response":{"exit_code":0}}"#;
const GEMINI_AGENT: &str =
    r#"{"session_id":"s1","tool_name":"invoke_agent","tool_input":{"agent_name":"helper"}}"#;

fn claude(event: &str, extra: &str) -> String {
    format!(
        r#"{{"hook_event_name":"{event}","session_id":"s1","prompt":"hello","agent_id":"a1","agent_type":"Explore"{extra}}}"#
    )
}

fn codex(event: &str) -> String {
    format!(
        r#"{{"hook_event_name":"{event}","session_id":"s1","turn_id":"u1","prompt":"hello","source":"startup","tool_name":"Bash","tool_use_id":"t3","tool_input":{{"command":"echo hi"}}}}"#
    )
}

#[test]
fn hooks_in_an_ungoverned_directory_create_no_phronesis_dir() {
    let tmp = tempfile::tempdir().expect("tmp");
    let cwd = tmp.path().join("project/src");
    std::fs::create_dir_all(&cwd).expect("mkdir");

    let session_start = claude("SessionStart", r#","source":"startup""#);
    let prompt = claude("UserPromptSubmit", "");
    let sub_start = claude("SubagentStart", "");
    let sub_stop = claude("SubagentStop", "");
    let stop = claude("Stop", "");
    let session_end = claude("SessionEnd", "");
    let codex_events: Vec<(String, String)> = [
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "SubagentStart",
        "SubagentStop",
        "Stop",
        "Interrupt",
        "SessionEnd",
    ]
    .iter()
    .map(|e| (e.to_string(), codex(e)))
    .collect();

    let mut calls: Vec<(Vec<&str>, &str)> = vec![
        (vec!["pre-check"], EDIT),
        (vec!["post-check"], EDIT),
        (vec!["pre-check"], BASH),
        (vec!["post-check"], BASH),
        (vec!["pre-check"], GEMINI_AGENT),
        (vec!["post-check"], GEMINI_AGENT),
        (vec!["claude-hook", "SessionStart"], &session_start),
        (vec!["claude-hook", "UserPromptSubmit"], &prompt),
        (vec!["claude-hook", "BeforeAgent"], &prompt),
        (vec!["claude-hook", "SubagentStart"], &sub_start),
        (vec!["claude-hook", "SubagentStop"], &sub_stop),
        (vec!["claude-hook", "Stop"], &stop),
        (vec!["claude-hook", "AfterAgent"], &stop),
        (vec!["claude-hook", "SessionEnd"], &session_end),
        (vec!["claude-hook", "PreToolUse"], EDIT),
        (vec!["claude-hook", "PostToolUse"], EDIT),
        (vec!["session-context"], "{}"),
        (vec!["interaction-context"], "{}"),
    ];
    for (event, payload) in &codex_events {
        calls.push((vec!["codex-hook", event.as_str()], payload.as_str()));
    }

    for (args, payload) in calls {
        let out = run(&cwd, &args, payload);
        let stray = phronesis_dirs(tmp.path());
        assert!(
            stray.is_empty(),
            "`phr-mcp {}` in an ungoverned dir created {stray:?} (exit {:?}, stderr: {})",
            args.join(" "),
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn ungoverned_hooks_keep_their_exit_and_output_contract() {
    let tmp = tempfile::tempdir().expect("tmp");
    // Ungoverned pre/post-check allow.
    assert_eq!(run(tmp.path(), &["pre-check"], EDIT).status.code(), Some(0));
    assert_eq!(
        run(tmp.path(), &["post-check"], EDIT).status.code(),
        Some(0)
    );
    // The lifecycle adapters still print exactly one JSON object and exit 0.
    for args in [
        ["claude-hook", "UserPromptSubmit"],
        ["codex-hook", "UserPromptSubmit"],
    ] {
        let out = run(tmp.path(), &args, &claude("UserPromptSubmit", ""));
        assert_eq!(out.status.code(), Some(0), "{args:?}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        serde_json::from_str::<serde_json::Value>(stdout.trim())
            .unwrap_or_else(|e| panic!("{args:?} stdout is not one JSON object ({e}): {stdout}"));
    }
}
