//! Tests for audit trend computation. Split from the original `audit.rs`
//! `mod tests`.

use crate::action_log::LogEntry;
use crate::audit::*;
use serde_json::json;

// ── compute_trend tests ───────────────────────────────────────────────────

fn audit_entry(ts: u64, per_rule: serde_json::Value) -> LogEntry {
    let mut e = LogEntry::new("mcp", "audit_codebase")
        .with("files_scanned", 100u64)
        .with("blocked_total", 0u64)
        .with("warned_total", 0u64);
    e.data.insert("per_rule".to_string(), per_rule);
    e.ts = ts;
    e
}

#[test]
fn compute_trend_returns_empty_with_no_snapshots() {
    let trend = compute_trend(&[], &TrendOpts::default());
    assert!(trend.rules.is_empty());
    assert_eq!(trend.snapshots_considered, 0);
}

#[test]
fn compute_trend_computes_net_change_for_single_rule() {
    let snaps = vec![
        audit_entry(
            1_700_000_000,
            json!({"no-unwrap": {"level":"block","hits":18}}),
        ),
        audit_entry(
            1_700_500_000,
            json!({"no-unwrap": {"level":"block","hits":14}}),
        ),
        audit_entry(
            1_701_000_000,
            json!({"no-unwrap": {"level":"block","hits":10}}),
        ),
    ];
    let trend = compute_trend(&snaps, &TrendOpts::default());
    assert_eq!(trend.snapshots_considered, 3);
    assert_eq!(trend.rules.len(), 1);
    let r = &trend.rules[0];
    assert_eq!(r.rule_id, "no-unwrap");
    assert_eq!(r.level, Level::Block);
    assert_eq!(r.first_hits, 18);
    assert_eq!(r.last_hits, 10);
    assert_eq!(r.net_change, -8);
    assert_eq!(r.history.len(), 3);
}

#[test]
fn compute_trend_respects_last_limit() {
    let snaps = vec![
        audit_entry(1, json!({"r": {"level":"block","hits":1}})),
        audit_entry(2, json!({"r": {"level":"block","hits":2}})),
        audit_entry(3, json!({"r": {"level":"block","hits":3}})),
        audit_entry(4, json!({"r": {"level":"block","hits":4}})),
    ];
    let trend = compute_trend(
        &snaps,
        &TrendOpts {
            last: Some(2),
            ..TrendOpts::default()
        },
    );
    assert_eq!(trend.snapshots_considered, 2);
    assert_eq!(trend.rules[0].first_hits, 3);
    assert_eq!(trend.rules[0].last_hits, 4);
}

#[test]
fn compute_trend_respects_rule_filter() {
    let snaps = vec![
        audit_entry(
            1,
            json!({"a":{"level":"block","hits":1},"b":{"level":"block","hits":5}}),
        ),
        audit_entry(
            2,
            json!({"a":{"level":"block","hits":2},"b":{"level":"block","hits":6}}),
        ),
    ];
    let trend = compute_trend(
        &snaps,
        &TrendOpts {
            rule_filter: Some("a".to_string()),
            ..TrendOpts::default()
        },
    );
    assert_eq!(trend.rules.len(), 1);
    assert_eq!(trend.rules[0].rule_id, "a");
}

#[test]
fn compute_trend_handles_rule_appearing_mid_series() {
    let snaps = vec![
        audit_entry(1, json!({"a":{"level":"block","hits":10}})),
        audit_entry(
            2,
            json!({"a":{"level":"block","hits":8},"b":{"level":"warn","hits":3}}),
        ),
    ];
    let trend = compute_trend(&snaps, &TrendOpts::default());
    let b = trend.rules.iter().find(|r| r.rule_id == "b").unwrap();
    // b only appears in the second snapshot — first_hits should reflect that.
    assert_eq!(b.first_hits, 3);
    assert_eq!(b.last_hits, 3);
    assert_eq!(b.net_change, 0);
    assert_eq!(b.history.len(), 2);
    assert_eq!(b.history[0].hits, None);
    assert_eq!(b.history[1].hits, Some(3));
}

#[test]
fn compute_trend_sorts_by_net_change_ascending() {
    let snaps = vec![
        audit_entry(
            1,
            json!({"big-improve":{"level":"block","hits":50},"regress":{"level":"block","hits":5}}),
        ),
        audit_entry(
            2,
            json!({"big-improve":{"level":"block","hits":20},"regress":{"level":"block","hits":8}}),
        ),
    ];
    let trend = compute_trend(&snaps, &TrendOpts::default());
    // big-improve (-30) should come before regress (+3)
    assert_eq!(trend.rules[0].rule_id, "big-improve");
    assert_eq!(trend.rules[1].rule_id, "regress");
}

#[test]
fn compute_trend_only_one_snapshot_yields_zero_net_change() {
    let snaps = vec![audit_entry(1, json!({"r":{"level":"block","hits":5}}))];
    let trend = compute_trend(&snaps, &TrendOpts::default());
    assert_eq!(trend.snapshots_considered, 1);
    assert_eq!(trend.rules[0].net_change, 0);
    assert_eq!(trend.rules[0].first_hits, 5);
    assert_eq!(trend.rules[0].last_hits, 5);
}
