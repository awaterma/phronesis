//! Core types for the RETE pattern matching engine
//!
//! This module defines the fundamental data structures used by the RETE network
//! for rule-based logic: Facts, Conditions, Actions, and Rules.

use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::info;

/// A fact in the RETE engine
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Fact {
    /// Unique identifier for the fact
    pub id: String,
    /// Predicate describing the relationship (e.g., "located_at")
    pub predicate: String,
    /// Arguments for the predicate (e.g., ["entity_id", "location_id"])
    pub args: Vec<String>,
    /// Timestamp when the fact was created
    pub timestamp: u64,
    /// Stable label identifying the subsystem that produced this fact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// A condition in a rule
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Condition {
    /// Predicate to match
    pub predicate: String,
    /// Arguments with possible variables (e.g., ["?user", "active"])
    pub args: Vec<String>,
    /// Optional Rhai script for complex condition evaluation
    pub script: Option<String>,
}

/// An action to perform when a rule fires
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Action {
    /// Type of action to perform
    pub action_type: String,
    /// Parameters for the action
    pub params: Vec<String>,
    /// Optional structured data for extended action types (e.g., emit_capsule)
    /// Skipped during serialization if None for backward compatibility
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// A rule in the RETE engine
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    /// Unique identifier for the rule
    pub id: String,
    /// Priority of the rule (higher number = higher priority)
    pub priority: i32,
    /// Conditions that must be met for the rule to fire
    pub conditions: Vec<Condition>,
    /// Actions to perform when the rule fires
    pub actions: Vec<Action>,
}

impl Rule {
    /// Variable-shaped tokens (`?name`) in this rule's action params and
    /// string data that no condition binds, deduplicated in first-seen order.
    ///
    /// A condition binds a variable by naming it as a whole argument
    /// (`["?file"]`). Substitution is textual, so a token also counts as
    /// bound when a condition variable occurs inside it — that is how it
    /// renders at fire time. Everything reported here renders literally when
    /// the rule fires; a message that merely *reads* like a variable
    /// (`"?reason: ..."`) is reported too, since the engine cannot tell the
    /// two apart. Hosts surface this as a diagnostic, never a load failure.
    pub fn unbound_action_variables(&self) -> Vec<String> {
        let bound: Vec<&str> = self
            .conditions
            .iter()
            .flat_map(|c| &c.args)
            .map(String::as_str)
            .filter(|a| a.starts_with('?'))
            .collect();
        let mut out: Vec<String> = Vec::new();
        let texts = self.actions.iter().flat_map(Action::texts);
        for token in texts.flat_map(variable_tokens) {
            if !bound.contains(&token) && !out.iter().any(|o| o == token) {
                out.push(token.to_string());
            }
        }
        out
    }
}

impl Action {
    /// Every string that variable substitution touches: the params, then the
    /// string leaves of `data`.
    pub(crate) fn texts(&self) -> Vec<&str> {
        let mut texts: Vec<&str> = self.params.iter().map(String::as_str).collect();
        if let Some(data) = &self.data {
            collect_json_strings(data, &mut texts);
        }
        texts
    }
}

fn collect_json_strings<'a>(value: &'a serde_json::Value, out: &mut Vec<&'a str>) {
    match value {
        serde_json::Value::String(s) => out.push(s),
        serde_json::Value::Array(items) => items.iter().for_each(|v| collect_json_strings(v, out)),
        serde_json::Value::Object(map) => map.values().for_each(|v| collect_json_strings(v, out)),
        _ => {}
    }
}

/// Every variable-shaped token in `text`, together with its byte range: a
/// `?` followed by an ASCII letter or `_`, then any run of ASCII
/// alphanumerics and `_`. `??` and a lone `?` are punctuation, not
/// variables. This is the single tokenizer shared by substitution
/// (`apply_bindings`) and unbound-variable detection (`warn_unbound`,
/// `unbound_action_variables`), so both agree on token boundaries: a bound
/// `?f` must not match, or be treated as binding, the longer `?file`.
pub fn variable_token_spans(text: &str) -> Vec<(std::ops::Range<usize>, &str)> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'?'
            && bytes
                .get(i + 1)
                .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_')
        {
            let start = i;
            i += 1;
            while bytes
                .get(i)
                .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
            {
                i += 1;
            }
            out.push((start..i, &text[start..i]));
        } else {
            i += 1;
        }
    }
    out
}

/// Every variable-shaped token in `text`, as whole `?ident` strings. See
/// [`variable_token_spans`] for the exact tokenization rule.
pub fn variable_tokens(text: &str) -> Vec<&str> {
    variable_token_spans(text)
        .into_iter()
        .map(|(_, token)| token)
        .collect()
}

/// Performance statistics for the RETE engine
#[derive(Debug, Default)]
pub struct PerformanceStats {
    /// Total time spent evaluating rules (cumulative across session)
    pub total_evaluation_time: Duration,
    /// Number of rule evaluations (cumulative across session)
    pub evaluation_count: u64,
    /// Total time spent asserting facts (cumulative across session)
    pub total_assertion_time: Duration,
    /// Number of fact assertions (cumulative across session)
    pub assertion_count: u64,
    /// Assertions in the current cycle
    pub cycle_assertion_count: u64,
    /// Time spent asserting in the current cycle
    pub cycle_assertion_time: Duration,
    /// Evaluations in the current cycle
    pub cycle_evaluation_count: u64,
    /// Time spent evaluating in the current cycle
    pub cycle_evaluation_time: Duration,
}

impl PerformanceStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_assertion(&mut self, duration: Duration) {
        self.assertion_count += 1;
        self.total_assertion_time += duration;
        self.cycle_assertion_count += 1;
        self.cycle_assertion_time += duration;
    }

    pub fn record_evaluation(&mut self, duration: Duration) {
        self.evaluation_count += 1;
        self.total_evaluation_time += duration;
        self.cycle_evaluation_count += 1;
        self.cycle_evaluation_time += duration;
    }

    /// Reset per-cycle counters. Call at the start of each RETE cycle.
    pub fn reset_cycle(&mut self) {
        self.cycle_assertion_count = 0;
        self.cycle_assertion_time = Duration::ZERO;
        self.cycle_evaluation_count = 0;
        self.cycle_evaluation_time = Duration::ZERO;
    }

    pub fn log_summary(&self, rules_count: usize, facts_count: usize) {
        let avg_assertion = if self.assertion_count > 0 {
            self.total_assertion_time.as_micros() as f64 / self.assertion_count as f64
        } else {
            0.0
        };
        let avg_evaluation = if self.evaluation_count > 0 {
            self.total_evaluation_time.as_micros() as f64 / self.evaluation_count as f64
        } else {
            0.0
        };

        info!(
            "RETE Performance: rules={}, facts={}, cycle=[{} assertions in {:.1}ms, {} evals in {:.1}us], cumulative=[{} assertions (avg {:.2}us), {} evals (avg {:.2}us)]",
            rules_count,
            facts_count,
            self.cycle_assertion_count,
            self.cycle_assertion_time.as_micros() as f64 / 1000.0,
            self.cycle_evaluation_count,
            self.cycle_evaluation_time.as_micros() as f64,
            self.assertion_count,
            avg_assertion,
            self.evaluation_count,
            avg_evaluation
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variable_tokens_find_names_and_skip_punctuation() {
        assert_eq!(
            variable_tokens("?a in ?file_2, not ?? or ? or ?1"),
            vec!["?a", "?file_2"]
        );
        assert!(variable_tokens("no variables").is_empty());
    }

    fn rule(conditions: &[&[&str]], message: &str) -> Rule {
        Rule {
            id: "r".to_string(),
            priority: 0,
            conditions: conditions
                .iter()
                .map(|args| Condition {
                    predicate: "p".to_string(),
                    args: args.iter().map(|a| a.to_string()).collect(),
                    script: None,
                })
                .collect(),
            actions: vec![Action {
                action_type: "constraint_violation".to_string(),
                params: vec![message.to_string()],
                data: Some(serde_json::json!({"body": "for ?who"})),
            }],
        }
    }

    #[test]
    fn unbound_action_variables_reports_only_names_no_condition_binds() {
        let r = rule(&[&["?file", "src"]], "?reason: ?file is ?file");
        assert_eq!(r.unbound_action_variables(), vec!["?reason", "?who"]);
        let bound = rule(&[&["?reason", "?who"]], "?reason");
        assert!(bound.unbound_action_variables().is_empty());
    }

    /// Regression: a bound `?f` must not mask the longer, still-unbound
    /// `?file` — a condition binds a variable only by naming it exactly,
    /// not by being a prefix of it.
    #[test]
    fn unbound_action_variables_does_not_treat_a_prefix_bound_variable_as_binding_a_longer_one() {
        let r = rule(&[&["?f"]], "Function added in ?file (file var is ?f)");
        assert_eq!(r.unbound_action_variables(), vec!["?file", "?who"]);
    }

    #[test]
    fn fact_serialization_roundtrips_with_source() {
        let fact = Fact {
            id: "f1".to_string(),
            predicate: "defines".to_string(),
            args: vec!["module".to_string(), "item".to_string()],
            timestamp: 0,
            source: Some("graph:rust".to_string()),
        };
        let json = serde_json::to_string(&fact).unwrap();
        let restored: Fact = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.source.as_deref(), Some("graph:rust"));
    }

    #[test]
    fn fact_deserializes_without_source() {
        let restored: Fact =
            serde_json::from_str(r#"{"id":"f1","predicate":"p","args":[],"timestamp":0}"#).unwrap();
        assert_eq!(restored.source, None);
    }

    #[test]
    fn new_starts_at_zero() {
        let s = PerformanceStats::new();
        assert_eq!(s.assertion_count, 0);
        assert_eq!(s.evaluation_count, 0);
        assert_eq!(s.cycle_assertion_count, 0);
        assert_eq!(s.total_assertion_time, Duration::ZERO);
    }

    #[test]
    fn record_assertion_bumps_cumulative_and_cycle() {
        let mut s = PerformanceStats::new();
        s.record_assertion(Duration::from_micros(10));
        s.record_assertion(Duration::from_micros(20));
        assert_eq!(s.assertion_count, 2);
        assert_eq!(s.cycle_assertion_count, 2);
        assert_eq!(s.total_assertion_time, Duration::from_micros(30));
        assert_eq!(s.cycle_assertion_time, Duration::from_micros(30));
    }

    #[test]
    fn record_evaluation_bumps_cumulative_and_cycle() {
        let mut s = PerformanceStats::new();
        s.record_evaluation(Duration::from_micros(5));
        assert_eq!(s.evaluation_count, 1);
        assert_eq!(s.cycle_evaluation_count, 1);
        assert_eq!(s.total_evaluation_time, Duration::from_micros(5));
    }

    #[test]
    fn reset_cycle_clears_cycle_but_keeps_cumulative() {
        let mut s = PerformanceStats::new();
        s.record_assertion(Duration::from_micros(10));
        s.record_evaluation(Duration::from_micros(7));
        s.reset_cycle();
        // Cycle counters cleared...
        assert_eq!(s.cycle_assertion_count, 0);
        assert_eq!(s.cycle_assertion_time, Duration::ZERO);
        assert_eq!(s.cycle_evaluation_count, 0);
        assert_eq!(s.cycle_evaluation_time, Duration::ZERO);
        // ...cumulative preserved.
        assert_eq!(s.assertion_count, 1);
        assert_eq!(s.evaluation_count, 1);
    }

    #[test]
    fn log_summary_handles_zero_and_nonzero_counts() {
        // Both the divide-by-zero guard and the averaging path must execute
        // without panicking. (No subscriber installed; this exercises the
        // pre-`info!` arithmetic, which runs unconditionally.)
        let empty = PerformanceStats::new();
        empty.log_summary(0, 0);

        let mut active = PerformanceStats::new();
        active.record_assertion(Duration::from_micros(40));
        active.record_evaluation(Duration::from_micros(8));
        active.log_summary(3, 12);
    }
}
