use std::fs;
use std::process::Command;

fn phr() -> &'static str {
    env!("CARGO_BIN_EXE_phr-mcp")
}

#[test]
fn state_and_clean_report_and_remove_only_cache_files() {
    let dir = tempfile::tempdir().unwrap();
    let phr_dir = dir.path().join(".phronesis");
    fs::create_dir_all(&phr_dir).unwrap();
    fs::write(phr_dir.join("graph.jsonl"), "graph").unwrap();
    fs::write(phr_dir.join("rules.json"), "authored").unwrap();
    let state = Command::new(phr())
        .args(["state", "--path"])
        .arg(dir.path())
        .arg("--json")
        .output()
        .unwrap();
    assert!(state.status.success());
    let rows: serde_json::Value = serde_json::from_slice(&state.stdout).unwrap();
    assert!(
        rows.as_array()
            .unwrap()
            .iter()
            .any(|row| row["path"] == ".phronesis/graph.jsonl" && row["bytes"] == 5)
    );
    let clean = Command::new(phr())
        .args(["clean", "--cache", "--path"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(
        clean.status.success(),
        "{}",
        String::from_utf8_lossy(&clean.stderr)
    );
    assert!(String::from_utf8_lossy(&clean.stdout).contains("graph.jsonl"));
    assert!(!phr_dir.join("graph.jsonl").exists());
    assert!(phr_dir.join("rules.json").exists());
}

#[test]
fn migrate_durable_cli_reports_absent_target_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("durable.md");
    let out = Command::new(phr())
        .arg("migrate-durable")
        .arg(&path)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("does not exist"));
    assert!(!path.exists());
}

#[test]
fn metrics_cli_writes_a_scrape_and_accepts_since_cutoff() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join(".phronesis")).unwrap();
    fs::write(dir.path().join(".phronesis/log.jsonl"), "").unwrap();
    let outpath = dir.path().join("metrics.prom");
    let output = Command::new(phr())
        .current_dir(dir.path())
        .args(["metrics", "--since", "1d", "--out"])
        .arg(&outpath)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let scrape = fs::read_to_string(outpath).unwrap();
    assert!(scrape.contains("phronesis"));
}
