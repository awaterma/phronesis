//! Audit table/JSON renderers and date helpers. Split from the original
//! `audit.rs`; functions moved verbatim.

use serde_json::json;

use super::trend::DebtTrend;
use super::types::{AuditReport, Level};

// ── Renderers ────────────────────────────────────────────────────────────────

const EXCLUDED_NAMES_SHOWN: usize = 5;

/// One footer line naming the files `.phronesisignore` excluded from
/// lexical rules. Empty string when nothing was excluded.
fn excluded_line(report: &AuditReport) -> String {
    if report.lexical_excluded.is_empty() {
        return String::new();
    }
    let total = report.lexical_excluded.len();
    let mut names: Vec<String> = report
        .lexical_excluded
        .iter()
        .take(EXCLUDED_NAMES_SHOWN)
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    if total > EXCLUDED_NAMES_SHOWN {
        names.push(format!("+{} more", total - EXCLUDED_NAMES_SHOWN));
    }
    format!(
        "{total} file(s) excluded from lexical rules by .phronesisignore (structural rules still ran): {}\n",
        names.join(", ")
    )
}

/// Render an `AuditReport` as a human-readable terminal table.
/// `expand` switches from per-rule summary to per-file detail with line numbers.
pub fn render_table(report: &AuditReport, expand: bool) -> String {
    if report.per_rule.is_empty() {
        return format!(
            "no audit violations found ({} files scanned in {}ms)\n{}",
            report.files_scanned,
            report.scan_duration_ms,
            excluded_line(report)
        );
    }

    let id_width = report
        .per_rule
        .iter()
        .map(|r| r.rule_id.as_str().len())
        .max()
        .unwrap_or(0)
        .max("Rule".len());

    let mut out = String::new();
    out.push_str(&format!(
        "{:<id_width$}  Level  Hits  Files\n",
        "Rule",
        id_width = id_width
    ));
    for r in &report.per_rule {
        out.push_str(&format!(
            "{:<id_width$}  {:<5}  {:>4}  {:>5}\n",
            r.rule_id,
            r.level.as_str(),
            r.hits,
            r.files.len(),
            id_width = id_width,
        ));
        if expand {
            for f in &r.files {
                // AST-predicate hits carry a per-function detail and a
                // meaningless placeholder line of `1` per hit; surface
                // the names instead of `lines: 1, 1, ...`. Content and
                // whole-file hits have empty details and real line
                // numbers, so they keep the `lines:` form.
                let has_details = f.details.iter().any(|d| !d.is_empty());
                if has_details {
                    let details_str = f
                        .details
                        .iter()
                        .filter(|d| !d.is_empty())
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ");
                    out.push_str(&format!(
                        "    {} \u{2014} {}\n",
                        f.path.display(),
                        details_str
                    ));
                } else {
                    let lines_str = f
                        .lines
                        .iter()
                        .map(|n| n.to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    out.push_str(&format!(
                        "    {} \u{2014} lines: {}\n",
                        f.path.display(),
                        lines_str
                    ));
                }
            }
        }
    }

    let total_blocked: u32 = report
        .per_rule
        .iter()
        .filter(|r| r.level == Level::Block)
        .map(|r| r.hits)
        .sum();
    let total_warned: u32 = report
        .per_rule
        .iter()
        .filter(|r| r.level == Level::Warn)
        .map(|r| r.hits)
        .sum();
    out.push('\n');
    out.push_str(&format!(
        "Total: {} blocked, {} warned across {} rules, {} files scanned in {}ms\n",
        total_blocked,
        total_warned,
        report.per_rule.len(),
        report.files_scanned,
        report.scan_duration_ms,
    ));
    out.push_str(&excluded_line(report));
    out
}

/// Render an `AuditReport` as JSON. Stable shape: `{generated_at,
/// scan_duration_ms, files_scanned, totals:{blocked,warned,rules},
/// rules:[{rule_id, level, hits, files:[{path,lines,details}]}]}`.
/// `details` is a per-hit array parallel to `lines`; entries are empty
/// strings for content/whole-file hits (which carry real line numbers)
/// and human-readable strings like `"ladder (9 let bindings)"` for
/// AST-predicate hits.
pub fn render_json(report: &AuditReport) -> String {
    let total_blocked: u32 = report
        .per_rule
        .iter()
        .filter(|r| r.level == Level::Block)
        .map(|r| r.hits)
        .sum();
    let total_warned: u32 = report
        .per_rule
        .iter()
        .filter(|r| r.level == Level::Warn)
        .map(|r| r.hits)
        .sum();
    let rules: Vec<_> = report
        .per_rule
        .iter()
        .map(|r| {
            let files: Vec<_> = r
                .files
                .iter()
                .map(|f| {
                    json!({
                        "path": f.path.display().to_string(),
                        "lines": f.lines,
                        "details": f.details,
                    })
                })
                .collect();
            json!({
                "rule_id": r.rule_id,
                "level": r.level.as_str(),
                "hits": r.hits,
                "files": files,
            })
        })
        .collect();
    let lexical_excluded: Vec<String> = report
        .lexical_excluded
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    let payload = json!({
        "generated_at": report.generated_at,
        "scan_duration_ms": report.scan_duration_ms,
        "files_scanned": report.files_scanned,
        "lexical_excluded": lexical_excluded,
        "totals": {
            "blocked": total_blocked,
            "warned": total_warned,
            "rules": report.per_rule.len(),
        },
        "rules": rules,
    });
    payload.to_string()
}

/// Render a `DebtTrend` as a human-readable terminal table.
///
/// Columns: rule id, one column per snapshot (ISO date), then delta.
/// Missing values render as an em-dash. Zero net change shows as `0 ·`.
pub fn render_trend_table(trend: &DebtTrend) -> String {
    if trend.snapshots_considered == 0 {
        return "no audit snapshots recorded yet; run audit_codebase to take the first one\n"
            .to_string();
    }
    if trend.rules.is_empty() {
        return "no rules tracked in any snapshot\n".to_string();
    }

    let id_width = trend
        .rules
        .iter()
        .map(|r| r.rule_id.as_str().len())
        .max()
        .unwrap_or(0)
        .max("Rule".len());

    // Column headers: ISO short date for each snapshot timestamp.
    let date_labels: Vec<String> = trend.rules[0]
        .history
        .iter()
        .map(|p| short_iso_date(p.ts))
        .collect();

    let mut out = String::new();
    out.push_str(&format!("{:<id_width$}", "Rule", id_width = id_width));
    for label in &date_labels {
        out.push_str(&format!("  {:>10}", label));
    }
    out.push_str("  \u{0394}\n");

    for r in &trend.rules {
        out.push_str(&format!("{:<id_width$}", r.rule_id, id_width = id_width));
        for p in &r.history {
            match p.hits {
                Some(h) => out.push_str(&format!("  {:>10}", h)),
                None => out.push_str(&format!("  {:>10}", "\u{2013}")),
            }
        }
        let arrow = if r.net_change < 0 {
            " \u{2193}"
        } else if r.net_change > 0 {
            " \u{2191}"
        } else {
            " \u{00b7}"
        };
        let signed = if r.net_change > 0 {
            format!("+{}", r.net_change)
        } else {
            r.net_change.to_string()
        };
        out.push_str(&format!("  {}{}\n", signed, arrow));
    }

    out.push('\n');
    if trend.snapshots_considered < 2 {
        out.push_str("(need at least 2 snapshots to compute trend)\n");
    } else {
        out.push_str(&format!(
            "{} snapshots considered, {} \u{2192} {}\n",
            trend.snapshots_considered,
            short_iso_date(trend.first_snapshot_ts),
            short_iso_date(trend.last_snapshot_ts),
        ));
    }
    out
}

/// Render a `DebtTrend` as JSON.
pub fn render_trend_json(trend: &DebtTrend) -> String {
    let rules: Vec<_> = trend
        .rules
        .iter()
        .map(|r| {
            let history: Vec<_> = r
                .history
                .iter()
                .map(|p| {
                    json!({
                        "ts": p.ts,
                        "hits": p.hits,
                    })
                })
                .collect();
            json!({
                "rule_id": r.rule_id,
                "level": r.level.as_str(),
                "history": history,
                "first_hits": r.first_hits,
                "last_hits": r.last_hits,
                "net_change": r.net_change,
            })
        })
        .collect();
    let payload = json!({
        "generated_at": trend.generated_at,
        "snapshots_considered": trend.snapshots_considered,
        "first_snapshot_ts": trend.first_snapshot_ts,
        "last_snapshot_ts": trend.last_snapshot_ts,
        "rules": rules,
    });
    payload.to_string()
}

pub fn short_iso_date(ts: u64) -> String {
    // YYYY-MM-DD without pulling chrono.
    let days = (ts / 86_400) as i64;
    let (y, m, d) = days_to_ymd(days);
    format!("{:04}-{:02}-{:02}", y, m, d)
}

// Civil calendar conversion from Howard Hinnant's date algorithms.
// http://howardhinnant.github.io/date_algorithms.html#civil_from_days
pub(super) fn days_to_ymd(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let (y, mp, d) = {
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = (z - era * 146_097) as u64; // [0, 146096]
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
        let y = yoe as i64 + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
        (y, mp, d)
    };
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m, d)
}
