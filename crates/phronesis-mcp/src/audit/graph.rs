//! Audit structural graph merge helpers. Split from the original
//! `audit.rs`; functions moved verbatim.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::engine::{build_per_rule, rule_matches_filter, rule_report_order};
use super::types::{AuditReport, Level, PerFileHits};

// ── Structural (graph) rules ────────────────────────────────────────────────

/// Fold structural findings into an `AuditReport`.
///
/// Graph rules cannot be evaluated by the file-scanning loop above — their
/// conditions join relations across the whole repository rather than matching
/// text in one file — so they are evaluated separately by
/// `graph::audit::audit_graph_rules` and merged here.
///
/// Findings carry no line number: the graph records that a function is
/// untested, not where it sits. They use the same line-1 placeholder plus
/// detail string that AST hits already use, so renderers need no new case.
/// The repo-relative prefix a scoped audit reports under, or `None` for a
/// whole-tree scan.
///
/// Graph rules are evaluated over the entire graph by design — the test that
/// covers a function may live anywhere — but a caller who scoped the audit to
/// one directory is asking "what is wrong *here*", and answering with another
/// module's debt reads as their own.
pub fn graph_scope_prefix(project_root: &Path, scan_root: &Path) -> Option<String> {
    let rel = scan_root.strip_prefix(project_root).ok()?;
    let s = rel.to_str()?.trim_end_matches('/');
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

/// True when `file` (repo-relative) lies under `scope`.
///
/// Compares whole path segments: `src/journey` must not swallow
/// `src/journeyman.rs`.
pub(super) fn within_scope(file: &str, scope: &str) -> bool {
    file == scope
        || file
            .strip_prefix(scope)
            .is_some_and(|rest| rest.starts_with('/'))
}

pub fn merge_graph_hits(
    report: &mut AuditReport,
    hits: &[crate::graph::audit::GraphHit],
    rule_filter: Option<&str>,
    scope: Option<&str>,
) {
    let mut accum: BTreeMap<String, (Level, BTreeMap<PathBuf, PerFileHits>)> = BTreeMap::new();
    for hit in hits {
        if let Some(want) = rule_filter
            && !rule_matches_filter(&hit.rule_id, want)
        {
            continue;
        }
        if let Some(scope) = scope
            && !within_scope(&hit.file, scope)
        {
            continue;
        }
        let Some(level) = Level::from_action_type(&hit.action_type) else {
            continue;
        };
        let entry = accum
            .entry(hit.rule_id.clone())
            .or_insert_with(|| (level, BTreeMap::new()));
        entry
            .1
            .entry(PathBuf::from(&hit.file))
            .or_default()
            .push_detail(hit.detail.clone());
    }
    report.per_rule.extend(build_per_rule(accum));
    // `build_per_rule` sorts within its own batch; re-sort the union so the
    // merged report keeps the documented ordering rather than showing
    // structural rules bolted on the end.
    report.per_rule.sort_by(rule_report_order);
}
