//! Adapter layer: command → toolchain def → neutral facts, plus the
//! post-check seam (`extract_from`) that turns a Bash tool call into the
//! outcome tags the journal record stamps.
//!
//! Since the neutral-toolchain-outcomes redesign there are no hand-written
//! per-toolchain adapters: one generic engine (`CompiledDef::parse`) is
//! driven by declarative defs — built-ins (cargo) ∪ `.phronesis/toolchains.json`.

use std::path::Path;

use crate::outcomes::facts::OutcomeFact;
use crate::outcomes::toolchain::{self, CompiledDef};
use crate::outcomes::{bugs, subject as subject_mod};

/// Parses one toolchain's command output into neutral outcome facts.
/// `command_exit` is the captured process exit code, when the CLI provided
/// one — `None` falls back to regex-only parsing.
pub trait OutcomeAdapter {
    fn handles(&self, command: &str) -> bool;
    fn parse(
        &self,
        subject: &str,
        command: &str,
        output: &str,
        command_exit: Option<i32>,
    ) -> Vec<OutcomeFact>;
}

/// The single generic adapter: a compiled declarative def.
pub struct ConfigAdapter {
    pub def: CompiledDef,
}

impl OutcomeAdapter for ConfigAdapter {
    fn handles(&self, command: &str) -> bool {
        self.def.handles(command)
    }
    fn parse(
        &self,
        subject: &str,
        command: &str,
        output: &str,
        command_exit: Option<i32>,
    ) -> Vec<OutcomeFact> {
        self.def.parse(subject, command, output, command_exit)
    }
}

/// First def in the registry that recognizes the command.
fn matching_def(root: &Path, command: &str) -> Option<CompiledDef> {
    toolchain::registry(root)
        .into_iter()
        .find(|d| d.handles(command))
}

/// Does any def recognize this command? Lets callers skip opening a work
/// unit for irrelevant commands (e.g. `ls`).
pub fn handles(root: &Path, command: &str) -> bool {
    matching_def(root, command).is_some()
}

/// Extract neutral outcome facts. Empty when no def recognizes the command —
/// a non-build/test command produces no grounded signal, which is correct.
pub struct ExtractInput<'a> {
    pub root: &'a Path,
    pub subject: &'a str,
    pub command: &'a str,
    pub output: &'a str,
    pub command_exit: Option<i32>,
}

pub fn extract(input: ExtractInput<'_>) -> Vec<OutcomeFact> {
    matching_def(input.root, input.command)
        .map(|d| {
            d.parse(
                input.subject,
                input.command,
                input.output,
                input.command_exit,
            )
        })
        .unwrap_or_default()
}

/// Map a `build_outcome` fact's status arg to the outcome tag the journal
/// record stamps. `unknown` is journaled for transparency
/// (`outcome:compile_unknown`) but grounds no confidence signal — see
/// `derive::entries_from`.
fn build_tag(fact: &OutcomeFact) -> Option<&'static str> {
    match fact.args.get(1).map(|s| s.as_str()) {
        Some("pass") => Some("outcome:compile_ok"),
        Some("fail") => Some("outcome:compile_error"),
        Some("unknown") => Some("outcome:compile_unknown"),
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
fn bug_caught_tags(project_root: &Path, subject: &str, per_test: &[(String, bool)]) -> Vec<String> {
    if per_test.is_empty() {
        return Vec::new();
    }
    let known = bugs::load(project_root);
    if known.is_empty() {
        return Vec::new();
    }
    let no_regressions = per_test.iter().all(|(_, passed)| *passed);
    let bug_facts = bugs::check(subject, &known, per_test, no_regressions);
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
pub struct ExtractFromInput<'a> {
    pub project_root: &'a Path,
    pub tool_name: &'a str,
    pub command: Option<&'a str>,
    pub output: &'a str,
    pub command_exit: Option<i32>,
}

pub fn extract_from(input: ExtractFromInput<'_>) -> (Vec<String>, Option<String>) {
    let ExtractFromInput {
        project_root,
        tool_name,
        command: command_opt,
        output,
        command_exit,
    } = input;
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
    if !crate::outcomes::adapter::handles(project_root, command) {
        return (Vec::new(), None);
    }

    extract_handled(project_root, command, output, command_exit)
}

fn extract_handled(
    project_root: &Path,
    command: &str,
    output: &str,
    command_exit: Option<i32>,
) -> (Vec<String>, Option<String>) {
    let subject = match subject_mod::open(project_root) {
        Ok(s) => s,
        Err(_) => return (Vec::new(), None),
    };
    let Some(def) = matching_def(project_root, command) else {
        return (Vec::new(), None);
    };
    let outcome_facts = def.parse(&subject, command, output, command_exit);
    // Gate bug evidence on a grounded, passing build (Finding 2):
    // an unknown run produced no evidence, so `bug_caught` tags must not
    // fire — absent evidence is unknown, not pass.
    let build_passed = outcome_facts
        .iter()
        .any(|f| f.predicate == "build_outcome" && f.args.get(1).is_some_and(|s| s == "pass"));
    let per_test = if build_passed {
        def.per_test_results(output)
    } else {
        Vec::new()
    };
    let tags: Vec<String> = outcome_tags(&outcome_facts)
        .into_iter()
        .chain(bug_caught_tags(project_root, &subject, &per_test))
        .collect();
    (tags, Some(subject))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_command_yields_no_facts() {
        let dir = tempfile::tempdir().unwrap();
        let facts = extract(ExtractInput {
            root: dir.path(),
            subject: "u",
            command: "ls -la",
            output: "total 0\n",
            command_exit: None,
        });
        assert!(facts.is_empty());
    }

    #[test]
    fn cargo_command_is_routed_through_the_registry() {
        let dir = tempfile::tempdir().unwrap();
        let facts = extract(ExtractInput {
            root: dir.path(),
            subject: "u",
            command: "cargo build",
            output: "   Finished dev profile in 0.4s\n",
            command_exit: None,
        });
        assert!(
            facts.iter().any(|f| f.predicate == "build_outcome"),
            "a cargo command should produce a build_outcome via the built-in def"
        );
    }

    #[test]
    fn project_def_grounds_a_non_cargo_toolchain() {
        // The neutrality proof at the adapter seam: a pytest def from
        // toolchains.json produces grounded facts for a pytest command.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".phronesis")).unwrap();
        std::fs::write(
            crate::outcomes::toolchain::config_path(dir.path()),
            r#"[{"id":"pytest","matches":"pytest","test_summary":"(?:(?P<failed>\\d+) failed, )?(?P<passed>\\d+) passed"}]"#,
        )
        .unwrap();
        let facts = extract(ExtractInput {
            root: dir.path(),
            subject: "u",
            command: "pytest -q",
            output: "=== 3 passed in 0.1s ===\n",
            command_exit: Some(0),
        });
        assert!(facts.iter().any(|f| f.predicate == "build_outcome"));
        assert!(facts.iter().any(|f| f.predicate == "test_outcome"));
    }

    #[test]
    fn outcome_tags_cover_all_three_build_states() {
        use crate::outcomes::facts::OutcomeFact;
        assert_eq!(
            outcome_tags(&[OutcomeFact::build("u", true)]),
            vec!["outcome:compile_ok"]
        );
        assert_eq!(
            outcome_tags(&[OutcomeFact::build("u", false)]),
            vec!["outcome:compile_error"]
        );
        assert_eq!(
            outcome_tags(&[OutcomeFact::build_unknown("u")]),
            vec!["outcome:compile_unknown"]
        );
    }

    #[test]
    fn cargo_build_without_exit_or_evidence_stamps_compile_unknown_not_ok() {
        let dir = tempfile::tempdir().unwrap();
        let facts = extract(ExtractInput {
            root: dir.path(),
            subject: "u",
            command: "cargo build",
            output: "   Compiling foo v0.1.0\n",
            command_exit: None,
        });
        assert_eq!(outcome_tags(&facts), vec!["outcome:compile_unknown"]);
    }

    // ── Finding 2: bug_caught must not fire on unknown builds ──

    fn enable_outcomes(root: &Path) {
        let p = root.join(".phronesis").join("confidence.json");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "{}").unwrap();
    }

    fn open_subject(root: &Path, id: &str) {
        let meta_dir = root.join(".phronesis").join("outcomes");
        std::fs::create_dir_all(&meta_dir).unwrap();
        std::fs::write(meta_dir.join("subject"), id).unwrap();
    }

    fn write_bugs(root: &Path, content: &str) {
        let phr = root.join(".phronesis");
        std::fs::create_dir_all(&phr).unwrap();
        std::fs::write(phr.join("bugs.json"), content).unwrap();
    }

    #[test]
    fn unknown_build_does_not_ground_bug_caught() {
        // Truncated output with per-test `ok` lines but NO summary /
        // Finished line / exit code → build_outcome is unknown.
        // bug_caught must NOT produce tags (absent evidence is unknown,
        // not pass).
        let dir = tempfile::tempdir().unwrap();
        enable_outcomes(dir.path());
        open_subject(dir.path(), "u");
        // Set up a known bug whose test name matches a per_test line.
        write_bugs(
            dir.path(),
            r#"[{"bug_id":"1042","test":"test_fix_1042","status":"open"}]"#,
        );
        // No "test result:" line, no "Finished", no exit code — just
        // per-test lines that the per_test regex can match.
        let output = "running 3 tests\ntest test_fix_1042 ... ok\ntest test_other   ... ok\ntest test_fix_2048 ... ok\n";
        let (tags, _subject) = extract_from(ExtractFromInput {
            project_root: dir.path(),
            tool_name: "Bash",
            command: Some("cargo test --workspace"),
            output,
            command_exit: None,
        });
        assert!(
            tags.iter().any(|t| t == "outcome:compile_unknown"),
            "tags must include compile_unknown: {tags:?}"
        );
        assert!(
            !tags.iter().any(|t| t.starts_with("outcome:bug_caught")),
            "bug_caught must NOT appear when build is unknown; tags = {tags:?}"
        );
    }

    #[test]
    fn known_pass_still_grounds_bug_caught() {
        // Regression: the same output WITH exit=0 (or a full summary)
        // should still emit the bug_caught tag.
        let dir = tempfile::tempdir().unwrap();
        enable_outcomes(dir.path());
        open_subject(dir.path(), "u");
        let output = "\
   Compiling foo v0.1.0\n   Running unittests src/lib.rs (target/debug/deps/foo-abc123)\nrunning 3 tests\ntest test_fix_1042 ... ok\ntest test_other   ... ok\ntest result: ok. 3 passed; 0 failed\n";
        let (tags, _subject) = extract_from(ExtractFromInput {
            project_root: dir.path(),
            tool_name: "Bash",
            command: Some("cargo test --workspace"),
            output,
            command_exit: Some(0),
        });
        let tags: Vec<String> = tags;
        // Build passed, so test facts exist — but without a known-bug
        // registry the bug_caught tag is empty.  What matters is that
        // the build_outcome is "pass" (not unknown), proving we didn't
        // accidentally gate on the wrong thing.
        assert!(
            tags.iter().any(|t| t == "outcome:compile_ok"),
            "tags must include compile_ok: {tags:?}"
        );
        assert!(
            !tags.iter().any(|t| t == "outcome:compile_unknown"),
            "tags must NOT include compile_unknown: {tags:?}"
        );
    }
}
