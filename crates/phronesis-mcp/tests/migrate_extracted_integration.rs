use std::fs;
use std::process::Command;

fn run_cmd(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .arg("migrate-extracted-rules")
        .args(args)
        .output()
        .expect("failed to spawn phr-mcp")
}

const OLD_SHAPE: &str = r#"{
  "rules": [
    {
      "id": "rust-patterns-guide-anti-patterns-12",
      "phase": "pre",
      "priority": 5,
      "when": [ { "markdown_rule": ["docs/RUST-PATTERNS-GUIDE.md", "Anti-Patterns"] } ],
      "then": { "block": "[anti_pattern] Overuse of unwrap() panics in production." }
    },
    {
      "id": "rust-patterns-guide-idioms-1",
      "phase": "pre",
      "priority": 5,
      "when": [ { "markdown_rule": ["docs/RUST-PATTERNS-GUIDE.md", "Idioms"] } ],
      "then": { "block": "[pattern] Prefer iterators over index loops." }
    },
    {
      "id": "enforce-no-unwrap-in-src",
      "phase": "pre",
      "priority": 8,
      "when": [ { "new_content_contains": ".unwrap()" }, { "file_path_matches": "src" } ],
      "then": { "block": "No .unwrap() in src/." }
    }
  ]
}"#;

#[test]
fn migrates_extracted_rules_in_place_with_backup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.json");
    fs::write(&path, OLD_SHAPE).unwrap();

    let out = run_cmd(&[path.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let migrated = fs::read_to_string(&path).unwrap();
    // unwrap-keyword rule → log, prefix gone
    assert!(migrated.contains(r#""log": "Overuse of unwrap() panics in production.""#));
    // plain pattern rule → warn, prefix gone
    assert!(migrated.contains(r#""warn": "Prefer iterators over index loops.""#));
    // structural rule untouched (still block, message intact)
    assert!(migrated.contains(r#""block": "No .unwrap() in src/.""#));
    assert!(!migrated.contains("[pattern]"));
    assert!(!migrated.contains("[anti_pattern]"));
    // backup written
    assert!(path.with_extension("json.bak").exists());
}

#[test]
fn dry_run_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.json");
    fs::write(&path, OLD_SHAPE).unwrap();

    let out = run_cmd(&["--dry-run", path.to_str().unwrap()]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(r#""warn": "Prefer iterators over index loops.""#));
    assert_eq!(fs::read_to_string(&path).unwrap(), OLD_SHAPE);
    assert!(!path.with_extension("json.bak").exists());
}

#[test]
fn nothing_to_migrate_reports_and_exits_zero() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.json");
    fs::write(
        &path,
        r#"{"rules":[{"id":"x","phase":"pre","priority":5,"when":[{"new_content_contains":"todo!"}],"then":{"warn":"No todo!"}}]}"#,
    )
    .unwrap();

    let out = run_cmd(&[path.to_str().unwrap()]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("no extracted rules"));
    assert!(!path.with_extension("json.bak").exists());
}
