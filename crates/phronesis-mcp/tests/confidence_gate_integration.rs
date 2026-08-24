//! End-to-end confidence-gate tests: post-check captures grounded build/test
//! outcomes into the per-subject ledger, and pre-check warns on a governed
//! Git mutation based on the accumulated confidence band. Neither band
//! blocks — incomplete or failing confidence evidence is advisory only. See
//! `docs/specs/SPEC-confidence-scoring.md` and
//! `docs/specs/SPEC-structural-rule-migration.md` §"Confidence gate
//! severity".

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Two confidence gate rules (approach A): warn at <=1 passed signals
/// (missing/failing evidence), warn at exactly 2 (one signal missing). 3
/// would pass clean (no rule fires). Neither band blocks.
///
/// Message text is kept byte-identical to `confidence_rules()` in
/// `crates/phronesis-mcp/src/init.rs` (checked by
/// `hand_written_gate_rules_match_generated_messages_byte_for_byte`) so this
/// fixture can't silently drift from the real generated rule while testing
/// the matcher/band logic in isolation. The producer/consumer seam itself —
/// driving the actual `init --packs confidence` output through the real
/// binary — is covered separately by
/// `real_generated_confidence_rules_warn_not_block_end_to_end`.
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
      "then": { "warn": "Low confidence — compile/tests/known-bug evidence is incomplete or failing. Run `phr-mcp confidence` for the per-signal report before presenting this as done." }
    },
    {
      "id": "confidence-medium-warns-commit",
      "phase": "pre",
      "priority": 29,
      "when": [
        { "bash_command_matches": "git (commit|merge|rebase|cherry-pick|revert|pull)" },
        { "__script__": "facts_count('signal_pass', ['*','*']) == 2" }
      ],
      "then": { "warn": "Medium confidence — one grounded signal is missing. Review before presenting this as done." }
    }
  ]
}"#;

/// A blocking rule unrelated to confidence, used to prove that low
/// confidence's shift to `warn` didn't accidentally make `pre-check` unable
/// to exit 2 at all — an unrelated `block` rule must still exit 2 even when
/// it fires alongside the (now non-blocking) low-confidence warning.
const GATE_RULES_WITH_UNRELATED_BLOCK: &str = r#"{
  "rules": [
    {
      "id": "confidence-low-blocks-commit",
      "phase": "pre",
      "priority": 30,
      "when": [
        { "bash_command_matches": "git (commit|merge|rebase|cherry-pick|revert|pull)" },
        { "__script__": "facts_count('signal_pass', ['*','*']) <= 1" }
      ],
      "then": { "warn": "Low confidence — compile/tests/known-bug evidence is incomplete or failing. Run `phr-mcp confidence` for the per-signal report before presenting this as done." }
    },
    {
      "id": "unrelated-block-on-commit",
      "phase": "pre",
      "priority": 40,
      "when": [
        { "bash_command_matches": "git commit" }
      ],
      "then": { "block": "unrelated blocking rule fired" }
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

fn write_rules_with_unrelated_block(root: &Path) {
    std::fs::write(
        phr_dir(root).join("rules.json"),
        GATE_RULES_WITH_UNRELATED_BLOCK,
    )
    .unwrap();
}

/// The `then.warn` (or `then.block`) message string for `id` inside a parsed
/// `rules.json` `Value`. Panics if the rule or its message is missing —
/// callers use this to pull the *real* generated message out of the file
/// `phr-mcp init` actually wrote, rather than a hand-copied literal that can
/// drift from it.
fn rule_message(rules: &serde_json::Value, id: &str) -> String {
    let rule = rules["rules"]
        .as_array()
        .expect("rules array")
        .iter()
        .find(|r| r["id"] == id)
        .unwrap_or_else(|| panic!("rule `{id}` not found in generated rules.json"));
    let then = rule["then"].as_object().expect("then object");
    then.values()
        .next()
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("rule `{id}` has no string message"))
        .to_string()
}

/// The `then` verb (`"warn"`, `"block"`, ...) for `id` inside a parsed
/// `rules.json` `Value`.
fn rule_verb(rules: &serde_json::Value, id: &str) -> String {
    let rule = rules["rules"]
        .as_array()
        .expect("rules array")
        .iter()
        .find(|r| r["id"] == id)
        .unwrap_or_else(|| panic!("rule `{id}` not found in generated rules.json"));
    rule["then"]
        .as_object()
        .expect("then object")
        .keys()
        .next()
        .unwrap_or_else(|| panic!("rule `{id}` has no verb"))
        .to_string()
}

/// Producer/consumer seam test: this hand-copied `GATE_RULES` fixture must
/// not silently drift from what `confidence_rules()` in
/// `crates/phronesis-mcp/src/init.rs` actually generates. If someone edits
/// one without the other, this fails loudly instead of letting the other
/// tests in this file quietly assert against stale message text.
#[test]
fn hand_written_gate_rules_match_generated_messages_byte_for_byte() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(["init", "--packs", "confidence"])
        .current_dir(dir.path())
        .output()
        .expect("spawn init");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let generated: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".phronesis/rules.json")).unwrap(),
    )
    .unwrap();
    let fixture: serde_json::Value = serde_json::from_str(GATE_RULES).unwrap();

    for id in [
        "confidence-low-blocks-commit",
        "confidence-medium-warns-commit",
    ] {
        assert_eq!(
            rule_verb(&generated, id),
            rule_verb(&fixture, id),
            "rule `{id}` action verb diverged between generated rules.json and the GATE_RULES test fixture"
        );
        assert_eq!(
            rule_message(&generated, id),
            rule_message(&fixture, id),
            "rule `{id}` message diverged between generated rules.json and the GATE_RULES test fixture"
        );
    }
}

/// End-to-end producer/consumer seam: drives the *actual* `init --packs
/// confidence` output (not a hand-copied fixture) through the real
/// `pre-check`/`post-check` binary across the full band range — no
/// evidence (low), two signals (medium), and 3/3 including a known-bug fix
/// (high) — asserting against the generated rule's own message text so
/// producer/consumer drift is structurally impossible here, and confirming
/// low/medium warn (never exit 2) while high passes clean (exit 0).
#[test]
fn real_generated_confidence_rules_warn_not_block_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(["init", "--packs", "confidence"])
        .current_dir(dir.path())
        .output()
        .expect("spawn init");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let rules: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".phronesis/rules.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        rule_verb(&rules, "confidence-low-blocks-commit"),
        "warn",
        "generated low-confidence rule must warn, not block"
    );
    let low_message = rule_message(&rules, "confidence-low-blocks-commit");
    let medium_message = rule_message(&rules, "confidence-medium-warns-commit");

    // No evidence at all -> low band -> warn (exit 1), never block (exit 2).
    let (code, stderr) = run_hook("pre-check", COMMIT_PAYLOAD, dir.path());
    assert_eq!(
        code, 1,
        "real generated low-confidence rule must warn, not block; stderr: {stderr}"
    );
    assert_ne!(code, 2, "low confidence must never exit 2");
    assert!(
        stderr.contains(&low_message),
        "stderr should contain the real generated rule's own message; stderr: {stderr}"
    );

    // Two grounded signals -> medium band -> warn (exit 1).
    seed_outcomes(
        dir.path(),
        "u",
        &["outcome:compile_ok", "outcome:test_pass"],
    );
    let (code, stderr) = run_hook("pre-check", COMMIT_PAYLOAD, dir.path());
    assert_eq!(
        code, 1,
        "real generated medium-confidence rule must warn; stderr: {stderr}"
    );
    assert!(stderr.contains(&medium_message), "stderr: {stderr}");

    // 3/3 signals (compile + tests + known-bug fix) -> high band -> clean.
    std::fs::write(
        phr_dir(dir.path()).join("bugs.json"),
        r#"[{"bug_id":"1042","test":"auth::rejects_expired","status":"open"}]"#,
    )
    .unwrap();
    let test_payload = r#"{
        "tool_name":"Bash",
        "tool_input":{"command":"cargo test"},
        "tool_output":{"stdout":"running 2 tests\ntest auth::rejects_expired ... ok\ntest other::thing ... ok\ntest result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n"}
    }"#;
    let (code, stderr) = run_hook("post-check", test_payload, dir.path());
    assert_eq!(code, 0, "post-check must not fail; stderr: {stderr}");
    let (code, stderr) = run_hook("pre-check", COMMIT_PAYLOAD, dir.path());
    assert_eq!(
        code, 0,
        "3/3 signals through the real generated rules must pass clean; stderr: {stderr}"
    );
}

/// Disabled-confidence coverage through the real generated rule set: a
/// project that never ran `phr-mcp init --packs confidence` (so no
/// `.phronesis/confidence.json` and no gate rules on disk) must see no
/// confidence gate at all — the governed commit proceeds clean regardless
/// of evidence state. Complements the mechanism-level
/// `disabled_project_skips_outcome_capture` test below.
#[test]
fn real_project_without_confidence_pack_has_no_gate_at_all() {
    let dir = tempfile::tempdir().unwrap();
    // `base` (the default `init` selection) does not include the
    // `confidence` pack's gate rules unless explicitly requested.
    let out = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(["init", "--packs", "none"])
        .current_dir(dir.path())
        .output()
        .expect("spawn init");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !dir.path().join(".phronesis/confidence.json").exists(),
        "packs=none must not scaffold confidence.json"
    );
    let (code, stderr) = run_hook("pre-check", COMMIT_PAYLOAD, dir.path());
    assert_eq!(
        code, 0,
        "no confidence pack -> no gate rule -> commit proceeds clean; stderr: {stderr}"
    );
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
fn low_confidence_warns_but_does_not_block_commit() {
    let dir = tempfile::tempdir().unwrap();
    enable_confidence(dir.path());
    write_rules(dir.path());
    // No ledger at all → zero signals → low → warn, never exit 2.
    let (code, stderr) = run_hook("pre-check", COMMIT_PAYLOAD, dir.path());
    assert_eq!(
        code, 1,
        "commit must only warn at zero signals, never block; stderr: {stderr}"
    );
    assert_ne!(code, 2, "low confidence must never exit 2");
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

/// The same gate, driven by `xcodebuild test` through the Bash tool. The
/// payload carries an empty `file_path` (as Bash events do) and no exit
/// code — exactly the shape a downstream Swift project reported as "tests
/// never registered": cargo was the only built-in def, so nothing
/// recognized the command and the 55-pass result never reached the journal.
#[test]
fn post_check_captures_xcodebuild_test_then_commit_warns() {
    let dir = tempfile::tempdir().unwrap();
    enable_confidence(dir.path());
    write_rules(dir.path());

    let test_payload = r#"{
        "tool_name":"Bash",
        "tool_input":{"command":"xcodebuild test -scheme App -destination 'platform=macOS' 2>&1 | tail -20","file_path":""},
        "tool_output":{"stdout":"Test Suite 'All tests' passed at 2026-08-22 10:00:01.000.\n\t Executed 55 tests, with 0 failures (0 unexpected) in 1.201 (1.210) seconds\n** TEST SUCCEEDED **\n"}
    }"#;
    let (code, _) = run_hook("post-check", test_payload, dir.path());
    assert_eq!(code, 0);

    let journal =
        std::fs::read_to_string(dir.path().join(".phronesis/journey/events.jsonl")).unwrap();
    assert!(journal.contains("outcome:compile_ok"), "journal: {journal}");
    assert!(journal.contains("outcome:test_pass"), "journal: {journal}");

    let (code, stderr) = run_hook("pre-check", COMMIT_PAYLOAD, dir.path());
    assert_eq!(code, 1, "stderr: {stderr}");
    assert!(stderr.contains("Medium confidence"));
}

/// Escape hatch: a test runner phronesis has no def for can still feed the
/// signal explicitly via `phr-mcp signal tests pass`.
#[test]
fn explicit_signal_command_feeds_the_tests_signal() {
    let dir = tempfile::tempdir().unwrap();
    enable_confidence(dir.path());
    write_rules(dir.path());
    seed_outcomes(dir.path(), "u", &["outcome:compile_ok"]);

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(["signal", "tests", "pass"])
        .env("PHRONESIS_PROJECT_ROOT", dir.path())
        .output()
        .expect("run phr-mcp signal");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let (code, stderr) = run_hook("pre-check", COMMIT_PAYLOAD, dir.path());
    assert_eq!(code, 1, "stderr: {stderr}");
    assert!(stderr.contains("Medium confidence"), "stderr: {stderr}");
}

/// The same gate, driven by `cargo nextest run`.
///
/// The cargo def's `matches` pattern has always accepted `cargo nextest`, but
/// its only summary pattern was libtest's `test result:` line, which nextest
/// never emits. So the def claimed the command, parsed nothing, and grounded
/// no test signal — meaning a project whose gate is `cargo nextest run` could
/// never lift a commit out of the low band no matter how green the suite was.
#[test]
fn post_check_captures_cargo_nextest_then_commit_warns() {
    let dir = tempfile::tempdir().unwrap();
    enable_confidence(dir.path());
    write_rules(dir.path());

    let test_payload = r#"{
        "tool_name":"Bash",
        "tool_input":{"command":"cargo nextest run"},
        "tool_output":{"stdout":"    Starting 234 tests across 21 binaries\nSummary [  61.425s] 234 tests run: 234 passed, 9 skipped\n"}
    }"#;
    let (code, _) = run_hook("post-check", test_payload, dir.path());
    assert_eq!(code, 0);

    let journal =
        std::fs::read_to_string(dir.path().join(".phronesis/journey/events.jsonl")).unwrap();
    assert!(journal.contains("outcome:compile_ok"), "journal: {journal}");
    assert!(
        journal.contains("outcome:test_pass"),
        "a green nextest run must ground a test signal; journal: {journal}"
    );

    let (code, stderr) = run_hook("pre-check", COMMIT_PAYLOAD, dir.path());
    assert_eq!(
        code, 1,
        "a green nextest run should lift the commit out of block; stderr: {stderr}"
    );
    assert!(stderr.contains("Medium confidence"));
}

/// A failing nextest run must ground a *failure*, not silence — and the
/// resulting low band must still only warn, not block.
#[test]
fn failing_cargo_nextest_warns_but_does_not_block_commit() {
    let dir = tempfile::tempdir().unwrap();
    enable_confidence(dir.path());
    write_rules(dir.path());

    let test_payload = r#"{
        "tool_name":"Bash",
        "tool_input":{"command":"cargo nextest run"},
        "tool_output":{"stdout":"Summary [   2.439s]   2 tests run: 0 passed, 2 failed, 241 skipped\n"}
    }"#;
    let (code, _) = run_hook("post-check", test_payload, dir.path());
    assert_eq!(code, 0);

    let journal =
        std::fs::read_to_string(dir.path().join(".phronesis/journey/events.jsonl")).unwrap();
    assert!(
        journal.contains("outcome:test_fail"),
        "failing nextest must ground a test failure; journal: {journal}"
    );

    let (code, _) = run_hook("pre-check", COMMIT_PAYLOAD, dir.path());
    assert_eq!(
        code, 1,
        "failing tests must only warn (low confidence), never block"
    );
}

#[test]
fn failing_cargo_test_warns_but_does_not_block_commit() {
    let dir = tempfile::tempdir().unwrap();
    enable_confidence(dir.path());
    write_rules(dir.path());

    // Tests fail → compile signal only (1 signal) → still low → warn, not block.
    let test_payload = r#"{
        "tool_name":"Bash",
        "tool_input":{"command":"cargo test"},
        "tool_output":{"stdout":"test result: FAILED. 3 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out\n"}
    }"#;
    let (code, _) = run_hook("post-check", test_payload, dir.path());
    assert_eq!(code, 0);

    let (code, stderr) = run_hook("pre-check", COMMIT_PAYLOAD, dir.path());
    assert_eq!(
        code, 1,
        "failing tests leave only 1 signal → warn, not block; stderr: {stderr}"
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
    // Sanity: the broadened regex still matches the original `git commit`,
    // and low confidence only warns.
    let dir = tempfile::tempdir().unwrap();
    enable_confidence(dir.path());
    write_rules(dir.path());
    let (code, stderr) = run_hook("pre-check", &bash_payload("git commit -m 'x'"), dir.path());
    assert_eq!(
        code, 1,
        "git commit must warn (not block) at low; stderr: {stderr}"
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
    assert_eq!(
        code, 1,
        "git merge must warn (not block) at low; stderr: {stderr}"
    );
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
    assert_eq!(
        code, 1,
        "git rebase must warn (not block) at low; stderr: {stderr}"
    );
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
        code, 1,
        "git cherry-pick must warn (not block) at low; stderr: {stderr}"
    );
    assert!(stderr.contains("Low confidence"));
}

#[test]
fn gate_fires_on_git_revert() {
    let dir = tempfile::tempdir().unwrap();
    enable_confidence(dir.path());
    write_rules(dir.path());
    let (code, stderr) = run_hook("pre-check", &bash_payload("git revert HEAD"), dir.path());
    assert_eq!(
        code, 1,
        "git revert must warn (not block) at low; stderr: {stderr}"
    );
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
    assert_eq!(
        code, 1,
        "git pull must warn (not block) at low; stderr: {stderr}"
    );
    assert!(stderr.contains("Low confidence"));
}

#[test]
fn low_confidence_never_exits_2_but_unrelated_block_still_does() {
    // Explicit proof (SPEC-structural-rule-migration §"Confidence gate
    // severity"): low confidence must never block, but an unrelated `block`
    // rule firing on the same command must still exit 2.
    let dir = tempfile::tempdir().unwrap();
    enable_confidence(dir.path());
    write_rules_with_unrelated_block(dir.path());
    let (code, stderr) = run_hook("pre-check", COMMIT_PAYLOAD, dir.path());
    assert_eq!(
        code, 2,
        "an unrelated blocking rule must still exit 2; stderr: {stderr}"
    );
    assert!(stderr.contains("unrelated blocking rule fired"));
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
