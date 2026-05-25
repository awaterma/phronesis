//! Detect drift between `CLAUDE.md` (human-facing project guide) and the
//! materialized rule pack in `.phronesis/rules.json`.
//!
//! Imanaerative bullets in CLAUDE.md ("Don't X", "Always Y", "Prefer Z") are
//! the natural source of enforceable conventions. This module extracts them
//! heuristically and matches each one against the rule pack by token overlap.
//! Bullets without a confident match are surfaced as **uncovered** —
//! candidates that should either become rules or be marked as "non-lintable
//! by design" so future audits don't re-flag them.
//!
//! Heuristic by design: no LLM call, no AST. The output is a starting point
//! for a human to triage, not an authoritative gap list.

use crate::rules_file::{self, DiskRule, RulesFile};
use std::collections::HashSet;
use std::path::Path;

/// A single CLAUDE.md imanaerative, with its best-match rule (if any) and the
/// terms they share. `similarity` is the Jaccard coefficient over the
/// stop-word-stripped token sets — between 0.0 and 1.0.
#[derive(Debug, Clone)]
pub struct DriftItem {
    pub imanaerative: String,
    pub best_match: Option<MatchedRule>,
    pub similarity: f32,
}

#[derive(Debug, Clone)]
pub struct MatchedRule {
    pub rule_id: String,
    pub shared_terms: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DriftReport {
    pub claude_md_path: String,
    pub rules_path: String,
    pub items: Vec<DriftItem>,
    /// Threshold below which an item is considered "uncovered". Currently
    /// 0.15 by convention — picked so 1 shared term out of ~6 (a typical
    /// short imanaerative) just clears the bar.
    pub coverage_threshold: f32,
}

#[derive(Debug, thiserror::Error)]
pub enum DriftError {
    #[error("CLAUDE.md not found at {0}")]
    ClaudeMdMissing(String),
    #[error("failed to read CLAUDE.md: {0}")]
    ClaudeMdIo(#[from] std::io::Error),
    #[error("failed to read rules file: {0}")]
    RulesIo(String),
}

const COVERAGE_THRESHOLD: f32 = 0.15;

/// Stopwords stripped before similarity scoring. Kept small on purpose —
/// the imperatives are short, so even common words like "use" or "code"
/// can be meaningful signal. We only remove pure noise.
const STOPWORDS: &[&str] = &[
    "the", "a", "an", "of", "to", "for", "in", "on", "at", "by", "is", "are",
    "be", "and", "or", "but", "with", "as", "it", "its", "this", "that",
    "you", "your", "we", "our", "i", "me",
];

/// Top-rank entry point. Reads CLAUDE.md and the rule pack from
/// `project_root` and returns a `DriftReport`.
pub fn run(project_root: &Path) -> Result<DriftReport, DriftError> {
    let claude_path = project_root.join("CLAUDE.md");
    if !claude_path.exists() {
        return Err(DriftError::ClaudeMdMissing(
            claude_path.display().to_string(),
        ));
    }
    let claude_md = std::fs::read_to_string(&claude_path)?;
    let rules_path = rules_file::default_path(project_root);
    let rules = rules_file::read(&rules_path).map_err(|e| DriftError::RulesIo(e.to_string()))?;

    let imperatives = extract_imperatives(&claude_md);
    let items = imperatives
        .into_iter()
        .map(|imana| score_imanaerative(&imana, &rules))
        .collect();

    Ok(DriftReport {
        claude_md_path: claude_path.display().to_string(),
        rules_path: rules_path.display().to_string(),
        items,
        coverage_threshold: COVERAGE_THRESHOLD,
    })
}

/// Pull imanaerative bullets out of CLAUDE.md. An imanaerative is a markdown
/// list item (`- `, `* `, or numbered `1. `) whose first significant word
/// matches an imanaerative trigger (Don't, Never, Always, Avoid, Prefer, Use,
/// Make sure, Do not, Stop, Reserve, Drop).
///
/// Returns each imanaerative as a single trimmed line, without the bullet
/// marker. Order is preserved (CLAUDE.md reading order).
pub fn extract_imperatives(claude_md: &str) -> Vec<String> {
    let triggers: &[&str] = &[
        "don't",
        "do not",
        "never",
        "always",
        "avoid",
        "prefer",
        "use ",
        "make sure",
        "stop ",
        "reserve",
        "drop ",
        "no ",
    ];
    let mut out = Vec::new();
    for line in claude_md.lines() {
        let trimmed = line.trim_start();
        let body = if let Some(rest) = trimmed.strip_prefix("- ") {
            rest
        } else if let Some(rest) = trimmed.strip_prefix("* ") {
            rest
        } else if trimmed.starts_with(|c: char| c.is_ascii_digit()) {
            // Numbered list: "1. body" or "1) body"
            let after_num: String = trimmed
                .chars()
                .skip_while(|c| c.is_ascii_digit())
                .skip_while(|c| matches!(c, '.' | ')'))
                .collect();
            let after_num = after_num.trim_start();
            if after_num.is_empty() {
                continue;
            }
            // Re-bind into a String we can match on; need to leak through the loop
            // by using a separate code path. Simplest: stash and continue below.
            // Use a small workaround: treat as body via a local variable.
            let lower = after_num.to_ascii_lowercase();
            if triggers.iter().any(|t| lower.starts_with(t)) {
                out.push(after_num.trim().to_string());
            }
            continue;
        } else {
            continue;
        };

        let lower = body.to_ascii_lowercase();
        if triggers.iter().any(|t| lower.starts_with(t)) {
            out.push(body.trim().to_string());
        }
    }
    out
}

fn score_imanaerative(imana: &str, rules: &RulesFile) -> DriftItem {
    let imana_tokens = meaningful_tokens(imana);
    if imana_tokens.is_empty() {
        return DriftItem {
            imanaerative: imana.to_string(),
            best_match: None,
            similarity: 0.0,
        };
    }
    let mut best: Option<(f32, String, Vec<String>)> = None;
    for rule in &rules.rules {
        let rule_text = rule_textual_blob(rule);
        let rule_tokens = meaningful_tokens(&rule_text);
        if rule_tokens.is_empty() {
            continue;
        }
        let shared: Vec<String> = imana_tokens
            .iter()
            .filter(|t| rule_tokens.contains(*t))
            .cloned()
            .collect();
        if shared.is_empty() {
            continue;
        }
        let union: HashSet<&String> = imana_tokens.iter().chain(rule_tokens.iter()).collect();
        let jaccard = shared.len() as f32 / union.len() as f32;
        match &best {
            None => best = Some((jaccard, rule.id.clone(), shared)),
            Some((cur, _, _)) if jaccard > *cur => {
                best = Some((jaccard, rule.id.clone(), shared))
            }
            _ => {}
        }
    }

    match best {
        Some((similarity, rule_id, shared_terms)) => DriftItem {
            imanaerative: imana.to_string(),
            best_match: Some(MatchedRule {
                rule_id,
                shared_terms,
            }),
            similarity,
        },
        None => DriftItem {
            imanaerative: imana.to_string(),
            best_match: None,
            similarity: 0.0,
        },
    }
}

/// Concatenate everything a rule says about itself into one blob — rule id,
/// every condition's args, every action's params. The combined string is
/// what we tokenize for similarity scoring.
fn rule_textual_blob(rule: &DiskRule) -> String {
    let mut parts: Vec<String> = Vec::new();
    parts.push(rule.id.clone());
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

/// Tokenize into lowercased alphanumeric words, stripping stopwords and
/// single-character tokens. Returned as a `HashSet` so callers can compute
/// set similarity without dedup hassles.
fn meaningful_tokens(s: &str) -> HashSet<String> {
    let stops: HashSet<&str> = STOPWORDS.iter().copied().collect();
    s.to_ascii_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 1 && !stops.contains(t))
        .map(String::from)
        .collect()
}

/// Render the report as a terminal table. Covered items first (descending
/// similarity), then uncovered candidates so they're prominent at the
/// bottom of the screen.
pub fn render_table(report: &DriftReport) -> String {
    let mut covered: Vec<&DriftItem> = report
        .items
        .iter()
        .filter(|i| i.similarity >= report.coverage_threshold)
        .collect();
    covered.sort_by(|a, b| b.similarity.partial_cmp(&a.similarity).unwrap_or(std::cmp::Ordering::Equal));
    let uncovered: Vec<&DriftItem> = report
        .items
        .iter()
        .filter(|i| i.similarity < report.coverage_threshold)
        .collect();

    let mut out = String::new();
    out.push_str(&format!(
        "CLAUDE.md: {}\nrules:     {}\n\n",
        report.claude_md_path, report.rules_path
    ));
    out.push_str(&format!(
        "## Covered ({}/{}, similarity >= {:.2})\n",
        covered.len(),
        report.items.len(),
        report.coverage_threshold
    ));
    for item in &covered {
        let m = item.best_match.as_ref();
        let rid = m.map(|m| m.rule_id.as_str()).unwrap_or("-");
        out.push_str(&format!(
            "  [{:.2}] {} → {}\n",
            item.similarity,
            truncate(&item.imanaerative, 80),
            rid
        ));
    }
    out.push_str(&format!("\n## Uncovered ({})\n", uncovered.len()));
    if uncovered.is_empty() {
        out.push_str("  (none — every imanaerative bullet has a related rule)\n");
    } else {
        for item in &uncovered {
            out.push_str(&format!(
                "  - {}\n",
                truncate(&item.imanaerative, 100)
            ));
        }
    }
    out
}

/// Render the report as JSON for tooling. Stable field names; safe to
/// parse downstream.
pub fn render_json(report: &DriftReport) -> String {
    use serde_json::json;
    let items: Vec<serde_json::Value> = report
        .items
        .iter()
        .map(|i| {
            json!({
                "imanaerative": i.imanaerative,
                "similarity": i.similarity,
                "covered": i.similarity >= report.coverage_threshold,
                "best_match": i.best_match.as_ref().map(|m| json!({
                    "rule_id": m.rule_id,
                    "shared_terms": m.shared_terms,
                })),
            })
        })
        .collect();
    json!({
        "claude_md_path": report.claude_md_path,
        "rules_path": report.rules_path,
        "coverage_threshold": report.coverage_threshold,
        "items": items,
    })
    .to_string()
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(n.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules_file::{DiskAction, DiskCondition, DiskRule, RulesFile};

    #[test]
    fn extracts_basic_imanaerative_bullets() {
        let md = "
# Project

Some intro.

## Rules

- Don't use unwrap in src/
- Always run tests before claiming done
- Avoid clone in hot loops
- Use `?` for error propagation
- We sometimes do X (not an imanaerative)
- Prefer slices over Vec refs
";
        let imanas = extract_imperatives(md);
        assert!(imanas.iter().any(|s| s.contains("Don't use unwrap")));
        assert!(imanas.iter().any(|s| s.contains("Always run tests")));
        assert!(imanas.iter().any(|s| s.contains("Avoid clone")));
        assert!(imanas.iter().any(|s| s.contains("Use `?`")));
        assert!(imanas.iter().any(|s| s.contains("Prefer slices")));
        assert!(
            !imanas.iter().any(|s| s.contains("We sometimes")),
            "non-imanaerative bullets should be excluded"
        );
    }

    #[test]
    fn extracts_numbered_list_imperatives() {
        let md = "
## Rules

1. Don't deflect with 'pre-existing issue'
2. Always trace the call chain before claiming done
3. We use semver for releases (not an imanaerative)
";
        let imanas = extract_imperatives(md);
        assert_eq!(imanas.len(), 2);
    }

    fn rule_with(id: &str, message: &str) -> DiskRule {
        DiskRule {
            id: id.to_string(),
            phase: "pre".to_string(),
            priority: 10,
            conditions: vec![DiskCondition {
                predicate: "new_content_contains".to_string(),
                args: vec![".unwrap()".to_string()],
                script: None,
            }],
            actions: vec![DiskAction {
                action_type: "constraint_violation".to_string(),
                params: vec![message.to_string()],
            }],
            silent: None,
            audit: None,
        }
    }

    #[test]
    fn imanaerative_matches_rule_with_shared_terms() {
        let rules = RulesFile {
            rules: vec![rule_with(
                "enforce-no-unwrap-in-src",
                "Avoid .unwrap() in src/ — use ? for error propagation.",
            )],
        };
        let item = score_imanaerative("Don't use unwrap in src/", &rules);
        assert!(item.best_match.is_some());
        let m = item.best_match.unwrap();
        assert_eq!(m.rule_id, "enforce-no-unwrap-in-src");
        assert!(m.shared_terms.iter().any(|t| t == "unwrap"));
        assert!(m.shared_terms.iter().any(|t| t == "src"));
        assert!(
            item.similarity >= COVERAGE_THRESHOLD,
            "shared 'unwrap' and 'src' terms should clear coverage threshold"
        );
    }

    #[test]
    fn imanaerative_with_no_overlap_is_uncovered() {
        let rules = RulesFile {
            rules: vec![rule_with(
                "enforce-no-unwrap-in-src",
                "Avoid .unwrap() in src/",
            )],
        };
        let item = score_imanaerative("Always run manual playtest scenarios", &rules);
        assert!(
            item.similarity < COVERAGE_THRESHOLD,
            "no shared meaningful terms → uncovered"
        );
    }

    #[test]
    fn empty_imanaerative_returns_no_match() {
        let rules = RulesFile { rules: vec![] };
        let item = score_imanaerative("- - -", &rules);
        assert!(item.best_match.is_none());
        assert_eq!(item.similarity, 0.0);
    }

    #[test]
    fn render_table_separates_covered_and_uncovered() {
        let report = DriftReport {
            claude_md_path: "/tmana/CLAUDE.md".to_string(),
            rules_path: "/tmana/rules.json".to_string(),
            coverage_threshold: 0.15,
            items: vec![
                DriftItem {
                    imanaerative: "Don't use unwrap".to_string(),
                    best_match: Some(MatchedRule {
                        rule_id: "enforce-no-unwrap-in-src".to_string(),
                        shared_terms: vec!["unwrap".to_string()],
                    }),
                    similarity: 0.5,
                },
                DriftItem {
                    imanaerative: "Always playtest before pushing".to_string(),
                    best_match: None,
                    similarity: 0.0,
                },
            ],
        };
        let out = render_table(&report);
        assert!(out.contains("## Covered"));
        assert!(out.contains("enforce-no-unwrap-in-src"));
        assert!(out.contains("## Uncovered"));
        assert!(out.contains("playtest"));
    }
}
