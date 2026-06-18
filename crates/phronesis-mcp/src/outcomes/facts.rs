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

fn status(passed: bool) -> String {
    if passed { "pass" } else { "fail" }.to_string()
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
}
