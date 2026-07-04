//! Detect drift between `CLAUDE.md` (human-facing project guide) and the
//! materialized rule pack in `.phronesis/rules.json`.
//!
//! Imperative bullets in CLAUDE.md ("Don't X", "Always Y", "Prefer Z") are
//! the natural source of enforceable conventions. This module extracts them
//! heuristically and matches each one against the rule pack by token overlap.
//! Bullets without a confident match are surfaced as **uncovered** —
//! candidates that should either become rules or be marked as "non-lintable
//! by design" so future audits don't re-flag them.
//!
//! Heuristic by design: no LLM call, no AST. The output is a starting point
//! for a human to triage, not an authoritative gap list.

use crate::rules_file::{self, DiskRule, RulesFile};
use phr::RuleId;
use std::collections::HashSet;
use std::path::Path;

/// A single CLAUDE.md imperative, with its best-match rule (if any) and the
/// terms they share. `similarity` is the Jaccard coefficient over the
/// stop-word-stripped token sets — between 0.0 and 1.0.
#[derive(Debug, Clone)]
pub struct DriftItem {
    pub imperative: String,
    pub best_match: Option<MatchedRule>,
    pub similarity: f32,
}

#[derive(Debug, Clone)]
pub struct MatchedRule {
    pub rule_id: RuleId,
    pub shared_terms: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DriftReport {
    pub claude_md_path: String,
    pub rules_path: String,
    pub items: Vec<DriftItem>,
    /// Threshold below which an item is considered "uncovered". Currently
    /// 0.15 by convention — picked so 1 shared term out of ~6 (a typical
    /// short imperative) just clears the bar.
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
    "the", "a", "an", "of", "to", "for", "in", "on", "at", "by", "is", "are", "be", "and", "or",
    "but", "with", "as", "it", "its", "this", "that", "you", "your", "we", "our", "i", "me",
];

/// Imperative trigger words. A bullet whose first significant word matches
/// one of these is extracted as an enforced directive.
const TRIGGERS: &[&str] = &[
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

/// Top-level entry point. Reads CLAUDE.md and the rule pack from
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
        .map(|imp| score_imperative(&imp, &rules))
        .collect();

    Ok(DriftReport {
        claude_md_path: claude_path.display().to_string(),
        rules_path: rules_path.display().to_string(),
        items,
        coverage_threshold: COVERAGE_THRESHOLD,
    })
}

/// Extract the body text from a markdown list item, stripping the bullet
/// marker. Handles `- `, `* `, and numbered (`1. ` / `1) `) forms. Returns
/// `None` for non-list lines or numbered items whose body is empty after
/// stripping the digit and delimiter.
///
/// For `- ` / `* ` bullets the returned string is the raw remainder after the
/// two-character marker — NOT trimmed. This preserves leading whitespace so
/// that `is_imperative` checks run against the literal rest: a bullet like
/// `-   never push` (extra spaces) will NOT match the `"never"` trigger
/// because the lower-cased body starts with spaces. Callers that want a
/// clean display value should call `.trim()` on the returned string.
///
/// For numbered items the body is `trim_start()`-ed (standard: digits and
/// delimiter consume the indent, any remaining whitespace is noise).
fn bullet_body(trimmed: &str) -> Option<String> {
    if let Some(rest) = trimmed.strip_prefix("- ") {
        return Some(rest.to_string());
    }
    if let Some(rest) = trimmed.strip_prefix("* ") {
        return Some(rest.to_string());
    }
    if trimmed.starts_with(|c: char| c.is_ascii_digit()) {
        let after_num: String = trimmed
            .chars()
            .skip_while(|c| c.is_ascii_digit())
            .skip_while(|c| matches!(c, '.' | ')'))
            .collect();
        let s = after_num.trim();
        if s.is_empty() {
            return None;
        }
        return Some(s.to_string());
    }
    None
}

/// Return `true` when `body` (a bullet's text content) starts with an
/// imperative trigger word. Case-insensitive.
fn is_imperative(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    TRIGGERS.iter().any(|t| lower.starts_with(t))
}

/// Pull imperative bullets out of CLAUDE.md. An imperative is a markdown
/// list item (`- `, `* `, or numbered `1. `) whose first significant word
/// matches an imperative trigger (Don't, Never, Always, Avoid, Prefer, Use,
/// Make sure, Do not, Stop, Reserve, Drop).
///
/// Returns each imperative as a single trimmed line, without the bullet
/// marker. Order is preserved (CLAUDE.md reading order).
pub fn extract_imperatives(claude_md: &str) -> Vec<String> {
    claude_md
        .lines()
        .filter_map(|line| {
            let body = bullet_body(line.trim_start())?;
            if is_imperative(&body) {
                // Push the trimmed display value (matches v0.16.2: the old loop
                // did `out.push(body.trim().to_string())` for all bullet kinds).
                Some(body.trim().to_string())
            } else {
                None
            }
        })
        .collect()
}

fn score_imperative(imp: &str, rules: &RulesFile) -> DriftItem {
    let imp_tokens = meaningful_tokens(imp);
    if imp_tokens.is_empty() {
        return DriftItem {
            imperative: imp.to_string(),
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
        let shared: Vec<String> = imp_tokens
            .iter()
            .filter(|t| rule_tokens.contains(*t))
            .cloned()
            .collect();
        if shared.is_empty() {
            continue;
        }
        let union: HashSet<&String> = imp_tokens.iter().chain(rule_tokens.iter()).collect();
        let jaccard = shared.len() as f32 / union.len() as f32;
        match &best {
            None => best = Some((jaccard, rule.id.clone(), shared)),
            Some((cur, _, _)) if jaccard > *cur => best = Some((jaccard, rule.id.clone(), shared)),
            _ => {}
        }
    }

    match best {
        Some((similarity, rule_id, shared_terms)) => DriftItem {
            imperative: imp.to_string(),
            best_match: Some(MatchedRule {
                rule_id: rule_id.into(),
                shared_terms,
            }),
            similarity,
        },
        None => DriftItem {
            imperative: imp.to_string(),
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
    covered.sort_by(|a, b| {
        b.similarity
            .partial_cmp(&a.similarity)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
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
            truncate(&item.imperative, 80),
            rid
        ));
    }
    out.push_str(&format!("\n## Uncovered ({})\n", uncovered.len()));
    if uncovered.is_empty() {
        out.push_str("  (none — every imperative bullet has a related rule)\n");
    } else {
        for item in &uncovered {
            out.push_str(&format!("  - {}\n", truncate(&item.imperative, 100)));
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
                "imperative": i.imperative,
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

    /// A `- ` bullet with extra spaces before the trigger word must NOT be
    /// extracted. The old (v0.16.2) loop ran the trigger `starts_with` check on
    /// the raw remainder after stripping `"- "`, so leading spaces in the
    /// remainder prevented a match. A normal `- Never …` (single space, no
    /// extra indent) IS extracted.
    #[test]
    fn bullet_with_extra_indent_after_dash_is_not_extracted() {
        let md = "\
-   never push without review
- Never commit untested code
";
        let imps = extract_imperatives(md);
        assert!(
            !imps.iter().any(|s| s.contains("never push")),
            "bullet with extra spaces after dash must NOT be extracted"
        );
        assert!(
            imps.iter().any(|s| s.contains("Never commit")),
            "normal bullet (single space) must be extracted"
        );
    }

    #[test]
    fn extracts_basic_imperative_bullets() {
        let md = "
# Project

Some intro.

## Rules

- Don't use unwrap in src/
- Always run tests before claiming done
- Avoid clone in hot loops
- Use `?` for error propagation
- We sometimes do X (not an imperative)
- Prefer slices over Vec refs
";
        let imps = extract_imperatives(md);
        assert!(imps.iter().any(|s| s.contains("Don't use unwrap")));
        assert!(imps.iter().any(|s| s.contains("Always run tests")));
        assert!(imps.iter().any(|s| s.contains("Avoid clone")));
        assert!(imps.iter().any(|s| s.contains("Use `?`")));
        assert!(imps.iter().any(|s| s.contains("Prefer slices")));
        assert!(
            !imps.iter().any(|s| s.contains("We sometimes")),
            "non-imperative bullets should be excluded"
        );
    }

    #[test]
    fn extracts_numbered_list_imperatives() {
        let md = "
## Rules

1. Don't deflect with 'pre-existing issue'
2. Always trace the call chain before claiming done
3. We use semver for releases (not an imperative)
";
        let imps = extract_imperatives(md);
        assert_eq!(imps.len(), 2);
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
            doc_excepted: None,
        }
    }

    #[test]
    fn imperative_matches_rule_with_shared_terms() {
        let rules = RulesFile {
            rules: vec![rule_with(
                "enforce-no-unwrap-in-src",
                "Avoid .unwrap() in src/ — use ? for error propagation.",
            )],
        };
        let item = score_imperative("Don't use unwrap in src/", &rules);
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
    fn imperative_with_no_overlap_is_uncovered() {
        let rules = RulesFile {
            rules: vec![rule_with(
                "enforce-no-unwrap-in-src",
                "Avoid .unwrap() in src/",
            )],
        };
        let item = score_imperative("Always run manual playtest scenarios", &rules);
        assert!(
            item.similarity < COVERAGE_THRESHOLD,
            "no shared meaningful terms → uncovered"
        );
    }

    #[test]
    fn empty_imperative_returns_no_match() {
        let rules = RulesFile { rules: vec![] };
        let item = score_imperative("- - -", &rules);
        assert!(item.best_match.is_none());
        assert_eq!(item.similarity, 0.0);
    }

    #[test]
    fn render_table_separates_covered_and_uncovered() {
        let report = DriftReport {
            claude_md_path: "/tmp/CLAUDE.md".to_string(),
            rules_path: "/tmp/rules.json".to_string(),
            coverage_threshold: 0.15,
            items: vec![
                DriftItem {
                    imperative: "Don't use unwrap".to_string(),
                    best_match: Some(MatchedRule {
                        rule_id: "enforce-no-unwrap-in-src".into(),
                        shared_terms: vec!["unwrap".to_string()],
                    }),
                    similarity: 0.5,
                },
                DriftItem {
                    imperative: "Always playtest before pushing".to_string(),
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
