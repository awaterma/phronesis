//! cargo build/check/test output → neutral outcome facts.
//!
//! Parsing only — no process execution. The hook hands us the captured output
//! of a `cargo` tool call; we turn it into `build_outcome` / `test_outcome`.
//!
//! Two signals, deliberately separated:
//! - **compilation** — failed iff the compiler emitted an error code
//!   (`error[E…]`) or said it `could not compile`. Warnings do not fail a
//!   build, and a *test* failure (`error: test failed …`) is not a compile
//!   failure — so we key only on compiler-error signals, not a bare `error:`.
//! - **tests** — summed across the per-binary `test result:` lines cargo
//!   prints (one per test binary / doc-test run).

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use crate::outcomes::adapter::OutcomeAdapter;
use crate::outcomes::facts::OutcomeFact;
use crate::outcomes::{bugs, subject as subject_mod};

/// `test result: ok. 12 passed; 0 failed; …` — captures passed and failed.
static TEST_RESULT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"test result: \w+\. (\d+) passed; (\d+) failed")
        .expect("TEST_RESULT regex is valid")
});

/// A compiler error code, e.g. `error[E0425]`. Distinct from a bare `error:`,
/// which cargo also prints for a test *failure*.
static COMPILE_ERROR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"error\[E\d+\]").expect("COMPILE_ERROR regex is valid"));

/// A per-test result line: `test path::name ... ok` / `... FAILED`. Anchored to
/// line start (multiline) so the `test result:` summary line never matches.
static TEST_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^test (\S+) \.\.\. (ok|FAILED)").expect("TEST_LINE regex is valid")
});

/// Per-test results parsed from cargo test output: `(test_name, passed)`. Used
/// by the known-bug registry to detect a specific bug-test going green.
/// `ignored`/`measured` lines are not results and are skipped.
pub fn per_test_results(output: &str) -> Vec<(String, bool)> {
    TEST_LINE
        .captures_iter(output)
        .map(|c| (c[1].to_string(), &c[2] == "ok"))
        .collect()
}

pub struct CargoAdapter;

impl CargoAdapter {
    fn is_test_command(command: &str) -> bool {
        command.contains("cargo test")
            || command.contains("cargo nextest")
            || command.contains("nextest run")
    }

    fn compiled(output: &str) -> bool {
        !(COMPILE_ERROR.is_match(output) || output.contains("could not compile"))
    }
}

/// Sum all `test result:` lines in `output`. Returns `Some((passed, failed))`
/// when at least one result line is present, or `None` when cargo emitted no
/// test results (e.g. compile-only run or no test targets).
fn sum_test_results(output: &str) -> Option<(usize, usize)> {
    TEST_RESULT
        .captures_iter(output)
        .map(|caps| {
            (
                caps[1].parse::<usize>().unwrap_or(0),
                caps[2].parse::<usize>().unwrap_or(0),
            )
        })
        .reduce(|(p1, f1), (p2, f2)| (p1 + p2, f1 + f2))
}

impl OutcomeAdapter for CargoAdapter {
    fn handles(&self, command: &str) -> bool {
        command.contains("cargo build")
            || command.contains("cargo check")
            || command.contains("cargo test")
            || command.contains("cargo nextest")
    }

    fn parse(&self, subject: &str, command: &str, output: &str) -> Vec<OutcomeFact> {
        let compiled = Self::compiled(output);
        let mut facts = vec![OutcomeFact::build(subject, compiled)];
        // Tests only run if the code compiled; a compile failure means there is
        // no test signal at all (only the build_outcome above).
        if Self::is_test_command(command)
            && compiled
            && let Some((passed, failed)) = sum_test_results(output)
        {
            facts.push(OutcomeFact::test(subject, passed, failed));
        }
        facts
    }
}

/// Map a `build_outcome` fact's `pass`/`fail` arg to the outcome tag the
/// journal record stamps.
fn build_tag(fact: &OutcomeFact) -> Option<&'static str> {
    match fact.args.get(1).map(|s| s.as_str()) {
        Some("pass") => Some("outcome:compile_ok"),
        Some("fail") => Some("outcome:compile_error"),
        _ => None,
    }
}

/// Map a `test_outcome` fact (`[subject, passed, failed, total]`) to a tag.
/// `total > 0 && failed == 0` is a pass; otherwise fail.
fn test_tag(fact: &OutcomeFact) -> Option<&'static str> {
    let failed = fact.args.get(2)?.parse::<usize>().ok()?;
    let total = fact.args.get(3)?.parse::<usize>().ok()?;
    if total > 0 && failed == 0 {
        Some("outcome:test_pass")
    } else if total > 0 {
        Some("outcome:test_fail")
    } else {
        None
    }
}

/// Translate a slice of outcome facts into their journal tag strings.
/// Only `build_outcome` and `test_outcome` predicates produce tags; all
/// others are silently skipped.
fn outcome_tags(facts: &[OutcomeFact]) -> Vec<String> {
    facts
        .iter()
        .filter_map(|f| match f.predicate {
            "build_outcome" => build_tag(f),
            "test_outcome" => test_tag(f),
            _ => None,
        })
        .map(|s| s.to_string())
        .collect()
}

/// Emit `outcome:bug_caught:<id>` tags for any known bug whose test went
/// green with no regressions. Returns an empty `Vec` when there are no
/// per-test results or no known bugs.
fn bug_caught_tags(project_root: &Path, subject: &str, output: &str) -> Vec<String> {
    let per_test = per_test_results(output);
    if per_test.is_empty() {
        return Vec::new();
    }
    let known = bugs::load(project_root);
    if known.is_empty() {
        return Vec::new();
    }
    let no_regressions = per_test.iter().all(|(_, passed)| *passed);
    let bug_facts = bugs::check(subject, &known, &per_test, no_regressions);
    bug_facts
        .iter()
        .filter_map(|f| {
            // bug_check_outcome args: [subject, bug_id, status]
            if let (Some(id), Some(status)) = (f.args.get(1), f.args.get(2))
                && status == "fixed"
            {
                Some(format!("outcome:bug_caught:{}", id))
            } else {
                None
            }
        })
        .collect()
}

/// Post-check side of confidence scoring at the journey-fold-in seam:
/// translate a Bash tool call (`command` + `output`) into the outcome tags
/// the journal record stamps, plus the resolved `subject` so per-subject
/// reads still work. Pure with respect to the filesystem *except* for the
/// `subject` lifecycle ops (`subject::open` / `subject::settle`), which the
/// confidence subsystem still owns.
///
/// Behavior matches the pre-fold-in `capture_outcomes`:
/// - `git commit` settles the open subject and emits no tags (the commit's
///   post-check doesn't record an outcome — it concludes a unit).
/// - Non-handled commands (`ls`, etc.) return `(empty, None)`.
/// - Confidence not enabled → `(empty, None)` (opt-in guard preserved).
/// - Handled commands open/reuse a subject, parse the output, return tags
///   from build/test/bug results and the subject so the hook stamps both.
///
/// Returns: `(outcome_tags, subject)`.
pub fn extract_from(
    project_root: &Path,
    tool_name: &str,
    command_opt: Option<&str>,
    output: &str,
) -> (Vec<String>, Option<String>) {
    if !matches!(tool_name, "Bash" | "run_shell_command") {
        return (Vec::new(), None);
    }
    if !crate::outcomes::enabled(project_root) {
        return (Vec::new(), None);
    }
    let Some(command) = command_opt else {
        return (Vec::new(), None);
    };
    if command.contains("git commit") {
        let _ = subject_mod::settle(project_root);
        return (Vec::new(), None);
    }
    if !crate::outcomes::adapter::handles(command) {
        return (Vec::new(), None);
    }
    let subject = match subject_mod::open(project_root) {
        Ok(s) => s,
        Err(_) => return (Vec::new(), None),
    };
    let outcome_facts = crate::outcomes::adapter::extract(&subject, command, output);
    let tags: Vec<String> = outcome_tags(&outcome_facts)
        .into_iter()
        .chain(bug_caught_tags(project_root, &subject, output))
        .collect();
    (tags, Some(subject))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_status(facts: &[OutcomeFact]) -> Option<&str> {
        facts
            .iter()
            .find(|f| f.predicate == "build_outcome")
            .map(|f| f.args[1].as_str())
    }

    fn test_fact(facts: &[OutcomeFact]) -> Option<&OutcomeFact> {
        facts.iter().find(|f| f.predicate == "test_outcome")
    }

    #[test]
    fn handles_recognizes_cargo_build_check_test() {
        let a = CargoAdapter;
        assert!(a.handles("cargo build --workspace"));
        assert!(a.handles("cargo check"));
        assert!(a.handles("cargo test"));
        assert!(a.handles("cargo nextest run"));
        assert!(!a.handles("ls"));
        assert!(!a.handles("git commit -m x"));
    }

    #[test]
    fn successful_build_is_pass() {
        let out = "   Compiling foo v0.1.0\n    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.42s\n";
        let facts = CargoAdapter.parse("u", "cargo build", out);
        assert_eq!(build_status(&facts), Some("pass"));
        assert!(
            test_fact(&facts).is_none(),
            "build command emits no test fact"
        );
    }

    #[test]
    fn warnings_only_build_is_still_pass() {
        let out = "warning: unused variable: `x`\n   --> src/main.rs:2:9\n    Finished dev profile in 0.3s\n";
        let facts = CargoAdapter.parse("u", "cargo build", out);
        assert_eq!(build_status(&facts), Some("pass"));
    }

    #[test]
    fn compile_error_is_fail() {
        let out = "error[E0425]: cannot find value `x` in this scope\n --> src/main.rs:2:5\nerror: could not compile `foo` (bin \"foo\") due to 1 previous error\n";
        let facts = CargoAdapter.parse("u", "cargo build", out);
        assert_eq!(build_status(&facts), Some("fail"));
    }

    #[test]
    fn could_not_compile_without_error_code_is_fail() {
        let out = "error: linking with `cc` failed\nerror: could not compile `foo`\n";
        let facts = CargoAdapter.parse("u", "cargo build", out);
        assert_eq!(build_status(&facts), Some("fail"));
    }

    #[test]
    fn tests_all_pass() {
        let out = "running 12 tests\ntest a ... ok\ntest result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n";
        let facts = CargoAdapter.parse("u", "cargo test", out);
        assert_eq!(build_status(&facts), Some("pass"));
        let t = test_fact(&facts).expect("test_outcome present");
        // [subject, passed, failed, total]
        assert_eq!(t.args, vec!["u", "12", "0", "12"]);
    }

    #[test]
    fn tests_with_failures() {
        let out = "test result: FAILED. 10 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s\nerror: test failed, to rerun pass `--lib`\n";
        let facts = CargoAdapter.parse("u", "cargo test", out);
        // A test failure is NOT a compile failure.
        assert_eq!(build_status(&facts), Some("pass"));
        let t = test_fact(&facts).expect("test_outcome present");
        assert_eq!(t.args, vec!["u", "10", "2", "12"]);
    }

    #[test]
    fn multiple_test_binaries_are_summed() {
        let out = "test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n\
                   test result: ok. 5 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s\n";
        let facts = CargoAdapter.parse("u", "cargo test", out);
        let t = test_fact(&facts).expect("test_outcome present");
        assert_eq!(t.args, vec!["u", "8", "1", "9"]);
    }

    #[test]
    fn per_test_results_parses_names_and_status() {
        let out = "running 3 tests\ntest mod_a::ok_one ... ok\ntest mod_b::fails ... FAILED\ntest mod_c::ignored_one ... ignored\ntest result: FAILED. 1 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out\n";
        let results = per_test_results(out);
        assert_eq!(
            results,
            vec![
                ("mod_a::ok_one".to_string(), true),
                ("mod_b::fails".to_string(), false),
            ],
            "ok/FAILED parsed; ignored and the summary line excluded"
        );
    }

    #[test]
    fn test_command_that_fails_to_compile_has_no_test_fact() {
        let out = "error[E0599]: no method named `frobnicate` found\nerror: could not compile `foo` (test \"it\") due to 1 previous error\n";
        let facts = CargoAdapter.parse("u", "cargo test", out);
        assert_eq!(build_status(&facts), Some("fail"));
        assert!(
            test_fact(&facts).is_none(),
            "no test signal when compilation fails"
        );
    }
}
