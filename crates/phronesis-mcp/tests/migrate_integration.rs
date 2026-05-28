use std::process::Command;

fn run_migrate(args: &[&str]) -> std::process::Output {
    let bin = env!("CARGO_BIN_EXE_phr-mcp");
    Command::new(bin)
        .arg("migrate-rules")
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn migrate_converts_v1_to_v2_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.json");
    std::fs::write(
        &path,
        r#"{ "rules": [
        { "id": "r1", "phase": "pre", "priority": 10,
          "conditions": [ {"predicate":"new_content_contains","args":[".unwrap()"]} ],
          "actions": [ {"action_type":"constraint_violation","params":["no unwrap"]} ] }
    ] }"#,
    )
    .unwrap();

    let out = run_migrate(&[path.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"when\""));
    assert!(text.contains("\"then\""));
    assert!(text.contains("\"block\""));
    assert!(!text.contains("\"action_type\""));
    // Backup preserved.
    assert!(path.with_extension("json.bak").exists());
}

#[test]
fn migrate_preserves_or_clauses() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.json");
    std::fs::write(&path, r#"{ "rules": [
        { "id": "r1", "phase": "pre", "priority": 5,
          "when": [ { "or": [ { "new_content_contains": "a" }, { "new_content_contains": "b" } ] } ],
          "then": { "warn": "m" } }
    ] }"#).unwrap();
    let out = run_migrate(&[path.to_str().unwrap()]);
    assert!(out.status.success());
    let text = std::fs::read_to_string(&path).unwrap();
    // OR is preserved on disk, NOT expanded.
    assert!(text.contains("\"or\""));
    assert!(!text.contains("#or0"));
}

#[test]
fn migrate_dry_run_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.json");
    let v1 = r#"{ "rules": [
        { "id": "r", "phase": "pre", "priority": 1,
          "conditions": [ {"predicate":"new_content_contains","args":["todo"]} ],
          "actions": [ {"action_type":"constraint_violation","params":["no todo"]} ] }
    ] }"#;
    std::fs::write(&path, v1).unwrap();
    let original = std::fs::read(&path).unwrap();

    let out = run_migrate(&["--dry-run", path.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // File on disk must be unchanged.
    let after = std::fs::read(&path).unwrap();
    assert_eq!(original, after, "dry-run must not modify the file");

    // stdout must contain the converted v2 form.
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("\"when\""));
    assert!(stdout.contains("\"then\""));
    assert!(stdout.contains("\"block\""));

    // No backup should have been created.
    assert!(!path.with_extension("json.bak").exists());
}

#[test]
fn migrate_idempotent_on_v2() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.json");
    std::fs::write(
        &path,
        r#"{ "rules": [
        { "id": "r", "phase": "pre", "priority": 1,
          "conditions": [ {"predicate":"new_content_contains","args":["todo"]} ],
          "actions": [ {"action_type":"constraint_violation","params":["no todo"]} ] }
    ] }"#,
    )
    .unwrap();

    // First run: v1 → v2.
    let out = run_migrate(&[path.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "first run stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let first = std::fs::read(&path).unwrap();

    // Second run: v2 → v2 (stable no-op rewrite).
    let out = run_migrate(&[path.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "second run stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let second = std::fs::read(&path).unwrap();

    assert_eq!(
        first, second,
        "re-migrating a v2 file must be byte-identical"
    );
}

#[test]
fn migrate_check_exit_codes() {
    let dir = tempfile::tempdir().unwrap();
    let v1 = dir.path().join("v1.json");
    std::fs::write(
        &v1,
        r#"{ "rules": [ { "id":"r","phase":"pre","priority":1,
        "conditions":[{"predicate":"p","args":["x"]}],
        "actions":[{"action_type":"log","params":["m"]}] } ] }"#,
    )
    .unwrap();
    let out = run_migrate(&["--check", v1.to_str().unwrap()]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "v1 file should report needs-migration"
    );

    let v2 = dir.path().join("v2.json");
    std::fs::write(
        &v2,
        r#"{ "rules": [ { "id":"r","phase":"pre","priority":1,
        "when":[{"p":"x"}], "then":{"log":"m"} } ] }"#,
    )
    .unwrap();
    let out = run_migrate(&["--check", v2.to_str().unwrap()]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "v2 file should report up-to-date"
    );
}
