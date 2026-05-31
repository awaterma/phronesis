//! Detect drift between ADR-style decision documents in
//! `.phronesis/wiki/decisions/` and the current rule pack. Heuristic
//! by design (no LLM call); output is a triage list.

use std::path::{Path, PathBuf};

use phr::RuleId;
use thiserror::Error;

use crate::rules_file::{self, DiskRule};
use crate::wiki::{self, Decision};

const COVERAGE_THRESHOLD: f32 = 0.15;

const STOPWORDS: &[&str] = &[
    "the", "a", "an", "of", "to", "for", "in", "on", "at", "by", "is", "are", "be", "and", "or",
    "but", "with", "as", "it", "its", "this", "that", "you", "your", "we", "our", "i", "me",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bucket {
    Covered,
    LikelyCovered,
    Uncovered,
    Superseded,
}

#[derive(Debug, Clone)]
pub struct MatchedRule {
    pub rule_id: RuleId,
    pub shared_terms: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DriftItem {
    pub decision: Decision,
    pub bucket: Bucket,
    pub best_match: Option<MatchedRule>,
    pub similarity: f32,
}

#[derive(Debug, Clone)]
pub struct DriftReport {
    pub wiki_dir: String,
    pub rules_path: String,
    pub items: Vec<DriftItem>,
    pub coverage_threshold: f32,
}

#[derive(Debug, Error)]
pub enum DriftError {
    #[error(transparent)]
    Wiki(#[from] wiki::WikiError),
    #[error("failed to read rules file: {0}")]
    RulesIo(String),
}

/// Default entry: walks `<project_root>/.phronesis/wiki/decisions/`.
pub fn run(project_root: &Path) -> Result<DriftReport, DriftError> {
    let dir = wiki::default_wiki_dir(project_root).join("decisions");
    run_with_dir(project_root, &dir)
}

/// Override entry: scan an arbitrary decisions directory. Useful for tests
/// and for callers who put the wiki elsewhere.
pub fn run_with_dir(project_root: &Path, decisions_dir: &Path) -> Result<DriftReport, DriftError> {
    let decisions = wiki::walk_decisions(decisions_dir)?;
    let rules_path = rules_file::default_path(project_root);
    let rules = rules_file::read(&rules_path).map_err(|e| DriftError::RulesIo(e.to_string()))?;
    let rule_id_set: std::collections::HashSet<&str> =
        rules.rules.iter().map(|r| r.id.as_str()).collect();

    let items = decisions
        .into_iter()
        .map(|d| score_decision(d, &rules.rules, &rule_id_set))
        .collect();

    Ok(DriftReport {
        wiki_dir: decisions_dir.display().to_string(),
        rules_path: rules_path.display().to_string(),
        items,
        coverage_threshold: COVERAGE_THRESHOLD,
    })
}

fn score_decision(
    decision: Decision,
    rules: &[DiskRule],
    rule_id_set: &std::collections::HashSet<&str>,
) -> DriftItem {
    // Superseded decisions are history; exclude from active drift scoring.
    if decision.frontmatter.status == "superseded" {
        return DriftItem {
            decision,
            bucket: Bucket::Superseded,
            best_match: None,
            similarity: 0.0,
        };
    }

    // 1. Explicit `enforces:` shortcut. If any listed rule id exists in the
    //    pack, the decision is deterministically Covered.
    let enforces = decision.frontmatter.enforces.clone();
    for rid in &enforces {
        if rule_id_set.contains(rid.as_str()) {
            return DriftItem {
                decision,
                bucket: Bucket::Covered,
                best_match: Some(MatchedRule {
                    rule_id: rid.clone().into(),
                    shared_terms: Vec::new(),
                }),
                similarity: 1.0,
            };
        }
    }

    // 2. Fuzzy fallback. Jaccard token overlap between the decision body
    //    and each rule's textual blob (id + condition args + action params).
    let decision_tokens = meaningful_tokens(&decision.body);
    if decision_tokens.is_empty() {
        return DriftItem {
            decision,
            bucket: Bucket::Uncovered,
            best_match: None,
            similarity: 0.0,
        };
    }

    let mut best: Option<(f32, RuleId, Vec<String>)> = None;
    for rule in rules {
        let rule_tokens = meaningful_tokens(&rule_blob(rule));
        if rule_tokens.is_empty() {
            continue;
        }
        let shared: Vec<String> = decision_tokens
            .iter()
            .filter(|t| rule_tokens.contains(*t))
            .cloned()
            .collect();
        if shared.is_empty() {
            continue;
        }
        let union: std::collections::HashSet<&String> =
            decision_tokens.iter().chain(rule_tokens.iter()).collect();
        let jaccard = shared.len() as f32 / union.len() as f32;
        match &best {
            None => best = Some((jaccard, rule.id.clone().into(), shared)),
            Some((cur, _, _)) if jaccard > *cur => {
                best = Some((jaccard, rule.id.clone().into(), shared))
            }
            _ => {}
        }
    }

    match best {
        Some((similarity, rule_id, shared_terms)) if similarity >= COVERAGE_THRESHOLD => {
            DriftItem {
                decision,
                bucket: Bucket::LikelyCovered,
                best_match: Some(MatchedRule {
                    rule_id,
                    shared_terms,
                }),
                similarity,
            }
        }
        _ => DriftItem {
            decision,
            bucket: Bucket::Uncovered,
            best_match: None,
            similarity: 0.0,
        },
    }
}

fn rule_blob(rule: &DiskRule) -> String {
    let mut parts: Vec<String> = vec![rule.id.clone()];
    for c in &rule.conditions {
        for a in &c.args {
            parts.push(a.clone());
        }
    }
    for a in &rule.actions {
        for p in &a.params {
            parts.push(p.clone());
        }
    }
    parts.join(" ")
}

fn meaningful_tokens(s: &str) -> std::collections::HashSet<String> {
    let stops: std::collections::HashSet<&str> = STOPWORDS.iter().copied().collect();
    s.to_ascii_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 1 && !stops.contains(t))
        .map(String::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Build a project root with: a .phronesis/ dir containing a v2-shaped
    /// rules.json with the given rule ids, and a decisions directory
    /// (caller writes decision files into it).
    fn fixture_project(rule_ids: &[&str]) -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let phr = dir.path().join(".phronesis");
        fs::create_dir_all(&phr).unwrap();
        let rules_json = if rule_ids.is_empty() {
            r#"{"rules":[]}"#.to_string()
        } else {
            let rules: Vec<String> = rule_ids
                .iter()
                .map(|id| {
                    format!(
                        r#"{{"id":"{}","phase":"pre","priority":1,"when":[{{"new_content_contains":"x"}}],"then":{{"warn":"m"}}}}"#,
                        id
                    )
                })
                .collect();
            format!(r#"{{"rules":[{}]}}"#, rules.join(","))
        };
        fs::write(phr.join("rules.json"), rules_json).unwrap();
        let dec_dir = phr.join("wiki").join("decisions");
        fs::create_dir_all(&dec_dir).unwrap();
        let project_root = dir.path().to_path_buf();
        (dir, dec_dir)
    }

    fn write_decision(dir: &Path, name: &str, content: &str) {
        fs::write(dir.join(name), content).unwrap();
    }

    #[test]
    fn enforces_listing_an_existing_rule_buckets_as_covered() {
        let (tmp, dec) = fixture_project(&["warn-cargo-no-workspace"]);
        write_decision(
            &dec,
            "2026-05-29-x.md",
            "---\n\
             id: x\n\
             date: 2026-05-29\n\
             status: accepted\n\
             enforces:\n  - warn-cargo-no-workspace\n\
             ---\n\
             body about something completely unrelated to cargo\n",
        );
        let report = run_with_dir(tmp.path(), &dec).unwrap();
        assert_eq!(report.items.len(), 1);
        assert_eq!(report.items[0].bucket, Bucket::Covered);
        let m = report.items[0].best_match.as_ref().unwrap();
        assert_eq!(m.rule_id.as_str(), "warn-cargo-no-workspace");
        // Covered is deterministic — shared_terms is empty.
        assert!(m.shared_terms.is_empty());
        // Similarity reported as 1.0 to convey deterministic match.
        assert!((report.items[0].similarity - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn enforces_listing_nonexistent_rule_falls_through_to_fuzzy() {
        let (tmp, dec) = fixture_project(&["other-rule"]);
        write_decision(
            &dec,
            "x.md",
            "---\n\
             id: x\n\
             date: 2026-05-29\n\
             status: accepted\n\
             enforces:\n  - not-in-the-pack\n\
             ---\n\
             completely unrelated content\n",
        );
        let report = run_with_dir(tmp.path(), &dec).unwrap();
        // Bucket falls back to Uncovered since fuzzy doesn't match either.
        assert_eq!(report.items[0].bucket, Bucket::Uncovered);
    }

    #[test]
    fn no_enforces_with_strong_fuzzy_match_is_likely_covered() {
        let (tmp, dec) = fixture_project(&["enforce-no-unwrap-in-src"]);
        write_decision(
            &dec,
            "x.md",
            "---\n\
             id: x\n\
             date: 2026-05-29\n\
             status: accepted\n\
             ---\n\
             ## Decision\n\
             We enforce no unwrap in src code paths.\n",
        );
        let report = run_with_dir(tmp.path(), &dec).unwrap();
        assert_eq!(report.items[0].bucket, Bucket::LikelyCovered);
        assert!(report.items[0].best_match.is_some());
        assert!(report.items[0].similarity >= COVERAGE_THRESHOLD);
    }

    #[test]
    fn no_match_anywhere_is_uncovered() {
        let (tmp, dec) = fixture_project(&["unrelated-rule"]);
        write_decision(
            &dec,
            "x.md",
            "---\n\
             id: x\n\
             date: 2026-05-29\n\
             status: accepted\n\
             ---\n\
             completely orthogonal subject matter zzz qqq\n",
        );
        let report = run_with_dir(tmp.path(), &dec).unwrap();
        assert_eq!(report.items[0].bucket, Bucket::Uncovered);
        assert!(report.items[0].best_match.is_none());
    }

    #[test]
    fn superseded_decision_is_classified_as_superseded() {
        let (tmp, dec) = fixture_project(&[]);
        write_decision(
            &dec,
            "old.md",
            "---\n\
             id: old\n\
             date: 2025-01-01\n\
             status: superseded\n\
             superseded_by: new-thing\n\
             ---\n\
             ",
        );
        let report = run_with_dir(tmp.path(), &dec).unwrap();
        assert_eq!(report.items[0].bucket, Bucket::Superseded);
        assert!(report.items[0].best_match.is_none());
    }

    #[test]
    fn report_carries_threshold_and_paths() {
        let (tmp, dec) = fixture_project(&[]);
        let report = run_with_dir(tmp.path(), &dec).unwrap();
        assert!((report.coverage_threshold - COVERAGE_THRESHOLD).abs() < f32::EPSILON);
        assert!(report.wiki_dir.contains("decisions"));
        assert!(report.rules_path.contains("rules.json"));
    }
}
