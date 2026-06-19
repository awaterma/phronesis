//! The known-bug registry — the TDD "catch-the-bug" signal.
//!
//! `.phronesis/bugs.json` lists tests that *should* be red on the buggy
//! baseline (they exercise a known bug). When a suggestion turns such a test
//! green **with no regressions**, that's a grounded signal the fix is real, not
//! hallucinated. See `docs/specs/SPEC-confidence-scoring.md` §4.
//!
//! Honest limitation (milestone): a test can go green for the wrong reason. The
//! defense here is requiring zero regressions alongside the fix; confirming the
//! red→green transition against the baseline is a phase-2 refinement.

use std::path::Path;

use serde::Deserialize;

use crate::outcomes::facts::OutcomeFact;

/// One registry entry. `test` must match the name cargo prints
/// (e.g. `auth::tests::rejects_expired_token`).
#[derive(Debug, Clone, Deserialize)]
pub struct KnownBug {
    pub bug_id: String,
    pub test: String,
    /// `open` (the bug is live; its test should currently fail) or `fixed`.
    /// Only `open` bugs are scored — a `fixed` bug is history.
    #[serde(default = "default_status")]
    pub status: String,
}

fn default_status() -> String {
    "open".to_string()
}

/// Load `.phronesis/bugs.json`. Missing or malformed → empty (fail-open).
pub fn load(root: &Path) -> Vec<KnownBug> {
    let path = root.join(".phronesis").join("bugs.json");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    serde_json::from_str(&content).unwrap_or_default()
}

/// Produce `bug_check_outcome` facts for `subject` from per-test results.
///
/// For each *open* bug whose test appears in the run:
/// - test green **and** `no_regressions` → `fixed` (earns the bug signal),
/// - test still red → `open`,
/// - test green but other tests regressed → no `fixed` fact (the suggestion
///   broke something; the fix isn't trustworthy yet).
pub fn check(
    subject: &str,
    bugs: &[KnownBug],
    per_test: &[(String, bool)],
    no_regressions: bool,
) -> Vec<OutcomeFact> {
    let mut out = Vec::new();
    for bug in bugs.iter().filter(|b| b.status == "open") {
        let Some((_, passed)) = per_test.iter().find(|(name, _)| name == &bug.test) else {
            continue;
        };
        if *passed && no_regressions {
            out.push(OutcomeFact::bug_check(subject, &bug.bug_id, "fixed"));
        } else if !*passed {
            out.push(OutcomeFact::bug_check(subject, &bug.bug_id, "open"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bug(id: &str, test: &str) -> KnownBug {
        KnownBug {
            bug_id: id.to_string(),
            test: test.to_string(),
            status: "open".to_string(),
        }
    }

    fn statuses(facts: &[OutcomeFact]) -> Vec<(String, String)> {
        facts
            .iter()
            .map(|f| (f.args[1].clone(), f.args[2].clone()))
            .collect()
    }

    #[test]
    fn open_bug_test_green_with_no_regressions_is_fixed() {
        let bugs = vec![bug("1042", "auth::rejects_expired")];
        let per_test = vec![("auth::rejects_expired".to_string(), true)];
        assert_eq!(
            statuses(&check("u", &bugs, &per_test, true)),
            vec![("1042".to_string(), "fixed".to_string())]
        );
    }

    #[test]
    fn green_bug_test_with_regression_does_not_earn_fixed() {
        let bugs = vec![bug("1042", "auth::rejects_expired")];
        let per_test = vec![("auth::rejects_expired".to_string(), true)];
        // no_regressions = false → no fixed fact
        assert!(check("u", &bugs, &per_test, false).is_empty());
    }

    #[test]
    fn still_red_bug_test_is_open() {
        let bugs = vec![bug("1042", "auth::rejects_expired")];
        let per_test = vec![("auth::rejects_expired".to_string(), false)];
        assert_eq!(
            statuses(&check("u", &bugs, &per_test, false)),
            vec![("1042".to_string(), "open".to_string())]
        );
    }

    #[test]
    fn already_fixed_bug_is_not_scored() {
        let mut b = bug("1042", "auth::rejects_expired");
        b.status = "fixed".to_string();
        let per_test = vec![("auth::rejects_expired".to_string(), true)];
        assert!(check("u", &[b], &per_test, true).is_empty());
    }

    #[test]
    fn bug_test_absent_from_run_yields_nothing() {
        let bugs = vec![bug("1042", "auth::rejects_expired")];
        let per_test = vec![("other::test".to_string(), true)];
        assert!(check("u", &bugs, &per_test, true).is_empty());
    }

    #[test]
    fn load_missing_registry_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(dir.path()).is_empty());
    }

    #[test]
    fn load_parses_registry() {
        let dir = tempfile::tempdir().unwrap();
        let phr = dir.path().join(".phronesis");
        std::fs::create_dir_all(&phr).unwrap();
        std::fs::write(
            phr.join("bugs.json"),
            r#"[{"bug_id":"7","test":"a::b","status":"open"}]"#,
        )
        .unwrap();
        let bugs = load(dir.path());
        assert_eq!(bugs.len(), 1);
        assert_eq!(bugs[0].bug_id, "7");
        assert_eq!(bugs[0].test, "a::b");
    }

    #[test]
    fn load_malformed_registry_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let phr = dir.path().join(".phronesis");
        std::fs::create_dir_all(&phr).unwrap();
        std::fs::write(phr.join("bugs.json"), "{not an array").unwrap();
        assert!(load(dir.path()).is_empty());
    }
}
