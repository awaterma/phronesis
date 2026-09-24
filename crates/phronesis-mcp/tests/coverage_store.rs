use phronesis_mcp::coverage::store::{
    load_hits, load_index, write_store, CoverageIndex, HitRecord, COVERAGE_FORMAT,
};

fn temp_root() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn hit(test: &str, region: &str, rev: &str) -> HitRecord {
    HitRecord {
        v: COVERAGE_FORMAT,
        kind: "hit".into(),
        test: test.into(),
        region: region.into(),
        file: "src/lib.rs".into(),
        start_line: 1,
        end_line: 7,
        hit_kind: "region".into(),
        revision: rev.into(),
        tool: "cargo-llvm-cov".into(),
    }
}

#[test]
fn round_trips_records_and_index() {
    let root = temp_root();
    let idx = CoverageIndex {
        format: COVERAGE_FORMAT,
        revision: "a".repeat(40),
        imported_at: 1,
        tool: "cargo-llvm-cov".into(),
    };
    write_store(root.path(), &[hit("t1", "fn:safe_divide", &"a".repeat(40))], &idx).unwrap();
    assert_eq!(load_index(root.path()), Some(idx));
    assert_eq!(load_hits(root.path()).unwrap().len(), 1);
}

#[test]
fn replace_per_import_drops_prior_revision() {
    let root = temp_root();
    let idx = |rev: &str| CoverageIndex {
        format: COVERAGE_FORMAT,
        revision: rev.into(),
        imported_at: 2,
        tool: "cargo-llvm-cov".into(),
    };
    write_store(
        root.path(),
        &[hit("t1", "fn:safe_divide", &"a".repeat(40))],
        &idx(&"a".repeat(40)),
    )
    .unwrap();
    write_store(
        root.path(),
        &[hit("t2", "fn:safe_divide", &"b".repeat(40))],
        &idx(&"b".repeat(40)),
    )
    .unwrap();
    let hits = load_hits(root.path()).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].test, "t2");
}

#[test]
fn missing_store_loads_none_and_empty() {
    let root = temp_root();
    assert_eq!(load_index(root.path()), None);
    assert!(load_hits(root.path()).unwrap().is_empty());
}