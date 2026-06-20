//! `phr-mcp journey` CLI — render the `journey_*` facts a derivation pass
//! would assert against the current `.phronesis/journey/events.jsonl` and
//! `.phronesis/rules.json`. A "why did this fire" view, and (via
//! `--explain <rule-id>`) a per-rule introspection lens.
//!
//! Same render path serves the `get_journey` MCP tool — `compute` produces
//! the asserted facts + the per-fact "rules that reference it" attribution,
//! which both surfaces format.
//!
//! Failure mode: friendly stderr + non-zero exit. Loading is the only place
//! anything can fail (missing rules / journey config / journal). The
//! derivation pass itself is fail-open in the hook; in the CLI it's
//! fail-loud because the operator asked for it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use phr::{Fact, ReteNetwork, Rule};
use thiserror::Error;

use crate::journey::{self, derive, tagger::TaggerConfig};
use crate::rules_file;

/// Errors surfaced to the CLI / MCP handler. Loading errors keep the
/// "what file did this come from" context; the derive error wraps the
/// engine's own error.
#[derive(Debug, Error)]
pub enum JourneyCliError {
    #[error("rules: {0}")]
    Rules(#[from] rules_file::RulesFileError),
    #[error("journey config: {0}")]
    Config(journey::ConfigError),
    #[error("derive: {0}")]
    Derive(#[from] derive::DeriveError),
    #[error("facts_snapshot: {0}")]
    Facts(#[from] phr::ReteError),
    #[error("no rule with id '{0}'")]
    UnknownRule(String),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

/// A row in the rendered table / JSON: one asserted `journey_*` fact, plus
/// the rule ids whose `when` references the same (predicate, selector,
/// window) pair.
#[derive(Debug, Clone, serde::Serialize)]
pub struct JourneyRow {
    pub predicate: String,
    pub selector: String,
    pub window: String,
    /// Extra args beyond `[selector, window]` — count for `journey_count`,
    /// `k` for `journey_since_ge`, the count for `journey_distinct`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extra: Vec<String>,
    /// Rule ids whose `when` references this (predicate, selector) pair.
    pub rules: Vec<String>,
}

/// Compute the rows the CLI / MCP surface render. Async because the engine
/// asserts are async.
pub async fn compute(
    project_root: &Path,
    explain_rule: Option<&str>,
    now_ts: u64,
    current_sid: &str,
) -> Result<Vec<JourneyRow>, JourneyCliError> {
    let rules_path = rules_file::default_path(project_root);
    let disk_rules = rules_file::read(&rules_path)?;
    let rules: Vec<Rule> = disk_rules
        .rules
        .iter()
        .map(|d| rules_file::rule_from_disk(d).0)
        .collect();

    let cfg = match journey::load_config(project_root) {
        Ok(c) => c,
        Err(journey::ConfigError::NotFound(_)) => {
            // Operator (CLI or MCP) explicitly asked for journey output;
            // a missing config produces empty rows, which is opaque. Nudge
            // toward the scaffolder and continue with an empty config. The
            // hook stays silent — it loads journey advisorily, not on
            // demand. Malformed config still hard-errors via the
            // pass-through branch below.
            eprintln!(
                "phronesis: no .phronesis/journey.json found — run \
                 `phr-mcp init --packs journey` to scaffold one. \
                 Continuing with empty config."
            );
            TaggerConfig::default()
        }
        Err(e) => return Err(JourneyCliError::Config(e)),
    };

    // If an explain rule was requested, ensure it exists in the loaded rules.
    if let Some(rule_id) = explain_rule
        && !rules.iter().any(|r| r.id == rule_id)
    {
        return Err(JourneyCliError::UnknownRule(rule_id.to_string()));
    }

    let mut net = ReteNetwork::new();
    derive::assert_facts(&mut net, project_root, &rules, &cfg, current_sid, now_ts).await?;

    let facts: Vec<Fact> = net.facts_snapshot()?;

    // Build attribution: for each (predicate, selector) seen in any rule, the
    // rules that reference it. We index by (predicate, selector) — multiple
    // windows for the same selector still attribute to the same rule.
    let attribution = build_attribution(&rules);

    // Filter facts to journey_* and group them deterministically.
    let mut rows: Vec<JourneyRow> = facts
        .into_iter()
        .filter(|f| f.predicate.starts_with("journey_"))
        .map(|f| {
            let (selector, window, extra) = split_args(&f);
            let key = (f.predicate.clone(), selector.clone());
            let rules: Vec<String> = attribution.get(&key).cloned().unwrap_or_default();
            JourneyRow {
                predicate: f.predicate,
                selector,
                window,
                extra,
                rules,
            }
        })
        .collect();

    // Filter by explain rule if requested.
    if let Some(rule_id) = explain_rule {
        rows.retain(|r| r.rules.iter().any(|id| id == rule_id));
    }

    // Deterministic order: predicate → selector → window → extra.
    rows.sort_by(|a, b| {
        (
            a.predicate.as_str(),
            a.selector.as_str(),
            a.window.as_str(),
            &a.extra,
        )
            .cmp(&(
                b.predicate.as_str(),
                b.selector.as_str(),
                b.window.as_str(),
                &b.extra,
            ))
    });

    Ok(rows)
}

/// Decompose a journey fact's args into (selector, window, extra). The args
/// shape is the contract derive emits:
/// - `journey_occurrence` / `journey_seen` → `[selector, window]`
/// - `journey_count` / `journey_distinct` → `[selector, window, count]`
/// - `journey_since_ge` → `[selector, k]` (no window — k goes in `window` to
///   keep the row shape uniform for rendering).
fn split_args(fact: &Fact) -> (String, String, Vec<String>) {
    let selector = fact.args.first().cloned().unwrap_or_default();
    let window = fact.args.get(1).cloned().unwrap_or_default();
    let extra: Vec<String> = fact.args.iter().skip(2).cloned().collect();
    (selector, window, extra)
}

/// Walk every rule's `when` once, recording which rule ids reference each
/// `(predicate, selector)` pair. Used to fill the "RULES" column.
fn build_attribution(rules: &[Rule]) -> BTreeMap<(String, String), Vec<String>> {
    let mut out: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();

    for rule in rules {
        // Reuse the derive scan logic: a single rule's references.
        let scan = match derive::scan_rules(std::slice::from_ref(rule)) {
            Ok(s) => s,
            Err(_) => continue, // malformed conditions — skip; derive will error too.
        };
        let mut add = |pred: &str, sel: &str| {
            out.entry((pred.to_string(), sel.to_string()))
                .or_default()
                .insert(rule.id.clone());
        };
        for (s, _) in &scan.occurrence_pairs {
            add("journey_occurrence", s);
        }
        for (s, _) in &scan.count_pairs {
            add("journey_count", s);
        }
        for (s, _) in &scan.seen_pairs {
            add("journey_seen", s);
        }
        for s in scan.since_max_k.keys() {
            add("journey_since_ge", s);
        }
        for (s, _) in &scan.distinct_pairs {
            add("journey_distinct", s);
        }
    }

    out.into_iter()
        .map(|(k, v)| (k, v.into_iter().collect()))
        .collect()
}

/// Human-readable table rendering. Header + rows, padded to column widths.
pub fn render_table(rows: &[JourneyRow]) -> String {
    let mut out = String::new();
    let headers = ["PREDICATE", "ARGS", "RULES"];
    let mut widths = [headers[0].len(), headers[1].len(), headers[2].len()];

    let row_strings: Vec<[String; 3]> = rows
        .iter()
        .map(|r| {
            let args = format_args(&r.selector, &r.window, &r.extra);
            let rules = if r.rules.is_empty() {
                "-".to_string()
            } else {
                r.rules.join(", ")
            };
            [r.predicate.clone(), args, rules]
        })
        .collect();
    for cols in &row_strings {
        for i in 0..3 {
            widths[i] = widths[i].max(cols[i].len());
        }
    }
    // Header
    out.push_str(&format!(
        "{:<w0$}  {:<w1$}  {:<w2$}\n",
        headers[0],
        headers[1],
        headers[2],
        w0 = widths[0],
        w1 = widths[1],
        w2 = widths[2]
    ));
    if rows.is_empty() {
        out.push_str("(no journey facts asserted)\n");
        return out;
    }
    for cols in &row_strings {
        out.push_str(&format!(
            "{:<w0$}  {:<w1$}  {:<w2$}\n",
            cols[0],
            cols[1],
            cols[2],
            w0 = widths[0],
            w1 = widths[1],
            w2 = widths[2]
        ));
    }
    out
}

fn format_args(selector: &str, window: &str, extra: &[String]) -> String {
    let mut s = selector.to_string();
    if !window.is_empty() {
        s.push_str(" | ");
        s.push_str(window);
    }
    for e in extra {
        s.push_str(" | ");
        s.push_str(e);
    }
    s
}

/// JSON rendering — flat array of rows, schema mirrors `JourneyRow`.
pub fn render_json(rows: &[JourneyRow]) -> Result<String, JourneyCliError> {
    Ok(serde_json::to_string_pretty(rows)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use phr::Condition;

    fn rule(id: &str, conds: Vec<Condition>) -> Rule {
        Rule {
            id: id.to_string(),
            priority: 0,
            conditions: conds,
            actions: vec![],
        }
    }

    fn script(s: &str) -> Condition {
        Condition {
            predicate: "__script__".to_string(),
            args: vec![],
            script: Some(s.to_string()),
        }
    }

    #[test]
    fn split_args_handles_two_arg_shape() {
        let f = Fact {
            id: "x".into(),
            predicate: "journey_occurrence".into(),
            args: vec!["auth".into(), "s".into()],
            timestamp: 0,
        };
        let (s, w, e) = split_args(&f);
        assert_eq!(s, "auth");
        assert_eq!(w, "s");
        assert!(e.is_empty());
    }

    #[test]
    fn split_args_handles_three_arg_shape() {
        let f = Fact {
            id: "x".into(),
            predicate: "journey_count".into(),
            args: vec!["auth".into(), "s".into(), "3".into()],
            timestamp: 0,
        };
        let (s, w, e) = split_args(&f);
        assert_eq!(s, "auth");
        assert_eq!(w, "s");
        assert_eq!(e, vec!["3".to_string()]);
    }

    #[test]
    fn attribution_groups_rules_by_predicate_selector() {
        let rules = vec![
            rule(
                "a-churn",
                vec![script(
                    "facts_count('journey_occurrence', ['auth','s']) >= 3",
                )],
            ),
            rule(
                "a-no-tests",
                vec![
                    script("facts_count('journey_occurrence', ['auth','s']) >= 3"),
                    script("facts_count('journey_occurrence', ['tests','s']) == 0"),
                ],
            ),
        ];
        let attr = build_attribution(&rules);
        let auth_rules = attr
            .get(&("journey_occurrence".to_string(), "auth".to_string()))
            .unwrap();
        assert!(auth_rules.contains(&"a-churn".to_string()));
        assert!(auth_rules.contains(&"a-no-tests".to_string()));
        let tests_rules = attr
            .get(&("journey_occurrence".to_string(), "tests".to_string()))
            .unwrap();
        assert_eq!(tests_rules, &vec!["a-no-tests".to_string()]);
    }

    #[test]
    fn render_table_has_header_even_with_no_rows() {
        let out = render_table(&[]);
        assert!(out.contains("PREDICATE"));
        assert!(out.contains("no journey facts"));
    }

    #[test]
    fn render_table_includes_args_and_rules() {
        let rows = vec![JourneyRow {
            predicate: "journey_occurrence".to_string(),
            selector: "auth".to_string(),
            window: "s".to_string(),
            extra: vec![],
            rules: vec!["auth-churn".to_string()],
        }];
        let out = render_table(&rows);
        assert!(out.contains("journey_occurrence"));
        assert!(out.contains("auth"));
        assert!(out.contains("auth-churn"));
    }

    #[test]
    fn render_json_is_valid_json_array() {
        let rows = vec![JourneyRow {
            predicate: "journey_seen".into(),
            selector: "sql".into(),
            window: "5c".into(),
            extra: vec![],
            rules: vec![],
        }];
        let out = render_json(&rows).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(v.is_array());
        assert_eq!(v[0]["predicate"], "journey_seen");
        assert_eq!(v[0]["selector"], "sql");
    }
}
