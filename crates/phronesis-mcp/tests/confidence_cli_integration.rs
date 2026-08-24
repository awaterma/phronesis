//! `phr-mcp confidence` CLI tests — the read-only band/signals report over
//! the journey journal (per-subject reads via outcome:* tags). 0.13.0
//! folds the standalone `.phronesis/outcomes/<subject>.jsonl` ledger into
//! the journey journal; the seeding helper writes the new shape.

use std::path::Path;
use std::process::Command;

fn run(args: &[&str], root: &Path) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(args)
        .env("PHRONESIS_PROJECT_ROOT", root)
        .output()
        .expect("run phr-mcp");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
    )
}

/// Seed the journey journal with one record per outcome tag, plus mint the
/// open subject. Each `tag` is an `outcome:*` string; the helper expands it
/// into a JournalRecord whose `subject` is set so `read_recent_subject`
/// returns the records.
fn seed_journey(root: &Path, subject: &str, set_current: bool, tags: &[&str]) {
    let phr = root.join(".phronesis");
    let outcomes = phr.join("outcomes");
    std::fs::create_dir_all(&outcomes).unwrap();
    if set_current {
        std::fs::write(outcomes.join("current"), subject).unwrap();
    }
    let journey = phr.join("journey");
    std::fs::create_dir_all(&journey).unwrap();
    let mut lines = Vec::new();
    for (i, tag) in tags.iter().enumerate() {
        let line = serde_json::json!({
            "v": 1,
            "ts": (i as u64) + 1,
            "sid": "s-test",
            "seq": (i as u64) + 1,
            "tool": "Bash",
            "path": "<cmd>",
            "tags": [tag],
            "subject": subject,
        })
        .to_string();
        lines.push(line);
    }
    let body = lines.join("\n") + "\n";
    std::fs::write(journey.join("events.jsonl"), body).unwrap();
}

#[test]
fn reports_band_and_signals_for_open_unit() {
    let dir = tempfile::tempdir().unwrap();
    seed_journey(
        dir.path(),
        "u",
        true,
        &["outcome:compile_ok", "outcome:test_pass"],
    );
    let (code, stdout) = run(&["confidence"], dir.path());
    assert_eq!(code, 0);
    assert!(stdout.contains("confidence: medium"), "stdout: {stdout}");
    assert!(
        stdout.contains("compile") && stdout.contains("tests"),
        "stdout: {stdout}"
    );
}

#[test]
fn json_output_is_machine_readable() {
    let dir = tempfile::tempdir().unwrap();
    seed_journey(dir.path(), "u", true, &["outcome:compile_ok"]);
    let (code, stdout) = run(&["confidence", "--json"], dir.path());
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(v["subject"], "u");
    assert_eq!(v["band"], "low"); // 1 signal
    assert_eq!(v["signals"][0], "compile");
}

#[test]
fn no_open_unit_is_reported_gracefully() {
    let dir = tempfile::tempdir().unwrap();
    let (code, stdout) = run(&["confidence"], dir.path());
    assert_eq!(code, 0);
    assert!(stdout.contains("No open work unit"), "stdout: {stdout}");
}

#[test]
fn subject_override_targets_a_specific_unit() {
    let dir = tempfile::tempdir().unwrap();
    // open unit is "u" (empty), and we query "other" — seed only its records.
    seed_journey(dir.path(), "other", false, &["outcome:compile_ok"]);
    // Mint "u" as the current open unit to mirror the original test shape.
    let outcomes = dir.path().join(".phronesis").join("outcomes");
    std::fs::write(outcomes.join("current"), "u").unwrap();
    let (code, stdout) = run(&["confidence", "--subject", "other", "--json"], dir.path());
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["subject"], "other");
    assert_eq!(v["signals"][0], "compile");
}

// ── `phr-mcp signal` — the explicit escape hatch ──────────────────────────

#[test]
fn signal_tests_pass_opens_a_unit_and_is_visible_in_confidence() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".phronesis")).unwrap();
    std::fs::write(dir.path().join(".phronesis/confidence.json"), "{}").unwrap();
    let (code, stdout) = run(&["signal", "tests", "pass"], dir.path());
    assert_eq!(code, 0, "stdout: {stdout}");
    let (code, stdout) = run(&["confidence", "--json"], dir.path());
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(v["signals"], serde_json::json!(["tests"]));
}

#[test]
fn signal_fail_retracts_an_earlier_pass() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".phronesis/confidence.json"), "{}").ok();
    std::fs::create_dir_all(dir.path().join(".phronesis")).unwrap();
    std::fs::write(dir.path().join(".phronesis/confidence.json"), "{}").unwrap();
    seed_journey(
        dir.path(),
        "u",
        true,
        &["outcome:compile_ok", "outcome:test_pass"],
    );
    let (code, _) = run(&["signal", "tests", "fail"], dir.path());
    assert_eq!(code, 0);
    let (_, stdout) = run(&["confidence", "--json"], dir.path());
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(v["subject"], "u");
    assert_eq!(v["signals"], serde_json::json!(["compile"]));
}

#[test]
fn signal_refuses_when_confidence_is_not_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(["signal", "tests", "pass"])
        .env("PHRONESIS_PROJECT_ROOT", dir.path())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(!dir.path().join(".phronesis/journey/events.jsonl").exists());
}

#[test]
fn signal_rejects_unknown_names() {
    let dir = tempfile::tempdir().unwrap();
    let (code, _) = run(&["signal", "vibes", "pass"], dir.path());
    assert_ne!(code, 0);
}
