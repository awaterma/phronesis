//! `phr-mcp unit show` — one work item, joined across the journal and the
//! action log on `subject`.
//!
//! The question this answers is the one the spec opens §"Work items and
//! governed throughput" with: for one piece of work, which spec was it built
//! to, which rules fired, what evidence was recorded, and where did a human
//! step in. Nothing here writes; every read is fail-open.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::json;

use crate::action_log::{self, LogEntry, ReadOpts};
use crate::lifecycle::record::correction_text;
use crate::outcomes;

/// One human steer inside the work item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Intervention {
    pub ts: u64,
    /// `mid_turn` or `correction` — the two prompt modes that are steers.
    pub mode: String,
    /// The scrubbed prompt, or `None` under `prompt_text: "none"`.
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitRow {
    pub sha: String,
    pub band: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UnitReport {
    pub unit_id: String,
    /// True when a `unit_start` record exists for this id.
    pub explicit: bool,
    pub spec: Option<String>,
    pub first_ts: Option<u64>,
    pub last_ts: Option<u64>,
    pub kalpa: Option<String>,
    /// `pre_check` + `post_check` entries carrying this subject.
    pub rules_evaluated: u32,
    /// Those with at least one consequence.
    pub fired: u32,
    /// Those that exited 2.
    pub blocked: u32,
    /// Those that exited 1.
    pub warned: u32,
    /// Consequence counts by rule id, over the same entries.
    pub per_rule: BTreeMap<String, u32>,
    /// Names of passed grounded signals (`compile`, `tests`, `bug:<id>`).
    pub signals: Vec<String>,
    pub band: Option<String>,
    pub interventions: Vec<Intervention>,
    pub mid_turn: u32,
    pub correction: u32,
    pub commits: Vec<CommitRow>,
}

fn str_field<'a>(e: &'a LogEntry, key: &str) -> Option<&'a str> {
    e.data.get(key).and_then(|v| v.as_str())
}

/// Widen the `[first_ts, last_ts]` window to include `ts`.
fn widen(r: &mut UnitReport, ts: u64) {
    r.first_ts = Some(r.first_ts.map_or(ts, |t| t.min(ts)));
    r.last_ts = Some(r.last_ts.map_or(ts, |t| t.max(ts)));
}

/// Build the report for `unit_id`.
///
/// Reads the whole action log and its one rotated predecessor (`ReadOpts` with
/// no limit does both, oldest first) and filters on `subject` in process: the
/// filter is a field match, not a `kind`, so it cannot be pushed into
/// `read_recent`. The journal contributes the grounded outcome signals through
/// `outcomes::report`, which reads it per subject.
pub fn build(root: &Path, unit_id: &str) -> UnitReport {
    let mut r = UnitReport {
        unit_id: unit_id.to_string(),
        ..UnitReport::default()
    };

    let entries = action_log::read_recent(&action_log::default_path(root), &ReadOpts::default())
        .unwrap_or_default();
    for e in &entries {
        if str_field(e, "subject") != Some(unit_id) {
            continue;
        }
        widen(&mut r, e.ts);
        if let Some(k) = str_field(e, "kalpa") {
            r.kalpa = Some(k.to_string());
        }
        match (e.kind.as_str(), e.event.as_str()) {
            ("hook", "pre_check" | "post_check") => {
                r.rules_evaluated += 1;
                match e.data.get("exit").and_then(|v| v.as_i64()) {
                    Some(2) => r.blocked += 1,
                    Some(1) => r.warned += 1,
                    _ => {}
                }
                let cons = e.data.get("consequences").and_then(|v| v.as_array());
                let cons = cons.map(|c| c.as_slice()).unwrap_or(&[]);
                if !cons.is_empty() {
                    r.fired += 1;
                }
                for c in cons {
                    if let Some(id) = c.get("rule_id").and_then(|v| v.as_str()) {
                        *r.per_rule.entry(id.to_string()).or_insert(0) += 1;
                    }
                }
            }
            ("lifecycle", "unit_start") => {
                r.explicit = true;
                if let Some(s) = str_field(e, "spec") {
                    r.spec = Some(s.to_string());
                }
            }
            ("lifecycle", "prompt") => {
                let mode = str_field(e, "mode").unwrap_or("fresh");
                if !matches!(mode, "mid_turn" | "correction") {
                    continue;
                }
                if mode == "mid_turn" {
                    r.mid_turn += 1;
                } else {
                    r.correction += 1;
                }
                // The one accessor for prompt text: it consults the *current*
                // `prompt_text` value, so flipping the switch to "none" hides
                // text already written under "full" (spec §"Action log").
                r.interventions.push(Intervention {
                    ts: e.ts,
                    mode: mode.to_string(),
                    text: correction_text(root, e),
                });
            }
            ("lifecycle", "commit") => r.commits.push(CommitRow {
                sha: str_field(e, "sha").unwrap_or_default().to_string(),
                band: str_field(e, "confidence_band").map(str::to_string),
            }),
            _ => {}
        }
    }

    // Grounded evidence. Gated on `enabled`: with confidence scoring off there
    // are no signals to find, and `outcomes::report` would report band `low`
    // from an empty ledger, which reads as a measurement rather than as "not
    // measured".
    if outcomes::enabled(root)
        && let Some(rep) = outcomes::report(root, Some(unit_id))
    {
        r.signals = rep.signals;
        r.band = Some(rep.band.as_str().to_string());
    }

    r
}

fn hm(ts: u64) -> String {
    chrono::DateTime::from_timestamp(ts as i64, 0)
        .map(|dt| dt.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_else(|| ts.to_string())
}

fn ymd_hm(ts: u64) -> String {
    chrono::DateTime::from_timestamp(ts as i64, 0)
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| ts.to_string())
}

fn ymd(ts: u64) -> String {
    chrono::DateTime::from_timestamp(ts as i64, 0)
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%Y-%m-%d")
                .to_string()
        })
        .unwrap_or_default()
}

/// `2026-09-18 14:02 → 16:40` within one day, both dates across a boundary.
fn window(first: u64, last: u64) -> String {
    if ymd(first) == ymd(last) {
        format!("{} → {}", ymd_hm(first), hm(last))
    } else {
        format!("{} → {}", ymd_hm(first), ymd_hm(last))
    }
}

/// One prompt can be a page; the report is a scan, not a transcript.
fn clip(text: &str, max_chars: usize) -> String {
    let flat = text.replace('\n', " ");
    if flat.chars().count() <= max_chars {
        return flat;
    }
    let head: String = flat.chars().take(max_chars).collect();
    format!("{head} …")
}

/// The block from SPEC-agent-lifecycle-events §"Work items and governed
/// throughput", oldest first. Sections with nothing to say are omitted rather
/// than printed as zeros.
pub fn render(r: &UnitReport) -> String {
    let mut out = String::new();
    let kind = if r.explicit { "explicit" } else { "implicit" };
    out.push_str(&format!("unit: {}   {kind}", r.unit_id));
    if let Some(s) = &r.spec {
        out.push_str(&format!("   spec: {s}"));
    }
    out.push('\n');

    if let (Some(first), Some(last)) = (r.first_ts, r.last_ts) {
        out.push_str(&format!("window: {}", window(first, last)));
        if let Some(k) = &r.kalpa {
            out.push_str(&format!("   kalpa: {k}"));
        }
        out.push('\n');
    }

    if r.rules_evaluated > 0 {
        out.push_str(&format!(
            "{:<15}{:>4}   fired {}   blocked {}   warned {}\n",
            "rules evaluated", r.rules_evaluated, r.fired, r.blocked, r.warned
        ));
        if !r.per_rule.is_empty() {
            // Loudest first, then alphabetically, so the same data always
            // renders the same way. Every rule is printed: a governance report
            // that elides rules is worse than a long one.
            let mut rules: Vec<(&String, &u32)> = r.per_rule.iter().collect();
            rules.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
            let cells: Vec<String> = rules.iter().map(|(id, n)| format!("{id}  {n}")).collect();
            out.push_str(&format!("  {}\n", cells.join("   ")));
        }
    }

    if r.band.is_some() || !r.signals.is_empty() {
        // The spec's illustration reads `tests pass (12/12)`; the ledger holds
        // signal names and a band, not test counts, so the counts are not
        // printed rather than invented.
        let mut cells: Vec<String> = r.signals.iter().map(|s| format!("{s} pass")).collect();
        if let Some(b) = &r.band {
            cells.push(format!("band: {b}"));
        }
        out.push_str(&format!("{:<17}{}\n", "evidence", cells.join("   ")));
    }

    if !r.interventions.is_empty() {
        out.push_str(&format!(
            "{:<15}{:>4}   mid_turn {}   correction {}\n",
            "interventions",
            r.interventions.len(),
            r.mid_turn,
            r.correction
        ));
        for i in &r.interventions {
            let text = match &i.text {
                Some(t) => format!("\"{}\"", clip(t, 72)),
                None => "(text withheld)".to_string(),
            };
            out.push_str(&format!("  {}  {}  {text}\n", hm(i.ts), i.mode));
        }
    }

    if !r.commits.is_empty() {
        let cells: Vec<String> = r
            .commits
            .iter()
            .map(|c| match &c.band {
                Some(b) => format!("{}  band {b}", c.sha),
                None => c.sha.clone(),
            })
            .collect();
        out.push_str(&format!(
            "{:<15}{:>4}   {}\n",
            "commits",
            r.commits.len(),
            cells.join("   ")
        ));
    }
    out
}

/// `--json`: the same report as one object.
pub fn render_json(r: &UnitReport) -> String {
    json!({
        "unit_id": r.unit_id,
        "explicit": r.explicit,
        "spec": r.spec,
        "first_ts": r.first_ts,
        "last_ts": r.last_ts,
        "kalpa": r.kalpa,
        "rules_evaluated": r.rules_evaluated,
        "fired": r.fired,
        "blocked": r.blocked,
        "warned": r.warned,
        "per_rule": r.per_rule,
        "signals": r.signals,
        "band": r.band,
        "mid_turn": r.mid_turn,
        "correction": r.correction,
        "interventions": r.interventions.iter().map(|i| json!({
            "ts": i.ts, "mode": i.mode, "text": i.text
        })).collect::<Vec<_>>(),
        "commits": r.commits.iter().map(|c| json!({
            "sha": c.sha, "band": c.band
        })).collect::<Vec<_>>(),
    })
    .to_string()
}
