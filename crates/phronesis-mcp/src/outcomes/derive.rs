//! Derive confidence signals from a subject's journey journal entries.
//!
//! Approach A (SPEC-confidence-scoring §3): each grounded signal that passed
//! becomes one atomic `signal_pass(subject, name)` fact; gate rules count them
//! with `facts_count`. Derivation reads the **latest** outcome of each kind — a
//! re-run reflects the current state, so an earlier red run does not keep a
//! stale signal alive. This is the per-invocation re-derivation the stateless
//! hook model relies on.
//!
//! Storage: post-0.13.0, the per-subject outcome ledger has folded into the
//! journey journal (SPEC-journey-facts §"Subject and the outcomes fold-in").
//! `signals` reads `journey::journal::read_recent_subject` and reconstructs
//! the predicate shape `signals_from` expects from each record's outcome tags.

use std::path::Path;

use crate::journey::journal::{self, JournalRecord};
use crate::outcomes::facts::{Band, OutcomeFact};

/// A neutral derived entry: same `(predicate, args)` shape the old
/// `LedgerEntry` carried, recovered from a journal record's outcome tags.
/// Order in the returned vec is append-order — `signals_from` relies on
/// "latest of each kind wins" iterating in reverse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedEntry {
    pub predicate: String,
    pub args: Vec<String>,
}

/// Read `subject`'s journey records and return the `signal_pass` facts they
/// support now. Public signature unchanged from the pre-fold-in ledger
/// version — confidence-scoring callers (commit gate, CLI report, MCP tool)
/// don't change.
pub fn signals(root: &Path, subject: &str) -> Result<Vec<OutcomeFact>, journal::JournalError> {
    let records = journal::read_recent_subject(root, subject, journal::SUFFIX_HARD_CAP)?;
    let entries = entries_from(subject, &records);
    Ok(signals_from(subject, &entries))
}

/// The confidence band for `subject` from its passed-signal count. Gate rules
/// (approach A) count `signal_pass` directly; this host-side band is for the
/// report / CLI / approach B.
pub fn band(root: &Path, subject: &str) -> Result<Band, journal::JournalError> {
    Ok(Band::from_signal_count(signals(root, subject)?.len()))
}

/// Translate a slice of journey records carrying `outcome:*` tags into the
/// `(predicate, args)` shape the legacy `signals_from` consumes. Append
/// order is preserved.
pub fn entries_from(subject: &str, records: &[JournalRecord]) -> Vec<DerivedEntry> {
    let mut out = Vec::new();
    for rec in records {
        for tag in &rec.tags {
            match tag.as_str() {
                "outcome:compile_ok" => out.push(DerivedEntry {
                    predicate: "build_outcome".to_string(),
                    args: vec![subject.to_string(), "pass".to_string()],
                }),
                "outcome:compile_error" => out.push(DerivedEntry {
                    predicate: "build_outcome".to_string(),
                    args: vec![subject.to_string(), "fail".to_string()],
                }),
                "outcome:test_pass" => out.push(DerivedEntry {
                    predicate: "test_outcome".to_string(),
                    args: vec![
                        subject.to_string(),
                        "1".to_string(),
                        "0".to_string(),
                        "1".to_string(),
                    ],
                }),
                "outcome:test_fail" => out.push(DerivedEntry {
                    predicate: "test_outcome".to_string(),
                    args: vec![
                        subject.to_string(),
                        "0".to_string(),
                        "1".to_string(),
                        "1".to_string(),
                    ],
                }),
                t if t.starts_with("outcome:bug_caught:") => {
                    let id = &t["outcome:bug_caught:".len()..];
                    out.push(DerivedEntry {
                        predicate: "bug_check_outcome".to_string(),
                        args: vec![subject.to_string(), id.to_string(), "fixed".to_string()],
                    });
                }
                // `outcome:compile_unknown` (Task 4 decision): an unknown run
                // produced no evidence, so it carries no signal in either
                // direction — it neither grounds a compile signal nor
                // clobbers an earlier grounded pass/fail via latest-wins.
                "outcome:compile_unknown" => {}
                _ => {}
            }
        }
    }
    out
}

fn latest<'a>(entries: &'a [DerivedEntry], predicate: &str) -> Option<&'a DerivedEntry> {
    entries.iter().rev().find(|e| e.predicate == predicate)
}

/// Pure core: derive `signal_pass` facts from derived entries.
///
/// - `compile` — the latest `build_outcome` is `pass`.
/// - `tests` — the latest `test_outcome` ran at least one test and none failed.
///   (`total == 0` means no tests ran, which is not a passing signal.)
/// - `bug:<id>` — each known bug whose latest check is `fixed`.
pub fn signals_from(subject: &str, entries: &[DerivedEntry]) -> Vec<OutcomeFact> {
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

    fn entry(predicate: &str, args: &[&str]) -> DerivedEntry {
        DerivedEntry {
            predicate: predicate.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn names(facts: &[OutcomeFact]) -> Vec<String> {
        facts.iter().map(|f| f.args[1].clone()).collect()
    }

    fn rec(seq: u64, subject: &str, tags: &[&str]) -> JournalRecord {
        JournalRecord {
            v: 1,
            ts: 1000 + seq,
            sid: "s-test".to_string(),
            seq,
            tool: "Bash".to_string(),
            path: "<cmd>".to_string(),
            ext: None,
            module: None,
            tags: tags.iter().map(|s| s.to_string()).collect(),
            subject: Some(subject.to_string()),
            command_exit: None,
        }
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
    fn empty_entries_yields_no_signals() {
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
            entry("bug_check_outcome", &["u", "1042", "fixed"]),
            entry("bug_check_outcome", &["u", "1042", "open"]),
        ];
        assert_eq!(names(&signals_from("u", &entries)), vec!["compile"]);
    }

    #[test]
    fn entries_from_translates_outcome_tags() {
        let recs = vec![
            rec(1, "u", &["outcome:compile_ok"]),
            rec(2, "u", &["outcome:test_pass"]),
            rec(3, "u", &["outcome:bug_caught:1042"]),
        ];
        let e = entries_from("u", &recs);
        assert_eq!(e.len(), 3);
        assert_eq!(e[0].predicate, "build_outcome");
        assert_eq!(e[0].args, vec!["u", "pass"]);
        assert_eq!(e[1].predicate, "test_outcome");
        // synthetic args: passed=1, failed=0, total=1 — the shape signals_from reads.
        assert_eq!(e[1].args, vec!["u", "1", "0", "1"]);
        assert_eq!(e[2].predicate, "bug_check_outcome");
        assert_eq!(e[2].args, vec!["u", "1042", "fixed"]);
    }

    #[test]
    fn entries_from_handles_test_fail_and_compile_error() {
        let recs = vec![
            rec(1, "u", &["outcome:compile_error"]),
            rec(2, "u", &["outcome:test_fail"]),
        ];
        let e = entries_from("u", &recs);
        assert_eq!(e[0].args[1], "fail");
        assert_eq!(e[1].args[2], "1"); // failed > 0
    }

    #[test]
    fn entries_from_skips_unrelated_tags() {
        let recs = vec![rec(1, "u", &["auth", "build", "outcome:compile_ok"])];
        let e = entries_from("u", &recs);
        assert_eq!(e.len(), 1, "only outcome:* tags translate");
    }

    #[test]
    fn signals_reads_journal_via_subject_filter() {
        let dir = tempfile::tempdir().unwrap();
        journal::append(
            dir.path(),
            &rec(1, "u", &["outcome:compile_ok", "outcome:test_pass"]),
        )
        .unwrap();
        // unrelated subject — must not influence
        journal::append(dir.path(), &rec(2, "v", &["outcome:compile_error"])).unwrap();
        let s = signals(dir.path(), "u").unwrap();
        assert_eq!(names(&s), vec!["compile", "tests"]);
        assert_eq!(band(dir.path(), "u").unwrap(), Band::Medium);
    }

    #[test]
    fn compile_unknown_tag_grounds_no_signal() {
        let recs = vec![rec(1, "u", &["outcome:compile_unknown"])];
        let e = entries_from("u", &recs);
        assert!(e.is_empty(), "unknown must not become a derived entry");
        assert!(signals_from("u", &e).is_empty());
    }

    #[test]
    fn compile_unknown_does_not_clobber_an_earlier_grounded_pass() {
        // Decision (Task 4): unknown carries no information, so the latest
        // *grounded* outcome stands — a pass followed by an evidence-free run
        // keeps its compile signal; it just gains nothing new.
        let recs = vec![
            rec(1, "u", &["outcome:compile_ok"]),
            rec(2, "u", &["outcome:compile_unknown"]),
        ];
        let e = entries_from("u", &recs);
        assert_eq!(names(&signals_from("u", &e)), vec!["compile"]);
    }

    #[test]
    fn compile_unknown_alone_yields_low_band() {
        let dir = tempfile::tempdir().unwrap();
        journal::append(dir.path(), &rec(1, "u", &["outcome:compile_unknown"])).unwrap();
        assert!(signals(dir.path(), "u").unwrap().is_empty());
        assert_eq!(band(dir.path(), "u").unwrap(), Band::Low);
    }
}
