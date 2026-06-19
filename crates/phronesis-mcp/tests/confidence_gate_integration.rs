//! End-to-end confidence-gate tests: post-check captures grounded build/test
//! outcomes into the per-subject ledger, and pre-check blocks/warns a
//! `git commit` based on the accumulated confidence band. See
//! `docs/specs/SPEC-confidence-scoring.md`.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Two confidence gate rules (approach A): block a commit at <=1 passed
/// signals, warn at exactly 2. 3 would pass clean (no rule fires).
const GATE_RULES: &str = r#"{
  "rules": [
    {
      "id": "confidence-low-blocks-commit",
      "phase": "pre",
      "priority": 30,
      "when": [
        { "bash_command_matches": "git commit" },
        { "__script__": "facts_count('signal_pass', ['*','*']) <= 1" }
      ],
      "then": { "block": "Low confidence — resolve failing signals before committing." }
    },
    {
      "id": "confidence-medium-warns-commit",
      "phase": "pre",
      "priority": 29,
      "when": [
        { "bash_command_matches": "git commit" },
        { "__script__": "facts_count('signal_pass', ['*','*']) == 2" }
      ],
      "then": { "warn": "Medium confidence — one grounded signal missing." }
    }
  ]
}"#;

fn run_hook(subcommand: &str, payload: &str, root: &Path) -> (i32, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .arg(subcommand)
        .env("PHRONESIS_PROJECT_ROOT", root)
        .env("PHRONESIS_NO_ACTION_LOG", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn hook");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn phr_dir(root: &Path) -> std::path::PathBuf {
    let d = root.join(".phronesis");
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn enable_confidence(root: &Path) {
    std::fs::write(phr_dir(root).join("confidence.json"), "{}").unwrap();
}

fn write_rules(root: &Path) {
    std::fs::write(phr_dir(root).join("rules.json"), GATE_RULES).unwrap();
}

/// Seed the open work unit and its outcome history into the journey journal
/// — the 0.13.0 fold-in. Each `tag` is an `outcome:*` string; one journal
/// record per tag.
fn seed_outcomes(root: &Path, subject: &str, tags: &[&str]) {
    let outcomes = phr_dir(root).join("outcomes");
    std::fs::create_dir_all(&outcomes).unwrap();
    std::fs::write(outcomes.join("current"), subject).unwrap();
    let journey = phr_dir(root).join("journey");
    std::fs::create_dir_all(&journey).unwrap();
    let mut lines = Vec::new();
    for (i, tag) in tags.iter().enumerate() {
        let rec = serde_json::json!({
            "v": 1,
            "ts": (i as u64) + 1,
            "sid": "s-test",
            "seq": (i as u64) + 1,
            "tool": "Bash",
            "path": "<cmd>",
            "tags": [tag],
            "subject": subject,
        });
        lines.push(rec.to_string());
    }
    std::fs::write(journey.join("events.jsonl"), lines.join("\n") + "\n").unwrap();
}

const COMMIT_PAYLOAD: &str =
    r#"{"tool_name":"Bash","tool_input":{"command":"git commit -m \"x\""}}"#;

#[test]
fn low_confidence_blocks_commit() {
    let dir = tempfile::tempdir().unwrap();
    enable_confidence(dir.path());
    write_rules(dir.path());
    // No ledger at all → zero signals → low → block.
    let (code, stderr) = run_hook("pre-check", COMMIT_PAYLOAD, dir.path());
    assert_eq!(
        code, 2,
        "commit must be blocked at zero signals; stderr: {stderr}"
    );
    assert!(stderr.contains("Low confidence"));
}

#[test]
fn two_signals_warns_but_does_not_block() {
    let dir = tempfile::tempdir().unwrap();
    enable_confidence(dir.path());
    write_rules(dir.path());
    seed_outcomes(
        dir.path(),
        "u",
        &["outcome:compile_ok", "outcome:test_pass"],
    );
    let (code, stderr) = run_hook("pre-check", COMMIT_PAYLOAD, dir.path());
    assert_eq!(
        code, 1,
        "two signals should warn, not block; stderr: {stderr}"
    );
    assert!(stderr.contains("Medium confidence"));
}

#[test]
fn disabled_project_skips_outcome_capture() {
    // The opt-in guarantee: without `.phronesis/confidence.json`, post-check
    // does not capture outcomes and creates no `.phronesis/outcomes/` dir, so
    // projects that haven't enabled confidence see no behavior change.
    let dir = tempfile::tempdir().unwrap();
    let test_payload = r#"{
        "tool_name":"Bash",
        "tool_input":{"command":"cargo test"},
        "tool_output":{"stdout":"test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n"}
    }"#;
    let (code, _) = run_hook("post-check", test_payload, dir.path());
    assert_eq!(code, 0);
    assert!(
        !dir.path().join(".phronesis/outcomes").exists(),
        "capture must be opt-in (no confidence.json -> no outcomes dir)"
    );
}

#[test]
fn post_check_captures_cargo_test_then_commit_warns() {
    let dir = tempfile::tempdir().unwrap();
    enable_confidence(dir.path());
    write_rules(dir.path());

    // The agent runs `cargo test` and it passes — post-check captures it.
    let test_payload = r#"{
        "tool_name":"Bash",
        "tool_input":{"command":"cargo test --workspace"},
        "tool_output":{"stdout":"running 5 tests\ntest result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s\n"}
    }"#;
    let (code, _) = run_hook("post-check", test_payload, dir.path());
    assert_eq!(code, 0);

    // The journey journal now holds the outcome tags for the minted unit.
    let outcomes = dir.path().join(".phronesis/outcomes");
    assert!(outcomes.join("current").exists(), "a work unit was opened");
    let subject = std::fs::read_to_string(outcomes.join("current")).unwrap();
    let journal =
        std::fs::read_to_string(dir.path().join(".phronesis/journey/events.jsonl")).unwrap();
    assert!(journal.contains("outcome:compile_ok"), "journal: {journal}");
    assert!(journal.contains("outcome:test_pass"), "journal: {journal}");
    assert!(
        journal.contains(&format!("\"subject\":\"{}\"", subject.trim())),
        "journal subject mismatch: {journal}"
    );

    // Now a commit sees 2 signals → medium → warn (exit 1), not blocked.
    let (code, stderr) = run_hook("pre-check", COMMIT_PAYLOAD, dir.path());
    assert_eq!(
        code, 1,
        "captured signals should lift the commit out of block; stderr: {stderr}"
    );
    assert!(stderr.contains("Medium confidence"));
}

#[test]
fn failing_cargo_test_keeps_commit_blocked() {
    let dir = tempfile::tempdir().unwrap();
    enable_confidence(dir.path());
    write_rules(dir.path());

    // Tests fail → compile signal only (1 signal) → still low → block.
    let test_payload = r#"{
        "tool_name":"Bash",
        "tool_input":{"command":"cargo test"},
        "tool_output":{"stdout":"test result: FAILED. 3 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out\n"}
    }"#;
    let (code, _) = run_hook("post-check", test_payload, dir.path());
    assert_eq!(code, 0);

    let (code, stderr) = run_hook("pre-check", COMMIT_PAYLOAD, dir.path());
    assert_eq!(
        code, 2,
        "failing tests leave only 1 signal → blocked; stderr: {stderr}"
    );
    assert!(stderr.contains("Low confidence"));
}

#[test]
fn catching_known_bug_reaches_high_and_commit_passes_clean() {
    let dir = tempfile::tempdir().unwrap();
    enable_confidence(dir.path());
    write_rules(dir.path());
    std::fs::write(
        phr_dir(dir.path()).join("bugs.json"),
        r#"[{"bug_id":"1042","test":"auth::rejects_expired","status":"open"}]"#,
    )
    .unwrap();

    // cargo test: the known-bug test passes, nothing fails → compile + tests +
    // bug = 3 signals = high.
    let test_payload = r#"{
        "tool_name":"Bash",
        "tool_input":{"command":"cargo test"},
        "tool_output":{"stdout":"running 2 tests\ntest auth::rejects_expired ... ok\ntest other::thing ... ok\ntest result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n"}
    }"#;
    let (code, _) = run_hook("post-check", test_payload, dir.path());
    assert_eq!(code, 0);

    // 3/3 signals → neither low nor medium fires → commit proceeds clean.
    let (code, stderr) = run_hook("pre-check", COMMIT_PAYLOAD, dir.path());
    assert_eq!(
        code, 0,
        "3 signals (incl. known-bug) should pass clean; stderr: {stderr}"
    );
}

#[test]
fn commit_settles_the_work_unit() {
    let dir = tempfile::tempdir().unwrap();
    enable_confidence(dir.path());
    write_rules(dir.path());
    seed_outcomes(dir.path(), "u", &["outcome:compile_ok"]);
    // Post-check of a commit settles (closes) the open unit.
    let (code, _) = run_hook("post-check", COMMIT_PAYLOAD, dir.path());
    assert_eq!(code, 0);
    assert!(
        !dir.path().join(".phronesis/outcomes/current").exists(),
        "git commit should settle the open work unit"
    );
}
