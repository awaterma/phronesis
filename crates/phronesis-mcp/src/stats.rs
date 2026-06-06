//! Reader for `.phronesis/log.jsonl`. Aggregates hook-firing history per
//! rule and renders a terminal table or a JSON payload. Pure functions:
//! the CLI handler does the I/O, this module only transforms data.

use crate::action_log::LogEntry;
use phr::RuleId;

/// Inputs to `aggregate`. Built by the CLI handler from clap args.
#[derive(Debug, Clone, Default)]
pub struct StatsOpts {
    /// Window in seconds. `None` means "all time" — no time filter.
    pub since_secs: Option<u64>,
    /// When `Some`, restrict output to a single rule id.
    pub rule_filter: Option<String>,
    /// Unix seconds, injected so tests are deterministic.
    pub now_secs: u64,
}

/// Aggregated view across one or more log entries.
#[derive(Debug, Clone, PartialEq)]
pub struct Stats {
    /// Human-readable window label for headers/JSON, e.g. `"7d"` or `"all time"`.
    pub window_label: String,
    /// Unix seconds at which the snapshot was produced.
    pub generated_at: u64,
    /// One entry per rule that fired at least once in the window, sorted by
    /// `blocked + warned` descending.
    pub per_rule: Vec<RuleStats>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RuleStats {
    pub rule_id: RuleId,
    pub blocked: u32,
    pub warned: u32,
    /// Most recent fire timestamp (unix seconds). `0` when the rule never
    /// fired — but rules with zero fires aren't included in `Stats.per_rule`,
    /// so this is always > 0 for any returned `RuleStats`.
    pub last_fired_ts: u64,
}

/// Parse a duration string like `30m`, `24h`, `7d`, `2w` into seconds.
/// Returns `None` for any input that doesn't match. Callers should warn
/// and fall back to "all time" on `None`.
pub fn parse_since(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.len() < 2 {
        return None;
    }
    let (num, unit) = s.split_at(s.len() - 1);
    let n: u64 = num.parse().ok()?;
    let secs_per_unit: u64 = match unit {
        "m" => 60,
        "h" => 3_600,
        "d" => 86_400,
        "w" => 7 * 86_400,
        _ => return None,
    };
    n.checked_mul(secs_per_unit)
}

/// Build a `Stats` snapshot from a slice of log entries. Walks each
/// entry's `consequences` array and increments the matching rule's
/// counters. Entries with no consequences, and consequences whose
/// `action_type` is neither `constraint_violation` nor `constraint_warning`,
/// contribute nothing.
///
/// `opts.since_secs` and `opts.rule_filter` are applied here.
/// `opts.now_secs` is propagated into `Stats.generated_at` so callers can
/// pin output for tests.
pub fn aggregate(entries: &[LogEntry], opts: &StatsOpts) -> Stats {
    use std::collections::HashMap;

    let mut by_id: HashMap<String, RuleStats> = HashMap::new();
    let cutoff = opts
        .since_secs
        .map(|w| opts.now_secs.saturating_sub(w))
        .unwrap_or(0);

    for entry in entries {
        if entry.ts < cutoff {
            continue;
        }
        let Some(consequences) = entry.data.get("consequences").and_then(|v| v.as_array()) else {
            continue;
        };
        for c in consequences {
            let Some(rule_id) = c.get("rule_id").and_then(|v| v.as_str()) else {
                continue;
            };
            if let Some(filter) = opts.rule_filter.as_deref()
                && rule_id != filter
            {
                continue;
            }
            let action_type = c.get("action_type").and_then(|v| v.as_str()).unwrap_or("");
            let row = by_id.entry(rule_id.to_string()).or_insert(RuleStats {
                rule_id: rule_id.into(),
                blocked: 0,
                warned: 0,
                last_fired_ts: 0,
            });
            match action_type {
                "constraint_violation" => row.blocked += 1,
                "constraint_warning" => row.warned += 1,
                _ => continue,
            }
            if entry.ts > row.last_fired_ts {
                row.last_fired_ts = entry.ts;
            }
        }
    }

    // Drop rules whose only consequences were unknown action types (they
    // hit `or_insert` above but never got a counter bump).
    let mut per_rule: Vec<RuleStats> = by_id
        .into_values()
        .filter(|r| r.blocked + r.warned > 0)
        .collect();
    per_rule.sort_by(|a, b| {
        (b.blocked + b.warned)
            .cmp(&(a.blocked + a.warned))
            .then_with(|| a.rule_id.cmp(&b.rule_id))
    });

    Stats {
        window_label: window_label(opts.since_secs),
        generated_at: opts.now_secs,
        per_rule,
    }
}

fn window_label(since_secs: Option<u64>) -> String {
    let Some(s) = since_secs else {
        return "all time".to_string();
    };
    if s % (7 * 86_400) == 0 {
        format!("{}w", s / (7 * 86_400))
    } else if s % 86_400 == 0 {
        format!("{}d", s / 86_400)
    } else if s % 3_600 == 0 {
        format!("{}h", s / 3_600)
    } else if s % 60 == 0 {
        format!("{}m", s / 60)
    } else {
        format!("{}s", s)
    }
}

/// Render a human-readable table summary. Columns are width-padded to the
/// longest rule_id in the snapshot so the policy looks clean in a
/// terminal.
pub fn render_table(values: &Stats) -> String {
    if values.per_rule.is_empty() {
        return "no phronesis activity recorded yet\n".to_string();
    }
    let id_width = values
        .per_rule
        .iter()
        .map(|r| r.rule_id.as_str().len())
        .max()
        .unwrap_or(0)
        .max("Rule".len());

    let mut out = String::new();
    out.push_str(&format!(
        "{:<id_width$}  Blocked  Warned  Last fired\n",
        "Rule",
        id_width = id_width
    ));
    for r in &values.per_rule {
        let ago = humanize_ago(values.generated_at.saturating_sub(r.last_fired_ts));
        out.push_str(&format!(
            "{:<id_width$}  {:>7}  {:>6}  {} ago\n",
            r.rule_id,
            r.blocked,
            r.warned,
            ago,
            id_width = id_width,
        ));
    }

    let total_blocked: u32 = values.per_rule.iter().map(|r| r.blocked).sum();
    let total_warned: u32 = values.per_rule.iter().map(|r| r.warned).sum();
    out.push('\n');
    out.push_str(&format!(
        "Total: {} blocked, {} warned across {} rules (window: {})\n",
        total_blocked,
        total_warned,
        values.per_rule.len(),
        values.window_label,
    ));
    out
}

fn humanize_ago(secs: u64) -> String {
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3_600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3_600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

use serde_json::json;

/// Render the same `Stats` snapshot as a JSON object. Stable key order
/// inside the envelope is `window`, `generated_at`, `totals`, `rules`.
/// Per-rule keys: `rule_id`, `blocked`, `warned`, `last_fired_ts`.
pub fn render_json(values: &Stats) -> String {
    let total_blocked: u32 = values.per_rule.iter().map(|r| r.blocked).sum();
    let total_warned: u32 = values.per_rule.iter().map(|r| r.warned).sum();
    let rules: Vec<_> = values
        .per_rule
        .iter()
        .map(|r| {
            json!({
                "rule_id": r.rule_id,
                "blocked": r.blocked,
                "warned": r.warned,
                "last_fired_ts": r.last_fired_ts,
            })
        })
        .collect();
    let payload = json!({
        "window": values.window_label,
        "generated_at": values.generated_at,
        "totals": {
            "blocked": total_blocked,
            "warned": total_warned,
            "rules": values.per_rule.len(),
        },
        "rules": rules,
    });
    payload.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_since_handles_minutes_hours_days_weeks() {
        assert_eq!(parse_since("30m"), Some(30 * 60));
        assert_eq!(parse_since("24h"), Some(24 * 3_600));
        assert_eq!(parse_since("7d"), Some(7 * 86_400));
        assert_eq!(parse_since("2w"), Some(2 * 7 * 86_400));
    }

    #[test]
    fn parse_since_rejects_garbage() {
        assert_eq!(parse_since(""), None);
        assert_eq!(parse_since("x"), None);
        assert_eq!(parse_since("7"), None); // missing unit
        assert_eq!(parse_since("7y"), None); // unsupported unit
        assert_eq!(parse_since("-1h"), None); // negative — u64 parse fails
        assert_eq!(parse_since("12.5h"), None);
    }

    #[test]
    fn parse_since_tolerates_whitespace() {
        assert_eq!(parse_since("  7d  "), Some(7 * 86_400));
    }

    use crate::action_log::LogEntry;
    use serde_json::json;

    fn hook_entry(ts: u64, file: &str, consequences: serde_json::Value) -> LogEntry {
        let mut e = LogEntry::new("hook", "pre_check")
            .with("phase", "pre")
            .with("tool", "Edit")
            .with("file", file.to_string())
            .with("exit", 2);
        e.data.insert("consequences".to_string(), consequences);
        e.ts = ts;
        e
    }

    fn cons(rule_id: &str, action_type: &str) -> serde_json::Value {
        json!({
            "rule_id": rule_id,
            "action_type": action_type,
            "message": "m",
            "bindings": {}
        })
    }

    #[test]
    fn aggregate_empty_yields_empty_values() {
        let opts = StatsOpts {
            now_secs: 1_700_000_000,
            ..StatsOpts::default()
        };
        let values = aggregate(&[], &opts);
        assert!(values.per_rule.is_empty());
        assert_eq!(values.window_label, "all time");
        assert_eq!(values.generated_at, 1_700_000_000);
    }

    #[test]
    fn aggregate_counts_blocked_and_warned_separately() {
        let entries = vec![hook_entry(
            1_700_000_000,
            "src/a.rs",
            json!([
                cons("r1", "constraint_violation"),
                cons("r1", "constraint_warning"),
            ]),
        )];
        let values = aggregate(
            &entries,
            &StatsOpts {
                now_secs: 1_700_000_000,
                ..StatsOpts::default()
            },
        );
        assert_eq!(values.per_rule.len(), 1);
        assert_eq!(values.per_rule[0].rule_id, "r1");
        assert_eq!(values.per_rule[0].blocked, 1);
        assert_eq!(values.per_rule[0].warned, 1);
    }

    #[test]
    fn aggregate_groups_by_rule_id_across_entries() {
        let entries = vec![
            hook_entry(
                1_700_000_000,
                "src/a.rs",
                json!([cons("r1", "constraint_violation")]),
            ),
            hook_entry(
                1_700_000_010,
                "src/b.rs",
                json!([cons("r1", "constraint_violation")]),
            ),
            hook_entry(
                1_700_000_020,
                "src/c.rs",
                json!([cons("r2", "constraint_warning")]),
            ),
        ];
        let values = aggregate(
            &entries,
            &StatsOpts {
                now_secs: 1_700_000_100,
                ..StatsOpts::default()
            },
        );
        assert_eq!(values.per_rule.len(), 2);
        let r1 = values.per_rule.iter().find(|r| r.rule_id == "r1").unwrap();
        assert_eq!(r1.blocked, 2);
        let r2 = values.per_rule.iter().find(|r| r.rule_id == "r2").unwrap();
        assert_eq!(r2.warned, 1);
    }

    #[test]
    fn aggregate_sorts_by_total_descending() {
        let entries = vec![
            hook_entry(1, "f", json!([cons("low", "constraint_warning")])),
            hook_entry(
                2,
                "f",
                json!([
                    cons("high", "constraint_violation"),
                    cons("high", "constraint_violation"),
                    cons("high", "constraint_warning"),
                ]),
            ),
            hook_entry(
                3,
                "f",
                json!([
                    cons("mid", "constraint_warning"),
                    cons("mid", "constraint_warning"),
                ]),
            ),
        ];
        let values = aggregate(
            &entries,
            &StatsOpts {
                now_secs: 1_000,
                ..StatsOpts::default()
            },
        );
        let ids: Vec<_> = values.per_rule.iter().map(|r| r.rule_id.as_str()).collect();
        assert_eq!(ids, vec!["high", "mid", "low"]);
    }

    #[test]
    fn aggregate_tracks_last_fired_ts() {
        let entries = vec![
            hook_entry(100, "f", json!([cons("r1", "constraint_violation")])),
            hook_entry(200, "f", json!([cons("r1", "constraint_violation")])),
            hook_entry(150, "f", json!([cons("r1", "constraint_violation")])), // out-of-order
        ];
        let values = aggregate(
            &entries,
            &StatsOpts {
                now_secs: 1_000,
                ..StatsOpts::default()
            },
        );
        assert_eq!(values.per_rule[0].last_fired_ts, 200);
    }

    #[test]
    fn aggregate_ignores_passing_entries() {
        let entries = vec![hook_entry(1, "f", json!([]))];
        let values = aggregate(
            &entries,
            &StatsOpts {
                now_secs: 100,
                ..StatsOpts::default()
            },
        );
        assert!(values.per_rule.is_empty());
    }

    #[test]
    fn aggregate_ignores_unknown_action_types() {
        let entries = vec![hook_entry(
            1,
            "f",
            json!([cons("r1", "log"), cons("r1", "something_else")]),
        )];
        let values = aggregate(
            &entries,
            &StatsOpts {
                now_secs: 100,
                ..StatsOpts::default()
            },
        );
        assert!(
            values.per_rule.is_empty(),
            "non-decision action types must not produce a row"
        );
    }

    #[test]
    fn aggregate_respects_since_window() {
        // now = 1_700_000_000; window = 1h → cutoff = 1_700_000_000 - 3600
        let entries = vec![
            hook_entry(
                1_699_996_000,
                "f",
                json!([cons("old", "constraint_violation")]),
            ), // before cutoff
            hook_entry(
                1_699_999_500,
                "f",
                json!([cons("new", "constraint_violation")]),
            ), // after cutoff
        ];
        let opts = StatsOpts {
            since_secs: Some(3_600),
            now_secs: 1_700_000_000,
            ..StatsOpts::default()
        };
        let values = aggregate(&entries, &opts);
        assert_eq!(values.per_rule.len(), 1);
        assert_eq!(values.per_rule[0].rule_id, "new");
        assert_eq!(values.window_label, "1h");
    }

    #[test]
    fn aggregate_respects_rule_filter() {
        let entries = vec![
            hook_entry(1, "f", json!([cons("r1", "constraint_violation")])),
            hook_entry(2, "f", json!([cons("r2", "constraint_violation")])),
            hook_entry(3, "f", json!([cons("r3", "constraint_violation")])),
        ];
        let opts = StatsOpts {
            rule_filter: Some("r2".to_string()),
            now_secs: 100,
            ..StatsOpts::default()
        };
        let values = aggregate(&entries, &opts);
        assert_eq!(values.per_rule.len(), 1);
        assert_eq!(values.per_rule[0].rule_id, "r2");
    }

    #[test]
    fn window_label_picks_largest_clean_unit() {
        assert_eq!(window_label(None), "all time");
        assert_eq!(window_label(Some(60)), "1m");
        assert_eq!(window_label(Some(3_600)), "1h");
        assert_eq!(window_label(Some(86_400)), "1d");
        assert_eq!(window_label(Some(7 * 86_400)), "1w");
        assert_eq!(window_label(Some(2 * 7 * 86_400)), "2w");
        // 36 hours = 1.5d → falls through to hours
        assert_eq!(window_label(Some(36 * 3_600)), "36h");
        // 90 seconds → falls through to seconds (no clean minute)
        assert_eq!(window_label(Some(90)), "90s");
    }

    fn rule_value(id: &str, blocked: u32, warned: u32, last: u64) -> RuleStats {
        RuleStats {
            rule_id: id.into(),
            blocked,
            warned,
            last_fired_ts: last,
        }
    }

    #[test]
    fn render_table_renders_empty_message() {
        let values = Stats {
            window_label: "all time".to_string(),
            generated_at: 1_700_000_000,
            per_rule: vec![],
        };
        let out = render_table(&values);
        assert!(out.contains("no phronesis activity recorded yet"));
    }

    #[test]
    fn render_table_includes_header_and_rows_and_totals() {
        let values = Stats {
            window_label: "7d".to_string(),
            generated_at: 1_700_000_000,
            per_rule: vec![
                rule_value("no-unwrap", 14, 0, 1_699_999_880),
                rule_value("clone-heavy", 0, 23, 1_699_999_700),
            ],
        };
        let out = render_table(&values);
        // Header
        assert!(out.contains("Rule"));
        assert!(out.contains("Blocked"));
        assert!(out.contains("Warned"));
        assert!(out.contains("Last fired"));
        // Rows
        assert!(out.contains("no-unwrap"));
        assert!(out.contains("14"));
        assert!(out.contains("clone-heavy"));
        assert!(out.contains("23"));
        // Totals
        assert!(out.contains("Total: 14 blocked, 23 warned across 2 rules"));
        assert!(out.contains("window: 7d"));
    }

    #[test]
    fn render_table_humanizes_last_fired() {
        let values = Stats {
            window_label: "all time".to_string(),
            generated_at: 1_700_000_000,
            per_rule: vec![rule_value("r", 1, 0, 1_700_000_000 - 120)],
        };
        let out = render_table(&values);
        assert!(out.contains("2m ago"), "expected '2m ago' in:\n{}", out);
    }

    #[test]
    fn render_json_shape_for_populated_values() {
        let values = Stats {
            window_label: "7d".to_string(),
            generated_at: 1_700_000_000,
            per_rule: vec![
                rule_value("no-unwrap", 14, 0, 1_699_999_880),
                rule_value("clone-heavy", 0, 23, 1_699_999_700),
            ],
        };
        let out = render_json(&values);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["window"], "7d");
        assert_eq!(v["generated_at"], 1_700_000_000);
        assert_eq!(v["totals"]["blocked"], 14);
        assert_eq!(v["totals"]["warned"], 23);
        assert_eq!(v["totals"]["rules"], 2);
        let rules = v["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0]["rule_id"], "no-unwrap");
        assert_eq!(rules[0]["blocked"], 14);
        assert_eq!(rules[0]["last_fired_ts"], 1_699_999_880);
    }

    #[test]
    fn render_json_shape_for_empty_values() {
        let values = Stats {
            window_label: "all time".to_string(),
            generated_at: 1_700_000_000,
            per_rule: vec![],
        };
        let out = render_json(&values);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["rules"].as_array().unwrap().len(), 0);
        assert_eq!(v["totals"]["blocked"], 0);
        assert_eq!(v["totals"]["warned"], 0);
        assert_eq!(v["totals"]["rules"], 0);
    }
}
