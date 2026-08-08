//! Table and JSON rendering for drift reports.
//!
//! Evidence formatting lives here rather than as a `Display` impl so the
//! pure types in `types.rs` never import a formatting concern.

use std::fmt::Write as _;

use super::types::{AggregateReport, Availability, Evidence, MissingReason};

fn evidence_compact(evidence: &Evidence) -> String {
    match evidence {
        Evidence::Declared {
            rules,
            superseded_by,
        } => match superseded_by {
            Some(s) => format!("declared enforces={} superseded_by={s}", rules.join(",")),
            None => format!("declared enforces={}", rules.join(",")),
        },
        Evidence::Heuristic {
            score,
            threshold,
            matched_rules,
        } => {
            let cmp = if score >= threshold { ">=" } else { "<" };
            if matched_rules.is_empty() {
                format!("heuristic jaccard={score:.2} {cmp}{threshold:.2}")
            } else {
                format!(
                    "heuristic jaccard={score:.2} {cmp}{threshold:.2} matched={}",
                    matched_rules.join(",")
                )
            }
        }
        Evidence::Structural {
            symbol, resolves, ..
        } => format!("structural symbol={symbol} resolves={resolves}"),
    }
}

fn missing_reason_text(reason: MissingReason) -> &'static str {
    match reason {
        MissingReason::NoFile => "not present (no file)",
        MissingReason::NoDir => "not present (no directory)",
        MissingReason::NoGraph => "not present (no code graph)",
    }
}

pub fn render_table(agg: &AggregateReport) -> String {
    let mut out = String::new();

    for report in &agg.sources {
        match &report.availability {
            Availability::Missing { reason } => {
                let _ = writeln!(
                    out,
                    "{:<10} {}",
                    report.source.as_str(),
                    missing_reason_text(*reason)
                );
                continue;
            }
            Availability::Errored { detail } => {
                let _ = writeln!(out, "{:<10} error: {detail}", report.source.as_str());
                continue;
            }
            Availability::Present { scanned } => {
                let _ = writeln!(
                    out,
                    "{:<10} {} scanned, {} uncovered",
                    report.source.as_str(),
                    scanned,
                    report.uncovered_count
                );
            }
        }
        for item in &report.items {
            let verdict = serde_json::to_value(item.verdict)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| "unknown".to_string());
            let _ = writeln!(
                out,
                "  {:<16} {:<48} {}",
                verdict,
                truncate(&item.subject, 48),
                evidence_compact(&item.evidence)
            );
        }
    }

    let t = &agg.totals;
    let _ = writeln!(
        out,
        "\n{} present, {} missing, {} errored — {} uncovered total",
        t.sources_present, t.sources_missing, t.sources_errored, t.uncovered_total
    );
    if agg.truncated {
        let _ = writeln!(
            out,
            "Items truncated; re-run with a single --source and a higher --limit for detail."
        );
    }
    out
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let kept: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
}

pub fn render_json(agg: &AggregateReport) -> String {
    serde_json::to_string_pretty(agg).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drift::types::AggregateReport;
    use crate::drift::{
        Availability, DriftItem, DriftReport, Evidence, MissingReason, Source, Totals, Verdict,
    };

    fn agg(sources: Vec<DriftReport>) -> AggregateReport {
        AggregateReport {
            sources,
            totals: Totals::default(),
            truncated: false,
        }
    }

    #[test]
    fn table_labels_each_evidence_kind_distinctly() {
        let report = DriftReport {
            source: Source::Wiki,
            availability: Availability::Present { scanned: 2 },
            uncovered_count: 1,
            items: vec![
                DriftItem {
                    subject: "ADR-007 use rustix".to_string(),
                    verdict: Verdict::Covered,
                    category: None,
                    suggestion: None,
                    evidence: Evidence::Declared {
                        rules: vec!["rule-a".to_string()],
                        superseded_by: None,
                    },
                },
                DriftItem {
                    subject: "ADR-009 something".to_string(),
                    verdict: Verdict::Uncovered,
                    category: None,
                    suggestion: None,
                    evidence: Evidence::Heuristic {
                        score: 0.11,
                        threshold: 0.15,
                        matched_rules: vec![],
                    },
                },
            ],
        };
        let out = render_table(&agg(vec![report]));
        assert!(out.contains("declared"), "got {out}");
        assert!(out.contains("heuristic"), "got {out}");
        assert!(out.contains("0.11"), "score must be visible: {out}");
    }

    #[test]
    fn table_reports_a_missing_source_without_pretending_it_is_clean() {
        let out = render_table(&agg(vec![DriftReport::missing(
            Source::Memory,
            MissingReason::NoDir,
        )]));
        assert!(out.contains("memory"), "got {out}");
        assert!(out.contains("not present"), "got {out}");
    }

    #[test]
    fn table_reports_an_errored_source() {
        let out = render_table(&agg(vec![DriftReport::errored(
            Source::Wiki,
            "bad frontmatter in ADR-003".to_string(),
        )]));
        assert!(out.contains("error"), "got {out}");
        assert!(out.contains("bad frontmatter"), "got {out}");
    }

    #[test]
    fn table_announces_truncation() {
        let mut a = agg(vec![DriftReport::missing(
            Source::Wiki,
            MissingReason::NoDir,
        )]);
        a.truncated = true;
        let out = render_table(&a);
        assert!(
            out.contains("truncated"),
            "a silent cap reads as 'nothing more to see': {out}"
        );
    }

    #[test]
    fn json_round_trips_as_an_object() {
        let out = render_json(&agg(vec![DriftReport::missing(
            Source::Wiki,
            MissingReason::NoDir,
        )]));
        let v: serde_json::Value = serde_json::from_str(&out).expect("valid json");
        assert!(v.get("sources").is_some());
        assert!(v.get("totals").is_some());
    }
}
