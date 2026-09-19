# Lifecycle Events — Plan 5: Reporting (spec step 5)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the lifecycle events written by Plans 1–4 visible: a `lifecycle` section in `phr-mcp stats`, a per-kalpa report in `phr-mcp kalpa show`, lifecycle records and corrections in `phr-mcp journey` and `get_journey`, and two Prometheus families.

**Architecture:** All counting happens in one pure function, `stats::aggregate_lifecycle`, over `LogEntry` values read from `.phronesis/log.jsonl` (and its rotated `.1` predecessor). `stats` and `kalpa show` both call it, so the two surfaces cannot disagree. `journey_cli` gains a record-level view (the journal, not facts) for the `⟂` rendering and an action-log view for `--corrections` — the only surface where prompt text appears. `phronesis-metrics` adds one `match` arm.

**Tech Stack:** Rust 2024 edition (rust-version 1.90), serde/serde_json, chrono (already a `phronesis-mcp` dependency), `prometheus_client` `Counter`/`Family`/`Histogram`. No new dependencies.

**Spec:** `docs/specs/SPEC-agent-lifecycle-events.md` (revised 2026-09-18) — §"Action log" (stats and metrics paragraphs), §"Outcomes and kalpas / Reporting", §"CLI and MCP surface".

**Depends on:** Plan 1 (`docs/superpowers/plans/2026-09-18-lifecycle-events-1-foundation.md`) for every type it consumes, and merges **last**, after Plans 2, 3 and 4. It can be *written and unit-tested* as soon as Plan 1 lands — every test here writes its own fixture — but it merges at the end of the chain so its surfaces report on events that something actually emits. It shares no edit region with Plans 2, 3 or 4 and therefore needs no merge notes of its own.

**Files this plan owns exclusively:**

- `crates/phronesis-mcp/src/stats.rs`
- `crates/phronesis-mcp/src/journey_cli.rs`
- `crates/phronesis-mcp/src/server.rs`, `crates/phronesis-mcp/src/server_params.rs`
- `crates/phronesis-mcp/src/context.rs`, `crates/phronesis-mcp/src/context/render.rs`
- `crates/phronesis-metrics/src/families.rs`, `crates/phronesis-metrics/tests/derivation.rs`
- `crates/phronesis-mcp/tests/journey_cli_integration.rs`

**Files shared with earlier plans, all landed by the time this one merges:**

- `crates/phronesis-mcp/src/lifecycle/kalpa_cli.rs` — created by Plan 1 Task 11. This plan replaces only the `KalpaCmd::Show` arm and adds one private `report` fn (Task 3).
- `crates/phronesis-mcp/src/main.rs` — this plan adds fields to the existing `Stats` and `Journey` clap variants and rewrites `handle_stats` / `handle_journey`. It adds no new `Command` variant.
- `crates/phronesis-mcp/tests/kalpa_integration.rs` — created by Plan 1 Task 11; this plan appends.
- `CHANGELOG.md`.

This plan consumes the exact action-log fields `LifecycleEvent::to_log_entry` writes (`kind: "lifecycle"`, `event`, `host`, `sid`, `seq`, `session_id`, `turn_id`, `agent_id`, `agent_type`, `kalpa`, `subject`, `mode`, `prompt`, `prompt_bytes`, plus flattened extras `duration_secs`, `matched_start`, `inferred_from`, `sha`, `head_before`, `confidence_band`), the journal fields `kind`/`mode`/`host`/`turn`/`agent`/`agent_type`/`kalpa`, and `lifecycle::kalpa_cli::{header_line, KalpaCmd}`. Every test here writes its own fixture, so nothing waits on Plans 2–4.

## Global Constraints

- No new crate dependencies.
- Prompt text appears in exactly one output: `phr-mcp journey --corrections`. Never in stats, `kalpa show`, `get_journey` (either shape), the session-context render, or a metric label.
- No `kalpa` label on any metric: user-typed free text, the same reason rule ids are capped (`families.rs:170-172`).
- No ratios in v1 (spec §Non-goals). Raw counts only.
- Counter families register **without** the `_total` suffix; `prometheus-client` appends it at encode time.
- Every reporting surface is read-only and fail-open: an unreadable log prints an empty section, never an error.
- Conventional-commit messages. Run `cargo fmt` and `cargo clippy --all-targets -p phronesis-mcp -p phronesis-metrics -- -D warnings` before every commit.
- Machine note: if `cargo` fails with "You have not agreed to the Xcode license", stop and report; the human must run `sudo xcodebuild -license accept`.

## File structure

| path | responsibility |
|---|---|
| `crates/phronesis-mcp/src/stats.rs` (modify) | `LifecycleOpts`, `LifecycleStats`, `aggregate_lifecycle`, `render_lifecycle`, `retention_line`, `humanize_duration`, `render_json_with_lifecycle` |
| `crates/phronesis-mcp/src/main.rs` (modify) | `stats --kalpa`; `journey --lifecycle` / `--corrections`; kalpa headers |
| `crates/phronesis-mcp/src/lifecycle/kalpa_cli.rs` (modify) | `KalpaCmd::Show` prints the spec's report block |
| `crates/phronesis-mcp/src/journey_cli.rs` (modify) | `LifecycleRow`, `lifecycle_rows`, `render_lifecycle_table`, `CorrectionRow`, `corrections`, `render_corrections` |
| `crates/phronesis-mcp/src/server.rs` (modify) | `get_journey` returns `{facts, lifecycle}` when `include_lifecycle: true`, the current bare array otherwise |
| `crates/phronesis-mcp/src/server_params.rs` (modify) | `GetJourneyParams.include_lifecycle: bool` |
| `crates/phronesis-mcp/src/context.rs`, `src/context/render.rs` (modify) | the active kalpa in the session-context render |
| `crates/phronesis-metrics/src/families.rs` (modify) | `"lifecycle"` arm, `LifecycleLabels`, two registrations |
| `crates/phronesis-mcp/tests/journey_cli_integration.rs` (modify) | render, `--lifecycle`, `--corrections`, kalpa header |
| `crates/phronesis-mcp/tests/kalpa_integration.rs` (modify; created by Plan 1) | `kalpa show` counts, `stats --kalpa`, retention boundary |
| `crates/phronesis-metrics/tests/derivation.rs` (modify) | counter labels, histogram buckets |
| `CHANGELOG.md` (modify) | `## [Unreleased]` entries |

---

### Task 1: `stats::aggregate_lifecycle` and its renderer

**Files:**
- Modify: `crates/phronesis-mcp/src/stats.rs` (add after `aggregate`; rework `render_json` at :208)
- Test: unit tests in the same file's `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `action_log::LogEntry` and the fields `to_log_entry` writes (Plan 1 Task 4).
- Produces:

```rust
pub struct LifecycleOpts { pub since_secs: Option<u64>, pub kalpa: Option<String>, pub now_secs: u64 }  // Default
pub struct LifecycleStats { pub kalpa: Option<String>, pub oldest_entry_ts: Option<u64>, pub sessions: u32,
    pub events: BTreeMap<String,u32>, pub prompt_modes: BTreeMap<String,u32>,
    /// `subagent_start` records — what was launched.
    pub subagents: u32,
    /// The subset that paired with a stop (`matched_start: true`).
    pub subagents_matched: u32,
    /// Median over the matched pairs' durations alone. A pair whose start
    /// rotated away contributes no duration.
    pub subagent_median_secs: Option<u64>, pub commits: u32,
    pub confidence_bands: BTreeMap<String,u32>, pub interventions: u32 }
pub fn aggregate_lifecycle(entries: &[LogEntry], opts: &LifecycleOpts) -> LifecycleStats;
pub fn render_lifecycle(s: &LifecycleStats) -> String;          // the spec's five-line block
pub fn retention_line(oldest_entry_ts: Option<u64>) -> String;  // "counts since log entry 2026-09-17 14:02"
pub fn humanize_duration(secs: u64) -> String;                  // "30s" | "3m40s" | "1h04m"
pub fn render_json_with_lifecycle(values: &Stats, life: Option<&LifecycleStats>) -> String;
```

- [ ] **Step 1: Write the failing tests** — append inside `mod tests` in `stats.rs`

```rust
fn life(ts: u64, event: &str, fields: &[(&str, serde_json::Value)]) -> LogEntry {
    let mut e = LogEntry::new("lifecycle", event).with("host", "claude").with("sid", "s-1");
    e.ts = ts;
    for (k, v) in fields { e.data.insert((*k).to_string(), v.clone()); }
    e
}

fn fixture_log() -> Vec<LogEntry> {
    let k = |name: &str| ("kalpa", json!(name));
    vec![
        life(100, "prompt", &[("mode", json!("fresh")), k("k1"), ("prompt_bytes", json!(12))]),
        life(110, "prompt", &[("mode", json!("correction")), k("k1")]),
        life(120, "interrupt", &[k("k1"), ("inferred_from", json!("inflight"))]),
        life(125, "subagent_start", &[k("k1"), ("agent_id", json!("a1"))]),
        life(126, "subagent_start", &[k("k1"), ("agent_id", json!("a2"))]),
        life(130, "subagent_stop", &[k("k1"), ("duration_secs", json!(10)), ("matched_start", json!(true))]),
        life(140, "subagent_stop", &[k("k1"), ("duration_secs", json!(220)), ("matched_start", json!(true))]),
        // Unmatched: its start rotated away, so it contributes no duration and
        // is not counted among the matched pairs.
        life(150, "subagent_stop", &[k("k1"), ("duration_secs", json!(30)), ("matched_start", json!(false))]),
        life(160, "commit", &[k("k1"), ("sha", json!("0f3c")), ("confidence_band", json!("high"))]),
        life(170, "commit", &[k("k1"), ("sha", json!("aa11")), ("confidence_band", json!("medium"))]),
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
    let s = aggregate_lifecycle(&fixture_log(),
        &LifecycleOpts { since_secs: None, kalpa: Some("k1".into()), now_secs: 1_000 });
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
    assert_eq!(s.interventions, 1, "one correction, no mid_turn, fresh does not count");
    assert_eq!(s.interventions_per_commit(), Some(0.5));
}

#[test]
fn aggregate_lifecycle_retention_boundary_ignores_filters() {
    // The boundary answers "how far back can this log answer at all", so it is
    // the oldest lifecycle entry on disk regardless of --since / --kalpa.
    let s = aggregate_lifecycle(&fixture_log(),
        &LifecycleOpts { since_secs: Some(20), kalpa: Some("k2".into()), now_secs: 180 });
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
    let s = aggregate_lifecycle(&fixture_log(),
        &LifecycleOpts { since_secs: None, kalpa: Some("k1".into()), now_secs: 1_000 });
    let out = render_lifecycle(&s);
    assert!(out.contains("sessions        1"), "{out}");
    assert!(out.contains("prompts         2   fresh 1   mid_turn 0   correction 1"), "{out}");
    assert!(out.contains("interventions   1   (mid_turn + correction)"), "{out}");
    assert!(out.contains("interrupts      1"), "{out}");
    assert!(out.contains("sub-agents      2   starts, 2 matched   median 1m55s"), "{out}");
    assert!(out.contains("commits         2   (shell tool calls only)   confidence at commit: high 1  medium 1  low 0"), "{out}");
    assert!(out.contains("interventions / commit   0.50"), "{out}");
    assert!(!out.contains("prompt_bytes"), "no raw field names leak: {out}");
}

#[test]
fn render_lifecycle_omits_ratio_without_commits() {
    let s = aggregate_lifecycle(&[], &LifecycleOpts { since_secs: None, kalpa: None, now_secs: 1_000 });
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
    assert!(out.contains("commits         1   (shell tool calls only)"), "{out}");
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
        &ReadOpts { kind: Some("lifecycle".to_string()), ..ReadOpts::default() },
    )
    .unwrap();
    assert_eq!(entries.len(), 3, "the rotated predecessor is read");
    let s = aggregate_lifecycle(&entries, &LifecycleOpts::default());
    assert_eq!(s.subagent_median_secs, Some(20), "median over all three, not just the current file");
    assert_eq!(s.oldest_entry_ts, Some(10), "and the boundary is the oldest of the pair");
}

#[test]
fn render_json_with_lifecycle_adds_a_key_without_moving_the_others() {
    let values = Stats { window_label: "7d".into(), generated_at: 1, per_rule: vec![] };
    let s = aggregate_lifecycle(&fixture_log(),
        &LifecycleOpts { since_secs: None, kalpa: None, now_secs: 1_000 });
    let v: serde_json::Value =
        serde_json::from_str(&render_json_with_lifecycle(&values, Some(&s))).unwrap();
    assert_eq!(v["window"], "7d");
    assert_eq!(v["totals"]["rules"], 0);
    assert_eq!(v["lifecycle"]["commits"], 2);
    assert_eq!(v["lifecycle"]["prompts"]["correction"], 1);
    let plain: serde_json::Value = serde_json::from_str(&render_json(&values)).unwrap();
    assert!(plain.get("lifecycle").is_none(), "render_json stays byte-compatible");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-mcp --lib stats:: 2>&1 | tail -30`
Expected: compile error — `cannot find function aggregate_lifecycle`, `LifecycleOpts` not found.

- [ ] **Step 3: Implement** — in `stats.rs`, after `aggregate`

```rust
use std::collections::BTreeMap;

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

    let cutoff = opts.since_secs.map(|w| opts.now_secs.saturating_sub(w)).unwrap_or(0);
    let mut out = LifecycleStats { kalpa: opts.kalpa.clone(), ..LifecycleStats::default() };
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
        if let Some(sid) = e.data.get("sid").and_then(|v| v.as_str()) {
            sids.insert(sid);
        }
        match e.event.as_str() {
            "prompt" => {
                let mode = e.data.get("mode").and_then(|v| v.as_str()).unwrap_or("fresh");
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
                .map(|dt| dt.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
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
    out.push_str(&format!("{:<12}{:>4}\n", "sessions", s.sessions));
    out.push_str(&format!(
        "{:<12}{:>4}   fresh {}   mid_turn {}   correction {}\n",
        "prompts", n("prompt"), m("fresh"), m("mid_turn"), m("correction")
    ));
    out.push_str(&format!("{:<12}{:>4}   (mid_turn + correction)\n", "interventions", s.interventions));
    out.push_str(&format!("{:<12}{:>4}\n", "interrupts", n("interrupt")));
    out.push_str(&format!(
        "{:<12}{:>4}   starts, {} matched{}\n",
        "sub-agents", s.subagents, s.subagents_matched, median
    ));
    // The disclaimer is part of the line, not a footnote: commits made outside a
    // shell tool call are invisible here, so the denominator is undercounted and
    // must say so wherever it is printed (spec §"Success signal: commit").
    out.push_str(&format!(
        "{:<12}{:>4}   (shell tool calls only){}\n",
        "commits", s.commits, bands
    ));
    if let Some(r) = s.interventions_per_commit() {
        out.push_str(&format!("interventions / commit   {r:.2}\n"));
    }
    out
}
```

Then replace the existing `render_json` body with a delegation and add the lifecycle-aware form:

```rust
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
        .map(|r| json!({"rule_id": r.rule_id, "blocked": r.blocked, "warned": r.warned,
                        "last_fired_ts": r.last_fired_ts}))
        .collect();
    let mut payload = json!({
        "window": values.window_label,
        "generated_at": values.generated_at,
        "totals": {"blocked": total_blocked, "warned": total_warned, "rules": values.per_rule.len()},
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
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-mcp --lib stats:: 2>&1 | tail -30`
Expected: pass, including every pre-existing `render_json_*` test.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/stats.rs
git commit -m "feat(stats): lifecycle counts, median sub-agent duration, retention boundary"
```

---

### Task 2: `phr-mcp stats --kalpa` prints the lifecycle section

**Files:**
- Modify: `crates/phronesis-mcp/src/main.rs` (`Command::Stats` ~:82, dispatch ~:580, `handle_stats` ~:779)
- Test: `crates/phronesis-mcp/tests/kalpa_integration.rs`

**Interfaces:**
- Consumes: Task 1's `stats` API; `lifecycle::kalpa_cli::header_line` (Plan 1 Task 11).
- Produces: `fn handle_stats(since: Option<String>, rule: Option<String>, json: bool, kalpa: Option<String>) -> anyhow::Result<()>`.

- [ ] **Step 1: Write the failing tests** — append to `tests/kalpa_integration.rs` (reuse its `run_phr` helper from Plan 1 Task 11)

```rust
/// Fixture log written through the real projection, so the field names under
/// test are the ones `to_log_entry` actually writes.
fn seed_lifecycle_log(root: &std::path::Path, kalpa: &str) {
    use phronesis_mcp::action_log;
    use phronesis_mcp::lifecycle::{Host, Kind, LifecycleEvent, Mode, PromptText, Stamped};

    let path = action_log::default_path(root);
    let events: Vec<(u64, LifecycleEvent)> = vec![
        (1_700_000_000, LifecycleEvent::new(Kind::Prompt, Host::Claude).with_mode(Mode::Fresh).with_prompt("do the thing")),
        (1_700_000_100, LifecycleEvent::new(Kind::Interrupt, Host::Claude).with_extra("inferred_from", "inflight")),
        (1_700_000_110, LifecycleEvent::new(Kind::Prompt, Host::Claude).with_mode(Mode::Correction).with_prompt("no, the other thing")),
        (1_700_000_150, LifecycleEvent::new(Kind::SubagentStart, Host::Claude).with_agent("a1", Some("reviewer".into()))),
        (1_700_000_160, LifecycleEvent::new(Kind::SubagentStart, Host::Claude).with_agent("a2", Some("reviewer".into()))),
        (1_700_000_200, LifecycleEvent::new(Kind::SubagentStop, Host::Claude).with_agent("a1", Some("reviewer".into())).with_extra("duration_secs", 220u64).with_extra("matched_start", true)),
        (1_700_000_300, LifecycleEvent::new(Kind::SubagentStop, Host::Claude).with_agent("a2", Some("reviewer".into())).with_extra("duration_secs", 20u64).with_extra("matched_start", true)),
        (1_700_000_400, LifecycleEvent::new(Kind::Commit, Host::Claude).with_extra("sha", "0f3c").with_extra("confidence_band", "high")),
    ];
    for (i, (ts, ev)) in events.iter().enumerate() {
        let stamped = Stamped { ts: *ts, sid: "s-1".to_string(), seq: i as u64 + 1,
                                kalpa: Some(kalpa.to_string()), subject: None };
        action_log::append(&path, &ev.to_log_entry(&stamped, PromptText::Full)).unwrap();
    }
}

#[test]
fn stats_kalpa_prints_lifecycle_section_with_retention_boundary() {
    let d = tempfile::tempdir().unwrap();
    seed_lifecycle_log(d.path(), "lifecycle-events");
    let out = run_phr(d.path(), &["stats", "--kalpa", "lifecycle-events"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("counts since log entry 20"), "{stdout}");
    assert!(stdout.contains("correction 1"), "{stdout}");
    assert!(stdout.contains("median 2m00s"), "{stdout}");
    assert!(stdout.contains("confidence at commit: high 1"), "{stdout}");
    assert!(!stdout.contains("do the thing"), "prompt text must never reach stats: {stdout}");
}

#[test]
fn stats_kalpa_filter_excludes_other_kalpas() {
    let d = tempfile::tempdir().unwrap();
    seed_lifecycle_log(d.path(), "lifecycle-events");
    let out = run_phr(d.path(), &["stats", "--kalpa", "other-theme", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["lifecycle"]["commits"], 0);
    assert_eq!(v["lifecycle"]["sessions"], 0);
    assert_eq!(v["lifecycle"]["oldest_entry_ts"], 1_700_000_000u64, "boundary ignores the filter");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-mcp --test kalpa_integration stats_kalpa 2>&1 | tail -20`
Expected: FAIL — `unexpected argument '--kalpa'`.

- [ ] **Step 3: Implement**

Add to the `Stats` clap variant:

```rust
        /// Restrict the lifecycle section to one kalpa (see `phr-mcp kalpa`).
        #[arg(long, value_name = "NAME")]
        kalpa: Option<String>,
```

Dispatch: `Command::Stats { since, rule, json, kalpa } => handle_stats(since, rule, json, kalpa),`

In `handle_stats`, hoist the project root into a `let root = phronesis_mcp::security::project_root();` binding at the top (it is currently computed inline for `path`), keep the existing `now` / `since_secs` / `entries` / `values` code, and replace the render tail with:

```rust
    use phronesis_mcp::stats::{
        LifecycleOpts, aggregate_lifecycle, render_json_with_lifecycle, render_lifecycle, retention_line,
    };

    let life_entries = action_log::read_recent(
        &path,
        &ReadOpts { kind: Some("lifecycle".to_string()), ..ReadOpts::default() },
    )
    .unwrap_or_default();
    let life = aggregate_lifecycle(
        &life_entries,
        &LifecycleOpts { since_secs, kalpa, now_secs: now },
    );

    if json {
        println!("{}", render_json_with_lifecycle(&values, Some(&life)));
        return Ok(());
    }
    print!("{}", render_table(&values));
    println!();
    if let Some(header) = phronesis_mcp::lifecycle::kalpa_cli::header_line(&root, now) {
        println!("{header}");
    }
    println!("lifecycle      {}", retention_line(life.oldest_entry_ts));
    print!("{}", render_lifecycle(&life));
    Ok(())
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-mcp --test kalpa_integration --test cli_smoke 2>&1 | tail -20`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/main.rs crates/phronesis-mcp/tests/kalpa_integration.rs
git commit -m "feat(cli): phr-mcp stats --kalpa prints the lifecycle section"
```

---

### Task 3: `phr-mcp kalpa show` prints the report block

**Files:**
- Modify: `crates/phronesis-mcp/src/lifecycle/kalpa_cli.rs` (the `KalpaCmd::Show` arm from Plan 1 Task 11)
- Test: `crates/phronesis-mcp/tests/kalpa_integration.rs`

**Interfaces:**
- Consumes: Task 1's `stats` API; `state::read_kalpa`; `action_log::{default_path, read_recent, ReadOpts}`; this module's existing `age` and `now` helpers.
- Produces: private `fn report(root: &Path, kalpa: &str, now: u64) -> String`; `KalpaCmd::Show` output gains the block.

- [ ] **Step 1: Write the failing tests** — append to `tests/kalpa_integration.rs`

```rust
#[test]
fn kalpa_show_reports_counts_for_the_named_kalpa() {
    let d = tempfile::tempdir().unwrap();
    assert!(run_phr(d.path(), &["kalpa", "start", "lifecycle-events"]).status.success());
    seed_lifecycle_log(d.path(), "lifecycle-events");

    let out = run_phr(d.path(), &["kalpa", "show", "lifecycle-events"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("kalpa: lifecycle-events"), "{stdout}");
    assert!(stdout.contains("started "), "{stdout}");
    assert!(stdout.contains("counts since log entry 20"), "{stdout}");
    assert!(stdout.contains("sessions        1"), "{stdout}");
    assert!(stdout.contains("prompts         2"), "{stdout}");
    assert!(stdout.contains("interrupts      1"), "{stdout}");
    assert!(stdout.contains("sub-agents      2   starts, 2 matched   median 2m00s"), "{stdout}");
    assert!(stdout.contains("commits         1   (shell tool calls only)   confidence at commit: high 1  medium 0  low 0"), "{stdout}");
    assert!(!stdout.contains("do the thing"), "prompt text must never reach kalpa show: {stdout}");
}

#[test]
fn kalpa_show_of_a_closed_kalpa_still_counts_its_entries() {
    let d = tempfile::tempdir().unwrap();
    seed_lifecycle_log(d.path(), "old-theme");
    let out = run_phr(d.path(), &["kalpa", "show", "old-theme"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("kalpa: old-theme (closed)"), "{stdout}");
    assert!(stdout.contains("commits         1"), "{stdout}");
    // `seed_lifecycle_log` writes no `kalpa_start` entry, which is exactly the
    // case where the boundary has rotated off.
    assert!(stdout.contains("start not retained"), "{stdout}");
}

/// The other half: when the `kalpa_start` entry is still in the log, the header
/// prints the date it holds rather than the disclaimer.
#[test]
fn kalpa_show_prints_the_start_date_when_the_boundary_is_still_retained() {
    let d = tempfile::tempdir().unwrap();
    seed_lifecycle_log(d.path(), "old-theme");
    {
        use phronesis_mcp::action_log;
        use phronesis_mcp::lifecycle::{Host, Kind, LifecycleEvent, PromptText, Stamped};
        let stamped = Stamped {
            ts: 1_699_999_000,
            sid: "s-1".to_string(),
            seq: 0,
            kalpa: Some("old-theme".to_string()),
            subject: None,
        };
        action_log::append(
            &action_log::default_path(d.path()),
            &LifecycleEvent::new(Kind::KalpaStart, Host::Cli).to_log_entry(&stamped, PromptText::Full),
        )
        .unwrap();
    }
    let out = run_phr(d.path(), &["kalpa", "show", "old-theme"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("started 20"), "{stdout}");
    assert!(!stdout.contains("start not retained"), "{stdout}");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-mcp --test kalpa_integration kalpa_show 2>&1 | tail -20`
Expected: FAIL — the output is the bare header line, no `sessions` row.

- [ ] **Step 3: Implement** — in `kalpa_cli.rs`, above `run`

```rust
/// The report block from SPEC-agent-lifecycle-events §Reporting. Reads the same
/// action-log entries `phr-mcp stats` reads and counts them with the same
/// function, so the two surfaces cannot disagree.
fn report(root: &Path, kalpa: &str, now: u64) -> String {
    use crate::action_log::{self, ReadOpts};
    use crate::stats::{LifecycleOpts, aggregate_lifecycle, render_lifecycle, retention_line};

    let entries = action_log::read_recent(
        &action_log::default_path(root),
        &ReadOpts { kind: Some("lifecycle".to_string()), ..ReadOpts::default() },
    )
    .unwrap_or_default();
    let stats = aggregate_lifecycle(
        &entries,
        &LifecycleOpts { since_secs: None, kalpa: Some(kalpa.to_string()), now_secs: now },
    );

    // "when the `kalpa_start` entry has itself rotated off, the header prints
    // `start not retained` in place of the start date" (spec §Reporting). The
    // open `kalpa` file still knows `started_ts`, but a *closed* kalpa's start
    // is only knowable from the log — and `kalpa end` deletes the file, so the
    // date is gone with the rotation. Saying "start not retained" is honest;
    // printing the oldest retained entry as if it were the start is not.
    let start_retained = entries
        .iter()
        .any(|e| e.event == "kalpa_start" && e.data.get("kalpa").and_then(|v| v.as_str()) == Some(kalpa));
    let head = match state::read_kalpa(root).filter(|k| k.name == kalpa) {
        Some(k) => {
            let started = chrono::DateTime::from_timestamp(k.started_ts as i64, 0)
                .map(|dt| dt.with_timezone(&chrono::Local).format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| k.started_ts.to_string());
            format!(
                "kalpa: {kalpa}      started {started} ({})      {}",
                age(now.saturating_sub(k.started_ts)),
                retention_line(stats.oldest_entry_ts)
            )
        }
        None if start_retained => {
            let started = entries
                .iter()
                .find(|e| {
                    e.event == "kalpa_start"
                        && e.data.get("kalpa").and_then(|v| v.as_str()) == Some(kalpa)
                })
                .map(|e| e.ts)
                .unwrap_or_default();
            let when = chrono::DateTime::from_timestamp(started as i64, 0)
                .map(|dt| dt.with_timezone(&chrono::Local).format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| started.to_string());
            format!(
                "kalpa: {kalpa} (closed)      started {when}      {}",
                retention_line(stats.oldest_entry_ts)
            )
        }
        None => format!(
            "kalpa: {kalpa} (closed)      start not retained      {}",
            retention_line(stats.oldest_entry_ts)
        ),
    };
    format!("{head}\n{}", render_lifecycle(&stats))
}
```

Replace the `Show` arm:

```rust
        KalpaCmd::Show { name } => {
            let now = now();
            match (name, state::read_kalpa(root)) {
                (None, Some(k)) => Ok(report(root, &k.name, now)),
                (Some(n), _) => Ok(report(root, &n, now)),
                (None, None) => anyhow::bail!("no kalpa open"),
            }
        }
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-mcp --test kalpa_integration 2>&1 | tail -20`
Expected: pass, including Plan 1's `kalpa_start_show_end_round_trip_and_events` (it asserts `contains("kalpa: lifecycle-events")`, which the richer header still satisfies).

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/lifecycle/kalpa_cli.rs crates/phronesis-mcp/tests/kalpa_integration.rs
git commit -m "feat(cli): kalpa show reports sessions, prompts, interrupts, sub-agents, commits"
```

---

### Task 4: lifecycle records in `phr-mcp journey`

**Files:**
- Modify: `crates/phronesis-mcp/src/journey_cli.rs` (after `JourneyRow`, and `JourneyCliError`), `crates/phronesis-mcp/src/main.rs` (`Command::Journey`, `handle_journey` ~:910)
- Test: `crates/phronesis-mcp/tests/journey_cli_integration.rs`

**Interfaces:**
- Consumes: `journey::journal::{read_recent, SUFFIX_HARD_CAP, JournalError}`, `JournalRecord::is_lifecycle()` (Plan 1 Task 1), `lifecycle::kalpa_cli::header_line`.
- Produces:

```rust
pub struct LifecycleRow { pub ts: u64, pub sid: String, pub seq: u64, pub kind: String,
    pub mode: Option<String>, pub host: Option<String>, pub agent_type: Option<String>, pub kalpa: Option<String> }
pub fn lifecycle_rows(project_root: &Path, limit: usize) -> Result<Vec<LifecycleRow>, JourneyCliError>;
pub fn render_lifecycle_table(rows: &[LifecycleRow]) -> String;
pub fn render_lifecycle_json(rows: &[LifecycleRow]) -> Result<String, JourneyCliError>;
```

- [ ] **Step 1: Write the failing tests** — append to `tests/journey_cli_integration.rs`

```rust
/// Append v2 lifecycle journal records to a seeded project, and open a kalpa.
fn append_lifecycle_records(root: &Path) {
    let events = root.join(".phronesis/journey/events.jsonl");
    let mut lines = std::fs::read_to_string(&events).unwrap_or_default();
    for (ts, seq, kind, mode) in [
        (2000u64, 100u64, "prompt", Some("fresh")),
        (2001, 101, "interrupt", None),
        (2002, 102, "prompt", Some("correction")),
        (2003, 103, "subagent_stop", None),
    ] {
        let mut rec = serde_json::json!({
            "v": 2, "ts": ts, "sid": "s-test", "seq": seq,
            "tool": "__lifecycle", "path": "", "tags": [format!("lifecycle:{kind}")],
            "kind": kind, "host": "claude", "kalpa": "demo",
        });
        if let Some(m) = mode { rec["mode"] = serde_json::json!(m); }
        lines.push_str(&rec.to_string());
        lines.push('\n');
    }
    std::fs::write(&events, lines).unwrap();
    std::fs::write(
        root.join(".phronesis/journey/kalpa"),
        serde_json::json!({"name": "demo", "started_ts": 1000}).to_string(),
    ).unwrap();
}

#[test]
fn journey_table_renders_lifecycle_records_and_kalpa_header() {
    let dir = tempfile::tempdir().unwrap();
    seed!(dir.path(), AUTH_CHURN_RULES, AUTH_JOURNEY_JSON, 3, "auth");
    append_lifecycle_records(dir.path());

    let (code, stdout, stderr) = run(&["journey"], dir.path());
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.starts_with("kalpa: demo ("), "stdout: {stdout}");
    assert!(stdout.contains("journey_occurrence"), "facts still render: {stdout}");
    assert!(stdout.contains("⟂"), "stdout: {stdout}");
    assert!(stdout.contains("prompt/correction"), "stdout: {stdout}");
    assert!(stdout.contains("subagent_stop"), "stdout: {stdout}");
}

#[test]
fn journey_lifecycle_flag_shows_only_lifecycle_records() {
    let dir = tempfile::tempdir().unwrap();
    seed!(dir.path(), AUTH_CHURN_RULES, AUTH_JOURNEY_JSON, 3, "auth");
    append_lifecycle_records(dir.path());

    let (code, stdout, stderr) = run(&["journey", "--lifecycle"], dir.path());
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(!stdout.contains("journey_occurrence"), "stdout: {stdout}");
    assert!(stdout.contains("prompt/fresh"), "stdout: {stdout}");

    let (code, stdout, _) = run(&["journey", "--lifecycle", "--json"], dir.path());
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    let rows = v.as_array().unwrap();
    assert_eq!(rows.len(), 4, "stdout: {stdout}");
    assert_eq!(rows[0]["kind"], "prompt");
    assert_eq!(rows[0]["mode"], "fresh");
    assert_eq!(rows[3]["kind"], "subagent_stop");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-mcp --test journey_cli_integration journey_table_renders journey_lifecycle_flag 2>&1 | tail -20`
Expected: FAIL — `unexpected argument '--lifecycle'`, and no `⟂` in the default output.

- [ ] **Step 3: Implement**

Add a variant to `JourneyCliError`:

```rust
    #[error("journal: {0}")]
    Journal(#[from] journey::journal::JournalError),
```

After `JourneyRow` in `journey_cli.rs`:

```rust
/// One lifecycle journal record, as `phr-mcp journey` and `get_journey` render
/// it. Lifecycle records have no path, so the `kind`/`mode` pair takes the path
/// column and a `⟂` marker flags the row as not-a-tool-call.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LifecycleRow {
    pub ts: u64,
    pub sid: String,
    pub seq: u64,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kalpa: Option<String>,
}

/// The most recent `limit` lifecycle records, oldest first. Never prompt text:
/// the journal has none (spec §Privacy and scrubbing).
pub fn lifecycle_rows(project_root: &Path, limit: usize) -> Result<Vec<LifecycleRow>, JourneyCliError> {
    use crate::journey::journal;
    // Over-read: lifecycle records share the file with tool records, so asking
    // for exactly `limit` lines would under-fill this view.
    let read_n = (limit.saturating_mul(4) + 64).min(journal::SUFFIX_HARD_CAP);
    let records = journal::read_recent(project_root, read_n)?;
    let mut rows: Vec<LifecycleRow> = records
        .iter()
        .filter(|r| r.is_lifecycle())
        .map(|r| LifecycleRow {
            ts: r.ts,
            sid: r.sid.clone(),
            seq: r.seq,
            kind: r.kind.clone().unwrap_or_default(),
            mode: r.mode.clone(),
            host: r.host.clone(),
            agent_type: r.agent_type.clone(),
            kalpa: r.kalpa.clone(),
        })
        .collect();
    if rows.len() > limit {
        rows.drain(..rows.len() - limit);
    }
    Ok(rows)
}

fn kind_mode(row: &LifecycleRow) -> String {
    match row.mode.as_deref() {
        Some(m) => format!("{}/{}", row.kind, m),
        None => row.kind.clone(),
    }
}

/// Table rendering for lifecycle records; `⟂` marks every row.
pub fn render_lifecycle_table(rows: &[LifecycleRow]) -> String {
    let mut out = format!("{:<2}  {:<14}  {:<8}  {:<22}  {}\n", "", "SID", "SEQ", "KIND/MODE", "HOST");
    if rows.is_empty() {
        out.push_str("(no lifecycle records)\n");
        return out;
    }
    for r in rows {
        out.push_str(&format!(
            "{:<2}  {:<14}  {:<8}  {:<22}  {}\n",
            "⟂", r.sid, r.seq, kind_mode(r), r.host.as_deref().unwrap_or("-")
        ));
    }
    out
}

/// JSON rendering — flat array, schema mirrors `LifecycleRow`.
pub fn render_lifecycle_json(rows: &[LifecycleRow]) -> Result<String, JourneyCliError> {
    Ok(serde_json::to_string_pretty(rows)?)
}
```

In `main.rs`, add to the `Journey` clap variant:

```rust
        /// Show only lifecycle records (sub-agent start/stop, prompts,
        /// interrupts, turn stops, commits) instead of derived facts.
        #[arg(long)]
        lifecycle: bool,
```

Dispatch: `Command::Journey { json, explain, lifecycle } => handle_journey(json, explain, lifecycle).await,` (Task 5 adds a fourth field and updates both lines again).

In `handle_journey(json: bool, explain: Option<String>, lifecycle: bool)`, after `root` / `now` / `sid`:

```rust
    let header = phronesis_mcp::lifecycle::kalpa_cli::header_line(&root, now);
    let rows_life = journey_cli::lifecycle_rows(&root, 50).unwrap_or_default();

    if lifecycle {
        if json {
            match journey_cli::render_lifecycle_json(&rows_life) {
                Ok(s) => println!("{s}"),
                Err(e) => { eprintln!("error: {e}"); std::process::exit(1); }
            }
        } else {
            if let Some(h) = &header { println!("{h}"); }
            print!("{}", journey_cli::render_lifecycle_table(&rows_life));
        }
        return Ok(());
    }
```

and replace the existing table branch (`} else { print!("{}", journey_cli::render_table(&rows)); }`) with:

```rust
    } else {
        if let Some(h) = &header { println!("{h}"); }
        print!("{}", journey_cli::render_table(&rows));
        if !rows_life.is_empty() {
            println!();
            print!("{}", journey_cli::render_lifecycle_table(&rows_life));
        }
    }
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-mcp --test journey_cli_integration 2>&1 | tail -20`
Expected: pass, including the five pre-existing tests (the `--json` fact-array shape is untouched).

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/journey_cli.rs crates/phronesis-mcp/src/main.rs crates/phronesis-mcp/tests/journey_cli_integration.rs
git commit -m "feat(journey): render lifecycle records with a marker and a --lifecycle filter"
```

---

### Task 5: `phr-mcp journey --corrections`

**Files:**
- Modify: `crates/phronesis-mcp/src/journey_cli.rs`, `crates/phronesis-mcp/src/main.rs`
- Test: `crates/phronesis-mcp/tests/journey_cli_integration.rs`

**Interfaces:**
- Consumes: `action_log::{default_path, read_recent, ReadOpts}` and
  **`lifecycle::record::correction_text`** (Plan 1 Task 7) — the one accessor for
  correction text. Spec §"Action log": "Every consumer of correction text …
  goes through one accessor … No consumer greps the log directly." Reading
  `entry.data["prompt"]` here would make `prompt_text: "none"` a write-time-only
  switch, so flipping it would leave every prompt already on disk printable.
- Produces:

```rust
pub struct CorrectionRow { pub ts: u64, pub sid: String, pub prompt: Option<String> }
pub fn corrections(project_root: &Path) -> Vec<CorrectionRow>;   // oldest first, rotated log included
pub fn render_corrections(rows: &[CorrectionRow]) -> String;
```

- [ ] **Step 1: Write the failing tests** — append to `tests/journey_cli_integration.rs`

```rust
#[test]
fn journey_corrections_lists_scrubbed_prompts_oldest_first() {
    let dir = tempfile::tempdir().unwrap();
    seed!(dir.path(), AUTH_CHURN_RULES, AUTH_JOURNEY_JSON, 1, "auth");
    let lines = [
        serde_json::json!({"ts":1_700_000_000u64,"kind":"lifecycle","event":"prompt","host":"claude","sid":"s-1","seq":1,"mode":"fresh","prompt":"start here","prompt_bytes":10}),
        serde_json::json!({"ts":1_700_000_100u64,"kind":"lifecycle","event":"interrupt","host":"claude","sid":"s-1","seq":2,"inferred_from":"inflight"}),
        serde_json::json!({"ts":1_700_000_110u64,"kind":"lifecycle","event":"prompt","host":"claude","sid":"s-1","seq":3,"mode":"correction","prompt":"no, use the repo root","prompt_bytes":21}),
        serde_json::json!({"ts":1_700_000_900u64,"kind":"lifecycle","event":"prompt","host":"claude","sid":"s-2","seq":4,"mode":"correction","prompt":"second correction","prompt_bytes":17}),
        serde_json::json!({"ts":1_700_000_950u64,"kind":"hook","event":"pre_check","tool":"Edit","exit":0}),
    ];
    let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(dir.path().join(".phronesis/log.jsonl"), body).unwrap();

    let (code, stdout, stderr) = run(&["journey", "--corrections"], dir.path());
    assert_eq!(code, 0, "stderr: {stderr}");
    let first = stdout.find("no, use the repo root").expect("first correction");
    let second = stdout.find("second correction").expect("second correction");
    assert!(first < second, "oldest first: {stdout}");
    assert!(stdout.contains("s-1") && stdout.contains("s-2"), "sids printed: {stdout}");
    assert!(!stdout.contains("start here"), "fresh prompts are not corrections: {stdout}");
    assert!(!stdout.contains("pre_check"), "non-lifecycle entries ignored: {stdout}");
}

/// The rotated predecessor is read: a kalpa long enough to rotate the log must
/// not lose the oldest half of the list the feature exists to produce.
#[test]
fn journey_corrections_include_the_rotated_predecessor() {
    let dir = tempfile::tempdir().unwrap();
    seed!(dir.path(), AUTH_CHURN_RULES, AUTH_JOURNEY_JSON, 1, "auth");
    let correction = |ts: u64, seq: u64, text: &str| {
        serde_json::json!({"ts":ts,"kind":"lifecycle","event":"prompt","host":"claude",
                           "sid":"s-1","seq":seq,"mode":"correction","prompt":text,
                           "prompt_bytes":text.len()})
        .to_string()
    };
    std::fs::write(
        dir.path().join(".phronesis/log.jsonl.1"),
        format!("{}\n", correction(1_700_000_000, 1, "the oldest correction")),
    )
    .unwrap();
    std::fs::write(
        dir.path().join(".phronesis/log.jsonl"),
        format!("{}\n", correction(1_700_000_900, 2, "the newest correction")),
    )
    .unwrap();

    let (code, stdout, stderr) = run(&["journey", "--corrections"], dir.path());
    assert_eq!(code, 0, "stderr: {stderr}");
    let old = stdout.find("the oldest correction").expect("the rotated file is read");
    let new = stdout.find("the newest correction").expect("the current file is read");
    assert!(old < new, "oldest first across both files: {stdout}");
}

/// `prompt_text: "none"` is enforced at READ time: text already written under
/// `"full"` is hidden too, and the row still shows when the correction happened.
#[test]
fn journey_corrections_honor_prompt_text_none_at_read_time() {
    let dir = tempfile::tempdir().unwrap();
    seed!(dir.path(), AUTH_CHURN_RULES, AUTH_JOURNEY_JSON, 1, "auth");
    std::fs::write(
        dir.path().join(".phronesis/log.jsonl"),
        format!(
            "{}\n",
            serde_json::json!({"ts":1_700_000_110u64,"kind":"lifecycle","event":"prompt",
                               "host":"claude","sid":"s-1","seq":3,"mode":"correction",
                               "prompt":"already-on-disk text","prompt_bytes":20})
        ),
    )
    .unwrap();
    // Written under "full"; the switch flips afterwards.
    std::fs::write(
        dir.path().join(".phronesis/journey.json"),
        r#"{"version":1,"taggers":[],"modules":[],"lifecycle":{"prompt_text":"none"}}"#,
    )
    .unwrap();

    let (code, stdout, _) = run(&["journey", "--corrections"], dir.path());
    assert_eq!(code, 0);
    assert!(!stdout.contains("already-on-disk text"), "{stdout}");
    assert!(stdout.contains("(prompt text disabled)"), "{stdout}");
    assert!(stdout.contains("s-1"), "the row still shows when it happened: {stdout}");
}

#[test]
fn journey_corrections_on_an_empty_log_says_so() {
    let dir = tempfile::tempdir().unwrap();
    seed!(dir.path(), AUTH_CHURN_RULES, AUTH_JOURNEY_JSON, 1, "auth");
    let (code, stdout, _) = run(&["journey", "--corrections"], dir.path());
    assert_eq!(code, 0);
    assert!(stdout.contains("(no corrections recorded)"), "stdout: {stdout}");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-mcp --test journey_cli_integration journey_corrections 2>&1 | tail -20`
Expected: FAIL — `unexpected argument '--corrections'`.

- [ ] **Step 3: Implement** — in `journey_cli.rs`

```rust
/// One `prompt` entry with `mode: "correction"` from the action log — the only
/// surface that prints prompt text, and only because the text was scrubbed by
/// `lifecycle::scrub::scrub_prompt` at write time.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CorrectionRow {
    pub ts: u64,
    pub sid: String,
    /// `None` under `lifecycle.prompt_text: "none"`. The row still shows *when*
    /// the correction happened, which is the part the switch does not hide.
    pub prompt: Option<String>,
}

/// Every recorded correction, oldest first, across `.phronesis/log.jsonl` and
/// its rotated predecessor. Fail-open: an unreadable log yields none.
pub fn corrections(project_root: &Path) -> Vec<CorrectionRow> {
    use crate::action_log::{self, ReadOpts};
    let opts = ReadOpts {
        kind: Some("lifecycle".to_string()),
        event: Some("prompt".to_string()),
        ..ReadOpts::default()
    };
    // `limit: None` reads `.phronesis/log.jsonl` AND its rotated predecessor,
    // oldest first. "The list the feature exists to surface must not silently
    // lose its oldest half to rotation" (spec §"CLI and MCP surface").
    action_log::read_recent(&action_log::default_path(project_root), &opts)
        .unwrap_or_default()
        .iter()
        .filter(|e| e.data.get("mode").and_then(|v| v.as_str()) == Some("correction"))
        .map(|e| CorrectionRow {
            ts: e.ts,
            sid: e.data.get("sid").and_then(|v| v.as_str()).unwrap_or("-").to_string(),
            // The ONE accessor. It consults the current `prompt_text` value, so
            // flipping the switch to `"none"` hides text already written under
            // `"full"` as well as text not yet written.
            prompt: crate::lifecycle::record::correction_text(project_root, e),
        })
        .collect()
}

/// One block per correction: a `ts  sid` line, then the prompt, indented.
pub fn render_corrections(rows: &[CorrectionRow]) -> String {
    if rows.is_empty() {
        return "(no corrections recorded)\n".to_string();
    }
    let mut out = String::new();
    for r in rows {
        let when = chrono::DateTime::from_timestamp(r.ts as i64, 0)
            .map(|dt| dt.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| r.ts.to_string());
        out.push_str(&format!("{when}  {}\n", r.sid));
        match &r.prompt {
            Some(text) => {
                for line in text.lines() {
                    out.push_str(&format!("    {line}\n"));
                }
            }
            None => out.push_str("    (prompt text disabled)\n"),
        }
        out.push('\n');
    }
    out
}
```

In `main.rs`, add to the `Journey` variant:

```rust
        /// List the prompts recorded as corrections (a prompt that followed an
        /// interrupt), oldest first, with their scrubbed text.
        #[arg(long)]
        corrections: bool,
```

Dispatch: `Command::Journey { json, explain, lifecycle, corrections } => handle_journey(json, explain, lifecycle, corrections).await,`

At the top of `handle_journey(json, explain, lifecycle, corrections)`, right after `root`:

```rust
    if corrections {
        let rows = journey_cli::corrections(&root);
        if json {
            println!("{}", serde_json::to_string_pretty(&rows)?);
        } else {
            print!("{}", journey_cli::render_corrections(&rows));
        }
        return Ok(());
    }
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-mcp --test journey_cli_integration 2>&1 | tail -20`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/journey_cli.rs crates/phronesis-mcp/src/main.rs crates/phronesis-mcp/tests/journey_cli_integration.rs
git commit -m "feat(journey): --corrections lists post-interrupt prompts from the action log"
```

---

### Task 6: `get_journey` gains an opt-in `include_lifecycle` parameter

**Files:**
- Modify: `crates/phronesis-mcp/src/server.rs:1437-1459`
- Test: `crates/phronesis-mcp/src/server.rs` `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `journey_cli::{JourneyRow, LifecycleRow, lifecycle_rows, render_json}` (Task 4), `server_params::GetJourneyParams`.
- Produces: on `GetJourneyParams`, `#[serde(default)] pub include_lifecycle: bool`; and `fn journey_payload(rows: &[JourneyRow], lifecycle: &[LifecycleRow]) -> String` → `{"facts": [...], "lifecycle": [...]}`.

**Shape note — the existing return shape does not change.** Today `get_journey` returns `journey_cli::render_json(&rows)`, a pretty-printed bare array of fact rows. That stays **byte-for-byte identical** by default. Lifecycle records are opt-in through a new boolean parameter:

| `include_lifecycle` | response |
|---|---|
| absent or `false` (the default) | the current bare array of `{predicate, selector, window, extra, rules}` rows, unchanged |
| `true` | `{"facts": [...], "lifecycle": [...]}`, where `facts` is that same array |

A bare array has no room for a second collection, so an envelope is needed *when lifecycle records are asked for* — but a model that never asks keeps the response it already knows, and no existing caller breaks. This is why the release stays additive rather than changing a tool's contract. The **CLI** `phr-mcp journey --json` array shape is likewise untouched; it is pinned by `tests/journey_cli_integration.rs` and by operator scripts.

The parameter follows the bool pattern already used in `server_params.rs` (`AddPredicateProviderParams::replace` at `:73`, `GetActionLogParams::only_nonzero_exit` at `:183`): `#[serde(default)] pub <name>: bool`, which schemars renders as an optional boolean defaulting to `false`.

- [ ] **Step 1: Write the failing tests** — append to `mod tests` in `server.rs`

```rust
/// Default shape: the bare fact array `get_journey` has always returned.
/// Adding lifecycle records must not move an existing consumer's cheese.
#[test]
fn get_journey_payload_defaults_to_the_bare_fact_array() {
    let rows = vec![crate::journey_cli::JourneyRow {
        predicate: "journey_seen".into(), selector: "auth".into(), window: "s".into(),
        extra: vec![], rules: vec!["auth-churn".into()],
    }];
    let payload = crate::journey_cli::render_json(&rows).expect("render");
    let v: serde_json::Value = serde_json::from_str(&payload).expect("valid json");
    assert!(v.is_array(), "the default response is still a bare array: {payload}");
    assert_eq!(v[0]["predicate"], "journey_seen");
}

/// Opt-in shape: `include_lifecycle: true` wraps the same array under `facts`
/// and adds `lifecycle`.
#[test]
fn get_journey_payload_envelope_carries_facts_and_lifecycle() {
    let rows = vec![crate::journey_cli::JourneyRow {
        predicate: "journey_seen".into(), selector: "auth".into(), window: "s".into(),
        extra: vec![], rules: vec!["auth-churn".into()],
    }];
    let life = vec![crate::journey_cli::LifecycleRow {
        ts: 10, sid: "s-1".into(), seq: 3, kind: "prompt".into(),
        mode: Some("correction".into()), host: Some("claude".into()),
        agent_type: None, kalpa: Some("demo".into()),
    }];
    let payload = EpistemeMcp::journey_payload(&rows, &life);
    let v: serde_json::Value = serde_json::from_str(&payload).expect("valid json");
    assert_eq!(v["facts"][0]["predicate"], "journey_seen");
    assert_eq!(v["facts"], serde_json::json!(rows), "facts is the unchanged row array");
    assert_eq!(v["lifecycle"][0]["kind"], "prompt");
    assert_eq!(v["lifecycle"][0]["mode"], "correction");
    assert_eq!(v["lifecycle"][0]["kalpa"], "demo");
    assert!(!payload.contains("\"prompt\":"), "no prompt text over MCP: {payload}");
}

/// The parameter itself: absent means false, so an old caller's argument-less
/// invocation deserializes and keeps the old shape.
#[test]
fn get_journey_params_default_include_lifecycle_is_false() {
    let p: crate::server_params::GetJourneyParams =
        serde_json::from_str("{}").expect("empty params deserialize");
    assert!(!p.include_lifecycle);
    let p: crate::server_params::GetJourneyParams =
        serde_json::from_str(r#"{"include_lifecycle":true}"#).expect("params deserialize");
    assert!(p.include_lifecycle);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-mcp --lib server::tests::get_journey 2>&1 | tail -20`
Expected: FAIL — `no function or associated item named journey_payload`, and `GetJourneyParams` has no field `include_lifecycle`.

- [ ] **Step 3: Implement**

In `crates/phronesis-mcp/src/server_params.rs`, add the field to `GetJourneyParams` (`:279-285`):

```rust
#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct GetJourneyParams {
    /// Optional rule id; when set, only return facts that specific rule
    /// references. Mirrors the `phr-mcp journey --explain` CLI flag.
    #[serde(default)]
    pub explain_rule: Option<String>,
    /// When true, return `{"facts": [...], "lifecycle": [...]}` — the derived
    /// facts plus the recent lifecycle records (sub-agent start/stop, prompts
    /// with their mode, interrupts, turn stops, commits). Default `false`
    /// returns the bare array of fact rows, unchanged from earlier versions.
    #[serde(default)]
    pub include_lifecycle: bool,
}
```

In `crates/phronesis-mcp/src/server.rs`, in the plain `impl EpistemeMcp` block (**not** the `#[tool_router]` one, or the macro will try to register it):

```rust
    /// Serialize the opt-in `get_journey` envelope: derived facts plus the
    /// lifecycle records the CLI renders, with the same field names. Only
    /// reached when the caller passes `include_lifecycle: true`.
    fn journey_payload(
        rows: &[crate::journey_cli::JourneyRow],
        lifecycle: &[crate::journey_cli::LifecycleRow],
    ) -> String {
        serde_json::json!({ "facts": rows, "lifecycle": lifecycle }).to_string()
    }
```

Replace the tail of `get_journey` (`server.rs:1451-1458`):

```rust
        let lifecycle = if params.include_lifecycle {
            journey_cli::lifecycle_rows(&root, 50).unwrap_or_default()
        } else {
            Vec::new()
        };
        Self::log_event("get_journey", |e| {
            e.with("rows", rows.len() as u64)
                .with("lifecycle_rows", lifecycle.len() as u64)
                .with("explain_rule", params.explain_rule.clone().unwrap_or_default())
        });
        if params.include_lifecycle {
            return Self::ok_text(Self::journey_payload(&rows, &lifecycle));
        }
        // Default: the bare fact array this tool has always returned.
        let json = journey_cli::render_json(&rows).map_err(|e| Self::err(e.to_string()))?;
        Self::ok_text(json)
```

Extend the `#[tool(description = …)]` string on `get_journey` with one sentence, leaving the rest of it as it is:

```text
… JSON array of `{predicate, selector, window, extra, rules}` rows. Pass `include_lifecycle: true` to get `{"facts": [...], "lifecycle": [...]}` instead, adding the recent lifecycle records (sub-agent start/stop, prompts with `fresh`/`mid_turn`/`correction` mode, interrupts, turn stops, commits); prompt text is never included.
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-mcp --lib server:: 2>&1 | tail -20`
Expected: pass, including `journey_tool_is_registered` and every pre-existing `server::tests` case.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/server.rs crates/phronesis-mcp/src/server_params.rs
git commit -m "feat(mcp): get_journey gains an opt-in include_lifecycle parameter"
```

---

### Task 7: the active kalpa in the session-context render

**Files:**
- Modify: `crates/phronesis-mcp/src/context/render.rs` (`state_items`, `:753-773`), `crates/phronesis-mcp/src/context.rs` (`run_session_context`, `:261-286`)
- Test: unit tests in `crates/phronesis-mcp/src/context/render.rs`'s `#[cfg(test)] mod tests`

**Why:** spec §"Outcomes and kalpas / Forgotten kalpas are made visible, not expired" names three surfaces that must print the active kalpa and its age: `phr-mcp journey` (Task 4), `phr-mcp stats` (Task 2), **and the session-context render**. The third is the one a human never asks for — it is how a kalpa opened three weeks ago gets noticed at all — so it is the one that matters most and it is the only one no other plan covers.

Context has two render paths and both need the line: the token-aware one (`render::render` → `pack_charter` → `state_items`) and the legacy byte-for-byte one (`context::run_session_context`), which is what a project with no `.phronesis/context.json` still uses.

**Interfaces:**
- Consumes: `lifecycle::kalpa_cli::header_line(root, now) -> Option<String>` (Plan 1 Task 11).
- Produces: nothing public. One new `ItemKind::State` item with id `state:kalpa`.

- [ ] **Step 1: Write the failing tests** — append to `mod tests` in `context/render.rs`

```rust
#[test]
fn state_items_include_the_open_kalpa() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join(".phronesis/journey")).unwrap();
    std::fs::write(
        d.path().join(".phronesis/journey/kalpa"),
        serde_json::json!({"name": "lifecycle-events", "started_ts": 1}).to_string(),
    )
    .unwrap();
    let items = state_items(d.path());
    let kalpa = items
        .iter()
        .find(|i| i.id == "state:kalpa")
        .expect("a state:kalpa item");
    assert!(kalpa.body.contains("kalpa: lifecycle-events"), "{}", kalpa.body);
}

#[test]
fn state_items_omit_the_kalpa_line_when_none_is_open() {
    let d = tempfile::tempdir().unwrap();
    assert!(
        !state_items(d.path()).iter().any(|i| i.id == "state:kalpa"),
        "no kalpa file means no line at all"
    );
}

#[test]
fn legacy_session_context_prints_the_open_kalpa() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join(".phronesis/journey")).unwrap();
    std::fs::write(
        d.path().join(".phronesis/journey/kalpa"),
        serde_json::json!({"name": "demo", "started_ts": 1}).to_string(),
    )
    .unwrap();
    let out = super::run_session_context(d.path(), super::DEFAULT_MAX_BYTES);
    assert!(out.contains("kalpa: demo"), "{out}");
}
```

(`state_items` is `pub(crate)`, so the tests in the same module reach it directly; `run_session_context` is `pub` in the parent module.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-mcp --lib context::render::tests::state_items_include_the_open_kalpa context::render::tests::legacy_session_context 2>&1 | tail -20`
Expected: FAIL — `expect("a state:kalpa item")` panics, and the legacy body has no `kalpa:` text.

- [ ] **Step 3: Implement**

In `context/render.rs::state_items`, after the confidence block and before `graph_freshness_line`:

```rust
    // The kalpa is a cross-session theme, so a stale one is invisible unless
    // something says it out loud every session (spec §"Forgotten kalpas are
    // made visible, not expired"). `header_line` appends its own
    // "(stale? run phr-mcp kalpa end)" past 30 days.
    if let Some(line) = crate::lifecycle::kalpa_cli::header_line(root, unix_now()) {
        items.push(ItemKind::State.item("state:kalpa", format!("- {line}")));
    }
```

written with whatever constructor the surrounding lines use — as of today that is `ContextItem::new(ItemKind::State, "state:kalpa", format!("- {line}"))`, matching `state:subject` and `state:confidence` directly above. `unix_now()` is already a private helper in this module (used by `pack_turn`).

In `context.rs::run_session_context`, fold the same line into the body so the legacy path matches:

```rust
pub fn run_session_context(project_root: &Path, max_bytes: usize) -> String {
    let _ = crate::journey::current_sid(project_root);

    let rules_body = crate::rule_layers::resolve(project_root)
        .map(|resolved| {
            build_session_body(&RulesFile {
                rules: resolved.rules,
            })
        })
        .unwrap_or_default();

    let durable = build_durable_section(&read_durable_directives(project_root));

    // Same line the token-aware path emits as `state:kalpa`, so a project with
    // no context.json is not the one place a forgotten kalpa stays invisible.
    let kalpa = crate::lifecycle::kalpa_cli::header_line(
        project_root,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    )
    .map(|line| format!("- {line}\n"))
    .unwrap_or_default();

    let body = match (durable.is_empty(), rules_body.is_empty()) {
        (true, true) if kalpa.is_empty() => return String::new(),
        (true, true) => kalpa,
        (true, false) => format!("{kalpa}{rules_body}"),
        (false, true) => format!("{kalpa}{durable}"),
        (false, false) => format!("{kalpa}{durable}\n{rules_body}"),
    };
    wrap_additional_context("SessionStart", &body, max_bytes)
}
```

Note the first arm: an otherwise-empty render with an open kalpa now returns the kalpa line rather than the empty string. That is the point — the whole reason for this task is that a stale kalpa must surface in a project that has nothing else to say.

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-mcp --lib context:: 2>&1 | tail -30`
Expected: pass, including every pre-existing `context::` and `context::render::` test. A pre-existing test that asserts `run_session_context` returns `""` for a bare project still passes: a bare project has no `.phronesis/journey/kalpa` file, so `kalpa` is empty and the first arm returns `String::new()` exactly as before.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/context.rs crates/phronesis-mcp/src/context/render.rs
git commit -m "feat(context): print the active kalpa in the session-context render"
```


---

### Task 8: Prometheus families for lifecycle events

**Files:**
- Modify: `crates/phronesis-metrics/src/families.rs` (label structs ~:55-85, `build` ~:180-265, registrations ~:300-357)
- Test: `crates/phronesis-metrics/tests/derivation.rs`

**Interfaces:**
- Consumes: `LogRecord::{kind, event, str_field, num}`; action-log fields `host`, `mode`, `duration_secs`.
- Produces: `phronesis_lifecycle_events_total{host,event,mode}` and `phronesis_subagent_duration_seconds{host}`. No new Rust API.

- [ ] **Step 1: Write the failing tests** — append to `tests/derivation.rs`

```rust
#[test]
fn lifecycle_events_counter_labels_host_event_and_mode() {
    let out = render(
        vec![
            record(serde_json::json!({
                "ts": 100, "kind": "lifecycle", "event": "prompt", "host": "claude",
                "sid": "s-1", "seq": 1, "mode": "correction",
                "kalpa": "lifecycle-events", "prompt": "SECRET", "prompt_bytes": 6,
            })),
            record(serde_json::json!({
                "ts": 110, "kind": "lifecycle", "event": "interrupt", "host": "codex",
                "sid": "s-1", "seq": 2, "inferred_from": "hook",
            })),
        ],
        &Options::default(),
    );
    assert!(out.contains(r#"host="claude",event="prompt",mode="correction""#), "{out}");
    // Non-prompt events carry an empty mode label rather than a missing one.
    assert!(out.contains(r#"host="codex",event="interrupt",mode="""#), "{out}");
    assert!(!out.contains("kalpa"), "kalpa is free text and must not be a label: {out}");
    assert!(!out.contains("SECRET"), "prompt text must never reach a metric: {out}");
}

#[test]
fn subagent_duration_histogram_has_thirteen_exponential_buckets_and_a_host_label() {
    let out = render(
        vec![
            record(serde_json::json!({
                "ts": 100, "kind": "lifecycle", "event": "subagent_stop", "host": "claude",
                "sid": "s-1", "seq": 1, "agent_id": "a1", "agent_type": "reviewer",
                "duration_secs": 220, "matched_start": true,
            })),
            record(serde_json::json!({
                "ts": 200, "kind": "lifecycle", "event": "subagent_stop", "host": "claude",
                "sid": "s-1", "seq": 2, "agent_id": "a2", "duration_secs": 3, "matched_start": false,
            })),
            // No duration (unmatched stop): counted as an event, never observed.
            record(serde_json::json!({
                "ts": 300, "kind": "lifecycle", "event": "subagent_stop", "host": "claude",
                "sid": "s-1", "seq": 3, "matched_start": false,
            })),
        ],
        &Options::default(),
    );
    let buckets = out.lines()
        .filter(|l| l.starts_with("phronesis_subagent_duration_seconds_bucket"))
        .count();
    // `exponential_buckets(1.0, 2.0, 13)` is 13 finite buckets — the last is
    // 4096 s, about 68 min — plus `+Inf`.
    assert_eq!(buckets, 14, "exponential_buckets(1.0, 2.0, 13) plus +Inf:\n{out}");
    assert!(out.contains(r#"le="4096""#), "the last finite bucket is 4096 s: {out}");
    // `host` is a label on the histogram too, so one host's slow sub-agents do
    // not smear another's distribution.
    assert!(out.contains(r#"phronesis_subagent_duration_seconds_count{host="claude"} 2"#), "{out}");
    assert!(out.contains(r#"phronesis_subagent_duration_seconds_sum{host="claude"} 223"#), "{out}");
}

/// Two hosts, two series. Without the label they share one distribution and the
/// median of a mixed fleet means nothing.
#[test]
fn subagent_duration_is_split_by_host() {
    let out = render(
        vec![
            record(serde_json::json!({
                "ts": 100, "kind": "lifecycle", "event": "subagent_stop", "host": "claude",
                "sid": "s-1", "seq": 1, "duration_secs": 10, "matched_start": true,
            })),
            record(serde_json::json!({
                "ts": 110, "kind": "lifecycle", "event": "subagent_stop", "host": "codex",
                "sid": "s-1", "seq": 2, "duration_secs": 1000, "matched_start": true,
            })),
        ],
        &Options::default(),
    );
    assert!(out.contains(r#"phronesis_subagent_duration_seconds_sum{host="claude"} 10"#), "{out}");
    assert!(out.contains(r#"phronesis_subagent_duration_seconds_sum{host="codex"} 1000"#), "{out}");
    assert!(!out.contains("kalpa"), "still no kalpa label: {out}");
}

#[test]
fn lifecycle_records_respect_the_since_cutoff() {
    let out = render(
        vec![record(serde_json::json!({
            "ts": 10, "kind": "lifecycle", "event": "commit", "host": "claude",
            "sid": "s-1", "seq": 1, "sha": "0f3c",
        }))],
        &Options { since: Some(100), ..Options::default() },
    );
    assert!(!out.contains(r#"event="commit""#), "{out}");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-metrics --test derivation lifecycle subagent 2>&1 | tail -30`
Expected: FAIL — neither family appears in the exposition.

- [ ] **Step 3: Implement** — in `families.rs`

```rust
use prometheus_client::metrics::histogram::{Histogram, exponential_buckets};

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct LifecycleLabels {
    host: String,
    event: String,
    /// Prompt mode; `""` for every non-prompt event. An empty value keeps the
    /// label set closed; an absent label would split the series.
    mode: String,
}

/// The histogram's label set is `{host}` alone: a duration is a property of the
/// sub-agent, and `event` is always `subagent_stop` here.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct HostLabel {
    host: String,
}
```

In `build`, beside the other family declarations:

```rust
    let lifecycle_events = Family::<LifecycleLabels, Counter>::default();
    // 13 buckets, so the last finite one is 4096 s — about 68 min, the useful
    // range of a sub-agent's life. A `Family` of histograms needs an explicit
    // constructor: `Default` would give every series the default bucket set.
    let subagent_duration = Family::<HostLabel, Histogram>::new_with_constructor(|| {
        Histogram::new(exponential_buckets(1.0, 2.0, 13))
    });
```

New match arm after `"context"`:

```rust
            "lifecycle" => {
                // `kalpa` is deliberately not a label: user-typed free text,
                // the same reason rule ids are capped above.
                lifecycle_events
                    .get_or_create(&LifecycleLabels {
                        host: rec.str_field("host").unwrap_or("unknown").to_string(),
                        event: rec.event.clone(),
                        mode: rec.str_field("mode").unwrap_or_default().to_string(),
                    })
                    .inc();
                if rec.event == "subagent_stop"
                    && let Some(secs) = rec.num("duration_secs")
                {
                    subagent_duration
                        .get_or_create(&HostLabel {
                            host: rec.str_field("host").unwrap_or("unknown").to_string(),
                        })
                        .observe(secs as f64);
                }
            }
```

Registrations, beside the others (counter registered without `_total`):

```rust
    registry.register(
        "phronesis_lifecycle_events",
        "Agent lifecycle events recorded in the action log, by host, event, and prompt mode",
        lifecycle_events,
    );
    registry.register(
        "phronesis_subagent_duration_seconds",
        "Wall-clock seconds between a sub-agent's start and its stop, by host",
        subagent_duration,
    );
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-metrics 2>&1 | tail -30`
Expected: pass, including `exposition_is_well_formed` and `file_paths_never_become_labels`.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-metrics/src/families.rs crates/phronesis-metrics/tests/derivation.rs
git commit -m "feat(metrics): lifecycle event counter and sub-agent duration histogram"
```

---

### Task 9: CHANGELOG

**Files:**
- Modify: `CHANGELOG.md` (`## [Unreleased]`)

- [ ] **Step 1: Add the entries** — under `### Added` (create the subsection if absent)

```markdown
- `phr-mcp stats` prints a lifecycle section — sessions, prompts by mode,
  interrupts, sub-agents with median duration, and commits with their
  confidence band — plus the active kalpa and the retention boundary of the
  action log. `--kalpa <name>` restricts the section to one kalpa.
- `phr-mcp kalpa show [name]` reports the same counts for a kalpa, open or
  closed, with the date it started and the retention boundary.
- `phr-mcp journey` renders lifecycle records inline with a `⟂` marker and the
  record's kind/mode in place of a path, and prints the active kalpa in its
  header. `--lifecycle` shows only those records; `--corrections` lists the
  prompts that followed an interrupt, oldest first, with their scrubbed text.
- Prometheus: `phronesis_lifecycle_events_total{host,event,mode}` and
  `phronesis_subagent_duration_seconds{host}` (13 exponential buckets, 1 s to
  ~68 min). No kalpa label and no `agent_type` label — both are free text,
  user-typed and model-supplied respectively.
```

And, still under `### Added` (this is additive, not a change — nothing about the existing response moves):

```markdown
- The `get_journey` MCP tool takes an optional `include_lifecycle` boolean
  (default `false`). Left off, it returns exactly the bare array of fact rows
  it always has. Set to `true`, it returns
  `{"facts": [...], "lifecycle": [...]}` so lifecycle records travel with the
  facts they explain. Prompt text is never included either way, and
  `phr-mcp journey --json` is unchanged.
```

Nothing goes under `### Changed` for this plan: no existing output shape moves.

- [ ] **Step 1b: Add the upgrade note**

This plan merges last, so it is where the release's user-facing note lands. Spec
§Rollout: "**Upgrade note for users**, in the release notes as well as the
CHANGELOG." Add it as its own `### Upgrading` subsection under
`## [Unreleased]`, after `### Added`:

```markdown
### Upgrading

- **Upgrading the binary registers nothing.** Run `phr-mcp init` in each project
  to get the new hook registrations. Codex users then re-trust hooks via
  `/hooks`; Gemini users must trust the folder, or project hooks are skipped.
  Until you run `init`, `session-context` and `interaction-context` keep behaving
  exactly as they do today and no lifecycle event is recorded.
- **`s`-window journey rules now scope to a session.** The `.phronesis/journey/session`
  file used to be create-on-miss and never overwritten, so in practice a
  project's session id — and therefore every `s` window — spanned the file's
  lifetime. Each session-begin `SessionStart` now mints a new id, which is what
  the window name always claimed. This is a permanent semantic change, not a
  one-time boundary: review your `s`-window rules, which now see shorter windows.
  Rules using `Nc` or time windows are unaffected.
- **Claude users with outcomes enabled get the confidence gate on turn stop for
  the first time.** It is disabled the same way it is disabled for Codex today.
- **The hooks and the MCP server upgrade in lockstep.** A project that has
  written a `lifecycle:*` rule requires ≥ 0.35: an older binary fails closed with
  `UndefinedSelector` on the first such rule, taking every journey fact with it,
  and reads lifecycle records as odd `__lifecycle` tool records that shift
  positional windows.
```

- [ ] **Step 2: Verify the whole suite**

Run: `cargo test -p phronesis-mcp -p phronesis-metrics 2>&1 | tail -30`
Expected: no failures.

- [ ] **Step 3: Commit**

```bash
git add CHANGELOG.md
git commit -m "docs(changelog): lifecycle reporting surfaces and metrics"
```

---

## Self-review

**1. Spec coverage**

| spec requirement | task |
|---|---|
| §Action log: stats gains a `lifecycle` section — per event, per prompt mode, sub-agent count + in-process median, commit count | 1, 2 |
| §Action log: kalpa name in the stats header | 2 (`kalpa_cli::header_line`) |
| §Action log: `phronesis_lifecycle_events_total{host,event,mode}`, `mode` empty for non-prompt | 8 |
| §Action log: `phronesis_subagent_duration_seconds{host}`, `exponential_buckets(1.0, 2.0, 13)`, no kalpa label | 8 |
| §Action log: sub-agent median computed in-process from the log **and its one rotated predecessor** | 1 |
| §Reporting: `sub-agents` counts starts, `matched` is the paired subset, the median is over those durations alone | 1 |
| §Reporting: the band segment is omitted when no commit carries one; `start not retained` when the boundary has rotated off | 1, 3 |
| §Action log: `prompt_text` enforced at read time through `lifecycle::correction_text` | 5 |
| §Reporting: `kalpa show <name>` / `stats --kalpa <name>` read log + rotated predecessor, raw counts, retention boundary in the header | 1–3 (`ReadOpts` with no limit reads both files; `retention_line`) |
| §Reporting: the five-line block (sessions / prompts by mode / interrupts / sub-agents + median / commits + bands) | 1 (renderer), 3 (report) |
| §Reporting: no ratios | Constraints; `LifecycleStats` holds counts only |
| §CLI and MCP: journey renders lifecycle inline with `⟂` and kind/mode instead of path; kalpa in the header | 4 |
| §CLI and MCP: `journey --lifecycle` | 4 |
| §CLI and MCP: `journey --corrections` from the action log — ts, sid, scrubbed prompt, oldest first | 5 |
| §CLI and MCP: `get_journey` includes lifecycle records, no new tool | 6 (opt-in via `include_lifecycle`; the default response shape is unchanged) |
| §Reporting: the session-context render prints the active kalpa and its age | 7 |
| §Testing: `journey_cli_integration.rs`, `kalpa_integration.rs`, `derivation.rs` additions | 2–5, 8 |
| §Rollout: hand-written CHANGELOG under `## [Unreleased]` | 9 |
| §Rollout: the upgrade note (`init` registers nothing on upgrade, `s` windows narrow, the Claude stop gate, the lockstep requirement) | 9 |

Out of scope by design: commit detection (Plan 1 Task 9), kalpa start/end/validation (Plan 1 Task 11), and the adapters that emit events (Plans 2–4).

One spec line is deliberately deferred rather than implemented: §Reporting's `phr-mcp kalpa show <name>` sentence says the report reads "the lifecycle entries in `.phronesis/log.jsonl` **and its one rotated predecessor**". `action_log::read_recent` already reads both files, so this is satisfied for free — Task 1's `retention_line` exists precisely because the rotated predecessor is the far edge of what the log can answer. No separate work.

**2. Placeholder scan**

No "TBD", no "add error handling", no "similar to Task N". Every code step carries compilable Rust and every test step its assertions. Task 6's shape note states a decision rather than deferring work; Tasks 4 and 5 each spell out their own dispatch arm because the clap variant grows twice; Task 7 writes out both context render paths in full.

**3. Type consistency**

- `LifecycleOpts` / `LifecycleStats` / `aggregate_lifecycle` / `render_lifecycle` / `retention_line` / `humanize_duration` / `render_json_with_lifecycle`: defined in Task 1, used verbatim in Tasks 2 and 3.
- `LifecycleRow` (`ts, sid, seq, kind, mode, host, agent_type, kalpa`) and `lifecycle_rows(&Path, usize)`: defined Task 4, reused in Task 6.
- `CorrectionRow` / `corrections` / `render_corrections`: defined and used in Task 5.
- `EpistemeMcp::journey_payload(&[JourneyRow], &[LifecycleRow])` and `GetJourneyParams::include_lifecycle`: defined and used in Task 6.
- `lifecycle::kalpa_cli::header_line(&Path, u64) -> Option<String>` (Plan 1 Task 11): called unchanged by Tasks 2, 4 and 7.
- Plan 1 API used unchanged: `lifecycle::{Host, Kind, LifecycleEvent, Mode, PromptText, Stamped}` with `with_mode` / `with_agent` / `with_prompt` / `with_extra` / `to_log_entry`; `lifecycle::state::read_kalpa`; `lifecycle::kalpa_cli::{header_line, KalpaCmd, age, now}`; `journey::journal::{read_recent, SUFFIX_HARD_CAP, JournalError}`; `JournalRecord::is_lifecycle`.
- Action-log fields read here — `sid`, `mode`, `kalpa`, `host`, `duration_secs`, `confidence_band`, `prompt` — are exactly those `to_log_entry` writes in Plan 1 Task 4.
