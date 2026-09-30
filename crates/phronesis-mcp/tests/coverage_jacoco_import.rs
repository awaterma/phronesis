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
fn jacoco_directory_imports_constructor_hits_through_the_region_validator() {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/jacoco/java-store");
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    copy_dir(&fixture, root);
    std::fs::create_dir_all(root.join(".phronesis")).unwrap();
    std::fs::write(root.join(".phronesis/rules.json"), r#"{"rules":[]}"#).unwrap();
    git(root, &["init", "-q"]);
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "fixture"]);
    let out = phr(
        root,
        &[
            "--format",
            "jacoco-dir",
            "--tool",
            "jacoco+mvn",
            "--no-manifest",
            "cov",
        ],
    );
    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let store = std::fs::read_to_string(root.join(".phronesis/coverage.jsonl")).unwrap();
    assert!(
        store.contains("fn:core/src/main/java/com/x/Store.java::Store::new"),
        "{store}"
    );
    assert!(
        store.contains("fn:core/src/main/java/com/x/Store.java::Store::load"),
        "{store}"
    );
    assert!(
        store.contains("java:core::com::x::StoreTest::testLoad"),
        "{store}"
    );
}
