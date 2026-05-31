use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn run_decision_new(project_root: &Path, slug: &str) -> Output {
    let bin = env!("CARGO_BIN_EXE_phr-mcp");
    Command::new(bin)
        .args(["decision", "new", slug])
        .current_dir(project_root)
        .output()
        .expect("spawn phr-mcp decision new")
}

#[test]
fn decision_new_creates_file_with_template() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join(".phronesis/wiki/decisions")).unwrap();

    let out = run_decision_new(dir.path(), "my-first-decision");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Find the created file (matches today's date + slug).
    let dec_dir = dir.path().join(".phronesis/wiki/decisions");
    let files: Vec<_> = fs::read_dir(&dec_dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("md"))
        .collect();
    assert_eq!(files.len(), 1);
    let name = files[0].file_name().into_string().unwrap();
    assert!(name.ends_with("-my-first-decision.md"));
    // Filename starts with an ISO date.
    assert!(
        name.chars()
            .take(10)
            .all(|c| c.is_ascii_digit() || c == '-')
    );

    let content = fs::read_to_string(files[0].path()).unwrap();
    assert!(content.starts_with("---\n"));
    assert!(content.contains("id: my-first-decision"));
    assert!(content.contains("status: proposed"));
    assert!(content.contains("## Context"));
    assert!(content.contains("## Decision"));
    assert!(content.contains("## Enforcement"));
}

#[test]
fn decision_new_refuses_to_overwrite_existing() {
    let dir = tempfile::tempdir().unwrap();
    let dec_dir = dir.path().join(".phronesis/wiki/decisions");
    fs::create_dir_all(&dec_dir).unwrap();

    // First run succeeds.
    let out1 = run_decision_new(dir.path(), "same-slug");
    assert!(out1.status.success());

    // Second run with same slug on the same day must refuse.
    let out2 = run_decision_new(dir.path(), "same-slug");
    assert!(!out2.status.success());
    let stderr = String::from_utf8_lossy(&out2.stderr);
    assert!(stderr.to_lowercase().contains("exists") || stderr.to_lowercase().contains("refuse"));
}

#[test]
fn decision_new_rejects_invalid_slug() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join(".phronesis/wiki/decisions")).unwrap();

    // Spaces are invalid (slugs are kebab-case).
    let out = run_decision_new(dir.path(), "has spaces");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.to_lowercase().contains("slug"));
}
