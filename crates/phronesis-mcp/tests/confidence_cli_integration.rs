//! `phr-mcp confidence` CLI tests — the read-only band/signals report over
//! `.phronesis/outcomes/`.

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

fn seed(root: &Path, subject: &str, entries: &[&str]) {
    let outcomes = root.join(".phronesis").join("outcomes");
    std::fs::create_dir_all(&outcomes).unwrap();
    std::fs::write(outcomes.join("current"), subject).unwrap();
    std::fs::write(
        outcomes.join(format!("{subject}.jsonl")),
        entries.join("\n"),
    )
    .unwrap();
}

#[test]
fn reports_band_and_signals_for_open_unit() {
    let dir = tempfile::tempdir().unwrap();
    seed(
        dir.path(),
        "u",
        &[
            r#"{"ts":0,"predicate":"build_outcome","args":["u","pass"]}"#,
            r#"{"ts":0,"predicate":"test_outcome","args":["u","9","0","9"]}"#,
        ],
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
    seed(
        dir.path(),
        "u",
        &[r#"{"ts":0,"predicate":"build_outcome","args":["u","pass"]}"#],
    );
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
    // open unit is "u", but we query "other"
    seed(dir.path(), "u", &[]);
    let outcomes = dir.path().join(".phronesis").join("outcomes");
    std::fs::write(
        outcomes.join("other.jsonl"),
        "{\"ts\":0,\"predicate\":\"build_outcome\",\"args\":[\"other\",\"pass\"]}",
    )
    .unwrap();
    let (code, stdout) = run(&["confidence", "--subject", "other", "--json"], dir.path());
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["subject"], "other");
    assert_eq!(v["signals"][0], "compile");
}
