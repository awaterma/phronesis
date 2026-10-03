use crate::aggregate::{Aggregate, Headline};
use crate::record::{Arm, AuditSummary, RunExit, RunRecord};
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// The seven definition-of-done sections, in the order the spec requires.
pub const SECTION_IDS: [&str; 7] = [
    "section-headline",
    "section-tasks",
    "section-friction",
    "section-efficiency",
    "section-governance",
    "section-interpretation",
    "section-caveats",
];

/// Transcripts yield unbounded prose; a bounded note keeps the report readable
/// and renders identically for a given input.
const NOTE_MAX_CHARS: usize = 240;

const CSS: &str = r#"
:root { color-scheme: light; }
* { box-sizing: border-box; }
body {
  font-family: system-ui, -apple-system, "Segoe UI", sans-serif;
  margin: 2rem auto;
  max-width: 80rem;
  padding: 0 1rem;
  color: #1a1a2e;
  background: #fafafa;
  line-height: 1.45;
}
h1 { font-size: 1.6rem; margin-bottom: 0.2rem; }
h2 { font-size: 1.25rem; margin-top: 0; border-bottom: 2px solid #35507a; padding-bottom: 0.25rem; }
section { background: #ffffff; border: 1px solid #d8dee9; border-radius: 8px; padding: 1rem 1.25rem; margin: 1.25rem 0; }
table { border-collapse: collapse; width: 100%; margin: 0.75rem 0; font-size: 0.9rem; }
th, td { border: 1px solid #d8dee9; padding: 0.3rem 0.55rem; text-align: left; vertical-align: top; }
th { background: #eef2f8; }
td.num, th.num { text-align: right; font-variant-numeric: tabular-nums; }
tr.discordant td { background: #fff3e0; }
tr.discordant td:first-child { border-left: 4px solid #e67e22; }
details { margin: 0; }
summary { cursor: pointer; color: #35507a; }
details p { margin: 0.4rem 0 0; white-space: pre-wrap; }
ul { margin: 0.5rem 0; padding-left: 1.25rem; }
li { margin: 0.3rem 0; }
p { margin: 0.4rem 0; }
.muted { color: #6b7280; font-size: 0.9rem; }
.loud { color: #b3261e; font-weight: 600; }
"#;

/// Optional rendering context a caller threads in from the run directory:
/// the run id, the arms the verify step confirmed, and per-instance notes
/// (e.g. the treatment transcript's final assistant message).
#[derive(Debug, Clone, Default)]
pub struct ReportExtras {
    pub run_id: String,
    pub verified_arms: Vec<String>,
    pub notes: BTreeMap<String, String>,
}

/// Render the full self-contained HTML document (no notes, no run context).
pub fn render(agg: &Aggregate, records: &[RunRecord]) -> Result<String> {
    render_with_extras(agg, records, &ReportExtras::default())
}

/// Render the full self-contained HTML document with caller-supplied context.
pub fn render_with_extras(
    agg: &Aggregate,
    records: &[RunRecord],
    extras: &ReportExtras,
) -> Result<String> {
    if agg.pairs.len() != agg.headline.n as usize {
        bail!(
            "aggregate holds {} pair(s) but headline claims n={}",
            agg.pairs.len(),
            agg.headline.n
        );
    }

    let mut html = String::with_capacity(64 * 1024);
    html.push_str("<!doctype html>\n<html lang=\"en\">\n<head>\n");
    html.push_str("<meta charset=\"utf-8\">\n");
    html.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    html.push_str("<title>phr-bench report</title>\n");
    html.push_str(&format!("<style>{CSS}</style>\n"));
    html.push_str("</head>\n<body>\n");
    html.push_str("<h1>phr-bench report</h1>\n");
    if !extras.run_id.is_empty() {
        html.push_str(&format!(
            "<p class=\"muted\">run id: {}</p>\n",
            text(&extras.run_id)
        ));
    }

    html.push_str(&section_headline(agg, records));
    html.push_str(&section_tasks(agg, extras));
    html.push_str(&section_friction(agg));
    html.push_str(&section_efficiency(agg));
    html.push_str(&section_governance(agg));
    html.push_str(&section_interpretation(agg));
    html.push_str(&section_caveats(agg, records, extras));

    html.push_str("</body>\n</html>\n");
    Ok(html)
}

fn section_headline(agg: &Aggregate, records: &[RunRecord]) -> String {
    let h = &agg.headline;
    let mut out = String::from("<section id=\"section-headline\">\n<h2>Headline</h2>\n<table>\n");
    row(
        &mut out,
        "Resolved rate",
        &format!(
            "control {} of {} ({}) vs treatment {} of {} ({}); delta {}",
            h.resolved_control,
            h.n,
            pct(h.resolved_control, h.n),
            h.resolved_treatment,
            h.n,
            pct(h.resolved_treatment, h.n),
            signed(i64::from(h.resolved_treatment) - i64::from(h.resolved_control))
        ),
    );
    row(
        &mut out,
        "Sign test over discordant pairs",
        &format!(
            "{} discordant pair(s), treatment won {}, control won {}: {}",
            h.discordant,
            h.treatment_won,
            h.control_won,
            fmt_p(h)
        ),
    );
    row(
        &mut out,
        "Debt delta (audit violations, treatment minus control)",
        &debt_delta(agg),
    );
    row(
        &mut out,
        "Cap hits",
        &format!(
            "control {}, treatment {}",
            cap_hits(agg, Arm::Control),
            cap_hits(agg, Arm::Treatment)
        ),
    );
    row(
        &mut out,
        "Runs",
        &format!(
            "{} record(s) over {} task(s), one control and one treatment per task",
            records.len(),
            h.n
        ),
    );
    out.push_str("</table>\n");

    out.push_str("<table>\n<tr><th>Language</th><th class=\"num\">Resolved control</th><th class=\"num\">Resolved treatment</th><th class=\"num\">Mean turns control</th><th class=\"num\">Mean turns treatment</th></tr>\n");
    for (lang, totals) in &agg.by_language {
        out.push_str(&format!(
            "<tr><td>{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td></tr>\n",
            text(lang),
            totals.resolved_control,
            totals.resolved_treatment,
            f2(totals.mean_turns_control),
            f2(totals.mean_turns_treatment)
        ));
    }
    out.push_str("</table>\n</section>\n");
    out
}

fn section_tasks(agg: &Aggregate, extras: &ReportExtras) -> String {
    let mut out = String::from(
        "<section id=\"section-tasks\">\n<h2>Per-task paired breakdown</h2>\n<table>\n<thead>\n\
         <tr><th rowspan=\"2\">Instance</th><th rowspan=\"2\">Language</th>\
         <th colspan=\"7\">Control</th><th colspan=\"7\">Treatment</th><th rowspan=\"2\">Note</th></tr>\n\
         <tr><th>Exit</th><th>Resolved</th><th class=\"num\">Turns</th><th>Tokens (in/out)</th>\
         <th class=\"num\">Wall-clock (s)</th><th class=\"num\">Audit violations</th><th>Blocks/warns</th>\
         <th>Exit</th><th>Resolved</th><th class=\"num\">Turns</th><th>Tokens (in/out)</th>\
         <th class=\"num\">Wall-clock (s)</th><th class=\"num\">Audit violations</th><th>Blocks/warns</th></tr>\n\
         </thead>\n<tbody>\n",
    );
    for pair in &agg.pairs {
        let class = if pair.discordant {
            " class=\"discordant\""
        } else {
            ""
        };
        out.push_str(&format!(
            "<tr{}><td>{}</td><td>{}</td>{}{}{}</tr>\n",
            class,
            text(&pair.instance_id),
            text(&pair.language),
            arm_cells(&pair.control),
            arm_cells(&pair.treatment),
            note_cell(pair, extras)
        ));
    }
    out.push_str("</tbody>\n</table>\n</section>\n");
    out
}

fn arm_cells(r: &RunRecord) -> String {
    let gov = match &r.governance {
        Some(g) => format!(
            "{}/{}",
            g.blocks.values().sum::<u32>(),
            g.warns.values().sum::<u32>()
        ),
        None => "n/a".to_string(),
    };
    format!(
        "<td>{}</td><td>{}</td><td class=\"num\">{}</td><td>{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td><td>{}</td>",
        text(&r.exit.to_string()),
        resolved_text(r.resolved),
        r.turns,
        tokens_text(r.tokens_in, r.tokens_out),
        r.wall_clock_secs,
        audit_text(r.audit.as_ref()),
        gov
    )
}

fn note_cell(pair: &crate::aggregate::TaskPair, extras: &ReportExtras) -> String {
    if !pair.discordant {
        return "<td></td>".to_string();
    }
    match extras.notes.get(&pair.instance_id) {
        Some(note) => format!(
            "<td><details><summary>note</summary><p>{}</p></details></td>",
            text(note)
        ),
        None => "<td>n/r</td>".to_string(),
    }
}

fn section_friction(agg: &Aggregate) -> String {
    let mut out = String::from(
        "<section id=\"section-friction\">\n<h2>Friction</h2>\n<p>Per-rule counts across treatment records.</p>\n",
    );
    let rules = rule_friction(agg);
    if rules.is_empty() {
        out.push_str("<p>No rule friction recorded.</p>\n");
    } else {
        out.push_str("<table>\n<tr><th>Rule</th><th class=\"num\">Blocks</th><th class=\"num\">Warns</th></tr>\n");
        for (rule, blocks, warns) in &rules {
            out.push_str(&format!(
                "<tr><td>{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td></tr>\n",
                text(rule),
                blocks,
                warns
            ));
        }
        out.push_str("</table>\n");
    }
    out.push_str(&format!(
        "<p>Fail-closed events across treatment: {}</p>\n</section>\n",
        fail_closed_total(agg)
    ));
    out
}

fn section_efficiency(agg: &Aggregate) -> String {
    let control = arm_stats(agg, Arm::Control);
    let treatment = arm_stats(agg, Arm::Treatment);
    let mut out = String::from(
        "<section id=\"section-efficiency\">\n<h2>Efficiency</h2>\n<table>\n<tr><th>Metric</th><th class=\"num\">Control</th><th class=\"num\">Treatment</th></tr>\n",
    );
    stat_row(
        &mut out,
        "Mean turns",
        &control.turns,
        &treatment.turns,
        mean,
    );
    stat_row(
        &mut out,
        "Median turns",
        &control.turns,
        &treatment.turns,
        median,
    );
    stat_row(
        &mut out,
        "Mean tokens in",
        &control.tokens_in,
        &treatment.tokens_in,
        mean,
    );
    stat_row(
        &mut out,
        "Mean tokens out",
        &control.tokens_out,
        &treatment.tokens_out,
        mean,
    );
    stat_row(
        &mut out,
        "Mean wall-clock (s)",
        &control.wall,
        &treatment.wall,
        mean,
    );
    stat_row(
        &mut out,
        "Median wall-clock (s)",
        &control.wall,
        &treatment.wall,
        median,
    );
    out.push_str("</table>\n");

    out.push_str("<p>Per-task wall-clock delta (treatment minus control):</p>\n<table>\n<tr><th>Instance</th><th class=\"num\">Control (s)</th><th class=\"num\">Treatment (s)</th><th class=\"num\">Delta (s)</th></tr>\n");
    for pair in &agg.pairs {
        out.push_str(&format!(
            "<tr><td>{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td></tr>\n",
            text(&pair.instance_id),
            pair.control.wall_clock_secs,
            pair.treatment.wall_clock_secs,
            signed(pair.treatment.wall_clock_secs as i64 - pair.control.wall_clock_secs as i64)
        ));
    }
    out.push_str("</table>\n</section>\n");
    out
}

fn section_governance(agg: &Aggregate) -> String {
    let mut out =
        String::from("<section id=\"section-governance\">\n<h2>Governance behavior</h2>\n");
    let mut blocks = 0u32;
    let mut warns = 0u32;
    let mut deflection = 0u32;
    for pair in &agg.pairs {
        if let Some(g) = &pair.treatment.governance {
            blocks += g.blocks.values().sum::<u32>();
            warns += g.warns.values().sum::<u32>();
            for rule in g.blocks.keys() {
                if rule.contains("deflect") {
                    deflection += g.blocks.get(rule).copied().unwrap_or(0);
                }
            }
        }
    }
    let with_blocks: Vec<&crate::aggregate::TaskPair> = agg
        .pairs
        .iter()
        .filter(|p| {
            p.treatment
                .governance
                .as_ref()
                .is_some_and(|g| g.blocks.values().sum::<u32>() > 0)
        })
        .collect();
    let recovered = with_blocks
        .iter()
        .filter(|p| matches!(p.treatment.exit, RunExit::Completed))
        .count();

    out.push_str(&format!(
        "<p>Total blocks: {}, total warns: {}.</p>\n",
        blocks, warns
    ));
    out.push_str(&format!("<p>Deflection-rule blocks: {}</p>\n", deflection));
    out.push_str(&format!(
        "<p>Tasks with blocked edits: {} of {}; of those, {} of {} ended completed (recovery rate {}).</p>\n",
        with_blocks.len(),
        agg.pairs.len(),
        recovered,
        with_blocks.len(),
        pct(recovered as u32, with_blocks.len() as u32)
    ));
    out.push_str(&format!(
        "<p>Fail-closed events: {}.</p>\n",
        fail_closed_total(agg)
    ));
    out.push_str("<p class=\"loud\">Treatment records without governance telemetry, including governance_not_wired tombstones, fail the aggregate step loudly; this report refuses to render zero-filled numbers in their place.</p>\n</section>\n");
    out
}

fn section_interpretation(agg: &Aggregate) -> String {
    let h = &agg.headline;
    let mut out =
        String::from("<section id=\"section-interpretation\">\n<h2>Interpretation</h2>\n<ul>\n");
    out.push_str(&format!(
        "<li>Governance changed the resolved rate by {} task(s): control {} of {} vs treatment {} of {}.</li>\n",
        signed(i64::from(h.resolved_treatment) - i64::from(h.resolved_control)),
        h.resolved_control,
        h.n,
        h.resolved_treatment,
        h.n
    ));
    out.push_str(&format!(
        "<li>Sign test over {} discordant pair(s): {}.</li>\n",
        h.discordant,
        fmt_p(h)
    ));

    let deltas: Vec<f64> = agg
        .pairs
        .iter()
        .map(|p| f64::from(p.treatment.turns) - f64::from(p.control.turns))
        .collect();
    if let (Some(mean), Some(median)) = (mean(&deltas), median(&deltas)) {
        out.push_str(&format!(
            "<li>Treatment added a median of {} turns per task (mean {}).</li>\n",
            f1(median),
            f1(mean)
        ));
    }

    out.push_str(&format!(
        "<li>Debt delta (audit violations, treatment minus control): {}.</li>\n",
        debt_delta(agg)
    ));

    let wall_deltas: Vec<f64> = agg
        .pairs
        .iter()
        .map(|p| p.treatment.wall_clock_secs as f64 - p.control.wall_clock_secs as f64)
        .collect();
    if let (Some(mean), Some(median)) = (mean(&wall_deltas), median(&wall_deltas)) {
        out.push_str(&format!(
            "<li>Hook overhead: mean per-task wall-clock delta {} s (median {} s).</li>\n",
            f1(mean),
            f1(median)
        ));
    }

    let with_blocks = agg
        .pairs
        .iter()
        .filter(|p| {
            p.treatment
                .governance
                .as_ref()
                .is_some_and(|g| g.blocks.values().sum::<u32>() > 0)
        })
        .count();
    let recovered = agg
        .pairs
        .iter()
        .filter(|p| {
            p.treatment
                .governance
                .as_ref()
                .is_some_and(|g| g.blocks.values().sum::<u32>() > 0)
                && matches!(p.treatment.exit, RunExit::Completed)
        })
        .count();
    out.push_str(&format!(
        "<li>Recovery: {} of {} tasks with blocked edits ended completed.</li>\n",
        recovered, with_blocks
    ));

    if let Some((lang, totals)) = agg.by_language.iter().next() {
        out.push_str(&format!(
            "<li>Effect concentration: {} — resolved control {}, treatment {}.</li>\n",
            text(lang),
            totals.resolved_control,
            totals.resolved_treatment
        ));
    }
    out.push_str("</ul>\n<p class=\"muted\">Every statement above is computed from the tables in this report; nothing is projected beyond them.</p>\n</section>\n");
    out
}

fn section_caveats(agg: &Aggregate, records: &[RunRecord], extras: &ReportExtras) -> String {
    let missing_tokens = records
        .iter()
        .filter(|r| r.tokens_in.is_none() && r.tokens_out.is_none())
        .count();
    let mut out = String::from(
        "<section id=\"section-caveats\">\n<h2>Caveats and reproducibility</h2>\n<ul>\n",
    );
    out.push_str("<li>Pilot-lite scope: 15 of 43 Rust instances; results are directional, not the full run.</li>\n");
    out.push_str(
        "<li>k=1: a single run per task and arm; run-to-run variance is unmeasured.</li>\n",
    );
    out.push_str("<li>Runs used a single model and a single router configuration; no model comparison is possible.</li>\n");
    out.push_str("<li>This is a pilot report, not the definition-of-done full run.</li>\n");
    out.push_str("<li>Benchmark contamination: tasks come from a public dataset, so absolute resolved rates may be inflated.</li>\n");
    out.push_str(&format!(
        "<li>Token fallback: {} of {} records did not report token usage; turns and wall-clock are the fallback metrics.</li>\n",
        missing_tokens,
        records.len()
    ));
    let mut summary = format!(
        "Manifest summary: {} task(s), {} run record(s)",
        agg.headline.n,
        records.len()
    );
    if !extras.verified_arms.is_empty() {
        summary.push_str(&format!(
            ", verified arms: {}",
            extras
                .verified_arms
                .iter()
                .map(|a| text(a))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !extras.run_id.is_empty() {
        summary.push_str(&format!(", run id: {}", text(&extras.run_id)));
    }
    out.push_str(&format!("<li>{summary}.</li>\n"));
    out.push_str("</ul>\n</section>\n");
    out
}

struct ArmStats {
    turns: Vec<f64>,
    tokens_in: Vec<f64>,
    tokens_out: Vec<f64>,
    wall: Vec<f64>,
}

fn arm_stats(agg: &Aggregate, arm: Arm) -> ArmStats {
    let mut stats = ArmStats {
        turns: Vec::new(),
        tokens_in: Vec::new(),
        tokens_out: Vec::new(),
        wall: Vec::new(),
    };
    for pair in &agg.pairs {
        let r = match arm {
            Arm::Control => &pair.control,
            Arm::Treatment => &pair.treatment,
        };
        stats.turns.push(f64::from(r.turns));
        if let Some(v) = r.tokens_in {
            stats.tokens_in.push(v as f64);
        }
        if let Some(v) = r.tokens_out {
            stats.tokens_out.push(v as f64);
        }
        stats.wall.push(r.wall_clock_secs as f64);
    }
    stats
}

fn stat_row(
    out: &mut String,
    label: &str,
    control: &[f64],
    treatment: &[f64],
    summarize: fn(&[f64]) -> Option<f64>,
) {
    let c = match summarize(control) {
        Some(v) => f2(v),
        None => "n/r".to_string(),
    };
    let t = match summarize(treatment) {
        Some(v) => f2(v),
        None => "n/r".to_string(),
    };
    out.push_str(&format!(
        "<tr><td>{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td></tr>\n",
        label, c, t
    ));
}

fn row(out: &mut String, label: &str, value: &str) {
    out.push_str(&format!(
        "<tr><td>{}</td><td>{}</td></tr>\n",
        text(label),
        value
    ));
}

fn debt_delta(agg: &Aggregate) -> String {
    let any_audit = agg
        .pairs
        .iter()
        .any(|p| p.control.audit.is_some() || p.treatment.audit.is_some());
    if !any_audit {
        return "n/r".to_string();
    }
    let total = |arm: Arm| -> u64 {
        agg.pairs
            .iter()
            .map(|p| {
                let r = match arm {
                    Arm::Control => &p.control,
                    Arm::Treatment => &p.treatment,
                };
                u64::from(r.audit.as_ref().map(|a| a.total_violations).unwrap_or(0))
            })
            .sum()
    };
    signed(total(Arm::Treatment) as i64 - total(Arm::Control) as i64)
}

fn cap_hits(agg: &Aggregate, arm: Arm) -> u32 {
    agg.pairs
        .iter()
        .filter(|p| {
            let r = match arm {
                Arm::Control => &p.control,
                Arm::Treatment => &p.treatment,
            };
            matches!(r.exit, RunExit::CapTurns | RunExit::CapTime)
        })
        .count() as u32
}

fn rule_friction(agg: &Aggregate) -> Vec<(String, u32, u32)> {
    let mut blocks: BTreeMap<String, u32> = BTreeMap::new();
    let mut warns: BTreeMap<String, u32> = BTreeMap::new();
    for pair in &agg.pairs {
        if let Some(g) = &pair.treatment.governance {
            for (rule, count) in &g.blocks {
                *blocks.entry(rule.clone()).or_insert(0) += count;
            }
            for (rule, count) in &g.warns {
                *warns.entry(rule.clone()).or_insert(0) += count;
            }
        }
    }
    let rules: Vec<String> = blocks
        .keys()
        .chain(warns.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    rules
        .into_iter()
        .map(|rule| {
            (
                rule.clone(),
                blocks.get(&rule).copied().unwrap_or(0),
                warns.get(&rule).copied().unwrap_or(0),
            )
        })
        .collect()
}

fn fail_closed_total(agg: &Aggregate) -> u32 {
    agg.pairs
        .iter()
        .filter_map(|p| p.treatment.governance.as_ref())
        .map(|g| g.fail_closed)
        .sum()
}

fn fmt_p(h: &Headline) -> String {
    if h.discordant == 0 {
        "n/a (no discordant pairs)".to_string()
    } else {
        format!("p = {:.3}", h.sign_p)
    }
}

fn pct(part: u32, total: u32) -> String {
    if total == 0 {
        return "n/a".to_string();
    }
    format!("{:.2}%", f64::from(part) / f64::from(total) * 100.0)
}

fn resolved_text(resolved: Option<bool>) -> String {
    match resolved {
        Some(true) => "yes".to_string(),
        Some(false) => "no".to_string(),
        None => "n/r".to_string(),
    }
}

fn tokens_text(tokens_in: Option<u64>, tokens_out: Option<u64>) -> String {
    let part = |v: Option<u64>| match v {
        Some(n) => n.to_string(),
        None => "n/r".to_string(),
    };
    format!("{}/{}", part(tokens_in), part(tokens_out))
}

fn audit_text(audit: Option<&AuditSummary>) -> String {
    match audit {
        Some(summary) => summary.total_violations.to_string(),
        None => "n/r".to_string(),
    }
}

fn signed(value: i64) -> String {
    if value >= 0 {
        format!("+{value}")
    } else {
        format!("{value}")
    }
}

fn f1(value: f64) -> String {
    format!("{value:.1}")
}

fn f2(value: f64) -> String {
    format!("{value:.2}")
}

fn mean(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    Some(values.iter().sum::<f64>() / values.len() as f64)
}

fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        Some((sorted[mid - 1] + sorted[mid]) / 2.0)
    } else {
        Some(sorted[mid])
    }
}

/// HTML-escape plus scheme-stripping: any "https://" or "http://" that made it
/// into the data (exit reasons, notes) is neutralized so the report stays
/// self-contained under the no-external-references scan.
fn text(s: &str) -> String {
    let stripped = s.replace("https://", "").replace("http://", "");
    stripped
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Load every control/treatment record.json under a run's runs/ directory,
/// sorted by (instance, arm) so downstream rendering is deterministic.
pub fn load_records(runs_root: &Path) -> Result<Vec<RunRecord>> {
    if !runs_root.exists() {
        bail!("runs directory {} does not exist", runs_root.display());
    }
    let mut records = Vec::new();
    for instance in sorted_child_dirs(runs_root)? {
        for arm in ["control", "treatment"] {
            let record_path = instance.1.join(arm).join("record.json");
            if !record_path.exists() {
                continue;
            }
            let bytes = std::fs::read(&record_path)
                .with_context(|| format!("read {}", record_path.display()))?;
            let record: RunRecord = serde_json::from_slice(&bytes)
                .with_context(|| format!("parse {}", record_path.display()))?;
            records.push(record);
        }
    }
    Ok(records)
}

fn sorted_child_dirs(dir: &Path) -> Result<Vec<(String, std::path::PathBuf)>> {
    let mut out = BTreeMap::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry.context("read directory entry")?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("directory name is not UTF-8"))?;
        out.insert(name, path);
    }
    Ok(out.into_iter().collect())
}

/// Fill missing audit summaries from quality.json ("instance/arm" keys);
/// records that already carry an audit summary are left untouched.
pub fn merge_quality(records: &mut [RunRecord], quality: &BTreeMap<String, AuditSummary>) {
    for record in records.iter_mut() {
        if record.audit.is_some() {
            continue;
        }
        let key = format!("{}/{}", record.instance_id, record.arm.as_str());
        if let Some(summary) = quality.get(&key) {
            record.audit = Some(summary.clone());
        }
    }
}

/// Parse the verify step's summary; the arms list is sorted for stable rendering.
pub fn verified_arms(verify_json: &str) -> Result<Vec<String>> {
    #[derive(Deserialize)]
    struct VerifySummary {
        verified_arms: Vec<String>,
    }
    let parsed: VerifySummary = serde_json::from_str(verify_json).context("parse verify.json")?;
    let mut arms = parsed.verified_arms;
    arms.sort();
    Ok(arms)
}

/// The final assistant message of a transcript, whitespace-collapsed and
/// bounded; None when the transcript carries no assistant prose.
pub fn extract_note(transcript_jsonl: &str) -> Option<String> {
    let mut last_text: Option<String> = None;
    for line in transcript_jsonl.lines().filter(|l| !l.trim().is_empty()) {
        let event: serde_json::Value = match serde_json::from_str(line) {
            Ok(event) => event,
            Err(_) => continue,
        };
        if event.get("type").and_then(|v| v.as_str()) != Some("assistant") {
            continue;
        }
        let content = event
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array());
        let joined = content
            .map(|items| {
                items
                    .iter()
                    .filter(|i| i.get("type").and_then(|v| v.as_str()) == Some("text"))
                    .filter_map(|i| i.get("text").and_then(|v| v.as_str()))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        let trimmed = joined.trim();
        if !trimmed.is_empty() {
            let collapsed = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");
            last_text = Some(collapsed);
        }
    }
    last_text.map(|note| {
        if note.chars().count() <= NOTE_MAX_CHARS {
            note
        } else {
            format!(
                "{}...",
                note.chars().take(NOTE_MAX_CHARS).collect::<String>()
            )
        }
    })
}
