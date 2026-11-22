//! `phr-mcp provision` — explicit local setup from tracked presets.
//!
//! Covers the acceptance criteria from the documentation restructure plan:
//! dry-run, idempotency, conflict reporting with preservation of
//! customized files, and the anchored whole-directory ignore surviving
//! repeated provisioning.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn run_provision(project_root: &Path, args: &[&str]) -> Output {
    let bin = env!("CARGO_BIN_EXE_phr-mcp");
    Command::new(bin)
        .arg("provision")
        .args(args)
        .current_dir(project_root)
        .output()
        .expect("spawn phr-mcp provision")
}

fn write_preset(root: &Path, rel: &str, content: &str) {
    let path = root.join("config/phronesis").join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

#[cfg(unix)]
#[test]
fn provision_preserves_linked_destinations_without_external_writes() {
    use std::os::unix::fs::symlink;
    for linked_parent in [false, true] {
        let project = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write_preset(project.path(), "custom.json", "preset");
        let state = project.path().join(".phronesis");
        if linked_parent {
            symlink(outside.path(), &state).unwrap();
        } else {
            fs::create_dir(&state).unwrap();
            symlink(
                outside.path().join("custom.json"),
                state.join("custom.json"),
            )
            .unwrap();
        }
        for flags in [&["--dry-run"][..], &[][..]] {
            let out = run_provision(project.path(), flags);
            assert!(!out.status.success());
            assert!(String::from_utf8_lossy(&out.stderr).contains("symbolic link"));
            assert!(!outside.path().join("custom.json").exists());
            assert!(!project.path().join(".gitignore").exists());
        }
    }
}

#[cfg(unix)]
#[test]
fn provision_rejects_preset_link_cycles_without_modifying_state() {
    use std::os::unix::fs::symlink;
    let project = tempfile::tempdir().unwrap();
    write_preset(project.path(), "kernel.md", "preset");
    symlink(".", project.path().join("config/phronesis/cycle")).unwrap();
    let out = run_provision(project.path(), &[]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("symbolic link"));
    assert!(!project.path().join(".phronesis").exists());
    assert!(!project.path().join(".gitignore").exists());
}

#[test]
fn provision_writes_missing_files_and_gitignore() {
    let dir = tempfile::tempdir().unwrap();
    write_preset(dir.path(), "kernel.md", "# kernel\n");
    write_preset(dir.path(), "nudges/README.md", "# nudges\n");

    let out = run_provision(dir.path(), &[]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        fs::read_to_string(dir.path().join(".phronesis/kernel.md")).unwrap(),
        "# kernel\n"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join(".phronesis/nudges/README.md")).unwrap(),
        "# nudges\n"
    );
    let gitignore = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    assert!(gitignore.contains("/.phronesis/"));
}

#[test]
fn provision_is_idempotent_and_reports_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    write_preset(dir.path(), "kernel.md", "# kernel\n");
    write_preset(dir.path(), "bugs.json", "[]\n");

    let first = run_provision(dir.path(), &[]);
    assert!(first.status.success());

    // Customize one local file after provisioning.
    fs::write(
        dir.path().join(".phronesis/kernel.md"),
        "locally customized\n",
    )
    .unwrap();

    let second_out = run_provision(dir.path(), &[]);
    assert!(second_out.status.success());
    let stdout = String::from_utf8_lossy(&second_out.stdout);
    assert!(
        stdout.contains("conflict") && stdout.contains("kept local"),
        "conflict must be reported: {stdout}"
    );
    assert!(
        stdout.contains("already matches its preset"),
        "untouched file is reported as identical: {stdout}"
    );
    // Customized files are never overwritten.
    assert_eq!(
        fs::read_to_string(dir.path().join(".phronesis/kernel.md")).unwrap(),
        "locally customized\n"
    );
    // The untouched file stays identical to its preset.
    assert_eq!(
        fs::read_to_string(dir.path().join(".phronesis/bugs.json")).unwrap(),
        "[]\n"
    );
}

#[test]
fn provision_dry_run_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    write_preset(dir.path(), "kernel.md", "# kernel\n");

    let out = run_provision(dir.path(), &["--dry-run"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("dry run") && stdout.contains("kernel.md"),
        "dry run must describe intended writes: {stdout}"
    );
    assert!(!dir.path().join(".phronesis").exists());
    assert!(!dir.path().join(".gitignore").exists());
}

#[test]
fn provision_preserves_anchored_ignore_across_repeated_runs() {
    let dir = tempfile::tempdir().unwrap();
    write_preset(dir.path(), "kernel.md", "# kernel\n");
    run_provision(dir.path(), &[]);
    let first = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    run_provision(dir.path(), &[]);
    let second = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    assert_eq!(
        first, second,
        "repeated provisioning must not rewrite .gitignore"
    );
    assert!(second.contains("/.phronesis/"));
    assert!(!second.contains("!.phronesis/"));
}

#[test]
fn provision_without_presets_reports_nothing_to_do() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_provision(dir.path(), &[]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("no presets found"),
        "missing presets are a no-op, not an error: {stdout}"
    );
    assert!(dir.path().join(".gitignore").exists());
}
