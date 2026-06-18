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

use std::sync::LazyLock;

use regex::Regex;

use crate::outcomes::adapter::OutcomeAdapter;
use crate::outcomes::facts::OutcomeFact;

/// `test result: ok. 12 passed; 0 failed; …` — captures passed and failed.
static TEST_RESULT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"test result: \w+\. (\d+) passed; (\d+) failed")
        .expect("TEST_RESULT regex is valid")
});

/// A compiler error code, e.g. `error[E0425]`. Distinct from a bare `error:`,
/// which cargo also prints for a test *failure*.
static COMPILE_ERROR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"error\[E\d+\]").expect("COMPILE_ERROR regex is valid"));

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
        if Self::is_test_command(command) && compiled {
            let mut passed = 0usize;
            let mut failed = 0usize;
            let mut saw_result = false;
            for caps in TEST_RESULT.captures_iter(output) {
                saw_result = true;
                passed += caps[1].parse::<usize>().unwrap_or(0);
                failed += caps[2].parse::<usize>().unwrap_or(0);
            }
            if saw_result {
                facts.push(OutcomeFact::test(subject, passed, failed));
            }
        }
        facts
    }
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
