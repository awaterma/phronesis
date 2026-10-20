//! A broken installed pack fails closed like a broken `rules.json`, but it
//! must stay repairable through the hooks: the error names the broken pack,
//! and the pack CLI (`phr-mcp pack install|remove|list|validate`) is the one
//! command a load error admits. Editing the pack in place cannot repair it,
//! because the recorded install digest treats that as tampering. Also covers
//! `pack remove` itself and the install lock's refusal of a symlink.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use phronesis_mcp::language_packs::install::{install, remove};
use serde_json::{Value, json};

/// The hook's own binary: the repair exception admits only it.
const BIN: &str = env!("CARGO_BIN_EXE_phr-mcp");

/// The project root as the repair command must name it: absolute.
fn project(root: &Path) -> String {
    root.canonicalize().unwrap().display().to_string()
}

fn manifest(id: &str, ext: &str) -> Value {
    json!({
        "schema_version": 1,
        "adapter_api": 1,
        "id": id,
        "version": "0.1.0",
        "language": id,
        "extensions": [ext],
        "grammar": "python",
        "capabilities": ["structural_facts"],
        "queries": [{"id": "decls", "path": "queries/decls.scm", "emit": [
            {"kind": "declaration", "name": "${name}", "scope": "", "span_capture": "def"}
        ]}],
        "context_files": [],
        "fixtures": []
    })
}

fn write(dir: &Path, rel: &str, bytes: &[u8]) {
    let path = dir.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

/// A governed project with one installed pack `demo_a`.
fn project_with_pack() -> (tempfile::TempDir, tempfile::TempDir) {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), ".phronesis/rules.json", br#"{"rules": []}"#);
    write(root.path(), "README.md", b"hello\n");
    let src = tempfile::tempdir().unwrap();
    write(
        src.path(),
        "queries/decls.scm",
        b"(function_definition) @def",
    );
    write(
        src.path(),
        "pack.json",
        manifest("demo_a", "demoa").to_string().as_bytes(),
    );
    install(root.path(), src.path(), false).unwrap();
    (root, src)
}

fn break_pack(root: &Path) {
    fs::write(root.join(".phronesis/packs/demo_a/pack.json"), b"{broken").unwrap();
}

fn phr(root: &Path, args: &[&str], stdin: Option<&str>) -> (i32, String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(args)
        .current_dir(root)
        .env("PHRONESIS_PROJECT_ROOT", root)
        .env("PHRONESIS_NO_ACTION_LOG", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        use std::io::Write;
        let mut pipe = child.stdin.take().unwrap();
        pipe.write_all(stdin.unwrap_or("").as_bytes()).unwrap();
    }
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn pre_edit(root: &Path, file: &str) -> (i32, String) {
    let payload = json!({"tool_name": "Edit", "tool_input": {
        "file_path": root.join(file).display().to_string(),
        "old_string": "hello", "new_string": "hi"}});
    let (code, _, err) = phr(root, &["pre-check"], Some(&payload.to_string()));
    (code, err)
}

fn pre_bash(root: &Path, command: &str) -> (i32, String) {
    let payload = json!({"tool_name": "Bash", "tool_input": {"command": command}});
    let (code, _, err) = phr(root, &["pre-check"], Some(&payload.to_string()));
    (code, err)
}

#[test]
fn a_broken_installed_pack_names_itself_and_admits_only_the_pack_cli() {
    let (root, _src) = project_with_pack();
    let root = root.path();
    break_pack(root);

    // Fails closed project-wide, naming the broken pack and its repair.
    let (code, err) = pre_edit(root, "README.md");
    assert_eq!(code, 2, "{err}");
    assert!(err.contains(".phronesis/packs/demo_a"), "{err}");
    assert!(err.contains("pack remove demo_a --path"), "{err}");

    // Not a repair: a raw delete, or the pack CLI chained with anything.
    assert_eq!(pre_bash(root, "rm -rf .phronesis/packs/demo_a").0, 2);
    assert_eq!(
        pre_bash(root, "phr-mcp pack remove demo_a && rm -rf src").0,
        2
    );
    assert_eq!(pre_bash(root, "echo phr-mcp pack remove demo_a").0, 2);
    // Editing the broken manifest cannot repair it (the install digest
    // would reject the new bytes), so it is not admitted either.
    assert_eq!(pre_edit(root, ".phronesis/packs/demo_a/pack.json").0, 2);

    // A file merely named `phr-mcp` is not the hook's own binary, and the
    // pack CLI cannot be pointed at another project's pack store.
    fs::write(root.join("phr-mcp"), b"#!/bin/sh\necho ran\n").unwrap();
    assert_eq!(pre_bash(root, "./phr-mcp pack list").0, 2);
    let other = tempfile::tempdir().unwrap();
    assert_eq!(
        pre_bash(
            root,
            &format!("{BIN} pack remove victim --path {}", other.path().display())
        )
        .0,
        2
    );

    // The repair: this binary's pack CLI alone, allowed with a warning.
    // Relative forms resolve in the shell, not the hook: refused.
    assert_eq!(pre_bash(root, &format!("{BIN} pack remove demo_a")).0, 2);
    assert_eq!(
        pre_bash(root, &format!("{BIN} pack remove demo_a --path .")).0,
        2
    );
    let (code, err) = pre_bash(
        root,
        &format!("{BIN} pack remove demo_a --path {}", project(root)),
    );
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("WARNING"), "{err}");
    assert_eq!(pre_bash(root, &format!("{BIN} pack list --json")).0, 1);

    // Running it removes the pack, and governance resumes.
    let (code, out, err) = phr(root, &["pack", "remove", "demo_a"], None);
    assert_eq!(code, 0, "{out}{err}");
    assert!(!root.join(".phronesis/packs/demo_a").exists());
    assert_eq!(pre_edit(root, "README.md").0, 0);
}

#[test]
fn pack_remove_removes_only_an_installed_pack_directory() {
    let (root, _src) = project_with_pack();
    let root = root.path();

    // Not installed, a built-in, or not an identifier: refused, nothing touched.
    assert!(remove(root, "nope").is_err());
    assert!(remove(root, "go").is_err());
    assert!(remove(root, "../demo_a").is_err());
    assert!(remove(root, "").is_err());
    assert!(root.join(".phronesis/packs/demo_a/pack.json").exists());

    // A symlinked pack directory is refused rather than followed.
    #[cfg(unix)]
    {
        let outside = tempfile::tempdir().unwrap();
        write(outside.path(), "keep.txt", b"keep");
        std::os::unix::fs::symlink(outside.path(), root.join(".phronesis/packs/linked")).unwrap();
        assert!(remove(root, "linked").is_err());
        assert!(outside.path().join("keep.txt").exists());
        fs::remove_file(root.join(".phronesis/packs/linked")).unwrap();
    }

    // An intact pack and a broken one are both removable.
    remove(root, "demo_a").unwrap();
    assert!(!root.join(".phronesis/packs/demo_a").exists());
}

#[cfg(unix)]
#[test]
fn the_install_lock_never_follows_a_symlink() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("created-by-install");
    fs::create_dir_all(root.path().join(".phronesis/packs")).unwrap();
    std::os::unix::fs::symlink(&target, root.path().join(".phronesis/packs/.install.lock"))
        .unwrap();

    let src = tempfile::tempdir().unwrap();
    write(
        src.path(),
        "queries/decls.scm",
        b"(function_definition) @def",
    );
    write(
        src.path(),
        "pack.json",
        manifest("demo_a", "demoa").to_string().as_bytes(),
    );
    let err = install(root.path(), src.path(), false).unwrap_err();
    assert!(err.to_string().contains("install.lock"), "{err}");
    assert!(
        !target.exists(),
        "the lock followed a symlink out of the project"
    );
    assert!(!root.path().join(".phronesis/packs/demo_a").exists());
}

#[test]
fn codex_admits_only_the_pack_cli_while_a_pack_is_broken() {
    let (root, _src) = project_with_pack();
    let root = root.path();
    break_pack(root);
    let codex = |command: &str| -> Value {
        let payload = json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "session_id": "s",
            "turn_id": "t",
            "tool_use_id": "u",
            "tool_input": {"command": command}
        });
        let (code, out, err) = phr(
            root,
            &["codex-hook", "PreToolUse"],
            Some(&payload.to_string()),
        );
        assert_eq!(code, 0, "Codex decisions exit 0: {err}");
        serde_json::from_str(&out).unwrap_or(Value::Null)
    };
    let denied = codex("rm -rf .phronesis/packs/demo_a");
    assert_eq!(
        denied["hookSpecificOutput"]["permissionDecision"], "deny",
        "{denied}"
    );
    assert!(
        denied["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .is_some_and(|r| r.contains("pack remove demo_a --path")),
        "{denied}"
    );
    let allowed = codex(&format!(
        "{BIN} pack remove demo_a --path {}",
        project(root)
    ));
    assert_ne!(
        allowed["hookSpecificOutput"]["permissionDecision"], "deny",
        "{allowed}"
    );
}
