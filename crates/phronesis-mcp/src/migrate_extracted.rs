//! Salvage migration for rules produced by `extract_rules` before 0.14.0
//! (block-action, bracket-prefixed messages). Implements the "Salvage
//! path" in docs/specs/SPEC-extract-rules-defaults.md.

use crate::rules_file::{SourceRule, WhenClause};

/// Message keywords covered by structural Rust-pack rules
/// (SPEC-extract-rules-defaults Problem 4a). Matched case-insensitively.
const STRUCTURAL_KEYWORDS: &[&str] = &["unwrap", "clone", "deref", "&string", "&vec", "thiserror"];

/// Extraction-time discriminators the old extractor prefixed onto messages.
const PREFIXES: &[&str] = &[
    "[pattern] ",
    "[anti_pattern] ",
    "[context] ",
    "[problem] ",
    "[directive] ",
];

#[derive(Debug, Default, PartialEq, serde::Serialize)]
pub struct MigrationSummary {
    pub examined: usize,
    pub changed: usize,
    pub prefixes_stripped: usize,
    pub demoted_to_warn: usize,
    pub demoted_to_log: usize,
}

/// A rule is "extracted" iff any `when` leaf uses the `markdown_rule`
/// predicate — the one condition shape unique to extractor output.
fn is_extracted(rule: &SourceRule) -> bool {
    rule.when.iter().any(clause_has_markdown_rule)
}

fn clause_has_markdown_rule(clause: &WhenClause) -> bool {
    match clause {
        WhenClause::Leaf(cond) => cond.predicate == "markdown_rule",
        WhenClause::Or(alts) => alts.iter().any(clause_has_markdown_rule),
    }
}

fn severity_rank(action_type: &str) -> u8 {
    match action_type {
        "constraint_violation" => 2,
        "constraint_warning" => 1,
        _ => 0,
    }
}

fn duplicates_structural_rule(message: &str) -> bool {
    let lower = message.to_lowercase();
    STRUCTURAL_KEYWORDS.iter().any(|k| lower.contains(k))
}

/// Rewrite extracted rules in place. Non-extracted rules are untouched.
pub fn migrate_extracted(sources: &mut [SourceRule]) -> MigrationSummary {
    let mut summary = MigrationSummary::default();
    for rule in sources.iter_mut().filter(|r| is_extracted(r)) {
        summary.examined += 1;
        let mut rule_changed = false;

        if let Some(message) = rule.then.params.first_mut()
            && let Some(stripped) = PREFIXES.iter().find_map(|p| message.strip_prefix(p))
        {
            *message = stripped.to_string();
            summary.prefixes_stripped += 1;
            rule_changed = true;
        }

        let message = rule.then.params.first().map(String::as_str).unwrap_or("");
        let (target, is_log) = if duplicates_structural_rule(message) {
            ("log", true)
        } else {
            ("constraint_warning", false)
        };
        if severity_rank(&rule.then.action_type) > severity_rank(target) {
            rule.then.action_type = target.to_string();
            if is_log {
                summary.demoted_to_log += 1;
            } else {
                summary.demoted_to_warn += 1;
            }
            rule_changed = true;
        }

        if rule_changed {
            summary.changed += 1;
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules_file::SourceRule;

    fn rule(verb: &str, msg: &str, extracted: bool) -> SourceRule {
        let when = if extracted {
            serde_json::json!([{ "markdown_rule": ["docs/RUST-PATTERNS-GUIDE.md", "Anti-Patterns"] }])
        } else {
            serde_json::json!([{ "new_content_contains": ".unwrap()" }])
        };
        // json! doesn't accept computed keys, so insert the verb after the fact
        let mut obj = serde_json::json!({
            "id": "rust-patterns-guide-anti-patterns-12",
            "phase": "pre",
            "priority": 5,
            "when": when,
            "then": {}
        });
        obj["then"][verb] = serde_json::Value::String(msg.to_string());
        serde_json::from_value(obj).unwrap()
    }

    fn msg(r: &SourceRule) -> &str {
        &r.then.params[0]
    }

    #[test]
    fn strips_all_five_prefixes() {
        for prefix in ["pattern", "anti_pattern", "context", "problem", "directive"] {
            let mut rules = vec![rule("warn", &format!("[{prefix}] Use the thing."), true)];
            let summary = migrate_extracted(&mut rules);
            assert_eq!(
                msg(&rules[0]),
                "Use the thing.",
                "prefix [{prefix}] not stripped"
            );
            assert_eq!(summary.prefixes_stripped, 1);
        }
    }

    #[test]
    fn demotes_block_to_warn() {
        let mut rules = vec![rule("block", "Prefer iterators over index loops.", true)];
        let summary = migrate_extracted(&mut rules);
        assert_eq!(rules[0].then.action_type, "constraint_warning");
        assert_eq!(summary.demoted_to_warn, 1);
        assert_eq!(summary.demoted_to_log, 0);
    }

    #[test]
    fn demotes_structural_duplicate_block_to_log() {
        let mut rules = vec![rule(
            "block",
            "[anti_pattern] Avoid: Clone to Satisfy Borrow Checker",
            true,
        )];
        let summary = migrate_extracted(&mut rules);
        assert_eq!(rules[0].then.action_type, "log");
        assert_eq!(msg(&rules[0]), "Avoid: Clone to Satisfy Borrow Checker");
        assert_eq!(summary.demoted_to_log, 1);
    }

    #[test]
    fn demotes_structural_duplicate_warn_to_log() {
        let mut rules = vec![rule("warn", "Never call .unwrap() in src.", true)];
        let summary = migrate_extracted(&mut rules);
        assert_eq!(rules[0].then.action_type, "log");
        assert_eq!(summary.demoted_to_log, 1);
    }

    #[test]
    fn plain_warn_stays_warn_and_log_stays_log() {
        let mut rules = vec![
            rule("warn", "Prefer builders for many-arg constructors.", true),
            rule("log", "Reference note.", true),
        ];
        let summary = migrate_extracted(&mut rules);
        assert_eq!(rules[0].then.action_type, "constraint_warning");
        assert_eq!(rules[1].then.action_type, "log");
        assert_eq!(summary.changed, 0);
    }

    #[test]
    fn non_extracted_rules_are_untouched() {
        let mut rules = vec![rule("block", "[pattern] Should not change.", false)];
        let summary = migrate_extracted(&mut rules);
        assert_eq!(rules[0].then.action_type, "constraint_violation");
        assert_eq!(msg(&rules[0]), "[pattern] Should not change.");
        assert_eq!(summary.examined, 0);
        assert_eq!(summary.changed, 0);
    }

    #[test]
    fn idempotent_second_run_changes_nothing() {
        let mut rules = vec![rule(
            "block",
            "[problem] Overuse of unwrap panics in prod.",
            true,
        )];
        let first = migrate_extracted(&mut rules);
        assert_eq!(first.changed, 1);
        let second = migrate_extracted(&mut rules);
        assert_eq!(second.changed, 0);
        assert_eq!(second.prefixes_stripped, 0);
    }
}
