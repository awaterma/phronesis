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

#[test]
fn default_package_reports_resolve_within_the_selected_module_and_refuse_ambiguity() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    for module in ["core", "other"] {
        let directory = root.join(module).join("src/main/java");
        std::fs::create_dir_all(&directory).expect("sources");
        std::fs::write(
            directory.join("Store.java"),
            "public class Store {\n public int load() {\n  return 7;\n }\n}\n",
        )
        .expect("source");
    }
    std::fs::write(root.join(".gitignore"), ".phronesis/\n").expect("ignore");
    git(root, &["init", "-q"]);
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "fixture"]);
    let dir = root.join(".phronesis/reports/1");
    std::fs::create_dir_all(&dir).expect("report");
    std::fs::write(dir.join("jacoco.xml"),r#"<report><package name=""><sourcefile name="Store.java"><line nr="3" mi="0" ci="1"/></sourcefile></package></report>"#).expect("xml");
    std::fs::write(dir.join("module.txt"), "core").expect("module");
    std::fs::write(dir.join("TN"), "java:core::StoreTest::testLoad").expect("test");
    let args = [
        "--format",
        "jacoco-dir",
        "--tool",
        "jacoco+mvn",
        "--no-manifest",
        ".phronesis/reports",
    ];
    let output = phr(root, &args);
    assert!(output.status.success(), "{output:?}");
    let records = root.join(".phronesis/coverage.jsonl");
    let before = std::fs::read(&records).expect("records");
    let text = String::from_utf8_lossy(&before);
    assert!(
        text.contains("fn:core/src/main/java/Store.java::Store::load"),
        "{text}"
    );
    assert!(!text.contains("other/src/main/java"), "{text}");
    std::fs::write(dir.join("module.txt"), "missing").expect("unqualified");
    let output = phr(root, &args);
    assert!(
        !output.status.success(),
        "ambiguous fallback must fail: {output:?}"
    );
    assert_eq!(std::fs::read(records).expect("records"), before);
}
#[test]
fn jacoco_manifest_must_hash_every_resolved_source_before_replacing_evidence() {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/jacoco/java-store");
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    copy_dir(&fixture, root);
    std::fs::create_dir_all(root.join(".phronesis")).expect("state");
    std::fs::write(root.join(".phronesis/rules.json"), r#"{"rules":[]}"#).expect("rules");
    git(root, &["init", "-q"]);
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "fixture"]);
    let first = phr(
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
    assert!(first.status.success(), "{first:?}");
    let paths = [
        root.join(".phronesis/coverage.jsonl"),
        root.join(".phronesis/coverage.index"),
    ];
    let before: Vec<_> = paths
        .iter()
        .map(|path| std::fs::read(path).expect("evidence"))
        .collect();
    let revision = String::from_utf8(
        Command::new("git")
            .current_dir(root)
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("HEAD")
            .stdout,
    )
    .expect("utf8");
    std::fs::write(
        root.join("cov/manifest.json"),
        serde_json::json!({"revision":revision.trim(),"files":{}}).to_string(),
    )
    .expect("empty manifest");
    let output = phr(
        root,
        &[
            "--format",
            "jacoco-dir",
            "--tool",
            "jacoco+mvn",
            "--allow-dirty",
            "cov",
        ],
    );
    assert!(
        !output.status.success(),
        "missing source digest was accepted: {output:?}"
    );
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains(
            "manifest has no digest for covered source core/src/main/java/com/x/Store.java"
        ),
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
