//! Derive confidence signals from a subject's outcome ledger.
//!
//! Approach A (SPEC-confidence-scoring §3): each grounded signal that passed
//! becomes one atomic `signal_pass(subject, name)` fact; gate rules count them
//! with `facts_count`. Derivation reads the **latest** outcome of each kind — a
//! re-run reflects the current state, so an earlier red run does not keep a
//! stale signal alive. This is the per-invocation re-derivation the stateless
//! hook model relies on.

use std::path::Path;

use crate::outcomes::facts::{Band, OutcomeFact};
use crate::outcomes::ledger::{self, LedgerEntry};

/// Read `subject`'s ledger and return the `signal_pass` facts it supports now.
pub fn signals(root: &Path, subject: &str) -> Result<Vec<OutcomeFact>, ledger::LedgerError> {
    let entries = ledger::read(root, subject)?;
    Ok(signals_from(subject, &entries))
}

/// The confidence band for `subject` from its passed-signal count. Gate rules
/// (approach A) count `signal_pass` directly; this host-side band is for the
/// report / CLI / approach B.
pub fn band(root: &Path, subject: &str) -> Result<Band, ledger::LedgerError> {
    Ok(Band::from_signal_count(signals(root, subject)?.len()))
}

fn latest<'a>(entries: &'a [LedgerEntry], predicate: &str) -> Option<&'a LedgerEntry> {
    entries.iter().rev().find(|e| e.predicate == predicate)
}

/// Pure core: derive `signal_pass` facts from ledger entries.
///
/// - `compile` — the latest `build_outcome` is `pass`.
/// - `tests` — the latest `test_outcome` ran at least one test and none failed.
///   (`total == 0` means no tests ran, which is not a passing signal.)
///
/// The known-bug `bug:<id>` signal is added in a later commit.
pub fn signals_from(subject: &str, entries: &[LedgerEntry]) -> Vec<OutcomeFact> {
    let mut out = Vec::new();

    if let Some(b) = latest(entries, "build_outcome")
        && b.args.get(1).is_some_and(|s| s == "pass")
    {
        out.push(OutcomeFact::signal(subject, "compile"));
    }

    if let Some(t) = latest(entries, "test_outcome") {
        // args: [subject, passed, failed, total]
        let failed = t
            .args
            .get(2)
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(1);
        let total = t
            .args
            .get(3)
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(0);
        if failed == 0 && total > 0 {
            out.push(OutcomeFact::signal(subject, "tests"));
        }
    }

    // bug:<id> — each known bug whose latest check is "fixed". BTreeMap keeps
    // the output deterministic (the contract derivation relies on).
    let mut bug_latest: std::collections::BTreeMap<&str, &str> = std::collections::BTreeMap::new();
    for e in entries
        .iter()
        .filter(|e| e.predicate == "bug_check_outcome")
    {
        if let (Some(id), Some(status)) = (e.args.get(1), e.args.get(2)) {
            bug_latest.insert(id.as_str(), status.as_str());
        }
    }
    for (id, status) in bug_latest {
        if status == "fixed" {
            out.push(OutcomeFact::signal(subject, &format!("bug:{id}")));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(predicate: &str, args: &[&str]) -> LedgerEntry {
        LedgerEntry {
            ts: 0,
            predicate: predicate.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn names(facts: &[OutcomeFact]) -> Vec<String> {
        facts.iter().map(|f| f.args[1].clone()).collect()
    }

    #[test]
    fn all_green_yields_compile_and_tests() {
        let entries = vec![
            entry("build_outcome", &["u", "pass"]),
            entry("test_outcome", &["u", "10", "0", "10"]),
        ];
        let s = signals_from("u", &entries);
        assert_eq!(names(&s), vec!["compile", "tests"]);
        assert_eq!(Band::from_signal_count(s.len()), Band::Medium);
    }

    #[test]
    fn failing_build_yields_no_signals() {
        let entries = vec![entry("build_outcome", &["u", "fail"])];
        assert!(signals_from("u", &entries).is_empty());
    }

    #[test]
    fn failing_tests_yield_only_compile() {
        let entries = vec![
            entry("build_outcome", &["u", "pass"]),
            entry("test_outcome", &["u", "8", "2", "10"]),
        ];
        assert_eq!(names(&signals_from("u", &entries)), vec!["compile"]);
    }

    #[test]
    fn no_tests_run_is_not_a_test_signal() {
        let entries = vec![
            entry("build_outcome", &["u", "pass"]),
            entry("test_outcome", &["u", "0", "0", "0"]),
        ];
        assert_eq!(names(&signals_from("u", &entries)), vec!["compile"]);
    }

    #[test]
    fn latest_outcome_wins_over_earlier_red_run() {
        let entries = vec![
            entry("build_outcome", &["u", "fail"]),
            entry("test_outcome", &["u", "0", "5", "5"]),
            // a later green re-run
            entry("build_outcome", &["u", "pass"]),
            entry("test_outcome", &["u", "5", "0", "5"]),
        ];
        assert_eq!(
            names(&signals_from("u", &entries)),
            vec!["compile", "tests"]
        );
    }

    #[test]
    fn empty_ledger_yields_no_signals() {
        assert!(signals_from("u", &[]).is_empty());
    }

    #[test]
    fn fixed_bug_adds_a_bug_signal_three_of_three_is_high() {
        let entries = vec![
            entry("build_outcome", &["u", "pass"]),
            entry("test_outcome", &["u", "5", "0", "5"]),
            entry("bug_check_outcome", &["u", "1042", "fixed"]),
        ];
        let s = signals_from("u", &entries);
        assert_eq!(names(&s), vec!["compile", "tests", "bug:1042"]);
        assert_eq!(Band::from_signal_count(s.len()), Band::High);
    }

    #[test]
    fn open_bug_check_is_not_a_signal_and_latest_wins() {
        let entries = vec![
            entry("build_outcome", &["u", "pass"]),
            // first fixed, then re-checked open → latest (open) wins, no signal
            entry("bug_check_outcome", &["u", "1042", "fixed"]),
            entry("bug_check_outcome", &["u", "1042", "open"]),
        ];
        assert_eq!(names(&signals_from("u", &entries)), vec!["compile"]);
    }

    #[test]
    fn signals_and_band_read_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        ledger::append(
            dir.path(),
            "u",
            &[OutcomeFact::build("u", true), OutcomeFact::test("u", 4, 0)],
        )
        .unwrap();
        let s = signals(dir.path(), "u").unwrap();
        assert_eq!(names(&s), vec!["compile", "tests"]);
        assert_eq!(band(dir.path(), "u").unwrap(), Band::Medium);
    }
}
