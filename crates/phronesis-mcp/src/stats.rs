//! Reader for `.phronesis/log.jsonl`. Aggregates hook-firing history per
//! rule and renders a terminal table or a JSON payload. Pure functions:
//! the CLI handler does the I/O, this module only transforms data.

use crate::action_log::LogEntry;
use phr::RuleId;
use std::collections::BTreeMap;

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

/// Inputs to `aggregate_lifecycle`. Mirrors `StatsOpts` plus the kalpa filter,
/// which is meaningless for rule stats.
#[derive(Debug, Clone, Default)]
pub struct LifecycleOpts {
    pub since_secs: Option<u64>,
    /// When `Some(name)`, count only entries whose `kalpa` field equals it.
    pub kalpa: Option<String>,
    pub now_secs: u64,
}

/// Raw counts over the lifecycle entries of the action log. No ratios: a ratio
/// over an uncontrolled retention window misleads (spec §Non-goals).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LifecycleStats {
    pub kalpa: Option<String>,
    /// `ts` of the oldest lifecycle entry still readable, *before* the window
    /// and kalpa filters — the retention boundary the header quotes.
    pub oldest_entry_ts: Option<u64>,
    /// Distinct `sid` values among the counted entries.
    pub sessions: u32,
    pub events: BTreeMap<String, u32>,
    pub prompt_modes: BTreeMap<String, u32>,
    /// `subagent_start` entries: what was launched. Spec §Reporting:
    /// "`sub-agents` counts `subagent_start` records".
    pub subagents: u32,
    /// The subset that paired — `subagent_stop` entries with
    /// `matched_start: true`. A stop whose start rotated away is not one.
    pub subagents_matched: u32,
    /// Median over the **matched** pairs' durations alone.
    pub subagent_median_secs: Option<u64>,
    pub commits: u32,
    pub confidence_bands: BTreeMap<String, u32>,
    /// Prompts with mode `mid_turn` or `correction`: the human changed the
    /// plan rather than replying. The autonomy signal's numerator.
    pub interventions: u32,
}

impl LifecycleStats {
    /// `interventions / commits`, or `None` when there are no commits.
    pub fn interventions_per_commit(&self) -> Option<f64> {
        (self.commits > 0).then(|| f64::from(self.interventions) / f64::from(self.commits))
    }
}

/// Count the lifecycle entries of the action log. `entries` may hold any mix of
/// kinds; anything but `kind == "lifecycle"` is ignored.
pub fn aggregate_lifecycle(entries: &[LogEntry], opts: &LifecycleOpts) -> LifecycleStats {
    use std::collections::BTreeSet;

    let cutoff = opts
        .since_secs
        .map(|w| opts.now_secs.saturating_sub(w))
        .unwrap_or(0);
    let mut out = LifecycleStats {
        kalpa: opts.kalpa.clone(),
        ..LifecycleStats::default()
    };
    let mut sids: BTreeSet<&str> = BTreeSet::new();
    let mut durations: Vec<u64> = Vec::new();

    for e in entries {
        if e.kind != "lifecycle" {
            continue;
        }
        out.oldest_entry_ts = Some(match out.oldest_entry_ts {
            Some(t) => t.min(e.ts),
            None => e.ts,
        });
        if e.ts < cutoff {
            continue;
        }
        if let Some(k) = opts.kalpa.as_deref()
            && e.data.get("kalpa").and_then(|v| v.as_str()) != Some(k)
        {
            continue;
        }
        *out.events.entry(e.event.clone()).or_insert(0) += 1;
        // `kalpa_start` and `kalpa_end` are CLI boundary markers, not session
        // activity — their sid is the CLI's throwaway session, not an agent
        // session that worked under the kalpa.
        if !matches!(e.event.as_str(), "kalpa_start" | "kalpa_end")
            && let Some(sid) = e.data.get("sid").and_then(|v| v.as_str())
        {
            sids.insert(sid);
        }
        match e.event.as_str() {
            "prompt" => {
                let mode = e
                    .data
                    .get("mode")
                    .and_then(|v| v.as_str())
                    .unwrap_or("fresh");
                *out.prompt_modes.entry(mode.to_string()).or_insert(0) += 1;
                if matches!(mode, "mid_turn" | "correction") {
                    out.interventions += 1;
                }
            }
            "subagent_start" => out.subagents += 1,
            "subagent_stop" => {
                // Only a matched stop is a pair, and only a pair has a duration
                // worth a median: an unmatched stop's start rotated away or was
                // never recorded, so its `duration_secs` is absent or wrong.
                if e.data.get("matched_start").and_then(|v| v.as_bool()) == Some(true) {
                    out.subagents_matched += 1;
                    if let Some(d) = e.data.get("duration_secs").and_then(|v| v.as_u64()) {
                        durations.push(d);
                    }
                }
            }
            "commit" => {
                out.commits += 1;
                if let Some(b) = e.data.get("confidence_band").and_then(|v| v.as_str()) {
                    *out.confidence_bands.entry(b.to_string()).or_insert(0) += 1;
                }
            }
            _ => {}
        }
    }

    out.sessions = sids.len() as u32;
    durations.sort_unstable();
    out.subagent_median_secs = match durations.len() {
        0 => None,
        n if n % 2 == 1 => Some(durations[n / 2]),
        n => Some((durations[n / 2 - 1] + durations[n / 2]) / 2),
    };
    out
}

/// `30s` / `3m40s` / `1h04m` — compact enough for a report column.
pub fn humanize_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3_600 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h{:02}m", secs / 3_600, (secs % 3_600) / 60)
    }
}

/// The retention disclaimer every lifecycle report carries: the action log
/// rotates at 50 MiB keeping one predecessor, so a long kalpa's early events
/// are gone and the header must say from when the counts are honest.
pub fn retention_line(oldest_entry_ts: Option<u64>) -> String {
    match oldest_entry_ts {
        None => "counts since log entry (none)".to_string(),
        Some(ts) => {
            let when = chrono::DateTime::from_timestamp(ts as i64, 0)
                .map(|dt| {
                    dt.with_timezone(&chrono::Local)
                        .format("%Y-%m-%d %H:%M")
                        .to_string()
                })
                .unwrap_or_else(|| ts.to_string());
            format!("counts since log entry {when}")
        }
    }
}

/// The report block from SPEC-agent-lifecycle-events §Reporting. Callers print
/// the kalpa / retention header themselves.
pub fn render_lifecycle(s: &LifecycleStats) -> String {
    let n = |k: &str| s.events.get(k).copied().unwrap_or(0);
    let m = |k: &str| s.prompt_modes.get(k).copied().unwrap_or(0);
    let b = |k: &str| s.confidence_bands.get(k).copied().unwrap_or(0);
    let median = s
        .subagent_median_secs
        .map(|d| format!("   median {}", humanize_duration(d)))
        .unwrap_or_default();
    // Omitted entirely when no commit in the window carries a band: printing
    // `high 0  medium 0  low 0` reads as "we measured and found none", which is
    // not what happened (spec §Reporting).
    let bands = if s.confidence_bands.is_empty() {
        String::new()
    } else {
        format!(
            "   confidence at commit: high {}  medium {}  low {}",
            b("high"),
            b("medium"),
            b("low")
        )
    };
    let mut out = String::new();
    out.push_str(&format!("{:<13}{:>4}\n", "sessions", s.sessions));
    out.push_str(&format!(
        "{:<13}{:>4}   fresh {}   mid_turn {}   correction {}\n",
        "prompts",
        n("prompt"),
        m("fresh"),
        m("mid_turn"),
        m("correction")
    ));
    out.push_str(&format!(
        "{:<13}{:>4}   (mid_turn + correction)\n",
        "interventions", s.interventions
    ));
    out.push_str(&format!("{:<13}{:>4}\n", "interrupts", n("interrupt")));
    out.push_str(&format!(
        "{:<13}{:>4}   starts, {} matched{}\n",
        "sub-agents", s.subagents, s.subagents_matched, median
    ));
    // The disclaimer is part of the line, not a footnote: commits made outside a
    // shell tool call are invisible here, so the denominator is undercounted and
    // must say so wherever it is printed (spec §"Success signal: commit").
    out.push_str(&format!(
        "{:<13}{:>4}   (shell tool calls only){}\n",
        "commits", s.commits, bands
    ));
    if let Some(r) = s.interventions_per_commit() {
        out.push_str(&format!("interventions / commit   {r:.2}\n"));
    }
    out
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
    render_json_with_lifecycle(values, None)
}

/// `render_json` plus an optional `lifecycle` key. Additive: every existing key
/// keeps its name, position, and meaning.
pub fn render_json_with_lifecycle(values: &Stats, life: Option<&LifecycleStats>) -> String {
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
    let mut payload = json!({
        "window": values.window_label,
        "generated_at": values.generated_at,
        "totals": {
            "blocked": total_blocked,
            "warned": total_warned,
            "rules": values.per_rule.len(),
        },
        "rules": rules,
    });
    if let Some(l) = life
        && let Some(obj) = payload.as_object_mut()
    {
        obj.insert("lifecycle".to_string(), json!({
            "kalpa": l.kalpa, "oldest_entry_ts": l.oldest_entry_ts, "sessions": l.sessions,
            "events": l.events, "prompts": l.prompt_modes, "subagents": l.subagents,
            "subagents_matched": l.subagents_matched,
            "subagent_median_secs": l.subagent_median_secs, "commits": l.commits,
            "interventions": l.interventions, "interventions_per_commit": l.interventions_per_commit(),
            "confidence_bands": l.confidence_bands,
        }));
    }
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

    fn life(ts: u64, event: &str, fields: &[(&str, serde_json::Value)]) -> LogEntry {
        let mut e = LogEntry::new("lifecycle", event)
            .with("host", "claude")
            .with("sid", "s-1");
        e.ts = ts;
        for (k, v) in fields {
            e.data.insert((*k).to_string(), v.clone());
        }
        e
    }

    fn fixture_log() -> Vec<LogEntry> {
        let k = |name: &str| ("kalpa", json!(name));
        vec![
            life(
                100,
                "prompt",
                &[
                    ("mode", json!("fresh")),
                    k("k1"),
                    ("prompt_bytes", json!(12)),
                ],
            ),
            life(110, "prompt", &[("mode", json!("correction")), k("k1")]),
            life(
                120,
                "interrupt",
                &[k("k1"), ("inferred_from", json!("inflight"))],
            ),
            life(125, "subagent_start", &[k("k1"), ("agent_id", json!("a1"))]),
            life(126, "subagent_start", &[k("k1"), ("agent_id", json!("a2"))]),
            life(
                130,
                "subagent_stop",
                &[
                    k("k1"),
                    ("duration_secs", json!(10)),
                    ("matched_start", json!(true)),
                ],
            ),
            life(
                140,
                "subagent_stop",
                &[
                    k("k1"),
                    ("duration_secs", json!(220)),
                    ("matched_start", json!(true)),
                ],
            ),
            // Unmatched: its start rotated away, so it contributes no duration and
            // is not counted among the matched pairs.
            life(
                150,
                "subagent_stop",
                &[
                    k("k1"),
                    ("duration_secs", json!(30)),
                    ("matched_start", json!(false)),
                ],
            ),
            life(
                160,
                "commit",
                &[
                    k("k1"),
                    ("sha", json!("0f3c")),
                    ("confidence_band", json!("high")),
                ],
            ),
            life(
                170,
                "commit",
                &[
                    k("k1"),
                    ("sha", json!("aa11")),
                    ("confidence_band", json!("medium")),
                ],
            ),
            {
                let mut e = life(180, "prompt", &[("mode", json!("fresh")), k("k2")]);
                e.data.insert("sid".to_string(), json!("s-2"));
                e
            },
            hook_entry(190, "f", json!([cons("r1", "constraint_violation")])),
        ]
    }

    #[test]
    fn aggregate_lifecycle_counts_events_modes_subagents_and_commits() {
        let s = aggregate_lifecycle(
            &fixture_log(),
            &LifecycleOpts {
                since_secs: None,
                kalpa: Some("k1".into()),
                now_secs: 1_000,
            },
        );
        assert_eq!(s.sessions, 1, "only s-1 carries k1 entries");
        assert_eq!(s.events.get("prompt"), Some(&2));
        assert_eq!(s.events.get("interrupt"), Some(&1));
        assert_eq!(s.prompt_modes.get("fresh"), Some(&1));
        assert_eq!(s.prompt_modes.get("correction"), Some(&1));
        assert_eq!(s.subagents, 2, "two starts were launched");
        assert_eq!(s.subagents_matched, 2, "two of the three stops paired");
        assert_eq!(
            s.subagent_median_secs,
            Some(115),
            "matched durations 10 and 220 only; the unmatched stop's 30 does not count"
        );
        assert_eq!(s.commits, 2);
        assert_eq!(s.confidence_bands.get("high"), Some(&1));
        assert_eq!(s.confidence_bands.get("medium"), Some(&1));
        assert_eq!(
            s.interventions, 1,
            "one correction, no mid_turn, fresh does not count"
        );
        assert_eq!(s.interventions_per_commit(), Some(0.5));
    }

    #[test]
    fn aggregate_lifecycle_retention_boundary_ignores_filters() {
        // The boundary answers "how far back can this log answer at all", so it is
        // the oldest lifecycle entry on disk regardless of --since / --kalpa.
        let s = aggregate_lifecycle(
            &fixture_log(),
            &LifecycleOpts {
                since_secs: Some(20),
                kalpa: Some("k2".into()),
                now_secs: 180,
            },
        );
        assert_eq!(s.oldest_entry_ts, Some(100));
        assert_eq!(s.events.get("prompt"), Some(&1));
        assert_eq!(s.events.get("commit"), None, "the commits are k1");
        assert_eq!(s.sessions, 1);
    }

    #[test]
    fn aggregate_lifecycle_of_empty_log_is_all_zero() {
        let s = aggregate_lifecycle(&[], &LifecycleOpts::default());
        assert_eq!(s.sessions, 0);
        assert_eq!(s.commits, 0);
        assert_eq!(s.subagent_median_secs, None);
        assert_eq!(s.oldest_entry_ts, None);
    }

    #[test]
    fn render_lifecycle_matches_the_spec_block() {
        let s = aggregate_lifecycle(
            &fixture_log(),
            &LifecycleOpts {
                since_secs: None,
                kalpa: Some("k1".into()),
                now_secs: 1_000,
            },
        );
        let out = render_lifecycle(&s);
        assert!(out.contains("sessions        1"), "{out}");
        assert!(
            out.contains("prompts         2   fresh 1   mid_turn 0   correction 1"),
            "{out}"
        );
        assert!(
            out.contains("interventions   1   (mid_turn + correction)"),
            "{out}"
        );
        assert!(out.contains("interrupts      1"), "{out}");
        assert!(
            out.contains("sub-agents      2   starts, 2 matched   median 1m55s"),
            "{out}"
        );
        assert!(out.contains("commits         2   (shell tool calls only)   confidence at commit: high 1  medium 1  low 0"), "{out}");
        assert!(out.contains("interventions / commit   0.50"), "{out}");
        assert!(
            !out.contains("prompt_bytes"),
            "no raw field names leak: {out}"
        );
    }

    #[test]
    fn render_lifecycle_omits_ratio_without_commits() {
        let s = aggregate_lifecycle(
            &[],
            &LifecycleOpts {
                since_secs: None,
                kalpa: None,
                now_secs: 1_000,
            },
        );
        assert!(!render_lifecycle(&s).contains("interventions / commit"));
    }

    /// "The `confidence at commit` segment is omitted entirely when no commit in the
    /// window carries a band, rather than printing zeros" — printing `high 0 medium
    /// 0 low 0` reads as "we measured and found none", which is not what happened.
    #[test]
    fn render_lifecycle_omits_the_band_segment_when_no_commit_carries_one() {
        let entries = vec![life(100, "commit", &[("sha", json!("0f3c"))])];
        let s = aggregate_lifecycle(&entries, &LifecycleOpts::default());
        let out = render_lifecycle(&s);
        assert!(
            out.contains("commits         1   (shell tool calls only)"),
            "{out}"
        );
        assert!(!out.contains("confidence at commit"), "{out}");
        assert!(!out.contains("low 0"), "{out}");
    }

    /// The commit line always carries its own disclaimer, because the denominator
    /// is read honestly or not at all: commits made outside a shell tool call — in
    /// another terminal, through a host's commit UI, by a wrapper script — are
    /// invisible here (spec §"Success signal: commit").
    #[test]
    fn the_commit_line_says_where_its_commits_came_from() {
        let s = aggregate_lifecycle(&fixture_log(), &LifecycleOpts::default());
        assert!(render_lifecycle(&s).contains("(shell tool calls only)"));
    }

    #[test]
    fn humanize_duration_and_retention_line_formats() {
        assert_eq!(humanize_duration(30), "30s");
        assert_eq!(humanize_duration(220), "3m40s");
        assert_eq!(humanize_duration(3_840), "1h04m");
        assert_eq!(retention_line(None), "counts since log entry (none)");
        assert!(retention_line(Some(1_700_000_000)).starts_with("counts since log entry 20"));
    }

    /// Both `stats` and `kalpa show` read `.phronesis/log.jsonl` **and its one
    /// rotated predecessor**, because a long kalpa's early events are in the
    /// rotated file and the median would otherwise be computed over half the data.
    /// `action_log::read_recent` with `limit: None` already reads both, oldest
    /// first; this pins it, because "for free" is exactly the kind of claim that
    /// stops being true.
    #[test]
    fn lifecycle_entries_are_read_from_the_rotated_predecessor_too() {
        use crate::action_log::{self, ReadOpts};
        let dir = tempfile::tempdir().unwrap();
        let path = action_log::default_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let line = |ts: u64, secs: u64| {
            format!(
                r#"{{"ts":{ts},"kind":"lifecycle","event":"subagent_stop","host":"claude","sid":"s-1","seq":{ts},"duration_secs":{secs},"matched_start":true}}"#
            )
        };
        std::fs::write(
            path.with_file_name("log.jsonl.1"),
            format!("{}\n{}\n", line(10, 10), line(20, 20)),
        )
        .unwrap();
        std::fs::write(&path, format!("{}\n", line(30, 300))).unwrap();

        let entries = action_log::read_recent(
            &path,
            &ReadOpts {
                kind: Some("lifecycle".to_string()),
                ..ReadOpts::default()
            },
        )
        .unwrap();
        assert_eq!(entries.len(), 3, "the rotated predecessor is read");
        let s = aggregate_lifecycle(&entries, &LifecycleOpts::default());
        assert_eq!(
            s.subagent_median_secs,
            Some(20),
            "median over all three, not just the current file"
        );
        assert_eq!(
            s.oldest_entry_ts,
            Some(10),
            "and the boundary is the oldest of the pair"
        );
    }

    #[test]
    fn render_json_with_lifecycle_adds_a_key_without_moving_the_others() {
        let values = Stats {
            window_label: "7d".into(),
            generated_at: 1,
            per_rule: vec![],
        };
        let s = aggregate_lifecycle(
            &fixture_log(),
            &LifecycleOpts {
                since_secs: None,
                kalpa: None,
                now_secs: 1_000,
            },
        );
        let v: serde_json::Value =
            serde_json::from_str(&render_json_with_lifecycle(&values, Some(&s))).unwrap();
        assert_eq!(v["window"], "7d");
        assert_eq!(v["totals"]["rules"], 0);
        assert_eq!(v["lifecycle"]["commits"], 2);
        assert_eq!(v["lifecycle"]["prompts"]["correction"], 1);
        let plain: serde_json::Value = serde_json::from_str(&render_json(&values)).unwrap();
        assert!(
            plain.get("lifecycle").is_none(),
            "render_json stays byte-compatible"
        );
    }
}
