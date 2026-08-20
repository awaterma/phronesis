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
