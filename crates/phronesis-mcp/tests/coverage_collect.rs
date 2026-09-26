//! Coverage collector tests: the collector's records must be exactly what
//! `region_map` independently produces — no reimplementation, no drift.
//! Golden anchor: the committed fixture's `cd6054b02dde` branch anchor.

use phronesis_mcp::coverage::collect::{
    LlvmCovDocument, collect_from_documents, leaf_ident, repo_rel,
};

/// Fixture source identical to the committed coverage-sample crate
/// (tests/fixtures/coverage-sample/src/lib.rs).
const FIXTURE_SRC: &str = r#"pub fn safe_divide(numerator: i32, denominator: i32) -> Result<i32, &'static str> {
    if denominator == 0 {
        return Err("division by zero");
    }

    Ok(numerator / denominator)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn divides_positive_values() {
        assert_eq!(safe_divide(8, 2), Ok(4));
    }
}
"#;

/// Build a minimal llvm-cov export (one function entry) as a JSON string.
fn doc_with_fn(name: &str, file: &str, count: u64, branches: &str) -> LlvmCovDocument {
    let json = format!(
        r#"{{"data": [{{"functions": [{{"name": "{name}", "count": {count}, "regions": [[1, 0, 7, 1, {count}]], "branches": {branches}, "filenames": ["{file}"]}}]}}]}}"#
    );
    serde_json::from_str(&json).expect("synthetic llvm-cov doc")
}

#[test]
fn collector_emits_the_region_map_anchor_for_the_fixture_branch() {
    let root = tempfile::tempdir().unwrap();
    let rel = "crates/phronesis-mcp/src/fixture-src/sample.rs";
    std::fs::create_dir_all(root.path().join(rel).parent().unwrap()).unwrap();
    std::fs::write(root.path().join(rel), FIXTURE_SRC).unwrap();

    // The v0-mangled names llvm-cov emits are handled by rustc-demangle (proven
    // against real exports); for the unit test we feed the demangled form and
    // exercise the collector's own logic: leaf extraction, fn-map validation,
    // and branch-site attribution.
    let name = "fixture_src::safe_divide";

    // The llvm-cov branch region covers line 2 (`if denominator == 0 {`).
    let docs = vec![(
        "divides_positive_values".to_string(),
        doc_with_fn(name, rel, 1, "[[2, 4, 2, 24, 1]]"),
    )];
    let records = collect_from_documents(root.path(), &"a".repeat(40), &docs).unwrap();

    let regions: Vec<&str> = records.iter().map(|r| r.region.as_str()).collect();
    assert!(
        regions.contains(&"fn:crates/phronesis-mcp/src/fixture-src/sample.rs::safe_divide"),
        "fn hit expected, got: {regions:?}"
    );
    // The branch hit must carry exactly the anchor the region map computes —
    // the same anchor as the committed fixture export.
    let branch = records
        .iter()
        .find(|r| r.hit_kind == "branch")
        .expect("branch hit expected");
    assert_eq!(
        branch.region,
        "branch:crates/phronesis-mcp/src/fixture-src/sample.rs::safe_divide:cd6054b02dde",
        "{branch:?}"
    );
}

#[test]
fn collector_drops_records_that_region_map_does_not_name() {
    let root = tempfile::tempdir().unwrap();
    let rel = "crates/phronesis-mcp/src/fixture-src/sample.rs";
    std::fs::create_dir_all(root.path().join(rel).parent().unwrap()).unwrap();
    std::fs::write(root.path().join(rel), FIXTURE_SRC).unwrap();

    // One real fn (covered) plus a demangled-form name for a function
    // tree-sitter does not name in this file — the latter must be dropped,
    // not fabricated. This is the anti-drift contract: the collector cannot
    // emit a record whose identity the region map does not independently
    // produce.
    let docs = vec![(
        "t".to_string(),
        doc_with_fn("fixture_src::safe_divide", rel, 1, "[]"),
    )];
    let phantom = doc_with_fn("fixture_src::does_not_exist", rel, 1, "[]");
    let docs = vec![docs.into_iter().next().unwrap(), ("p".to_string(), phantom)];
    let records = collect_from_documents(root.path(), &"a".repeat(40), &docs).unwrap();
    assert_eq!(records.len(), 1, "only the real fn survives: {records:?}");
    assert_eq!(
        records[0].region,
        "fn:crates/phronesis-mcp/src/fixture-src/sample.rs::safe_divide"
    );
}

#[test]
fn leaf_ident_rejects_closures_but_keeps_impl_methods() {
    assert_eq!(
        leaf_ident("phronesis_mcp::coverage::store::load_index").as_deref(),
        Some("load_index")
    );
    // closures are not tree-sitter function names
    assert_eq!(leaf_ident("a::b::compact::{closure#0}"), None);
    // impl methods ARE tree-sitter function names (fn drop in an impl block)
    assert_eq!(leaf_ident("<A as B>::drop").as_deref(), Some("drop"));
    // generic instantiation leaf survives; the fn-map validation filters
    assert_eq!(
        leaf_ident("std::vec::Vec::<u8>::new").as_deref(),
        Some("new")
    );
    // empty/garbage leaves rejected
    assert_eq!(leaf_ident("a::b::"), None);
}

#[test]
fn repo_rel_anchors_on_the_last_crates_component() {
    assert_eq!(
        repo_rel("/Volumes/Data/Git/phronesis/crates/phronesis-mcp/src/coverage/store.rs"),
        Some("crates/phronesis-mcp/src/coverage/store.rs".to_string())
    );
    // worktree paths resolve to the same relative shape
    assert_eq!(
        repo_rel("/Volumes/Data/Git/phronesis-wt/rv-fact/crates/phronesis-mcp/src/x.rs"),
        Some("crates/phronesis-mcp/src/x.rs".to_string())
    );
    // already-relative llvm-cov output passes through
    assert_eq!(
        repo_rel("crates/phronesis/src/network.rs"),
        Some("crates/phronesis/src/network.rs".to_string())
    );
    assert_eq!(repo_rel("/usr/local/lib/lib.rs"), None);
}
