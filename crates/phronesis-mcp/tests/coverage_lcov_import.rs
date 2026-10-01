use phronesis_mcp::properties::execute::artifact_sha256;
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

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
fn phr(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .current_dir(root)
        .args(["coverage", "import"])
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn lcov_directory_imports_python_body_hits_and_refuses_revision_mismatch() {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lcov/python-store");
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    copy_dir(&fixture, root);
    std::fs::create_dir_all(root.join(".phronesis")).unwrap();
    std::fs::write(root.join(".phronesis/rules.json"), r#"{"rules":[]}"#).unwrap();
    git(root, &["init", "-q"]);
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "fixture"]);
    let rev = String::from_utf8(
        Command::new("git")
            .current_dir(root)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    let digest = artifact_sha256(&std::fs::read(root.join("pkg/store.py")).unwrap());
    std::fs::write(
        root.join("cov/manifest.json"),
        serde_json::json!({"revision":rev,"files":{"pkg/store.py":digest}}).to_string(),
    )
    .unwrap();
    let out = phr(
        root,
        &[
            "--format",
            "lcov-dir",
            "--tool",
            "coverage.py",
            "--allow-dirty",
            "cov",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let store = std::fs::read_to_string(root.join(".phronesis/coverage.jsonl")).unwrap();
    assert!(store.contains("fn:pkg/store.py::load"), "{store}");
    assert!(!store.contains("fn:pkg/store.py::save"), "{store}");
    assert!(
        store.contains("python:pkg::tests::test_store::test_load"),
        "{store}"
    );
    let before = store;
    std::fs::write(
        root.join("cov/manifest.json"),
        r#"{"revision":"0000000000000000000000000000000000000000","files":{}}"#,
    )
    .unwrap();
    let out = phr(
        root,
        &[
            "--format",
            "lcov-dir",
            "--tool",
            "coverage.py",
            "--allow-dirty",
            "cov",
        ],
    );
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("revision"));
    assert_eq!(
        std::fs::read_to_string(root.join(".phronesis/coverage.jsonl")).unwrap(),
        before
    );
}

#[test]
fn lcov_directory_imports_swift_body_hits_and_reports_one_liners_unattributable() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lcov/swift-store");
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    copy_dir(&fixture, root);
    std::fs::create_dir_all(root.join(".phronesis")).unwrap();
    std::fs::write(root.join(".phronesis/rules.json"), r#"{"rules":[]}"#).unwrap();
    git(root, &["init", "-q"]);
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "fixture"]);
    let rev = String::from_utf8(
        Command::new("git")
            .current_dir(root)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    let digest = artifact_sha256(&std::fs::read(root.join("Sources/Store/Store.swift")).unwrap());
    std::fs::write(
        root.join("cov/manifest.json"),
        serde_json::json!({"revision":rev,"files":{"Sources/Store/Store.swift":digest}})
            .to_string(),
    )
    .unwrap();
    let out = phr(
        root,
        &[
            "--format",
            "lcov-dir",
            "--tool",
            "swift-cov",
            "--allow-dirty",
            "cov",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let store = std::fs::read_to_string(root.join(".phronesis/coverage.jsonl")).unwrap();
    assert!(
        store.contains("fn:Sources/Store/Store.swift::Store::load"),
        "{store}"
    );
    assert!(
        store.contains("swift:StoreTests::StoreTests::StoreTests::testLoad"),
        "{store}"
    );
    assert!(
        !store.contains("Store::oneLiner"),
        "a one-line Swift function is never attributed: {store}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        stderr.contains("unattributable") && stderr.contains("Store::oneLiner"),
        "{stderr}"
    );
}
#[test]
fn lcov_directory_imports_typescript_body_hits_and_reports_the_one_liner_without_fnda() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lcov/ts-store");
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    copy_dir(&fixture, root);
    std::fs::create_dir_all(root.join(".phronesis")).unwrap();
    std::fs::write(root.join(".phronesis/rules.json"), r#"{"rules":[]}"#).unwrap();
    git(root, &["init", "-q"]);
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "fixture"]);
    let rev = String::from_utf8(
        Command::new("git")
            .current_dir(root)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    let digest = artifact_sha256(&std::fs::read(root.join("src/store.ts")).unwrap());
    std::fs::write(
        root.join("cov/manifest.json"),
        serde_json::json!({"revision":rev,"files":{"src/store.ts":digest}}).to_string(),
    )
    .unwrap();
    let out = phr(
        root,
        &[
            "--format",
            "lcov-dir",
            "--tool",
            "c8+vitest",
            "--allow-dirty",
            "cov",
        ],
    );
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "{stderr}");
    let store = std::fs::read_to_string(root.join(".phronesis/coverage.jsonl")).unwrap();
    // The body function is attributed from its executed body lines, and the
    // record carries the graph's defines_test id (real shape: only a
    // literal trailing `.ts` leaves the module segment).
    assert!(store.contains("fn:src/store.ts::Store::load"), "{store}");
    assert!(
        store.contains("typescript:ts-store#test:store.test.ts::tests::store.test::Store loads"),
        "{store}"
    );
    // Review Focus 1 pins both one-liner shapes. `oneLiner`: its
    // declaration line has a positive `DA` (module evaluation marks it),
    // but its `FNDA` is zero, a legitimate negative: not a hit, and not
    // "unattributable" either (the function was instrumented and simply
    // not executed).
    assert!(!store.contains("oneLiner"), "{store}");
    // `missingFnda`: a one-liner `FN` entry with no `FNDA` record at all,
    // only a positive declaration-line `DA`. The plan's acceptance: it is
    // never attributed from that `DA` (imported as not-hit, reported
    // under `unattributable`), never guessed.
    assert!(!store.contains("missingFnda"), "{store}");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(stdout.contains("1 unattributable"), "{stdout}{stderr}");
    assert!(
        stderr.contains("src/store.ts::missingFnda: one-line function, no FNDA (typescript)"),
        "{stdout}{stderr}"
    );
}

#[test]
fn distinct_test_sections_are_rejected_without_replacing_existing_evidence() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    std::fs::create_dir_all(root.join("pkg")).expect("sources");
    std::fs::write(root.join("pkg/store.py"), "def load():\n    return 7\n").expect("source");
    std::fs::write(root.join(".gitignore"), ".phronesis/\n").expect("ignore");
    git(root, &["init", "-q"]);
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "fixture"]);
    let revision = String::from_utf8(
        Command::new("git")
            .current_dir(root)
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("HEAD")
            .stdout,
    )
    .expect("utf8");
    let dir = root.join(".phronesis/reports");
    std::fs::create_dir_all(&dir).expect("reports");
    let digest = artifact_sha256(&std::fs::read(root.join("pkg/store.py")).expect("source"));
    std::fs::write(
        dir.join("manifest.json"),
        serde_json::json!({"revision":revision.trim(),"files":{"pkg/store.py":digest}}).to_string(),
    )
    .expect("manifest");
    let good =
        "TN:python:project::tests::test_store::test_load\nSF:pkg/store.py\nDA:2,1\nend_of_record\n";
    std::fs::write(dir.join("coverage.lcov"), good).expect("report");
    let args = [
        "--format",
        "lcov-dir",
        "--tool",
        "coverage.py",
        ".phronesis/reports",
    ];
    let first = phr(root, &args);
    assert!(first.status.success(), "{first:?}");
    let paths = [
        root.join(".phronesis/coverage.jsonl"),
        root.join(".phronesis/coverage.index"),
    ];
    let before: Vec<_> = paths
        .iter()
        .map(|path| std::fs::read(path).expect("evidence"))
        .collect();
    std::fs::write(dir.join("coverage.lcov"),format!("{good}TN:python:project::tests::test_store::test_other\nSF:pkg/store.py\nDA:2,1\nend_of_record\n")).expect("mixed report");
    let mixed = phr(root, &args);
    assert!(
        !mixed.status.success(),
        "mixed TN sections were attributed to the last test: {mixed:?}"
    );
    let error = String::from_utf8_lossy(&mixed.stderr);
    assert!(
        error.contains("distinct TN")
            && error.contains("test_load")
            && error.contains("test_other"),
        "{error}"
    );
    for (path, expected) in paths.iter().zip(before) {
        assert_eq!(
            std::fs::read(path).expect("evidence"),
            expected,
            "failed import changed {}",
            path.display()
        );
    }
}
