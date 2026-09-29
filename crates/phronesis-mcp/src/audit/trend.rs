//! Audit debt-over-time trend computation. Split from the original
//! `audit.rs`; functions moved verbatim.

use std::collections::BTreeMap;

use crate::action_log::LogEntry;
use phr::RuleId;

use super::types::Level;

// ── Trend types and compute_trend ───────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct TrendOpts {
    /// Most-recent N snapshots. Default: all available.
    pub last: Option<usize>,
    /// Window in seconds (overrides `last` when set).
    pub since_secs: Option<u64>,
    pub rule_filter: Option<String>,
    /// Unix seconds, injected so tests are deterministic.
    pub now_secs: u64,
}

#[derive(Debug, Clone)]
pub struct DebtTrend {
    pub generated_at: u64,
    pub snapshots_considered: u32,
    pub first_snapshot_ts: u64,
    pub last_snapshot_ts: u64,
    /// Sorted by `net_change` ascending (biggest improvements first).
    pub rules: Vec<RuleTrend>,
}

#[derive(Debug, Clone)]
pub struct RuleTrend {
    pub rule_id: RuleId,
    pub level: Level,
    pub history: Vec<TrendPoint>,
    pub first_hits: u32,
    pub last_hits: u32,
    /// `last_hits - first_hits`. Negative = improvement.
    pub net_change: i32,
}

#[derive(Debug, Clone)]
pub struct TrendPoint {
    pub ts: u64,
    /// `None` when the rule wasn't present in this snapshot. Renderers
    /// show this as `–`.
    pub hits: Option<u32>,
}

/// Accumulate per-rule trend data from a windowed set of audit snapshots.
/// Respects `rule_filter`; returns sorted `Vec<RuleTrend>` (improvements first).
pub(super) fn rule_trends(snapshots: &[&LogEntry], rule_filter: Option<&str>) -> Vec<RuleTrend> {
    let mut rule_ids: BTreeMap<String, Level> = BTreeMap::new();
    for snap in snapshots {
        let Some(per_rule) = snap.data.get("per_rule").and_then(|v| v.as_object()) else {
            continue;
        };
        for (id, v) in per_rule {
            if let Some(filter) = rule_filter
                && id != filter
            {
                continue;
            }
            let rank_str = v.get("level").and_then(|x| x.as_str()).unwrap_or("");
            let level = match rank_str {
                "warn" => Level::Warn,
                _ => Level::Block,
            };
            rule_ids.entry(id.clone()).or_insert(level);
        }
    }
    let mut rules: Vec<RuleTrend> = rule_ids
        .into_iter()
        .map(|(rule_id, level)| {
            let history: Vec<TrendPoint> = snapshots
                .iter()
                .map(|snap| {
                    let hits = snap
                        .data
                        .get("per_rule")
                        .and_then(|v| v.as_object())
                        .and_then(|m| m.get(&rule_id))
                        .and_then(|v| v.get("hits"))
                        .and_then(|v| v.as_u64())
                        .map(|n| n as u32);
                    TrendPoint { ts: snap.ts, hits }
                })
                .collect();
            // first_hits = first non-None in history; last_hits = last non-None.
            let first_hits = history.iter().find_map(|p| p.hits).unwrap_or(0);
            let last_hits = history.iter().rev().find_map(|p| p.hits).unwrap_or(0);
            let net_change = (last_hits as i32) - (first_hits as i32);
            RuleTrend {
                rule_id: rule_id.into(),
                level,
                history,
                first_hits,
                last_hits,
                net_change,
            }
        })
        .collect();
    // Sort: improvements first (most-negative net_change), then by rule id.
    rules.sort_by(|a, b| {
        a.net_change
            .cmp(&b.net_change)
            .then_with(|| a.rule_id.cmp(&b.rule_id))
    });
    rules
}

/// Compute a debt-over-time view from a slice of audit log entries.
/// `entries` may contain non-`audit_codebase` events; they're skipped.
/// Snapshots are taken in chronological order; `last`/`since_secs` slice
/// the most recent window. Rules absent from a snapshot get a `None`
/// `TrendPoint` at that timestamp (no zero-substitution).
pub fn compute_trend(entries: &[LogEntry], opts: &TrendOpts) -> DebtTrend {
    // Resolve wall-clock `now`, filter to audit snapshots, and apply the
    // window (since_secs or last-N) in one block so the temporaries
    // (`cutoff`, `skip`) don't leak into the outer scope.
    let (now, snapshots) = {
        let now = if opts.now_secs == 0 {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        } else {
            opts.now_secs
        };
        let mut snaps: Vec<&LogEntry> = entries
            .iter()
            .filter(|e| e.event == "audit_codebase")
            .collect();
        snaps.sort_by_key(|e| e.ts);
        if let Some(since) = opts.since_secs {
            let cutoff = now.saturating_sub(since);
            snaps.retain(|e| e.ts >= cutoff);
        } else if let Some(n) = opts.last
            && snaps.len() > n
        {
            let skip = snaps.len() - n;
            snaps.drain(0..skip);
        }
        (now, snaps)
    };

    if snapshots.is_empty() {
        return DebtTrend {
            generated_at: now,
            snapshots_considered: 0,
            first_snapshot_ts: 0,
            last_snapshot_ts: 0,
            rules: Vec::new(),
        };
    }

    let first_ts = snapshots
        .first()
        .expect("snapshots non-empty: early-return above guards this")
        .ts;
    let last_ts = snapshots
        .last()
        .expect("snapshots non-empty: early-return above guards this")
        .ts;
    let rules = rule_trends(&snapshots, opts.rule_filter.as_deref());

    DebtTrend {
        generated_at: now,
        snapshots_considered: snapshots.len() as u32,
        first_snapshot_ts: first_ts,
        last_snapshot_ts: last_ts,
        rules,
    }
}
