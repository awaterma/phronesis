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
                t if t.starts_with("outcome:proof_pass:") => {
                    let property = &t["outcome:proof_pass:".len()..];
                    out.push(DerivedEntry {
                        predicate: "proof_outcome".to_string(),
                        args: vec![
                            subject.to_string(),
                            property.to_string(),
                            "passed".to_string(),
                        ],
                    });
                }
                t if t.starts_with("outcome:proof_fail:") => {
                    let property = &t["outcome:proof_fail:".len()..];
                    out.push(DerivedEntry {
                        predicate: "proof_outcome".to_string(),
                        args: vec![
                            subject.to_string(),
                            property.to_string(),
                            "failed".to_string(),
                        ],
                    });
                }
                "outcome:proof_run_fail" => out.push(DerivedEntry {
                    predicate: "proof_run_outcome".to_string(),
                    args: vec![subject.to_string(), "failed".to_string()],
                }),
                "outcome:proof_run_inconclusive" => out.push(DerivedEntry {
                    predicate: "proof_run_outcome".to_string(),
                    args: vec![subject.to_string(), "inconclusive".to_string()],
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

    // proof — per-property latest verifier result (SPEC-property-ontology.md
    // §3): the signal grounds only when at least one property has a proof and
    // every latest result is `passed` — failed/timeout/inconclusive never
    // count, matching the three-state discipline. BTreeMap keeps determinism.
    //
    // A run-level `proof_run_outcome` (failed, or inconclusive: SPEC-C S8's
    // "silence is a state") withholds the signal until a later run reports
    // per-property results — it names no property, so it can't displace one
    // by key, and without it an earlier pass would outlive a failed re-run.
    // The toolchain emits it after the run's own per-property facts.
    let mut proof_latest: std::collections::BTreeMap<&str, &str> =
        std::collections::BTreeMap::new();
    let mut proof_run_withheld = false;
    for e in entries.iter() {
        match e.predicate.as_str() {
            "proof_outcome" => {
                if let (Some(property), Some(status)) = (e.args.get(1), e.args.get(2)) {
                    proof_latest.insert(property.as_str(), status.as_str());
                    proof_run_withheld = false;
                }
            }
            "proof_run_outcome" => proof_run_withheld = true,
            _ => {}
        }
    }
    if !proof_run_withheld
        && !proof_latest.is_empty()
        && proof_latest.values().all(|s| *s == "passed")
    {
        out.push(OutcomeFact::signal(subject, "proof"));
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
            kind: None,
            mode: None,
            host: None,
            turn: None,
            agent: None,
            agent_type: None,
            kalpa: None,
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

// ---- SPEC-property-ontology.md §3 / B3: proof signal through a declarative
// proof toolchain ----

#[cfg(test)]
mod proof_tests {
    use super::*;
    use crate::outcomes::adapter::outcome_tags;
    use crate::outcomes::toolchain::{CompiledDef, DefSource, ToolchainDef};

    fn kani_def() -> ToolchainDef {
        serde_json::from_str(
            r#"{
                "id": "kani",
                "matches": "^cargo kani",
                "compile_fail": ["error\\[E\\d+\\]"],
                "compile_success": ["Verification complete"],
                "per_test": "(?m)Checks for property (?P<name>\\S+): (?P<status>SUCCESS|FAILURE)",
                "pass_tokens": ["SUCCESS"],
                "outcome_kind": "proof"
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn b3_proof_pass_through_a_toolchain_def_grounds_the_proof_signal() {
        let compiled = CompiledDef::compile(kani_def(), DefSource::Project).unwrap();
        assert!(
            compiled.is_proof,
            "the def must register as a proof toolchain"
        );

        let subject = "u";
        let output = "Checks for property safe_divide.zero_returns_error: SUCCESS\n\
                      Checks for property safe_divide.nonzero_returns_quotient: SUCCESS";
        let facts = compiled.parse(subject, "cargo kani --harness verify_zero", output, Some(0));

        let proof_facts: Vec<_> = facts
            .iter()
            .filter(|f| f.predicate == "proof_outcome")
            .collect();
        assert_eq!(proof_facts.len(), 2, "two properties proved: {facts:?}");

        let tags = outcome_tags(&facts);
        assert!(
            tags.iter()
                .any(|t| t == "outcome:proof_pass:safe_divide.zero_returns_error"),
            "proof pass tag must journal: {tags:?}"
        );

        let records: Vec<JournalRecord> = tags
            .iter()
            .enumerate()
            .map(|(i, t)| JournalRecord {
                v: 1,
                ts: i as u64,
                sid: "s".to_string(),
                seq: i as u64,
                tool: "Bash".to_string(),
                path: "<cmd>".to_string(),
                ext: None,
                module: None,
                tags: vec![t.clone()],
                subject: Some(subject.to_string()),
                command_exit: Some(0),
                kind: None,
                mode: None,
                host: None,
                turn: None,
                agent: None,
                agent_type: None,
                kalpa: None,
            })
            .collect();
        let entries = entries_from(subject, &records);
        let signals = signals_from(subject, &entries);
        assert!(
            signals
                .iter()
                .any(|f| f.predicate == "signal_pass" && f.args[1] == "proof"),
            "signal_pass(subject, \"proof\") must ground: {signals:?}"
        );

        // The Band lifts: proof joins compile and tests in the signal count.
        let band = Band::from_signal_count(signals.len());
        assert!(
            matches!(band, Band::Medium | Band::High),
            "a band grounded on proof signals must lift beyond low: {band:?}"
        );
    }

    /// One journal record per proof run: the def parses the run, the adapter
    /// turns its facts into tags, the hook stamps them on one record.
    fn proof_run_record(seq: u64, output: &str, exit: Option<i32>) -> JournalRecord {
        let compiled = CompiledDef::compile(kani_def(), DefSource::Project).unwrap();
        let facts = compiled.parse("u", "cargo kani", output, exit);
        JournalRecord {
            v: 1,
            ts: seq,
            sid: "s".to_string(),
            seq,
            tool: "Bash".to_string(),
            path: "<cmd>".to_string(),
            ext: None,
            module: None,
            tags: outcome_tags(&facts),
            subject: Some("u".to_string()),
            command_exit: exit,
            kind: None,
            mode: None,
            host: None,
            turn: None,
            agent: None,
            agent_type: None,
            kalpa: None,
        }
    }

    fn has_proof_signal(records: &[JournalRecord]) -> bool {
        signals_from("u", &entries_from("u", records))
            .iter()
            .any(|f| f.predicate == "signal_pass" && f.args[1] == "proof")
    }

    const PASSING_RUN: &str = "Checks for property safe_divide.zero_returns_error: SUCCESS\n";

    /// S8: "failed never upgrades confidence" — a later failing proof run
    /// (FAILURE line, non-zero exit) must not leave the earlier pass standing.
    #[test]
    fn a_failing_proof_run_retracts_an_earlier_proof_signal() {
        let first = proof_run_record(1, PASSING_RUN, Some(0));
        assert!(has_proof_signal(std::slice::from_ref(&first)));

        let failing = proof_run_record(
            2,
            "Checks for property safe_divide.zero_returns_error: FAILURE\n",
            Some(1),
        );
        assert!(
            !has_proof_signal(&[first.clone(), failing]),
            "a failed proof run must not leave a stale proof signal"
        );

        // A non-zero exit with no per-property lines at all (the verifier
        // crashed, or failed on a harness the regex doesn't name).
        let crashed = proof_run_record(2, "thread 'main' panicked\n", Some(1));
        assert!(!has_proof_signal(&[first, crashed]));
    }

    /// S8: "silence is a state" — a proof run that exits 0 but whose output
    /// matches no property is inconclusive, and inconclusive never keeps an
    /// earlier pass alive.
    #[test]
    fn a_silent_proof_run_retracts_an_earlier_proof_signal() {
        let first = proof_run_record(1, PASSING_RUN, Some(0));
        let garbage = proof_run_record(2, "<html>totally unparseable</html>\n", Some(0));
        assert!(
            !has_proof_signal(&[first.clone(), garbage]),
            "a zero-match proof run must not leave a stale proof signal"
        );

        // No exit code and no evidence: still a proof run that matched nothing.
        let unknown = proof_run_record(2, "", None);
        assert!(!has_proof_signal(&[first, unknown]));
    }

    /// The happy path survives: a clean run grounds proof, and a clean re-run
    /// after a failed or silent one grounds it again.
    #[test]
    fn a_passing_proof_run_after_a_failed_one_grounds_the_signal_again() {
        let pass = |seq| proof_run_record(seq, PASSING_RUN, Some(0));
        assert!(has_proof_signal(&[pass(1)]));
        let failing = proof_run_record(
            2,
            "Checks for property safe_divide.zero_returns_error: FAILURE\n",
            Some(1),
        );
        assert!(has_proof_signal(&[pass(1), failing.clone(), pass(3)]));
        let garbage = proof_run_record(2, "garbage\n", Some(0));
        assert!(has_proof_signal(&[pass(1), garbage, pass(3)]));

        // A property that failed stays failed until it is re-proved: a clean
        // run of a *different* property doesn't paper over it.
        let other = proof_run_record(
            3,
            "Checks for property safe_divide.nonzero_returns_quotient: SUCCESS\n",
            Some(0),
        );
        assert!(!has_proof_signal(&[pass(1), failing, other]));
    }
}
