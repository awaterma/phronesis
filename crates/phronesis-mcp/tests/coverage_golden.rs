//! Golden trace (spec acceptance A2): hydrate the sketch §4 edit through the
//! real pipeline — durable store from the committed fixture export, changed
//! regions from the tree-sitter region map, demand-gated facts, RETE join
//! with rule 5.1 exactly as spec §5.1 writes it — and assert the pairing is
//! exact: `rejects_zero_denominator` is the only test relevant to the branch
//! site, while all three tests are relevant to `fn:safe_divide`.
//!
//! Spec: `docs/specs/SPEC-coverage-evidence.md` §5.1, §9 A2.

use std::collections::HashSet;
use std::path::PathBuf;

use phr::Fact;
use phronesis_mcp::coverage::hydrate::{EditedFile, HydrationInput, facts_for_event};
use phronesis_mcp::coverage::import::import_export;
use phronesis_mcp::rules_file;
use tempfile::TempDir;

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

/// Rule 5.1 verbatim from SPEC §5.1, as a committed fixture (the repo's live
/// `.phronesis/rules.json` is gitignored — the committable artifact is this
/// file; the live rule is an orchestrator integration step).
fn rule_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/coverage-rules-5.1.json")
}

/// Stable fact ID matching the hook's `coverage:<predicate>:<args-hash>`.
fn fact_id(predicate: &str, args: &[String]) -> String {
    let joined = args.join("\u{1f}");
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in joined.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("coverage:{predicate}:{hash:012x}")
}

#[test]
fn golden_relevant_test_join_fires_for_the_right_pairs() {
    let d = TempDir::new().expect("tempdir");

    // The committed, real per-test export (A1).
    let export = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/coverage-sample/export.jsonl");
    let summary = import_export(d.path(), &export, 1).expect("import fixture export");
    assert_eq!(summary.tests, 3, "fixture export must carry 3 tests");
    assert_eq!(summary.revision.len(), 40, "revision must be a full sha");

    // The §4 edit: the error message changes, the condition does not.
    let relations: HashSet<String> = ["changed_region", "test_hits_region"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let input = HydrationInput {
        root: d.path(),
        rule_relations: relations,
        edited: vec![EditedFile {
            path: "src/lib.rs".into(),
            old: Some(OLD_SRC),
            new: NEW_SRC,
        }],
        head_sha: Some("a".repeat(40)),
    };
    let facts = facts_for_event(&input).expect("hydrate");
    assert!(
        facts.iter().any(|f| f.predicate == "changed_region"),
        "changed regions must hydrate: {facts:?}"
    );
    assert_eq!(
        facts
            .iter()
            .filter(|f| f.predicate == "test_hits_region")
            .count(),
        4,
        "the store's four hits (3 tests at the function + 1 branch hit) must all join: {facts:?}"
    );

    // Load rule 5.1 into a real RETE network and assert the hydrated facts.
    let rt = tokio::runtime::Runtime::new().expect("rt");
    let consequences = rt.block_on(async {
        let net = phronesis_mcp::net::build_network();
        let file = rules_file::read(&rule_fixture()).expect("read rules fixture");
        assert_eq!(file.rules.len(), 1, "fixture must hold exactly rule 5.1");
        let (rule, _phase) = rules_file::rule_from_disk(&file.rules[0]);
        net.add_rule(rule).await.expect("add rule 5.1");
        for f in &facts {
            net.assert_fact(Fact {
                id: fact_id(&f.predicate, &f.args),
                predicate: f.predicate.clone(),
                args: f.args.clone(),
                timestamp: 0,
                source: Some("coverage".to_string()),
            })
            .await
            .expect("assert fact");
        }
        net.fire_all_consequences().expect("fire")
    });

    // Exactly one log consequence per matching pair.
    // Log actions render as {"action_type":"log","message":"...","params":...}
    // — the substituted message is the pairing evidence.
    let logged: Vec<&str> = consequences
        .iter()
        .map(|c| c.payload["message"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        logged.len(),
        4,
        "expected 4 join pairs (3 fn + 1 branch): {logged:?}"
    );

    // The branch site pairs with exactly one test, and it is the branch-
    // exercising test.
    let branch_pairs: Vec<&str> = logged
        .iter()
        .filter(|p| p.contains("branch:safe_divide:"))
        .copied()
        .collect();
    assert_eq!(
        branch_pairs.len(),
        1,
        "exactly one branch pair: {branch_pairs:?}"
    );
    let bp = branch_pairs[0];
    assert!(
        bp.contains("rejects_zero_denominator"),
        "the branch pair must name rejects_zero_denominator: {bp}"
    );

    // A2's negative half: the function-only tests never pair with the
    // branch site; all three tests pair with fn:safe_divide.
    for p in &logged {
        assert!(
            !(p.contains("branch:safe_divide:")
                && (p.contains("divides_positive_values")
                    || p.contains("divides_negative_values"))),
            "function-only tests must not pair with the branch region: {p}"
        );
    }
    for test in ["divides_positive_values", "divides_negative_values"] {
        assert!(
            logged
                .iter()
                .any(|p| p.contains(test) && p.contains("fn:safe_divide")),
            "{test} must be relevant to fn:safe_divide"
        );
    }
}
