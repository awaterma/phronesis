//! Audit wrapper diagnostics. Split from the original `audit.rs`;
//! functions moved verbatim.

use std::path::Path;

use crate::rules_file::RulesFile;

use super::engine::rule_matches_filter;
use super::types::AuditReport;

// ── Wrapper diagnostics ─────────────────────────────────────────────────────

/// Explain a no-hits audit result when a recoverable misconfiguration is the
/// likely cause. Returns `Some(message)` for two cases:
///
/// 1. `audit_tagged_count == 0` — rules exist on disk but none carry
///    `audit: true`, so the walker never starts. The audit's JSON shape
///    (`{files_scanned: 0, rules: []}`) looks identical to a tool failure
///    from the outside; this message names the recoverable cause.
///
/// 2. `audit_tagged_count > 0 && files_scanned == 0` — rules are opted in
///    but the walker found nothing under `scan_root`. Almost always an
///    over-broad `.gitignore` / `.phronesisignore` or a misrouted `path`
///    argument.
///
/// 3. `rule_filter` names a rule no opted-in rule matches (exactly or via
///    an `#orN` expansion) — the walker never starts because the rule set
///    is empty, which would otherwise be misreported as case 2.
///
/// Returns `None` when the report has hits, or when zero hits is the
/// honest answer (rules opted in, files scanned, nothing matched).
///
/// Wrappers route the message to stderr (CLI) or prepend to the response
/// body (MCP). Audit's structured shape stays unchanged.
pub fn empty_result_diagnostic(
    report: &AuditReport,
    rules: &RulesFile,
    rule_filter: Option<&str>,
    scan_root: &Path,
) -> Option<String> {
    if !report.per_rule.is_empty() {
        return None;
    }
    let opted_in: Vec<&str> = rules
        .rules
        .iter()
        .filter(|r| r.audit == Some(true))
        .map(|r| r.id.as_str())
        .collect();
    let audit_tagged_count = opted_in.len();
    if let Some(filter) = rule_filter
        && audit_tagged_count > 0
        && !opted_in.iter().any(|id| rule_matches_filter(id, filter))
    {
        let near: Vec<String> = near_miss_rule_ids(&opted_in, filter);
        let hint = if near.is_empty() {
            String::new()
        } else {
            format!(" Did you mean: {}?", near.join(", "))
        };
        return Some(format!(
            "phronesis: no opted-in rule matches `{filter}` ({audit_tagged_count} rule(s) \
             carry `audit: true`).{hint}"
        ));
    }
    if audit_tagged_count == 0 {
        return Some(
            "phronesis: no rules have `audit: true` on disk. rules.json holds rules, \
             but none are opted into the whole-tree audit — so the walker never starts. \
             Add `\"audit\": true` to the rules you want surfaced here, or re-run \
             `phr-mcp init --rules-only --force` to refresh the starter pack."
                .to_string(),
        );
    }
    if report.files_scanned == 0 {
        return Some(format!(
            "phronesis: {} opted-in rule(s) on disk but walked 0 files under {}. \
             Check `.gitignore` / `.phronesisignore` for over-broad patterns, \
             or verify the `path` argument resolves to your source tree.",
            audit_tagged_count,
            scan_root.display(),
        ));
    }
    None
}

/// Cheap near-miss suggestions for an unmatched `--rule` filter: opted-in
/// ids (with any `#orN` suffix stripped, deduplicated) where one of the
/// two strings contains the other, case-insensitively. Capped at five.
pub(super) fn near_miss_rule_ids(opted_in: &[&str], filter: &str) -> Vec<String> {
    let needle = filter.to_ascii_lowercase();
    let mut out: Vec<String> = Vec::new();
    for id in opted_in {
        let base = crate::rules_file::base_rule_id(id);
        let hay = base.to_ascii_lowercase();
        if (hay.contains(&needle) || needle.contains(&hay)) && !out.iter().any(|o| o == base) {
            out.push(base.to_string());
        }
    }
    out.sort();
    out.truncate(5);
    out
}
