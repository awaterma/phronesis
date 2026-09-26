use phronesis_mcp::coverage::import::import_export;
use phronesis_mcp::coverage::store::{COVERAGE_FORMAT, HitRecord, load_hits, load_index};

fn rec(test: &str, region: &str, file: &str, rev: &str) -> HitRecord {
    HitRecord {
        v: COVERAGE_FORMAT,
        kind: "hit".into(),
        test: test.into(),
        region: region.into(),
        file: file.into(),
        start_line: 1,
        end_line: 7,
        hit_kind: "region".into(),
        revision: rev.into(),
        tool: "cargo-llvm-cov".into(),
    }
}

fn write_export(dir: &std::path::Path, lines: &[String]) -> std::path::PathBuf {
    let p = dir.join("export.jsonl");
    std::fs::write(&p, lines.join("\n") + "\n").unwrap();
    p
}

fn jsonl(recs: &[HitRecord]) -> Vec<String> {
    recs.iter()
        .map(|r| serde_json::to_string(r).unwrap())
        .collect()
}

#[test]
fn test_import_happy_path_writes_store_and_index() {
    let root = tempfile::tempdir().unwrap();
    let expdir = tempfile::tempdir().unwrap();
    let rev = "a".repeat(40);
    let export = write_export(
        expdir.path(),
        &jsonl(&[
            rec("test_a", "fn:foo", "src/lib.rs", &rev),
            rec("test_b", "fn:bar", "src/main.rs", &rev),
        ]),
    );

    let summary = import_export(root.path(), &export, 1_234_567_890).unwrap();
    assert_eq!(summary.records, 2);
    assert_eq!(summary.tests, 2);
    assert_eq!(summary.revision, rev);

    let hits = load_hits(root.path()).unwrap();
    assert_eq!(hits.len(), 2);
    let index = load_index(root.path()).unwrap();
    assert_eq!(index.revision, rev);
    assert_eq!(index.imported_at, 1_234_567_890);
}

#[test]
fn test_import_rejects_malformed_line() {
    let root = tempfile::tempdir().unwrap();
    let expdir = tempfile::tempdir().unwrap();
    let rev = "a".repeat(40);
    let lines = vec![
        serde_json::to_string(&rec("t", "fn:foo", "src/lib.rs", &rev)).unwrap(),
        "not json".to_string(),
    ];
    let export = write_export(expdir.path(), &lines);

    let err = import_export(root.path(), &export, 1)
        .unwrap_err()
        .to_string();
    assert!(err.contains("malformed JSON"), "got: {err}");
    // All-or-nothing: nothing was written.
    assert_eq!(load_index(root.path()), None);
    assert!(load_hits(root.path()).unwrap().is_empty());
}

#[test]
fn test_import_rejects_absolute_file_path() {
    let root = tempfile::tempdir().unwrap();
    let expdir = tempfile::tempdir().unwrap();
    let rev = "a".repeat(40);
    let export = write_export(
        expdir.path(),
        &jsonl(&[rec("t", "fn:foo", "/abs/lib.rs", &rev)]),
    );

    let err = import_export(root.path(), &export, 1)
        .unwrap_err()
        .to_string();
    assert!(err.contains("repo-relative"), "got: {err}");
    assert_eq!(load_index(root.path()), None);
}

#[test]
fn test_import_rejects_mixed_revisions() {
    let root = tempfile::tempdir().unwrap();
    let expdir = tempfile::tempdir().unwrap();
    let export = write_export(
        expdir.path(),
        &jsonl(&[
            rec("t1", "fn:foo", "src/lib.rs", &"a".repeat(40)),
            rec("t2", "fn:foo", "src/lib.rs", &"b".repeat(40)),
        ]),
    );

    let err = import_export(root.path(), &export, 1)
        .unwrap_err()
        .to_string();
    assert!(err.contains("mixes revisions"), "got: {err}");
    assert_eq!(load_index(root.path()), None);
}

#[test]
fn test_import_is_idempotent_per_revision() {
    let root = tempfile::tempdir().unwrap();
    let expdir = tempfile::tempdir().unwrap();
    let rev = "a".repeat(40);
    let export = write_export(
        expdir.path(),
        &jsonl(&[rec("t1", "fn:foo", "src/lib.rs", &rev)]),
    );

    let s1 = import_export(root.path(), &export, 1000).unwrap();
    let h1 = load_hits(root.path()).unwrap();
    let s2 = import_export(root.path(), &export, 2000).unwrap();
    let h2 = load_hits(root.path()).unwrap();

    assert_eq!(s1.records, s2.records);
    assert_eq!(s1.tests, s2.tests);
    assert_eq!(h1, h2);
}
