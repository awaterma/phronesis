use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn run_wiki_drift(project_root: &Path, args: &[&str]) -> Output {
    let bin = env!("CARGO_BIN_EXE_phr-mcp");
    Command::new(bin)
        .arg("wiki-drift")
        .args(args)
        .current_dir(project_root)
        .output()
        .expect("spawn phr-mcp wiki-drift")
}

fn fixture(rules_json: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let phr = dir.path().join(".phronesis");
    let dec = phr.join("wiki").join("decisions");
    fs::create_dir_all(&dec).unwrap();
    fs::write(phr.join("rules.json"), rules_json).unwrap();
    dir
}

#[test]
fn wiki_drift_table_lists_decision_buckets() {
    let dir = fixture(
        r#"{"rules":[{"id":"r","phase":"pre","priority":1,"when":[{"new_content_contains":"x"}],"then":{"warn":"m"}}]}"#,
    );
    let dec = dir.path().join(".phronesis/wiki/decisions");
    fs::write(
        dec.join("a.md"),
        "---\nid: a\ndate: 2026-05-29\nstatus: accepted\nenforces:\n  - r\n---\n",
    )
    .unwrap();
    fs::write(
        dec.join("b.md"),
        "---\nid: b\ndate: 2026-05-29\nstatus: accepted\n---\northogonal zzz\n",
    )
    .unwrap();

    let out = run_wiki_drift(dir.path(), &[]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("covered"));
    assert!(stdout.contains("uncovered"));
}

#[test]
fn wiki_drift_json_is_machine_readable() {
    let dir = fixture(r#"{"rules":[]}"#);
    fs::write(
        dir.path().join(".phronesis/wiki/decisions/x.md"),
        "---\nid: x\ndate: 2026-05-29\nstatus: accepted\n---\nbody\n",
    )
    .unwrap();
    let out = run_wiki_drift(dir.path(), &["--json"]);
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert!(v["items"].is_array());
    assert_eq!(v["items"][0]["id"], "x");
}

#[test]
fn wiki_drift_suggest_emits_draft_for_uncovered() {
    let dir = fixture(r#"{"rules":[]}"#);
    fs::write(
        dir.path().join(".phronesis/wiki/decisions/uncov.md"),
        "---\nid: my-decision\ndate: 2026-05-29\nstatus: accepted\n---\nimperative one-liner\n",
    )
    .unwrap();
    let out = run_wiki_drift(dir.path(), &["--suggest"]);
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("decision-my-decision"));
    assert!(stderr.contains("TODO"));
}

#[test]
fn wiki_drift_missing_dir_errors_clearly() {
    let dir = tempfile::tempdir().unwrap();
    // No .phronesis/wiki/decisions/ at all.
    let out = run_wiki_drift(dir.path(), &[]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.to_lowercase().contains("not found") || stderr.to_lowercase().contains("missing")
    );
}

#[test]
fn wiki_drift_override_wiki_dir_arg() {
    // Decisions live in a non-default location.
    let dir = fixture(r#"{"rules":[]}"#);
    let custom = dir.path().join("custom").join("decisions");
    fs::create_dir_all(&custom).unwrap();
    fs::write(
        custom.join("x.md"),
        "---\nid: x\ndate: 2026-05-29\nstatus: accepted\n---\n",
    )
    .unwrap();
    let out = run_wiki_drift(dir.path(), &["--wiki-dir", custom.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("x"));
}
