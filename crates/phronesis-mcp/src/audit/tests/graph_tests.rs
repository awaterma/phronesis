//! Tests for the structural graph merge helpers. Split from the original
//! `audit.rs` `mod graph_merge_tests`.

use crate::audit::*;
use crate::graph::audit::GraphHit;
use std::path::PathBuf;

fn hit(rule: &str, file: &str, action: &str) -> GraphHit {
    GraphHit {
        rule_id: rule.to_string(),
        action_type: action.to_string(),
        file: file.to_string(),
        detail: format!("{rule} on {file}"),
    }
}

fn empty_report() -> AuditReport {
    AuditReport {
        generated_at: 0,
        scan_duration_ms: 0,
        files_scanned: 0,
        per_rule: Vec::new(),
        lexical_excluded: Vec::new(),
    }
}

#[test]
fn structural_hits_become_audit_entries() {
    let mut r = empty_report();
    merge_graph_hits(
        &mut r,
        &[hit("warn-import-cycle", "src/a.rs", "constraint_warning")],
        None,
        None,
    );
    assert_eq!(r.per_rule.len(), 1);
    assert_eq!(r.per_rule[0].hits, 1);
}

#[test]
fn graph_hit_rule_filter_matches_or_expansions() {
    let mut r = empty_report();
    merge_graph_hits(
        &mut r,
        &[
            hit(
                "warn-untested-risky-call#or0",
                "src/a.rs",
                "constraint_warning",
            ),
            hit(
                "warn-untested-risky-call#or1",
                "src/b.rs",
                "constraint_warning",
            ),
            hit("warn-import-cycle", "src/c.rs", "constraint_warning"),
        ],
        Some("warn-untested-risky-call"),
        None,
    );
    let ids: Vec<&str> = r.per_rule.iter().map(|p| p.rule_id.as_str()).collect();
    assert_eq!(ids.len(), 2, "{ids:?}");
    assert!(
        ids.iter()
            .all(|id| id.starts_with("warn-untested-risky-call#or")),
        "{ids:?}"
    );
}

#[test]
fn graph_hits_outside_the_scan_scope_are_dropped() {
    // Graph rules must still *evaluate* over the whole graph — a test that
    // covers this file may live anywhere — but a scoped audit reports
    // findings in scope. Without this, `--path src/journey` returns
    // violations in src/init.rs and reads as debt in the caller's area.
    let mut r = empty_report();
    merge_graph_hits(
        &mut r,
        &[
            hit(
                "warn-import-cycle",
                "src/journey/mod.rs",
                "constraint_warning",
            ),
            hit("warn-import-cycle", "src/init.rs", "constraint_warning"),
        ],
        None,
        Some("src/journey"),
    );
    assert_eq!(r.per_rule.len(), 1, "one rule survives");
    assert_eq!(r.per_rule[0].hits, 1, "only the in-scope hit");
    assert_eq!(
        r.per_rule[0].files[0].path,
        PathBuf::from("src/journey/mod.rs")
    );
}

#[test]
fn a_scope_matches_on_path_boundaries_not_string_prefix() {
    // `src/journey` must not swallow `src/journeyman.rs`.
    let mut r = empty_report();
    merge_graph_hits(
        &mut r,
        &[hit(
            "warn-import-cycle",
            "src/journeyman.rs",
            "constraint_warning",
        )],
        None,
        Some("src/journey"),
    );
    assert!(r.per_rule.is_empty(), "sibling path must not match");
}

#[test]
fn hits_in_the_same_rule_group_by_file() {
    let mut r = empty_report();
    merge_graph_hits(
        &mut r,
        &[
            hit("warn-import-cycle", "src/a.rs", "constraint_warning"),
            hit("warn-import-cycle", "src/b.rs", "constraint_warning"),
        ],
        None,
        None,
    );
    assert_eq!(r.per_rule.len(), 1, "one rule");
    assert_eq!(r.per_rule[0].files.len(), 2, "two files");
    assert_eq!(r.per_rule[0].hits, 2);
}

#[test]
fn the_rule_filter_is_honored() {
    let mut r = empty_report();
    merge_graph_hits(
        &mut r,
        &[
            hit("warn-import-cycle", "src/a.rs", "constraint_warning"),
            hit("warn-untested-risky-call", "src/b.rs", "constraint_warning"),
        ],
        Some("warn-import-cycle"),
        None,
    );
    assert_eq!(r.per_rule.len(), 1);
    assert_eq!(r.per_rule[0].rule_id.to_string(), "warn-import-cycle");
}

#[test]
fn the_message_is_preserved_as_per_hit_detail() {
    let mut r = empty_report();
    merge_graph_hits(
        &mut r,
        &[hit("warn-import-cycle", "src/a.rs", "constraint_warning")],
        None,
        None,
    );
    assert_eq!(
        r.per_rule[0].files[0].details[0],
        "warn-import-cycle on src/a.rs"
    );
}

#[test]
fn blocking_structural_rules_outrank_warnings_in_the_merged_report() {
    let mut r = empty_report();
    merge_graph_hits(
        &mut r,
        &[
            hit("a-warn", "src/a.rs", "constraint_warning"),
            hit("z-block", "src/b.rs", "constraint_violation"),
        ],
        None,
        None,
    );
    assert_eq!(r.per_rule[0].rule_id.to_string(), "z-block");
}
