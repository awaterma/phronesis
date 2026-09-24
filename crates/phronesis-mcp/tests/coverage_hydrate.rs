use std::collections::HashSet;

use phronesis_mcp::coverage::hydrate::{EditedFile, HydrationInput, facts_for_event};
use phronesis_mcp::coverage::store::{COVERAGE_FORMAT, CoverageIndex, HitRecord, write_store};

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

fn hit(test: &str, region: &str, file: &str, kind: &str) -> HitRecord {
    HitRecord {
        v: COVERAGE_FORMAT,
        kind: "hit".into(),
        test: test.into(),
        region: region.into(),
        file: file.into(),
        start_line: 1,
        end_line: 7,
        hit_kind: kind.into(),
        revision: "a".repeat(40),
        tool: "cargo-llvm-cov".into(),
    }
}

fn write(root: &std::path::Path, hits: &[HitRecord], rev: &str) {
    write_store(
        root,
        hits,
        &CoverageIndex {
            format: COVERAGE_FORMAT,
            revision: rev.into(),
            imported_at: 1,
            tool: "cargo-llvm-cov".into(),
        },
    )
    .unwrap();
}

fn relations(rels: &[&str]) -> HashSet<String> {
    rels.iter().map(|s| s.to_string()).collect()
}

#[test]
fn test_demand_gate_skips_when_no_rule_mentions() {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        &[hit("test_a", "fn:foo", "src/a.rs", "region")],
        &"a".repeat(40),
    );

    let input = HydrationInput {
        root: root.path(),
        rule_relations: relations(&["file_path_matches"]),
        edited: vec![EditedFile {
            path: "src/a.rs".into(),
            old: Some(OLD_SRC),
            new: NEW_SRC,
        }],
        head_sha: Some("a".repeat(40)),
    };
    let facts = facts_for_event(&input).unwrap();
    assert!(
        facts.is_empty(),
        "demand gate must suppress everything: {facts:?}"
    );
}

#[test]
fn test_hydrate_scopes_to_edited_files() {
    let root = tempfile::tempdir().unwrap();
    let rev = "a".repeat(40);
    write(
        root.path(),
        &[
            hit("test_a", "fn:foo", "src/a.rs", "region"),
            hit("test_b", "fn:bar", "src/b.rs", "region"),
        ],
        &rev,
    );

    let input = HydrationInput {
        root: root.path(),
        rule_relations: relations(&["test_hits_region"]),
        edited: vec![EditedFile {
            path: "src/a.rs".into(),
            old: None,
            new: "x",
        }],
        head_sha: Some(rev.clone()),
    };
    let facts = facts_for_event(&input).unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].predicate, "test_hits_region");
    assert_eq!(
        facts[0].args,
        vec!["test_a".to_string(), "fn:foo".to_string()]
    );
}

#[test]
fn test_hydrate_reports_stale_coverage() {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        &[hit("test_a", "fn:foo", "src/a.rs", "region")],
        &"a".repeat(40),
    );

    let input = HydrationInput {
        root: root.path(),
        rule_relations: relations(&["test_hits_region", "coverage_stale"]),
        edited: vec![EditedFile {
            path: "src/a.rs".into(),
            old: None,
            new: "x",
        }],
        head_sha: Some("b".repeat(40)),
    };
    let facts = facts_for_event(&input).unwrap();
    assert!(
        facts
            .iter()
            .any(|f| f.predicate == "coverage_stale" && f.args.is_empty())
    );
    // Stale is a marker; hits are still asserted.
    assert!(facts.iter().any(|f| f.predicate == "test_hits_region"));
}

#[test]
fn test_hydrate_emits_changed_regions() {
    let root = tempfile::tempdir().unwrap();

    let input = HydrationInput {
        root: root.path(),
        rule_relations: relations(&["changed_region", "changed_function"]),
        edited: vec![EditedFile {
            path: "src/lib.rs".into(),
            old: Some(OLD_SRC),
            new: NEW_SRC,
        }],
        head_sha: Some("a".repeat(40)),
    };
    let facts = facts_for_event(&input).unwrap();

    let change_id = "head:aaaaaaaaaaaa".to_string();
    let regions: Vec<&String> = facts
        .iter()
        .filter(|f| f.predicate == "changed_region")
        .map(|f| &f.args[1])
        .collect();
    assert!(
        regions.iter().any(|r| r.starts_with("fn:safe_divide")),
        "missing function region: {regions:?}"
    );
    assert!(
        regions.iter().any(|r| r.starts_with("branch:safe_divide:")),
        "missing branch region: {regions:?}"
    );
    for f in &facts {
        assert_eq!(f.args[0], change_id, "wrong change id: {f:?}");
    }
    let functions: Vec<String> = facts
        .iter()
        .filter(|f| f.predicate == "changed_function")
        .map(|f| f.args[1].clone())
        .collect();
    assert_eq!(
        functions,
        vec!["fn:safe_divide".to_string()],
        "changed_function must be exactly fn:safe_divide: {functions:?}"
    );
}

#[test]
fn test_head_revision_uses_probe_value() {
    let root = tempfile::tempdir().unwrap();
    let input = HydrationInput {
        root: root.path(),
        rule_relations: relations(&["head_revision"]),
        edited: vec![],
        head_sha: Some("a".repeat(40)),
    };
    let facts = facts_for_event(&input).unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].predicate, "head_revision");
    assert_eq!(facts[0].args, vec!["a".repeat(40)]);
}
