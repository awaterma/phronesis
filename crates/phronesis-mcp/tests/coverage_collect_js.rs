//! `phr-mcp coverage collect --tool js-cov`: the emitted collection script
//! must carry the graph's `defines_test` ids (real shape, `it()` title
//! included) and detect the runner from `package.json`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn copy_dir(src: &Path, dst: &Path) {
    for e in std::fs::read_dir(src).unwrap() {
        let e = e.unwrap();
        let p = e.path();
        let d = dst.join(e.file_name());
        if p.is_dir() {
            std::fs::create_dir_all(&d).unwrap();
            copy_dir(&p, &d);
        } else {
            std::fs::copy(p, d).unwrap();
        }
    }
}

fn git(root: &Path, args: &[&str]) {
    let s = Command::new("git")
        .current_dir(root)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .status()
        .unwrap();
    assert!(s.success());
}

#[test]
fn js_cov_emit_script_lists_graph_tests_with_runner_native_names() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lcov/ts-store");
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    copy_dir(&fixture, root);
    std::fs::create_dir_all(root.join(".phronesis")).unwrap();
    std::fs::write(root.join(".phronesis/rules.json"), r#"{"rules":[]}"#).unwrap();
    git(root, &["init", "-q"]);
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "fixture"]);
    let rebuild = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .current_dir(root)
        .args(["graph", "rebuild"])
        .output()
        .unwrap();
    assert!(
        rebuild.status.success(),
        "{}",
        String::from_utf8_lossy(&rebuild.stderr)
    );
    let emit = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .current_dir(root)
        .args(["coverage", "collect", "--tool", "js-cov", "--emit-script"])
        .output()
        .unwrap();
    assert!(
        emit.status.success(),
        "{}",
        String::from_utf8_lossy(&emit.stderr)
    );
    let script = String::from_utf8_lossy(&emit.stdout).to_string();
    assert!(script.starts_with("#!/bin/sh\nset -eu\n"), "{script}");
    // Entries come from the graph's `defines_test` edges: the TN carries
    // the real id shape (module segment keeps `.test`, title is the
    // `it()` string with no describe nesting), and the runner-native name
    // is the title after the test file's module marker.
    assert!(
        script
            .contains("TN:typescript:ts-store#test:store.test.ts::tests::store.test::Store loads"),
        "{script}"
    );
    assert!(script.contains("-t 'Store loads'"), "{script}");
    // The runner was detected from package.json (vitest in devDependencies).
    assert!(
        script.contains("npx vitest --version"),
        "runner detection from package.json: {script}"
    );
    assert!(script.contains("\"runner\": \"vitest\""), "{script}");
    // The manifest section must resolve real paths with the `path` module —
    // `os` is no longer even required (regression: `os.path.realpathSync`
    // threw TypeError in Node at manifest-writing time).
    assert!(script.contains("path.realpathSync"), "{script}");
    assert!(!script.contains("os.path."), "{script}");
    assert!(
        script.contains("phr-mcp coverage import --format lcov-dir --tool c8+vitest"),
        "{script}"
    );
}
