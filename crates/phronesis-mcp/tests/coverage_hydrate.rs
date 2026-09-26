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

// ---- spec A3: evidence-gap closed-world facts (SPEC §5.2/§6) ----

#[test]
fn test_gap_facts_fire_on_empty_store() {
    let root = tempfile::tempdir().unwrap();
    let input = HydrationInput {
        root: root.path(),
        rule_relations: relations(&[
            "changed_region",
            "region_without_dynamic_evidence",
            "region_without_formal_evidence",
        ]),
        edited: vec![EditedFile {
            path: "src/lib.rs".into(),
            old: Some(OLD_SRC),
            new: NEW_SRC,
        }],
        head_sha: Some("a".repeat(40)),
    };
    let facts = facts_for_event(&input).unwrap();

    let dyn_gaps: Vec<&String> = facts
        .iter()
        .filter(|f| f.predicate == "region_without_dynamic_evidence")
        .map(|f| &f.args[0])
        .collect();
    assert!(
        dyn_gaps.iter().any(|r| r.starts_with("fn:safe_divide")),
        "function region must gap on an empty store: {dyn_gaps:?}"
    );
    assert!(
        dyn_gaps
            .iter()
            .any(|r| r.starts_with("branch:safe_divide:")),
        "branch region must gap on an empty store: {dyn_gaps:?}"
    );
    let formal_gaps: Vec<&String> = facts
        .iter()
        .filter(|f| f.predicate == "region_without_formal_evidence")
        .map(|f| &f.args[0])
        .collect();
    assert_eq!(
        formal_gaps.len(),
        2,
        "formal evidence is unconditionally absent until SPEC-property-ontology lands: {formal_gaps:?}"
    );
}

#[test]
fn test_no_dynamic_gap_when_store_covers_region() {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        &[
            hit(
                "rejects_zero_denominator",
                "fn:safe_divide",
                "src/lib.rs",
                "region",
            ),
            hit(
                "rejects_zero_denominator",
                "branch:safe_divide:cd6054b02dde",
                "src/lib.rs",
                "branch",
            ),
        ],
        &"a".repeat(40),
    );
    let input = HydrationInput {
        root: root.path(),
        rule_relations: relations(&[
            "changed_region",
            "region_without_dynamic_evidence",
            "region_without_formal_evidence",
        ]),
        edited: vec![EditedFile {
            path: "src/lib.rs".into(),
            old: Some(OLD_SRC),
            new: NEW_SRC,
        }],
        // Fresh store: only evidence current for HEAD suppresses a gap (D3;
        // the stale case lives in coverage_store_integrity.rs).
        head_sha: Some("a".repeat(40)),
    };
    let facts = facts_for_event(&input).unwrap();
    assert!(
        !facts
            .iter()
            .any(|f| f.predicate == "region_without_dynamic_evidence"),
        "regions with dynamic hits must not gap: {facts:?}"
    );
    // Formal evidence stays unconditionally absent (SPEC-property-ontology seam).
    assert_eq!(
        facts
            .iter()
            .filter(|f| f.predicate == "region_without_formal_evidence")
            .count(),
        2
    );
}

#[test]
fn test_gap_facts_are_demand_gated() {
    let root = tempfile::tempdir().unwrap();
    let input = HydrationInput {
        root: root.path(),
        rule_relations: relations(&["changed_region"]),
        edited: vec![EditedFile {
            path: "src/lib.rs".into(),
            old: Some(OLD_SRC),
            new: NEW_SRC,
        }],
        head_sha: Some("a".repeat(40)),
    };
    let facts = facts_for_event(&input).unwrap();
    assert!(
        facts
            .iter()
            .all(|f| f.predicate != "region_without_dynamic_evidence"
                && f.predicate != "region_without_formal_evidence"),
        "gap facts must not assert when no loaded rule mentions them: {facts:?}"
    );
}

#[test]
fn test_gap_facts_deduplicate_across_edits() {
    let root = tempfile::tempdir().unwrap();
    let input = HydrationInput {
        root: root.path(),
        rule_relations: relations(&["region_without_dynamic_evidence"]),
        edited: vec![
            EditedFile {
                path: "src/lib.rs".into(),
                old: Some(OLD_SRC),
                new: NEW_SRC,
            },
            EditedFile {
                path: "src/lib.rs".into(),
                old: Some(OLD_SRC),
                new: NEW_SRC,
            },
        ],
        head_sha: Some("a".repeat(40)),
    };
    let facts = facts_for_event(&input).unwrap();
    let dyn_gaps: Vec<&String> = facts
        .iter()
        .filter(|f| f.predicate == "region_without_dynamic_evidence")
        .map(|f| &f.args[0])
        .collect();
    let mut sorted = dyn_gaps.clone();
    sorted.sort();
    assert_eq!(dyn_gaps, sorted, "gap facts must be deduplicated");
}
