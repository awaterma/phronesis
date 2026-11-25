//! The reviewed repository presets remind agents before Rust source edits and
//! re-inject TDD guidance after recorded source creation/editing. Neither proves
//! that a test was written first.
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

fn run(root: &Path, args: &[&str], input: Option<&str>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_phr-mcp"));
    cmd.args(args)
        .current_dir(root)
        .env("PHRONESIS_PROJECT_ROOT", root)
        .env_remove("PHRONESIS_NO_JOURNEY")
        .env_remove("PHRONESIS_NO_ACTION_LOG")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    if let Some(input) = input {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    } else {
        drop(child.stdin.take());
    }
    child.wait_with_output().unwrap()
}

fn project() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let ph = root.path().join(".phronesis");
    std::fs::create_dir_all(ph.join("nudges")).unwrap();
    std::fs::create_dir_all(ph.join("journey")).unwrap();
    std::fs::write(ph.join("journey/session"), "tdd-tests").unwrap();
    for (path, body) in [
        (
            "rules.json",
            include_str!("../../../config/phronesis/rules.json"),
        ),
        (
            "journey.json",
            include_str!("../../../config/phronesis/journey.json"),
        ),
        (
            "context.json",
            include_str!("../../../config/phronesis/context.json"),
        ),
        (
            "nudges/tdd.md",
            include_str!("../../../config/phronesis/nudges/tdd.md"),
        ),
    ] {
        std::fs::write(ph.join(path), body).unwrap();
    }
    root
}

fn context(root: &Path) -> String {
    let out = run(root, &["interaction-context"], None);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    if out.stdout.is_empty() {
        return String::new(); // No active capsule or other context yet.
    }
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    value["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap()
        .to_string()
}

fn check_change(tool: &str, path: &str, warning: bool, capsule: bool) {
    let root = project();
    let file = root.path().join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    if tool == "Edit" {
        std::fs::write(&file, "old").unwrap();
    }
    assert!(
        !context(root.path()).contains("[phronesis nudge: test-driven-features-and-bugs]"),
        "no source activity yet"
    );
    let payload = serde_json::json!({
        "tool_name": tool,
        "tool_input": { "file_path": file, "old_string": "old", "new_string": "new", "content": "new" }
    }).to_string();
    let pre = run(root.path(), &["pre-check"], Some(&payload));
    assert_eq!(
        pre.status.code(),
        Some(if warning { 1 } else { 0 }),
        "{}",
        String::from_utf8_lossy(&pre.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&pre.stderr).contains("Use TDD for features and bug fixes"),
        warning
    );
    // Simulate the host completing this allowed edit, then report its post hook.
    std::fs::write(&file, "new").unwrap();
    let post = run(root.path(), &["post-check"], Some(&payload));
    assert!(
        post.status.success(),
        "{}",
        String::from_utf8_lossy(&post.stderr)
    );
    let rendered = context(root.path());
    assert_eq!(
        rendered.contains("[phronesis nudge: test-driven-features-and-bugs]"),
        capsule,
        "{rendered}"
    );
    if capsule {
        assert!(rendered.contains("write a focused behavioral or regression"));
        assert!(rendered.contains("does not prove"));
    }
}

#[test]
fn source_creation_warns_and_activates_tdd_capsule() {
    check_change("Write", "crates/example/src/new.rs", true, true);
}

#[test]
fn source_edit_warns_and_activates_tdd_capsule() {
    check_change("Edit", "crates/example/src/existing.rs", true, true);
}

#[test]
fn non_rust_source_creation_also_activates_capsule() {
    check_change("Write", "scripts/check.py", false, true);
    check_change("Write", "policy/check.rhai", false, true);
}

#[test]
fn documentation_changes_do_not_activate_tdd_capsule() {
    check_change("Write", "docs/note.md", false, false);
}

#[test]
fn writing_tests_activates_capsule_without_production_warning() {
    check_change("Write", "tests/regression.rs", false, true);
}
