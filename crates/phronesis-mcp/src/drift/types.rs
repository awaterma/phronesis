//! The common drift envelope. Pure data: serde only, no I/O, no formatting.
//!
//! See `docs/specs/SPEC-drift-consolidation.md` §2-§3.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// One drift corpus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    ClaudeMd,
    Memory,
    Wiki,
    /// Registered by SPEC-rule-staleness. Always reports `Missing` until then.
    Code,
}

impl Source {
    pub const ALL: &'static [Source] =
        &[Source::ClaudeMd, Source::Memory, Source::Wiki, Source::Code];

    pub fn as_str(self) -> &'static str {
        match self {
            Source::ClaudeMd => "claude_md",
            Source::Memory => "memory",
            Source::Wiki => "wiki",
            Source::Code => "code",
        }
    }
}

/// Whether a corpus could be read at all. An absent corpus is data, not a
/// fault: on a fresh project the wiki and memory directories legitimately
/// do not exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Availability {
    Present { scanned: usize },
    Missing { reason: MissingReason },
    Errored { detail: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MissingReason {
    NoFile,
    NoDir,
    NoGraph,
}

/// The coverage axis: does a rule already enforce this?
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Covered,
    LikelyCovered,
    Uncovered,
    Superseded,
    /// SPEC-rule-staleness §3.2.
    Moved,
    /// SPEC-rule-staleness §3.2.
    Stale,
}

/// Triage urgency. `Ord` is derived, so declaration order is the ordering:
/// least urgent first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    Covered,
    Superseded,
    Uncovered,
    Broken,
}

impl Verdict {
    pub fn family(self) -> Family {
        match self {
            Verdict::Covered | Verdict::LikelyCovered => Family::Covered,
            Verdict::Superseded => Family::Superseded,
            Verdict::Uncovered => Family::Uncovered,
            Verdict::Moved | Verdict::Stale => Family::Broken,
        }
    }
}

/// What kind of guidance this is. Orthogonal to [`Verdict`]: an
/// `Actionable` memory entry may be covered or uncovered. Only the memory
/// source classifies; others emit `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// Names a tool / command / code shape. Should become a rule.
    Actionable,
    /// Project-shareable ambient guidance. Belongs in durable.md.
    Ambient,
    /// Personal preference. Stays in MEMORY.md.
    Personal,
}

/// Why we believe what we believe. The three variants deliberately share no
/// field, so a consumer can tell them apart from the `kind` tag alone.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Evidence {
    /// An author wrote down which rules enforce this. Not inferred, and so
    /// carries no score.
    Declared {
        rules: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        superseded_by: Option<String>,
    },
    /// Token-overlap heuristic. A triage hint, not ground truth.
    Heuristic {
        score: f32,
        threshold: f32,
        matched_rules: Vec<String>,
    },
    /// Resolved against the code graph. Boolean, not a confidence.
    Structural {
        symbol: String,
        bound_to: Vec<String>,
        resolves: bool,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        relocated: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        bound_at: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        stale_at: Option<i64>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DriftItem {
    pub subject: String,
    pub verdict: Verdict,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<Category>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
    pub evidence: Evidence,
}

#[derive(Debug, Clone, Serialize)]
pub struct DriftReport {
    pub source: Source,
    pub availability: Availability,
    pub uncovered_count: usize,
    pub items: Vec<DriftItem>,
}

impl DriftReport {
    /// A report for a corpus that is not present.
    pub fn missing(source: Source, reason: MissingReason) -> Self {
        DriftReport {
            source,
            availability: Availability::Missing { reason },
            uncovered_count: 0,
            items: Vec::new(),
        }
    }

    /// A report for a corpus that is present but could not be read.
    pub fn errored(source: Source, detail: String) -> Self {
        DriftReport {
            source,
            availability: Availability::Errored { detail },
            uncovered_count: 0,
            items: Vec::new(),
        }
    }
}

/// Counts `Uncovered` and `Broken`, excluding anything classified
/// `Personal`.
///
/// Two exclusions, for the same reason: neither is drift, because nothing
/// is missing. A `Superseded` decision was deliberately replaced, and a
/// `Personal` memory entry belongs in `MEMORY.md` rather than in a rule.
///
/// The `Personal` exclusion has to be checked on `category`, not
/// `verdict`. Splitting the two axes means a personal entry scoring below
/// the threshold is `Verdict::Uncovered` — correctly, since no rule covers
/// it — but it must still not be counted, or the one number an operator
/// uses to decide whether there is work to do is inflated by entries that
/// should never become rules.
pub fn uncovered_count(items: &[DriftItem]) -> usize {
    items
        .iter()
        .filter(|i| i.category != Some(Category::Personal))
        .filter(|i| matches!(i.verdict.family(), Family::Uncovered | Family::Broken))
        .count()
}

#[derive(Debug, Clone, Serialize)]
pub struct AggregateReport {
    pub sources: Vec<DriftReport>,
    pub totals: Totals,
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Totals {
    pub sources_present: usize,
    pub sources_missing: usize,
    pub sources_errored: usize,
    pub uncovered_total: usize,
    pub by_family: BTreeMap<Family, usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structural_evidence_has_no_score_field() {
        let e = Evidence::Structural {
            symbol: "execute_all_agenda_items".to_string(),
            bound_to: vec!["crate::engine::Agenda::execute_all_agenda_items".to_string()],
            resolves: false,
            relocated: Vec::new(),
            bound_at: None,
            stale_at: None,
        };
        let json = serde_json::to_string(&e).expect("serialize");
        assert!(json.contains("\"kind\":\"structural\""), "got {json}");
        assert!(
            !json.contains("score"),
            "structural must not carry a score: {json}"
        );
        assert!(
            !json.contains("threshold"),
            "structural must not carry a threshold: {json}"
        );
    }

    #[test]
    fn declared_evidence_has_no_score_field() {
        let e = Evidence::Declared {
            rules: vec!["rule-a".to_string()],
            superseded_by: None,
        };
        let json = serde_json::to_string(&e).expect("serialize");
        assert!(json.contains("\"kind\":\"declared\""), "got {json}");
        assert!(
            !json.contains("score"),
            "declared must not carry a score: {json}"
        );
    }

    #[test]
    fn heuristic_evidence_has_no_resolves_field() {
        let e = Evidence::Heuristic {
            score: 0.42,
            threshold: 0.15,
            matched_rules: vec!["rule-a".to_string()],
        };
        let json = serde_json::to_string(&e).expect("serialize");
        assert!(json.contains("\"kind\":\"heuristic\""), "got {json}");
        assert!(
            !json.contains("resolves"),
            "heuristic must not carry resolves: {json}"
        );
    }

    #[test]
    fn family_orders_by_triage_urgency() {
        assert!(Family::Broken > Family::Uncovered);
        assert!(Family::Uncovered > Family::Superseded);
        assert!(Family::Superseded > Family::Covered);
    }

    #[test]
    fn verdict_families_are_assigned() {
        assert_eq!(Verdict::Covered.family(), Family::Covered);
        assert_eq!(Verdict::LikelyCovered.family(), Family::Covered);
        assert_eq!(Verdict::Uncovered.family(), Family::Uncovered);
        assert_eq!(Verdict::Superseded.family(), Family::Superseded);
        assert_eq!(Verdict::Moved.family(), Family::Broken);
        assert_eq!(Verdict::Stale.family(), Family::Broken);
    }

    #[test]
    fn category_is_orthogonal_to_verdict() {
        // A memory entry can be Actionable AND covered — the two axes are
        // independent. This is the spec correction in this plan's header.
        let item = DriftItem {
            subject: "always log via tracing".to_string(),
            verdict: Verdict::Covered,
            category: Some(Category::Actionable),
            suggestion: None,
            evidence: Evidence::Heuristic {
                score: 0.8,
                threshold: 0.15,
                matched_rules: vec!["rule-a".to_string()],
            },
        };
        assert_eq!(item.verdict, Verdict::Covered);
        assert_eq!(item.category, Some(Category::Actionable));
    }

    #[test]
    fn a_personal_entry_is_never_counted_as_drift() {
        // Personal guidance belongs in MEMORY.md. It is legitimately
        // Uncovered — no rule covers it — but it is not work to do.
        let personal = DriftItem {
            subject: "prefers terse replies".to_string(),
            verdict: Verdict::Uncovered,
            category: Some(Category::Personal),
            suggestion: None,
            evidence: Evidence::Heuristic {
                score: 0.0,
                threshold: 0.15,
                matched_rules: vec![],
            },
        };
        let actionable = DriftItem {
            category: Some(Category::Actionable),
            ..personal.clone()
        };
        assert_eq!(uncovered_count(std::slice::from_ref(&personal)), 0);
        assert_eq!(uncovered_count(std::slice::from_ref(&actionable)), 1);
    }

    #[test]
    fn a_superseded_decision_is_never_counted_as_drift() {
        let item = DriftItem {
            subject: "ADR-003".to_string(),
            verdict: Verdict::Superseded,
            category: None,
            suggestion: None,
            evidence: Evidence::Declared {
                rules: vec![],
                superseded_by: Some("ADR-007".to_string()),
            },
        };
        assert_eq!(uncovered_count(&[item]), 0);
    }

    #[test]
    fn source_all_lists_every_variant() {
        assert_eq!(Source::ALL.len(), 4);
        assert!(Source::ALL.contains(&Source::ClaudeMd));
        assert!(Source::ALL.contains(&Source::Code));
    }
}
