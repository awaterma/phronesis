//! Neutral, language-agnostic outcome facts.
//!
//! Outcome adapters parse one toolchain's output and emit these. Rules match
//! the neutral predicates here, never a toolchain's own vocabulary. The
//! `(predicate, args)` shape mirrors `clock_facts::ClockFact` so the hook can
//! wrap each one in a RETE `Fact` with the same machinery.

/// A grounded outcome, keyed by the confidence *subject* (the work unit the
/// signal is about). Ready to be turned into a RETE `Fact`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutcomeFact {
    pub predicate: &'static str,
    pub args: Vec<String>,
}

impl OutcomeFact {
    /// `build_outcome(subject, "pass" | "fail")` — did the code compile?
    pub fn build(subject: &str, passed: bool) -> Self {
        Self {
            predicate: "build_outcome",
            args: vec![subject.to_string(), status(passed)],
        }
    }

    /// `build_outcome(subject, "unknown")` — no exit code was captured, no
    /// failure pattern matched, and no explicit success evidence appeared.
    /// Absence of failure evidence is not proof of success
    /// (evidence-integrity spec, Task 4): unknown is journaled as
    /// `outcome:compile_unknown` and never grounds a confidence signal.
    pub fn build_unknown(subject: &str) -> Self {
        Self {
            predicate: "build_outcome",
            args: vec![subject.to_string(), "unknown".to_string()],
        }
    }

    /// `test_outcome(subject, passed, failed, total)` — test counts. `total`
    /// is `passed + failed` (tests actually run; ignored/filtered are not
    /// counted, matching what a rule means by "the tests").
    pub fn test(subject: &str, passed: usize, failed: usize) -> Self {
        let total = passed + failed;
        Self {
            predicate: "test_outcome",
            args: vec![
                subject.to_string(),
                passed.to_string(),
                failed.to_string(),
                total.to_string(),
            ],
        }
    }
}

impl OutcomeFact {
    /// `signal_pass(subject, name)` — one atomic "this grounded signal passed"
    /// fact (approach A in SPEC §3). `name` is `"compile"` / `"tests"` /
    /// `"bug:<id>"`. Gate rules count these with `facts_count`.
    pub fn signal(subject: &str, name: &str) -> Self {
        Self {
            predicate: "signal_pass",
            args: vec![subject.to_string(), name.to_string()],
        }
    }

    /// `bug_check_outcome(subject, bug_id, status)` — the TDD known-bug signal.
    /// `status` ∈ `fixed` (the bug's test went green with no regressions) /
    /// `open` (still red) / `regressed`.
    pub fn bug_check(subject: &str, bug_id: &str, status: &str) -> Self {
        Self {
            predicate: "bug_check_outcome",
            args: vec![subject.to_string(), bug_id.to_string(), status.to_string()],
        }
    }
}

fn status(passed: bool) -> String {
    if passed { "pass" } else { "fail" }.to_string()
}

/// The discretized confidence band — what the gate and the report speak in.
/// Gate rules in approach A count `signal_pass` facts directly; this is the
/// host-side rendering of the same thresholds (reporting / approach B).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band {
    High,
    Medium,
    Low,
}

impl Band {
    /// 3+ passed signals = high, 2 = medium, ≤1 = low (SPEC §3).
    pub fn from_signal_count(passed: usize) -> Self {
        match passed {
            0 | 1 => Band::Low,
            2 => Band::Medium,
            _ => Band::High,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Band::High => "high",
            Band::Medium => "medium",
            Band::Low => "low",
        }
    }
}

/// An outcome tag that carries a grounded signal. `outcome:compile_unknown`
/// is deliberately excluded: absent evidence must not displace grounded
/// evidence — in derivation OR in compaction retention.
pub fn is_grounded_outcome_tag(tag: &str) -> bool {
    tag.starts_with("outcome:") && tag != "outcome:compile_unknown"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_pass_carries_subject_and_status() {
        let f = OutcomeFact::build("unit-7", true);
        assert_eq!(f.predicate, "build_outcome");
        assert_eq!(f.args, vec!["unit-7".to_string(), "pass".to_string()]);
    }

    #[test]
    fn build_unknown_carries_unknown_status() {
        let f = OutcomeFact::build_unknown("unit-7");
        assert_eq!(f.predicate, "build_outcome");
        assert_eq!(f.args, vec!["unit-7".to_string(), "unknown".to_string()]);
    }

    #[test]
    fn build_fail_status_is_fail() {
        let f = OutcomeFact::build("unit-7", false);
        assert_eq!(f.args[1], "fail");
    }

    #[test]
    fn test_outcome_computes_total() {
        let f = OutcomeFact::test("unit-7", 12, 3);
        assert_eq!(f.predicate, "test_outcome");
        assert_eq!(
            f.args,
            vec![
                "unit-7".to_string(),
                "12".to_string(),
                "3".to_string(),
                "15".to_string(),
            ]
        );
    }

    #[test]
    fn test_outcome_all_pass_has_zero_failed() {
        let f = OutcomeFact::test("u", 5, 0);
        assert_eq!(f.args[2], "0");
        assert_eq!(f.args[3], "5");
    }

    #[test]
    fn signal_carries_subject_and_name() {
        let f = OutcomeFact::signal("u", "compile");
        assert_eq!(f.predicate, "signal_pass");
        assert_eq!(f.args, vec!["u".to_string(), "compile".to_string()]);
    }

    #[test]
    fn band_thresholds() {
        assert_eq!(Band::from_signal_count(0), Band::Low);
        assert_eq!(Band::from_signal_count(1), Band::Low);
        assert_eq!(Band::from_signal_count(2), Band::Medium);
        assert_eq!(Band::from_signal_count(3), Band::High);
        assert_eq!(Band::from_signal_count(4), Band::High);
        assert_eq!(Band::High.as_str(), "high");
        assert_eq!(Band::Low.as_str(), "low");
    }
}
