//! Coverage-store integrity: crash consistency between the records file and
//! the index (C9), one validator on import AND read (C10), stale evidence
//! never suppressing gap facts (D3), and a corrupt store surfacing as a
//! `store_corrupt(coverage, <reason>)` fact instead of failing open (D8).
//!
//! Spec: `docs/specs/SPEC-coverage-evidence.md` §3.5 / §4.

use std::collections::HashSet;
use std::path::Path;
use std::process::Command;

use phronesis_mcp::coverage::hydrate::{CoverageFact, EditedFile, HydrationInput, facts_for_event};
use phronesis_mcp::coverage::import::import_export;
use phronesis_mcp::coverage::select::{render_json, render_table, select};
use phronesis_mcp::coverage::store::{
    COVERAGE_FORMAT, CoverageIndex, HitRecord, load_hits, load_index, store_paths, write_store,
};

const OLD_SRC: &str = r#"pub fn safe_divide(numerator: i32, denominator: i32) -> Result<i32, &'static str> {
    if denominator == 0 {
        return Err("division by zero");
    }

    Ok(numerator / denominator)
}
"#;
const NEW_SRC: &str = r#"pub fn safe_divide(numerator: i32, denominator: i32) -> Result<i32, &'static str> {
    if denominator == 0 {
        return Err("invalid denominator");
    }

    Ok(numerator / denominator)
}
"#;
const BRANCH: &str = "branch:safe_divide:cd6054b02dde";

fn hit(test: &str, region: &str, kind: &str, rev: &str) -> HitRecord {
    HitRecord {
        v: COVERAGE_FORMAT,
        kind: "hit".into(),
        test: test.into(),
        region: region.into(),
        file: "src/lib.rs".into(),
        start_line: 1,
        end_line: 7,
        hit_kind: kind.into(),
        revision: rev.into(),
        tool: "cargo-llvm-cov".into(),
    }
}

fn index(rev: &str) -> CoverageIndex {
    CoverageIndex {
        format: COVERAGE_FORMAT,
        revision: rev.into(),
        imported_at: 1,
        tool: "cargo-llvm-cov".into(),
    }
}

fn covering_hits(rev: &str) -> Vec<HitRecord> {
    vec![
        hit("rejects_zero_denominator", "fn:safe_divide", "region", rev),
        hit("rejects_zero_denominator", BRANCH, "branch", rev),
    ]
}

fn relations(rels: &[&str]) -> HashSet<String> {
    rels.iter().map(|s| s.to_string()).collect()
}

fn hydrate(root: &Path, rels: &[&str], head: &str) -> Vec<CoverageFact> {
    let input = HydrationInput {
        root,
        rule_relations: relations(rels),
        edited: vec![EditedFile {
            path: "src/lib.rs".into(),
            old: Some(OLD_SRC),
            new: NEW_SRC,
        }],
        head_sha: Some(head.to_string()),
    };
    facts_for_event(&input).expect("hydration must not fail on store state")
}

fn gaps(facts: &[CoverageFact]) -> Vec<String> {
    facts
        .iter()
        .filter(|f| f.predicate == "region_without_dynamic_evidence")
        .map(|f| f.args[0].clone())
        .collect()
}

fn corrupt_reason(facts: &[CoverageFact]) -> Option<String> {
    facts
        .iter()
        .find(|f| f.predicate == "store_corrupt")
        .map(|f| {
            assert_eq!(f.args[0], "coverage", "first arg names the store: {f:?}");
            f.args[1].clone()
        })
}

const ALL: &[&str] = &[
    "changed_region",
    "test_hits_region",
    "coverage_revision",
    "coverage_stale",
    "region_without_dynamic_evidence",
    "store_corrupt",
];

fn write_export(dir: &Path, recs: &[HitRecord]) -> std::path::PathBuf {
    let p = dir.join("export.jsonl");
    let lines: Vec<String> = recs
        .iter()
        .map(|r| serde_json::to_string(r).unwrap())
        .collect();
    std::fs::write(&p, lines.join("\n") + "\n").unwrap();
    p
}

// ---------------------------------------------------------------- C9

/// Crash between the records rename and the index rename: the OLD index
/// (whose revision equals HEAD) now fronts the NEW records. The store must
/// read as corrupt, never as fresh evidence for the old revision.
#[test]
fn c9_torn_write_between_renames_is_corrupt_not_fresh() {
    let root = tempfile::tempdir().unwrap();
    let old = "a".repeat(40);
    let new = "b".repeat(40);
    write_store(
        root.path(),
        &[hit("old_test", "fn:other", "region", &old)],
        &index(&old),
    )
    .unwrap();
    let (_, index_path) = store_paths(root.path());
    let old_index_bytes = std::fs::read(&index_path).unwrap();

    // The second import completes the records rename, then "crashes":
    // restore the old index to simulate the index rename never happening.
    write_store(root.path(), &covering_hits(&new), &index(&new)).unwrap();
    std::fs::write(&index_path, old_index_bytes).unwrap();

    assert!(
        load_hits(root.path()).is_err(),
        "records that do not match the index must not load"
    );
    assert_eq!(
        load_index(root.path()),
        None,
        "an unverified index is not a revision claim"
    );

    let facts = hydrate(root.path(), ALL, &old);
    assert!(
        corrupt_reason(&facts).is_some(),
        "torn store must assert store_corrupt: {facts:?}"
    );
    assert!(
        !facts.iter().any(|f| f.predicate == "test_hits_region"),
        "new records must not be presented as the old revision: {facts:?}"
    );
    assert!(
        !facts.iter().any(|f| f.predicate == "coverage_revision"),
        "no revision claim from a torn store: {facts:?}"
    );
    let g = gaps(&facts);
    assert!(
        g.contains(&"fn:safe_divide".to_string()) && g.contains(&BRANCH.to_string()),
        "{g:?}"
    );
}

/// First-ever import crashes after the records rename: records exist with
/// no index. Those records carry no commit marker and must not load.
#[test]
fn c9_records_without_index_are_corrupt() {
    let root = tempfile::tempdir().unwrap();
    let rev = "a".repeat(40);
    write_store(root.path(), &covering_hits(&rev), &index(&rev)).unwrap();
    let (_, index_path) = store_paths(root.path());
    std::fs::remove_file(&index_path).unwrap();

    assert!(load_hits(root.path()).is_err());
    let facts = hydrate(root.path(), ALL, &rev);
    assert!(corrupt_reason(&facts).is_some(), "{facts:?}");
    assert_eq!(gaps(&facts).len(), 2, "{facts:?}");
}

/// Record revisions must agree with the index revision even when the
/// digest matches (a store written by a buggy writer).
#[test]
fn c9_record_revision_disagreeing_with_index_is_corrupt() {
    let root = tempfile::tempdir().unwrap();
    write_store(
        root.path(),
        &covering_hits(&"b".repeat(40)),
        &index(&"a".repeat(40)),
    )
    .unwrap();
    assert!(load_hits(root.path()).is_err());
}

// ---------------------------------------------------------------- C10

#[test]
fn c10_read_path_rejects_records_import_would_reject() {
    let rev = "a".repeat(40);
    let bad: Vec<(&str, HitRecord)> = vec![
        (
            "absolute path",
            HitRecord {
                file: "/etc/passwd".into(),
                ..hit("t", "fn:f", "region", &rev)
            },
        ),
        ("bogus hit_kind", hit("t", "fn:f", "bogus", &rev)),
        (
            "short revision",
            HitRecord {
                revision: "xyz".into(),
                ..hit("t", "fn:f", "region", &rev)
            },
        ),
        (
            "start > end",
            HitRecord {
                start_line: 9,
                end_line: 2,
                ..hit("t", "fn:f", "region", &rev)
            },
        ),
        ("region kind, branch id", hit("t", BRANCH, "region", &rev)),
        ("branch kind, fn id", hit("t", "fn:f", "branch", &rev)),
        (
            "empty tool",
            HitRecord {
                tool: String::new(),
                ..hit("t", "fn:f", "region", &rev)
            },
        ),
    ];
    for (label, rec) in bad {
        let root = tempfile::tempdir().unwrap();
        let idx = CoverageIndex {
            revision: rec.revision.clone(),
            tool: rec.tool.clone(),
            ..index(&rev)
        };
        write_store(root.path(), &[rec], &idx).unwrap();
        assert!(
            load_hits(root.path()).is_err(),
            "read path accepted: {label}"
        );
    }
}

#[test]
fn c10_import_rejects_hit_kind_region_prefix_disagreement() {
    let rev = "a".repeat(40);
    for rec in [
        hit("t", BRANCH, "region", &rev),
        hit("t", "fn:f", "branch", &rev),
    ] {
        let root = tempfile::tempdir().unwrap();
        let exp = tempfile::tempdir().unwrap();
        let err = import_export(root.path(), &write_export(exp.path(), &[rec]), 1)
            .unwrap_err()
            .to_string();
        assert!(err.contains("hit_kind"), "got: {err}");
    }
}

#[test]
fn c10_import_rejects_empty_and_mixed_tools() {
    let rev = "a".repeat(40);
    let root = tempfile::tempdir().unwrap();
    let exp = tempfile::tempdir().unwrap();
    let empty = HitRecord {
        tool: String::new(),
        ..hit("t", "fn:f", "region", &rev)
    };
    let err = import_export(root.path(), &write_export(exp.path(), &[empty]), 1)
        .unwrap_err()
        .to_string();
    assert!(err.contains("tool"), "got: {err}");

    let other = HitRecord {
        tool: "coverage.py".into(),
        ..hit("u", "fn:g", "region", &rev)
    };
    let err = import_export(
        root.path(),
        &write_export(exp.path(), &[hit("t", "fn:f", "region", &rev), other]),
        1,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("mixes tools"), "got: {err}");
    assert_eq!(load_index(root.path()), None);
}

#[test]
fn c10_import_dedupes_identical_records() {
    let rev = "a".repeat(40);
    let root = tempfile::tempdir().unwrap();
    let exp = tempfile::tempdir().unwrap();
    let r = hit("t", "fn:f", "region", &rev);
    let summary = import_export(
        root.path(),
        &write_export(exp.path(), &[r.clone(), r.clone(), r]),
        1,
    )
    .unwrap();
    assert_eq!(summary.records, 1, "duplicates must not be counted");
    assert_eq!(load_hits(root.path()).unwrap().len(), 1);
}

#[test]
fn c10_import_canonicalizes_revision_to_lowercase() {
    let lower = "abcdef0123".repeat(4);
    let upper = lower.to_ascii_uppercase();
    let root = tempfile::tempdir().unwrap();
    let exp = tempfile::tempdir().unwrap();
    // Mixed case of the SAME sha is one revision, not "mixes revisions".
    let summary = import_export(
        root.path(),
        &write_export(
            exp.path(),
            &[
                hit("t", "fn:f", "region", &upper),
                hit("u", "fn:f", "region", &lower),
            ],
        ),
        1,
    )
    .unwrap();
    assert_eq!(summary.revision, lower);
    assert_eq!(load_index(root.path()).unwrap().revision, lower);
    assert!(
        load_hits(root.path())
            .unwrap()
            .iter()
            .all(|h| h.revision == lower)
    );
    // HEAD (git always prints lowercase) equals the import: not stale.
    let facts = hydrate(root.path(), &["coverage_stale"], &lower);
    assert!(
        !facts.iter().any(|f| f.predicate == "coverage_stale"),
        "{facts:?}"
    );
}

// ---------------------------------------------------------------- D3

#[test]
fn d3_stale_hits_never_suppress_dynamic_gap() {
    let root = tempfile::tempdir().unwrap();
    write_store(
        root.path(),
        &covering_hits(&"a".repeat(40)),
        &index(&"a".repeat(40)),
    )
    .unwrap();

    let fresh = hydrate(root.path(), ALL, &"a".repeat(40));
    assert!(
        gaps(&fresh).is_empty(),
        "fresh hits suppress gaps: {fresh:?}"
    );

    let stale = hydrate(root.path(), ALL, &"b".repeat(40));
    assert!(stale.iter().any(|f| f.predicate == "coverage_stale"));
    let g = gaps(&stale);
    assert!(
        g.contains(&"fn:safe_divide".to_string()) && g.contains(&BRANCH.to_string()),
        "stale hits must not suppress region_without_dynamic_evidence: {stale:?}"
    );
}

fn git(root: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn git_project() -> (tempfile::TempDir, String) {
    let root = tempfile::tempdir().unwrap();
    git(root.path(), &["init", "-q"]);
    std::fs::create_dir_all(root.path().join("src")).unwrap();
    std::fs::write(root.path().join("src/lib.rs"), OLD_SRC).unwrap();
    git(root.path(), &["add", "."]);
    git(root.path(), &["commit", "-q", "-m", "init"]);
    let head = git(root.path(), &["rev-parse", "HEAD"]);
    std::fs::write(root.path().join("src/lib.rs"), NEW_SRC).unwrap();
    (root, head)
}

#[test]
fn d3_select_labels_stale_evidence() {
    let (root, head) = git_project();

    write_store(root.path(), &covering_hits(&head), &index(&head)).unwrap();
    let fresh = select(root.path(), None).unwrap();
    assert!(!fresh.tests.is_empty());
    assert!(
        fresh
            .tests
            .iter()
            .all(|t| t.evidence == "coverage_observation"),
        "{:?}",
        fresh.tests
    );
    let fresh_json: serde_json::Value = serde_json::from_str(&render_json(&fresh)).unwrap();
    assert!(
        fresh_json.get("coverage_note").is_none(),
        "fresh JSON shape unchanged: {fresh_json}"
    );

    let other = "c".repeat(40);
    write_store(root.path(), &covering_hits(&other), &index(&other)).unwrap();
    let stale = select(root.path(), None).unwrap();
    assert!(!stale.tests.is_empty());
    assert!(
        stale
            .tests
            .iter()
            .all(|t| t.evidence == "coverage_observation_stale"),
        "stale entries must be labeled: {:?}",
        stale.tests
    );
    let table = render_table(&stale);
    assert!(table.contains("coverage_observation_stale"), "{table}");
    let json: serde_json::Value = serde_json::from_str(&render_json(&stale)).unwrap();
    assert!(
        json["coverage_note"]
            .as_str()
            .unwrap_or("")
            .contains("stale"),
        "{json}"
    );
}

// ---------------------------------------------------------------- D8

fn corrupt_store(root: &Path) {
    let rev = "a".repeat(40);
    write_store(root, &covering_hits(&rev), &index(&rev)).unwrap();
    let (records_path, _) = store_paths(root);
    std::fs::write(records_path, "{not json\n").unwrap();
}

#[test]
fn d8_corrupt_store_asserts_store_corrupt_and_conservative_gaps() {
    let root = tempfile::tempdir().unwrap();
    corrupt_store(root.path());
    let facts = hydrate(root.path(), ALL, &"a".repeat(40));
    let reason = corrupt_reason(&facts).expect("store_corrupt fact");
    assert!(!reason.is_empty());
    assert_eq!(
        gaps(&facts).len(),
        2,
        "corrupt store = no evidence: {facts:?}"
    );
    assert!(
        !facts.iter().any(|f| f.predicate == "test_hits_region"),
        "{facts:?}"
    );
}

#[test]
fn d8_store_corrupt_is_demand_gated() {
    let root = tempfile::tempdir().unwrap();
    corrupt_store(root.path());
    let facts = hydrate(
        root.path(),
        &["region_without_dynamic_evidence"],
        &"a".repeat(40),
    );
    assert!(corrupt_reason(&facts).is_none(), "{facts:?}");
    assert_eq!(gaps(&facts).len(), 2, "{facts:?}");
}

#[test]
fn d8_select_reports_corrupt_store() {
    let (root, _head) = git_project();
    corrupt_store(root.path());
    let sel = select(root.path(), None).unwrap();
    let table = render_table(&sel);
    assert!(table.contains("corrupt"), "{table}");
    assert!(!table.contains("store is empty"), "{table}");
    let json: serde_json::Value = serde_json::from_str(&render_json(&sel)).unwrap();
    assert!(
        json["coverage_note"]
            .as_str()
            .unwrap_or("")
            .contains("corrupt"),
        "{json}"
    );
}
