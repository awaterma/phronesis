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
        { "bash_command_matches": "git (commit|merge|rebase|cherry-pick|revert|pull)" },
        { "__script__": "facts_count('signal_pass', ['*','*']) <= 1" }
      ],
      "then": { "block": "Low confidence — resolve failing signals before committing." }
    },
    {
      "id": "confidence-medium-warns-commit",
      "phase": "pre",
      "priority": 29,
      "when": [
        { "bash_command_matches": "git (commit|merge|rebase|cherry-pick|revert|pull)" },
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

// ─────────────────────────────────────────────────────────────────────
// SPEC-gate-merge-commits: gate fires on every commit-producing porcelain
// command, not just the literal `git commit`. See
// `docs/specs/SPEC-gate-merge-commits.md`.
// ─────────────────────────────────────────────────────────────────────

/// Helper: build a Bash pre-tool payload for a given command.
fn bash_payload(cmd: &str) -> String {
    serde_json::json!({
        "tool_name": "Bash",
        "tool_input": { "command": cmd },
    })
    .to_string()
}

#[test]
fn gate_fires_on_git_commit_at_low_confidence() {
    // Sanity: the broadened regex still matches the original `git commit`.
    let dir = tempfile::tempdir().unwrap();
    enable_confidence(dir.path());
    write_rules(dir.path());
    let (code, stderr) = run_hook("pre-check", &bash_payload("git commit -m 'x'"), dir.path());
    assert_eq!(
        code, 2,
        "git commit must still block at low; stderr: {stderr}"
    );
    assert!(stderr.contains("Low confidence"));
}

#[test]
fn gate_fires_on_git_merge_at_low_confidence() {
    let dir = tempfile::tempdir().unwrap();
    enable_confidence(dir.path());
    write_rules(dir.path());
    let (code, stderr) = run_hook(
        "pre-check",
        &bash_payload("git merge --no-ff feature-branch"),
        dir.path(),
    );
    assert_eq!(code, 2, "git merge must block at low; stderr: {stderr}");
    assert!(stderr.contains("Low confidence"));
}

#[test]
fn gate_fires_on_git_merge_at_medium_confidence() {
    let dir = tempfile::tempdir().unwrap();
    enable_confidence(dir.path());
    write_rules(dir.path());
    seed_outcomes(
        dir.path(),
        "u",
        &["outcome:compile_ok", "outcome:test_pass"],
    );
    let (code, stderr) = run_hook(
        "pre-check",
        &bash_payload("git merge --no-ff feature-branch"),
        dir.path(),
    );
    assert_eq!(
        code, 1,
        "git merge at 2 signals must warn; stderr: {stderr}"
    );
    assert!(stderr.contains("Medium confidence"));
}

#[test]
fn gate_fires_on_git_rebase() {
    let dir = tempfile::tempdir().unwrap();
    enable_confidence(dir.path());
    write_rules(dir.path());
    let (code, stderr) = run_hook("pre-check", &bash_payload("git rebase main"), dir.path());
    assert_eq!(code, 2, "git rebase must block at low; stderr: {stderr}");
    assert!(stderr.contains("Low confidence"));
}

#[test]
fn gate_fires_on_git_cherry_pick() {
    let dir = tempfile::tempdir().unwrap();
    enable_confidence(dir.path());
    write_rules(dir.path());
    let (code, stderr) = run_hook(
        "pre-check",
        &bash_payload("git cherry-pick abc123"),
        dir.path(),
    );
    assert_eq!(
        code, 2,
        "git cherry-pick must block at low; stderr: {stderr}"
    );
    assert!(stderr.contains("Low confidence"));
}

#[test]
fn gate_fires_on_git_revert() {
    let dir = tempfile::tempdir().unwrap();
    enable_confidence(dir.path());
    write_rules(dir.path());
    let (code, stderr) = run_hook("pre-check", &bash_payload("git revert HEAD"), dir.path());
    assert_eq!(code, 2, "git revert must block at low; stderr: {stderr}");
    assert!(stderr.contains("Low confidence"));
}

#[test]
fn gate_fires_on_git_pull_when_default_merge() {
    let dir = tempfile::tempdir().unwrap();
    enable_confidence(dir.path());
    write_rules(dir.path());
    let (code, stderr) = run_hook(
        "pre-check",
        &bash_payload("git pull origin main"),
        dir.path(),
    );
    assert_eq!(code, 2, "git pull must block at low; stderr: {stderr}");
    assert!(stderr.contains("Low confidence"));
}

// ─────────────────────────────────────────────────────────────────────
// SPEC-pack-opt-in-facts: the `nudge-verify-before-commit` rule from the
// `llm` pack self-deactivates when `.phronesis/confidence.json` exists,
// to avoid double-warning on every commit. See
// `docs/specs/SPEC-pack-opt-in-facts.md`.
// ─────────────────────────────────────────────────────────────────────

/// The nudge rule shipped with the `llm` pack, paired with the SPEC's
/// absence clause. We don't include the gate rules here — the point of
/// these tests is to drive *only* the nudge and observe whether the
/// marker fact silences it.
const NUDGE_RULES: &str = r#"{
  "rules": [
    {
      "id": "nudge-verify-before-commit",
      "phase": "pre",
      "priority": 5,
      "when": [
        { "new_content_contains": "git commit -m" },
        { "__script__": "facts_count('confidence_enabled', []) == 0" }
      ],
      "then": { "warn": "About to commit. Trace the call chain end-to-end before reporting done." }
    }
  ]
}"#;

fn write_nudge_rules(root: &Path) {
    std::fs::write(phr_dir(root).join("rules.json"), NUDGE_RULES).unwrap();
}

/// Bash payload whose content matches `new_content_contains: "git commit -m"`.
/// The fact extractor scans `tool_input.command` for the pattern; the
/// nudge then fires unless its absence clause silences it.
const NUDGE_TRIGGER_PAYLOAD: &str =
    r#"{"tool_name":"Bash","tool_input":{"command":"git commit -m \"work\""}}"#;

#[test]
fn nudge_silent_when_confidence_opted_in() {
    let dir = tempfile::tempdir().unwrap();
    enable_confidence(dir.path());
    write_nudge_rules(dir.path());
    let (code, stderr) = run_hook("pre-check", NUDGE_TRIGGER_PAYLOAD, dir.path());
    assert_eq!(
        code, 0,
        "with confidence opted in, the nudge must self-deactivate; stderr: {stderr}"
    );
    assert!(
        !stderr.contains("Trace the call chain"),
        "nudge body must not appear in stderr; got: {stderr}"
    );
}

#[test]
fn nudge_fires_when_confidence_off() {
    // No confidence.json -> no confidence_enabled fact -> absence clause
    // is true -> nudge fires (warn, exit 1).
    let dir = tempfile::tempdir().unwrap();
    write_nudge_rules(dir.path());
    let (code, stderr) = run_hook("pre-check", NUDGE_TRIGGER_PAYLOAD, dir.path());
    assert_eq!(
        code, 1,
        "without confidence, the nudge must warn; stderr: {stderr}"
    );
    assert!(
        stderr.contains("Trace the call chain"),
        "expected nudge body in stderr; got: {stderr}"
    );
}

#[test]
fn gate_does_not_fire_on_unrelated_git_command() {
    // Sanity: the broadened regex doesn't over-match observational commands.
    // `git status` and `git log` produce no commits; the gate must stay silent
    // regardless of signal state.
    let dir = tempfile::tempdir().unwrap();
    enable_confidence(dir.path());
    write_rules(dir.path());
    // Zero signals would normally trip the low-confidence rule if the bash
    // pattern matched.
    let (code, stderr) = run_hook("pre-check", &bash_payload("git status"), dir.path());
    assert_eq!(
        code, 0,
        "git status must not trip the gate; stderr: {stderr}"
    );
    let (code, stderr) = run_hook("pre-check", &bash_payload("git log --oneline"), dir.path());
    assert_eq!(code, 0, "git log must not trip the gate; stderr: {stderr}");
}
