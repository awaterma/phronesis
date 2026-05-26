//! Core types for the RETE pattern matching engine
//!
//! This module defines the fundamental data structures used by the RETE network
//! for rule-based card game logic: Facts, Conditions, Actions, and Rules.

use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::info;

/// A fact in the RETE engine
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Fact {
    /// Unique identifier for the fact
    pub id: String,
    /// Predicate describing the relationship (e.g., "player_at_state")
    pub predicate: String,
    /// Arguments for the predicate (e.g., ["player_id", "state_id"])
    pub args: Vec<String>,
    /// Timestamp when the fact was created
    pub timestamp: u64,
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Action {
    /// Type of action to perform
    pub action_type: String,
    /// Parameters for the action
    pub params: Vec<String>,
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
