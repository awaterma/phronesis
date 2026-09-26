//! Gate for the Claude adapter (spec §Testing, `tests/lifecycle_concurrency.rs`):
//! concurrent tool calls from several agents must leave the `inflight` file
//! consistent, and a prompt arriving after they have all completed must never
//! be classified as a `correction`.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

fn run(dir: &Path, args: &[&str], payload: &str) -> i32 {
    ensure_governed(dir);
    let mut child = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");
    let mut stdin = child.stdin.take().unwrap();
    let _ = stdin.write_all(payload.as_bytes());
    drop(stdin);
    child.wait().expect("wait").code().unwrap_or(-1)
}

/// Hooks record lifecycle state only under a governed root (one whose
/// `.phronesis/` holds a rules config). Lifecycle tests exercise that
/// recording, so give a bare fixture an empty rule set — rule evaluation is
/// unchanged, the root is just governed.
fn ensure_governed(root: &Path) {
    let phr = root.join(".phronesis");
    if phr.join("rules.json").is_file() || phr.join("loader.json").is_file() {
        return;
    }
    std::fs::create_dir_all(&phr).expect("mkdir .phronesis");
    std::fs::write(phr.join("rules.json"), r#"{"rules":[]}"#).expect("write rules.json");
}

fn journal(dir: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(dir.join(".phronesis/journey/events.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[test]
fn concurrent_tool_pairs_never_produce_a_correction() {
    const N: usize = 12;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();

    // Open a turn, so a spurious interrupt would be observable.
    run(
        &root,
        &["claude-hook", "UserPromptSubmit"],
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"go"}"#,
    );

    let handles: Vec<_> = (0..N)
        .map(|i| {
            let root = root.clone();
            std::thread::spawn(move || {
                // Half the calls belong to sub-agents, half to the parent.
                let agent = if i % 2 == 0 {
                    format!(r#","agent_id":"sub-{i}""#)
                } else {
                    String::new()
                };
                let pre = format!(
                    r#"{{"tool_name":"Bash","tool_use_id":"tu-{i}","tool_input":{{"command":"echo {i}"}}{agent}}}"#
                );
                assert_eq!(run(&root, &["pre-check"], &pre), 0);
                let post = format!(
                    r#"{{"tool_name":"Bash","tool_use_id":"tu-{i}","tool_input":{{"command":"echo {i}"}},"tool_response":{{"exit_code":0}}{agent}}}"#
                );
                assert_eq!(run(&root, &["post-check"], &post), 0);
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }

    // Every entry was popped by its own post-check.
    let inflight = std::fs::read_to_string(root.join(".phronesis/journey/inflight")).unwrap();
    assert_eq!(inflight.trim(), "", "leftover inflight entries: {inflight}");

    run(
        &root,
        &["claude-hook", "UserPromptSubmit"],
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"and now this"}"#,
    );

    let records = journal(&root);
    assert!(
        !records.iter().any(|r| r["kind"] == "interrupt"),
        "no interrupt may be inferred from completed tool calls"
    );
    let modes: Vec<&str> = records
        .iter()
        .filter(|r| r["kind"] == "prompt")
        .map(|r| r["mode"].as_str().unwrap())
        .collect();
    assert_eq!(modes.len(), 2, "{modes:?}");
    assert_eq!(modes[1], "mid_turn", "{modes:?}");
    assert!(!modes.contains(&"correction"), "{modes:?}");

    // Every tool record survived: the journal append is serialized too.
    assert_eq!(
        records.iter().filter(|r| r["tool"] == "Bash").count(),
        N,
        "lost tool records under concurrency"
    );
}

#[test]
fn a_subagents_live_tool_call_does_not_make_the_parent_prompt_a_correction() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    run(
        &root,
        &["claude-hook", "UserPromptSubmit"],
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"go"}"#,
    );
    // A sub-agent's call is still in flight; the parent speaks.
    run(
        &root,
        &["pre-check"],
        r#"{"tool_name":"Bash","tool_use_id":"tu-x","agent_id":"sub-1","tool_input":{"command":"sleep 100"}}"#,
    );
    run(
        &root,
        &["claude-hook", "UserPromptSubmit"],
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"meanwhile"}"#,
    );
    let records = journal(&root);
    assert!(
        !records.iter().any(|r| r["kind"] == "interrupt"),
        "scope leak"
    );
    let modes: Vec<&str> = records
        .iter()
        .filter(|r| r["kind"] == "prompt")
        .map(|r| r["mode"].as_str().unwrap())
        .collect();
    assert_eq!(modes, vec!["fresh", "mid_turn"]);
}
