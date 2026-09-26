//! `phr-mcp coverage collect` stamps HEAD on the evidence it writes, so it
//! must refuse when tracked files differ from HEAD (SPEC-coverage-evidence
//! §3.1: evidence is keyed to the revision that produced the observation),
//! unless `--allow-dirty` is given. Exercised through `--from-dir`, which
//! normalizes pre-collected exports and so needs no cargo-llvm-cov.

use std::path::Path;
use std::process::{Command, Output};

const SRC: &str = "pub fn safe_divide(n: i32, d: i32) -> Option<i32> {\n    if d == 0 {\n        return None;\n    }\n    Some(n / d)\n}\n";
const REL: &str = "crates/app/src/lib.rs";

fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("run git");
    assert!(out.status.success(), "git {args:?} failed: {out:?}");
}

/// A committed repo holding one covered source file.
fn repo() -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("tempdir");
    git(root.path(), &["init", "-q"]);
    git(root.path(), &["config", "user.email", "test@test.com"]);
    git(root.path(), &["config", "user.name", "Test"]);
    git(root.path(), &["config", "commit.gpgsign", "false"]);
    let file = root.path().join(REL);
    std::fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");
    std::fs::write(&file, SRC).expect("write source");
    git(root.path(), &["add", "."]);
    git(root.path(), &["commit", "-q", "-m", "initial"]);
    root
}

/// A `--from-dir` holding one llvm-cov export that hits `safe_divide`.
fn exports() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        r#"{{"data": [{{"functions": [{{"name": "app::safe_divide", "count": 1, "regions": [[1, 0, 6, 1, 1]], "branches": [], "filenames": ["{REL}"]}}]}}]}}"#
    );
    std::fs::write(dir.path().join("cov-app-divides.json"), json).expect("write export");
    dir
}

fn collect(root: &Path, from: &Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(["coverage", "collect", "--from-dir"])
        .arg(from)
        .arg("--path")
        .arg(root)
        .args(extra)
        .output()
        .expect("run phr-mcp coverage collect")
}

fn store_exists(root: &Path) -> bool {
    root.join(".phronesis/coverage.jsonl").exists()
        || root.join(".phronesis/coverage.index").exists()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn collect_on_clean_tree_imports_without_warning() {
    let root = repo();
    let from = exports();
    let out = collect(root.path(), from.path(), &[]);
    assert!(
        out.status.success(),
        "clean collect failed: {}",
        stderr(&out)
    );
    assert!(
        store_exists(root.path()),
        "clean collect must write the store"
    );
    assert!(
        !stderr(&out).contains("--allow-dirty"),
        "clean tree must not warn: {}",
        stderr(&out)
    );
}

#[test]
fn collect_refuses_unstaged_changes_and_writes_nothing() {
    let root = repo();
    std::fs::write(root.path().join(REL), format!("{SRC}// edit\n")).expect("edit");
    let from = exports();
    let out = collect(root.path(), from.path(), &[]);
    assert!(
        !out.status.success(),
        "dirty collect must exit non-zero: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let err = stderr(&out);
    assert!(err.contains(REL), "refusal must name the dirty path: {err}");
    assert!(
        err.contains("--allow-dirty"),
        "refusal must name the override: {err}"
    );
    assert!(
        !store_exists(root.path()),
        "refusal must not write the store"
    );
}

#[test]
fn collect_refuses_staged_changes() {
    let root = repo();
    std::fs::write(root.path().join(REL), format!("{SRC}// edit\n")).expect("edit");
    git(root.path(), &["add", REL]);
    let from = exports();
    let out = collect(root.path(), from.path(), &[]);
    assert!(!out.status.success(), "staged change must refuse");
    assert!(stderr(&out).contains(REL), "{}", stderr(&out));
    assert!(!store_exists(root.path()));
}

#[test]
fn collect_ignores_untracked_files_and_phronesis_state() {
    let root = repo();
    std::fs::write(root.path().join("notes.txt"), "scratch").expect("untracked");
    // Tracked tool state that hooks rewrite mid-session is not evidence input.
    std::fs::create_dir_all(root.path().join(".phronesis")).expect("mkdir");
    std::fs::write(root.path().join(".phronesis/context.json"), "{}").expect("state");
    git(root.path(), &["add", ".phronesis/context.json"]);
    git(root.path(), &["commit", "-q", "-m", "state"]);
    std::fs::write(root.path().join(".phronesis/context.json"), "{\"x\":1}").expect("edit");
    let from = exports();
    let out = collect(root.path(), from.path(), &[]);
    assert!(
        out.status.success(),
        "untracked files and .phronesis/ state must not block: {}",
        stderr(&out)
    );
}

#[test]
fn collect_allow_dirty_proceeds_with_warning() {
    let root = repo();
    std::fs::write(root.path().join(REL), format!("{SRC}// edit\n")).expect("edit");
    let from = exports();
    let out = collect(root.path(), from.path(), &["--allow-dirty"]);
    assert!(
        out.status.success(),
        "--allow-dirty must proceed: {}",
        stderr(&out)
    );
    assert!(
        store_exists(root.path()),
        "--allow-dirty must write the store"
    );
    let err = stderr(&out);
    assert!(err.contains("warning"), "must warn: {err}");
    assert!(err.contains(REL), "warning must name the dirty path: {err}");
}
