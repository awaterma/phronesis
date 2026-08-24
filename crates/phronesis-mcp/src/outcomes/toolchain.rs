//! Declarative toolchain definitions — the data that replaces hand-written
//! adapters (design: docs/superpowers/specs/2026-07-10-neutral-toolchain-outcomes-design.md).
//!
//! A `ToolchainDef` says how to *recognize* a build/test command (`matches`)
//! and, optionally, how to refine the exit-code signal: `compile_fail`
//! patterns force a build failure, `compile_success` patterns provide
//! explicit success evidence when no exit code was captured, `test_summary`
//! extracts passed/failed counts via named groups
//! `(?P<passed>)`/`(?P<failed>)`, and `per_test` extracts per-test results
//! via `(?P<name>)`/`(?P<status>)` + `pass_tokens`. Cargo ships as a
//! built-in def and runs through the exact same engine. Build outcomes are
//! three-state (pass / fail / unknown): with no exit code and no textual
//! evidence the outcome is `unknown`, never a silent pass.
//!
//! Loading is fail-open: a malformed file or entry warns on stderr and is
//! skipped — a bad toolchain def must never break the hook.

use std::path::{Path, PathBuf};

use regex::Regex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::outcomes::facts::OutcomeFact;

fn default_pass_tokens() -> Vec<String> {
    vec!["ok".to_string(), "PASSED".to_string(), "passed".to_string()]
}

/// One entry of `.phronesis/toolchains.json` (also the shape of built-ins).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolchainDef {
    /// Stable name, for diagnostics / `phr-mcp toolchains`.
    pub id: String,
    /// Regex tested against the command string — recognition only, it does
    /// not decide pass/fail. (A plain substring is a valid regex.)
    pub matches: String,
    /// Any match in the output forces `build_outcome = fail` regardless of
    /// exit code (recovers the compile-vs-test split).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compile_fail: Vec<String>,
    /// Explicit compile-success evidence: any match in the output proves the
    /// build ran and succeeded when no exit code was captured. Only consulted
    /// when `command_exit` is absent — a present exit code (plus the
    /// test-summary split) stays authoritative. Optional and backward
    /// compatible: defs written before 0.19.0 deserialize with an empty list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compile_success: Vec<String>,
    /// Regex(es) with named groups `(?P<passed>\d+)` (required) and optional
    /// `(?P<failed>\d+)`.
    ///
    /// A single tool can emit more than one summary format — `cargo test` and
    /// `cargo nextest run` are the same toolchain but share no summary line —
    /// so this accepts a list. The first pattern that matches anything wins,
    /// and all of *its* matches are summed (a multi-binary `cargo test` run
    /// emits one summary per binary). Patterns are never mixed, so output
    /// carrying two shapes cannot double-count.
    ///
    /// Deserializes from either a bare string or an array, so defs written
    /// against the single-pattern shape keep working unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_summary: Option<Patterns>,
    /// Regex with named groups `(?P<name>)` and `(?P<status>)`; feeds the
    /// known-bug registry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_test: Option<String>,
    /// Which `status` tokens mean "pass".
    #[serde(default = "default_pass_tokens")]
    pub pass_tokens: Vec<String>,
}

/// One regex or several, in a field that historically held exactly one.
///
/// Untagged so a bare JSON string still deserializes, and so a def that was
/// written with one pattern serializes back out as a string rather than
/// silently becoming an array.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Patterns {
    One(String),
    Many(Vec<String>),
}

impl Patterns {
    pub fn as_slice(&self) -> &[String] {
        match self {
            Self::One(pattern) => std::slice::from_ref(pattern),
            Self::Many(patterns) => patterns,
        }
    }
}

impl From<&str> for Patterns {
    fn from(pattern: &str) -> Self {
        Self::One(pattern.to_string())
    }
}

/// Where a compiled def came from — surfaced by `phr-mcp toolchains`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefSource {
    BuiltIn,
    Project,
    /// A project def whose `id` shadows a built-in.
    Override,
}

impl DefSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            DefSource::BuiltIn => "built-in",
            DefSource::Project => "project",
            DefSource::Override => "override",
        }
    }
}

#[derive(Debug, Error)]
pub enum ToolchainError {
    #[error("toolchain `{id}`: invalid regex in `{field}`: {source}")]
    BadRegex {
        id: String,
        field: &'static str,
        #[source]
        source: regex::Error,
    },
    #[error("toolchain `{id}`: `{field}` is missing required named group(s) {groups}")]
    MissingGroup {
        id: String,
        field: &'static str,
        groups: &'static str,
    },
}

/// A `ToolchainDef` with its regexes compiled and validated.
#[derive(Debug, Clone)]
pub struct CompiledDef {
    pub def: ToolchainDef,
    pub source: DefSource,
    matches: Regex,
    compile_fail: Vec<Regex>,
    compile_success: Vec<Regex>,
    /// Alternative summary formats, in declaration order. Empty when the def
    /// declares none.
    test_summary: Vec<Regex>,
    per_test: Option<Regex>,
}

fn compile_field(id: &str, field: &'static str, pattern: &str) -> Result<Regex, ToolchainError> {
    Regex::new(pattern).map_err(|source| ToolchainError::BadRegex {
        id: id.to_string(),
        field,
        source,
    })
}

struct RequiredGroups<'a> {
    id: &'a str,
    field: &'static str,
    regex: &'a Regex,
    groups: &'a [&'a str],
    label: &'static str,
}

fn require_groups(required: RequiredGroups<'_>) -> Result<(), ToolchainError> {
    let names: Vec<&str> = required.regex.capture_names().flatten().collect();
    if required.groups.iter().all(|g| names.contains(g)) {
        Ok(())
    } else {
        Err(ToolchainError::MissingGroup {
            id: required.id.to_string(),
            field: required.field,
            groups: required.label,
        })
    }
}

impl CompiledDef {
    pub fn compile(def: ToolchainDef, source: DefSource) -> Result<Self, ToolchainError> {
        let matches = compile_field(&def.id, "matches", &def.matches)?;
        let compile_fail = def
            .compile_fail
            .iter()
            .map(|p| compile_field(&def.id, "compile_fail", p))
            .collect::<Result<Vec<_>, _>>()?;
        let compile_success = def
            .compile_success
            .iter()
            .map(|p| compile_field(&def.id, "compile_success", p))
            .collect::<Result<Vec<_>, _>>()?;
        let test_summary = def
            .test_summary
            .iter()
            .flat_map(Patterns::as_slice)
            .map(|p| {
                let re = compile_field(&def.id, "test_summary", p)?;
                require_groups(RequiredGroups {
                    id: &def.id,
                    field: "test_summary",
                    regex: &re,
                    groups: &["passed"],
                    label: "(?P<passed>)",
                })?;
                Ok::<_, ToolchainError>(re)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let per_test = def
            .per_test
            .as_deref()
            .map(|p| {
                let re = compile_field(&def.id, "per_test", p)?;
                require_groups(RequiredGroups {
                    id: &def.id,
                    field: "per_test",
                    regex: &re,
                    groups: &["name", "status"],
                    label: "(?P<name>) and (?P<status>)",
                })?;
                Ok::<_, ToolchainError>(re)
            })
            .transpose()?;
        Ok(Self {
            def,
            source,
            matches,
            compile_fail,
            compile_success,
            test_summary,
            per_test,
        })
    }

    /// Does this def recognize the command as a build/test invocation?
    ///
    /// The command line is first split into command segments (on `&&`, `||`,
    /// `;`, `|`, and newlines) with leading `NAME=value` assignments and
    /// `env` prefixes stripped and comment segments dropped — see
    /// `outcomes::segment`. The `matches` regex then runs against each
    /// segment, so a head-anchored pattern (`^cargo\s+…`) recognizes
    /// `cd repo && cargo test` but not `echo cargo test`. Unanchored project
    /// patterns keep their old within-text behavior, scoped per segment.
    pub fn handles(&self, command: &str) -> bool {
        crate::outcomes::segment::command_heads(command)
            .iter()
            .any(|seg| self.matches.is_match(seg))
    }
}

/// Summed test counts extracted from `test_summary` matches.
/// Three-state build outcome (evidence-integrity spec, Task 4). `Unknown`
/// means "no exit code, no failure match, no explicit success evidence" —
/// recorded as `build_outcome(subject, "unknown")`, journaled as
/// `outcome:compile_unknown`, and never a confidence signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BuildStatus {
    Pass,
    Fail,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TestCounts {
    passed: usize,
    failed: usize,
}

impl CompiledDef {
    fn compile_fail_hit(&self, output: &str) -> bool {
        self.compile_fail.iter().any(|re| re.is_match(output))
    }

    fn compile_success_hit(&self, output: &str) -> bool {
        self.compile_success.iter().any(|re| re.is_match(output))
    }

    /// Test counts from the first `test_summary` pattern that matches.
    ///
    /// Within that pattern every match is summed, which is what makes a
    /// multi-binary `cargo test` run add up. Across patterns the first winner
    /// stops the search: two patterns describe two output formats of the same
    /// tool, so matching both would double-count rather than accumulate.
    ///
    /// `None` when the def has no summary regex or none of them matched.
    fn test_counts(&self, output: &str) -> Option<TestCounts> {
        self.test_summary.iter().find_map(|re| {
            let mut found = false;
            let mut counts = TestCounts {
                passed: 0,
                failed: 0,
            };
            for caps in re.captures_iter(output) {
                found = true;
                counts.passed += caps
                    .name("passed")
                    .and_then(|m| m.as_str().parse::<usize>().ok())
                    .unwrap_or(0);
                // A clean nextest run omits the `failed` token entirely, so an
                // absent group means zero, not a parse failure.
                counts.failed += caps
                    .name("failed")
                    .and_then(|m| m.as_str().parse::<usize>().ok())
                    .unwrap_or(0);
            }
            found.then_some(counts)
        })
    }

    /// Per-test results `(name, passed)` for the known-bug registry. Empty
    /// when the def has no `per_test` regex.
    pub fn per_test_results(&self, output: &str) -> Vec<(String, bool)> {
        let Some(re) = &self.per_test else {
            return Vec::new();
        };
        re.captures_iter(output)
            .filter_map(|c| {
                let name = c.name("name")?.as_str().to_string();
                let status = c.name("status")?.as_str();
                let passed = self.def.pass_tokens.iter().any(|t| t == status);
                Some((name, passed))
            })
            .collect()
    }

    /// The graded model (evidence-integrity spec, Task 4):
    ///
    /// | exit    | compile_fail | success evidence | build outcome |
    /// |---------|--------------|------------------|---------------|
    /// | 0       | no           | any              | pass          |
    /// | nonzero | any          | any              | fail — except a test summary attributes the failure to tests (compiled, tests failed) |
    /// | absent  | yes          | any              | fail          |
    /// | absent  | no           | yes              | pass          |
    /// | absent  | no           | no               | unknown       |
    ///
    /// "Success evidence" is a `compile_success` match or a `test_summary`
    /// match (tests that ran prove the code compiled). Absence of failure
    /// evidence is *not* success: with no exit code and no textual evidence
    /// the outcome is `unknown`, which grounds no confidence signal. A test
    /// fact is emitted only when the build passed and a summary matched.
    pub fn parse(
        &self,
        subject: &str,
        _command: &str,
        output: &str,
        command_exit: Option<i32>,
    ) -> Vec<OutcomeFact> {
        let counts = self.test_counts(output);
        let status = if self.compile_fail_hit(output) {
            BuildStatus::Fail
        } else {
            match command_exit {
                Some(0) => BuildStatus::Pass,
                Some(_) if counts.is_some() => BuildStatus::Pass,
                Some(_) => BuildStatus::Fail,
                None if counts.is_some() || self.compile_success_hit(output) => BuildStatus::Pass,
                None => BuildStatus::Unknown,
            }
        };
        let mut facts = vec![match status {
            BuildStatus::Pass => OutcomeFact::build(subject, true),
            BuildStatus::Fail => OutcomeFact::build(subject, false),
            BuildStatus::Unknown => OutcomeFact::build_unknown(subject),
        }];
        if status == BuildStatus::Pass
            && let Some(c) = counts
        {
            facts.push(OutcomeFact::test(subject, c.passed, c.failed));
        }
        facts
    }
}

/// XCTest summary lines, shared by `xcodebuild` and `swift test`:
///
/// ```text
/// Test Suite 'All tests' passed at 2026-08-22 10:00:01.000.
///      Executed 55 tests, with 0 failures (0 unexpected) in 1.201 (1.210) seconds
/// ```
///
/// `Executed N tests` counts tests *run*, not tests passed, and xcodebuild
/// prints one line per suite plus the `All tests` aggregate, so the summed
/// `passed` is an over-count. The confidence signal only asks "did at least
/// one test run and did none fail?", which the `failed` group answers
/// exactly; the count is diagnostic.
const XCTEST_SUMMARY: &str = r"Executed (?P<passed>\d+) tests?, with (?P<failed>\d+) failures?";

/// Swift Testing's run summary (`swift test` on Swift 6+, also under
/// xcodebuild when a target mixes frameworks):
///
/// ```text
/// ✔ Test run with 12 tests in 2 suites passed after 0.123 seconds.
/// ✘ Test run with 12 tests in 2 suites failed after 0.123 seconds with 2 issues.
/// ```
///
/// A failing run is the only branch that binds `failed`; a passing run leaves
/// it absent, which the engine reads as zero.
const SWIFT_TESTING_SUMMARY: &str = r"Test run with (?P<passed>\d+) tests?(?: in \d+ suites?)? (?:passed after [^\n]*|failed after [^\n]* with (?P<failed>\d+) issues?)";

/// Swift compiler diagnostics carry `file:line:col: error:`; XCTest
/// assertion failures carry `file:line: error:` (no column). Requiring the
/// column keeps a red test from being misread as a broken build.
const SWIFT_COMPILE_ERROR: &str = r"\.swift:\d+:\d+: error:";

/// The bundled defs: cargo, xcodebuild, and `swift build|test`. pytest/tsc
/// examples ship via `phr-mcp init` as project defs so the built-in surface
/// stays small. The cargo def must keep the retired `CargoAdapter`'s exact
/// semantics; the fidelity tests below are the acceptance bar.
pub fn builtin_defs() -> Vec<ToolchainDef> {
    let swift_summaries = || {
        Some(Patterns::Many(vec![
            XCTEST_SUMMARY.to_string(),
            SWIFT_TESTING_SUMMARY.to_string(),
        ]))
    };
    vec![
        cargo_builtin(),
        ToolchainDef {
            id: "xcodebuild".to_string(),
            matches: r"^xcodebuild(\s|$)".to_string(),
            compile_fail: vec![
                SWIFT_COMPILE_ERROR.to_string(),
                r"\*\* BUILD FAILED \*\*".to_string(),
            ],
            compile_success: vec![
                r"\*\* BUILD SUCCEEDED \*\*".to_string(),
                r"\*\* TEST SUCCEEDED \*\*".to_string(),
            ],
            test_summary: swift_summaries(),
            per_test: None,
            pass_tokens: default_pass_tokens(),
        },
        ToolchainDef {
            id: "swift".to_string(),
            matches: r"^swift\s+(build|test)\b".to_string(),
            compile_fail: vec![SWIFT_COMPILE_ERROR.to_string()],
            compile_success: vec![r"(?m)^Build complete!".to_string()],
            test_summary: swift_summaries(),
            per_test: None,
            pass_tokens: default_pass_tokens(),
        },
    ]
}

fn cargo_builtin() -> ToolchainDef {
    ToolchainDef {
        id: "cargo".to_string(),
        matches: r"^cargo\s+(build|check|test|nextest)\b".to_string(),
        compile_fail: vec![
            r"error\[E\d+\]".to_string(),
            "could not compile".to_string(),
        ],
        compile_success: vec![r"Finished .* profile".to_string()],
        // `matches` accepts `cargo nextest`, and nextest never emits libtest's
        // `test result:` line — it emits `Summary [...] N tests run: ...`.
        // With only the libtest pattern the def claimed the command, parsed
        // nothing, and grounded no test signal, so a project gated on
        // `cargo nextest run` could never reach one.
        test_summary: Some(Patterns::Many(vec![
            r"test result: \w+\. (?P<passed>\d+) passed; (?P<failed>\d+) failed".to_string(),
            // `failed` is optional: a clean run reports only passed/skipped,
            // and `[^,\n]*` steps over annotations like ` (1 flaky)`.
            r"Summary \[[^\]]*\]\s+\d+ tests run: (?P<passed>\d+) passed[^,\n]*(?:, (?P<failed>\d+) failed)?"
                .to_string(),
        ])),
        per_test: Some(r"(?m)^test (?P<name>\S+) \.\.\. (?P<status>ok|FAILED)".to_string()),
        pass_tokens: default_pass_tokens(),
    }
}

pub fn config_path(root: &Path) -> PathBuf {
    root.join(".phronesis").join("toolchains.json")
}

/// Project defs from `.phronesis/toolchains.json`. Fail-open at every level:
/// missing file → empty; unparseable file → stderr warning + empty; a
/// malformed entry (bad JSON shape, bad regex, missing named group) → stderr
/// warning, entry skipped.
pub fn load_project_defs(root: &Path) -> Vec<ToolchainDef> {
    let path = config_path(root);
    let raw = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            eprintln!("phronesis: toolchains.json unreadable, skipped: {e}");
            return Vec::new();
        }
    };
    let entries: Vec<serde_json::Value> = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("phronesis: toolchains.json is not a JSON array, skipped: {e}");
            return Vec::new();
        }
    };
    entries
        .into_iter()
        .filter_map(|v| match serde_json::from_value::<ToolchainDef>(v) {
            Ok(def) => {
                // Validate regexes now so the registry never carries a dud.
                match CompiledDef::compile(def.clone(), DefSource::Project) {
                    Ok(_) => Some(def),
                    Err(e) => {
                        eprintln!("phronesis: toolchain def skipped: {e}");
                        None
                    }
                }
            }
            Err(e) => {
                eprintln!("phronesis: toolchain entry skipped: {e}");
                None
            }
        })
        .collect()
}

/// Built-in defs ∪ project defs, compiled. A project def sharing a built-in's
/// `id` replaces it in place (source = `Override`) — lets a project retune
/// cargo parsing without a release. Assembled per hook invocation (cheap).
pub fn registry(root: &Path) -> Vec<CompiledDef> {
    let project = load_project_defs(root);
    let builtins = builtin_defs();
    let mut out: Vec<CompiledDef> = Vec::new();
    for def in &builtins {
        if let Some(over) = project.iter().find(|p| p.id == def.id) {
            if let Ok(c) = CompiledDef::compile(over.clone(), DefSource::Override) {
                out.push(c);
            }
            continue;
        }
        if let Ok(c) = CompiledDef::compile(def.clone(), DefSource::BuiltIn) {
            out.push(c);
        }
    }
    for def in project {
        if builtins.iter().any(|b| b.id == def.id) {
            continue; // already placed as Override
        }
        if let Ok(c) = CompiledDef::compile(def, DefSource::Project) {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_def_deserializes_with_defaults() {
        let d: ToolchainDef = serde_json::from_str(r#"{"id":"make","matches":"^make "}"#).unwrap();
        assert_eq!(d.id, "make");
        assert!(d.compile_fail.is_empty());
        assert!(d.test_summary.is_none());
        assert!(d.per_test.is_none());
        assert_eq!(d.pass_tokens, vec!["ok", "PASSED", "passed"]);
    }

    #[test]
    fn full_def_deserializes() {
        let d: ToolchainDef = serde_json::from_str(
            r#"{
                "id": "pytest",
                "matches": "pytest",
                "compile_fail": ["SyntaxError", "ImportError"],
                "test_summary": "(?:(?P<failed>\\d+) failed, )?(?P<passed>\\d+) passed",
                "per_test": "(?m)^(?P<name>\\S+) (?P<status>PASSED|FAILED)",
                "pass_tokens": ["PASSED"]
            }"#,
        )
        .unwrap();
        assert_eq!(d.compile_fail.len(), 2);
        assert_eq!(d.pass_tokens, vec!["PASSED"]);
    }

    #[test]
    fn compile_rejects_bad_matches_regex() {
        let d: ToolchainDef = serde_json::from_str(r#"{"id":"x","matches":"["}"#).unwrap();
        assert!(CompiledDef::compile(d, DefSource::Project).is_err());
    }

    #[test]
    fn compile_rejects_test_summary_without_passed_group() {
        let d: ToolchainDef =
            serde_json::from_str(r#"{"id":"x","matches":"x","test_summary":"(\\d+) passed"}"#)
                .unwrap();
        assert!(
            CompiledDef::compile(d, DefSource::Project).is_err(),
            "test_summary must carry a (?P<passed>) named group"
        );
    }

    #[test]
    fn compile_rejects_per_test_without_name_and_status_groups() {
        let d: ToolchainDef =
            serde_json::from_str(r#"{"id":"x","matches":"x","per_test":"^test (\\S+)"}"#).unwrap();
        assert!(CompiledDef::compile(d, DefSource::Project).is_err());
    }

    #[test]
    fn builtin_cargo_def_compiles_and_recognizes_cargo_commands() {
        let defs = builtin_defs();
        assert_eq!(defs.len(), 3, "cargo, xcodebuild, swift");
        let cargo = CompiledDef::compile(defs.into_iter().next().unwrap(), DefSource::BuiltIn)
            .expect("built-in cargo def must compile");
        assert!(cargo.handles("cargo build --workspace"));
        assert!(cargo.handles("cargo check"));
        assert!(cargo.handles("cargo test"));
        assert!(cargo.handles("cargo nextest run"));
        assert!(!cargo.handles("ls"));
        assert!(!cargo.handles("git commit -m x"));
    }

    #[test]
    fn load_project_defs_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_project_defs(dir.path()).is_empty());
    }

    #[test]
    fn load_project_defs_skips_malformed_entry_keeps_good_one() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".phronesis")).unwrap();
        std::fs::write(
            config_path(dir.path()),
            r#"[{"id":"good","matches":"pytest"},{"matches":"missing id"},{"id":"bad","matches":"["}]"#,
        )
        .unwrap();
        let defs = load_project_defs(dir.path());
        assert_eq!(defs.len(), 1, "malformed entries skipped fail-open");
        assert_eq!(defs[0].id, "good");
    }

    #[test]
    fn load_project_defs_whole_file_malformed_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".phronesis")).unwrap();
        std::fs::write(config_path(dir.path()), "not json").unwrap();
        assert!(load_project_defs(dir.path()).is_empty());
    }

    #[test]
    fn registry_appends_project_defs_after_builtins() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".phronesis")).unwrap();
        std::fs::write(
            config_path(dir.path()),
            r#"[{"id":"pytest","matches":"pytest"}]"#,
        )
        .unwrap();
        let reg = registry(dir.path());
        assert_eq!(reg.len(), builtin_defs().len() + 1);
        assert_eq!(reg[0].def.id, "cargo");
        assert_eq!(reg[0].source, DefSource::BuiltIn);
        assert_eq!(reg.last().unwrap().def.id, "pytest");
        assert_eq!(reg.last().unwrap().source, DefSource::Project);
    }

    #[test]
    fn registry_project_def_with_builtin_id_overrides_in_place() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".phronesis")).unwrap();
        std::fs::write(
            config_path(dir.path()),
            r#"[{"id":"cargo","matches":"^cargo-custom "}]"#,
        )
        .unwrap();
        let reg = registry(dir.path());
        assert_eq!(
            reg.len(),
            builtin_defs().len(),
            "override replaces, not appends"
        );
        assert_eq!(reg[0].source, DefSource::Override);
        assert!(reg[0].handles("cargo-custom build"));
        assert!(!reg[0].handles("cargo build"));
    }

    #[test]
    fn registry_with_no_project_file_is_builtins_only() {
        let dir = tempfile::tempdir().unwrap();
        let reg = registry(dir.path());
        assert_eq!(reg.len(), builtin_defs().len());
        assert_eq!(reg[0].def.id, "cargo");
    }

    // ── generic parse engine (synthetic def) ──────────────────────────────

    fn synthetic() -> CompiledDef {
        let def: ToolchainDef = serde_json::from_str(
            r#"{
                "id": "pytest",
                "matches": "pytest",
                "compile_fail": ["SyntaxError", "ImportError"],
                "test_summary": "(?:(?P<failed>\\d+) failed, )?(?P<passed>\\d+) passed",
                "per_test": "(?m)^(?P<name>\\S+) (?P<status>PASSED|FAILED)",
                "pass_tokens": ["PASSED"]
            }"#,
        )
        .unwrap();
        CompiledDef::compile(def, DefSource::Project).unwrap()
    }

    fn exit_only() -> CompiledDef {
        let def: ToolchainDef =
            serde_json::from_str(r#"{"id":"make","matches":"^make( |$)"}"#).unwrap();
        CompiledDef::compile(def, DefSource::Project).unwrap()
    }

    fn build_status(facts: &[crate::outcomes::facts::OutcomeFact]) -> Option<&str> {
        facts
            .iter()
            .find(|f| f.predicate == "build_outcome")
            .map(|f| f.args[1].as_str())
    }

    fn test_fact(
        facts: &[crate::outcomes::facts::OutcomeFact],
    ) -> Option<&crate::outcomes::facts::OutcomeFact> {
        facts.iter().find(|f| f.predicate == "test_outcome")
    }

    #[test]
    fn tier1_zero_exit_is_build_pass_with_no_config() {
        let facts = exit_only().parse("u", "make all", "cc -o main main.c\n", Some(0));
        assert_eq!(build_status(&facts), Some("pass"));
        assert!(test_fact(&facts).is_none());
    }

    #[test]
    fn tier1_nonzero_exit_is_build_fail_with_no_config() {
        let facts = exit_only().parse("u", "make all", "main.c:3: error\n", Some(2));
        assert_eq!(build_status(&facts), Some("fail"));
    }

    #[test]
    fn compile_fail_overrides_zero_exit() {
        // Spec table row 3: cargo-style "linker failed on exit 0" semantics.
        let facts = synthetic().parse("u", "pytest", "ImportError: no module named x\n", Some(0));
        assert_eq!(build_status(&facts), Some("fail"));
        assert!(
            test_fact(&facts).is_none(),
            "no test signal on a compile failure"
        );
    }

    #[test]
    fn nonzero_exit_with_test_summary_is_test_failure_not_build_failure() {
        // Spec's "pytest-exit-1" subtlety: the one place exit is not
        // authoritative for build.
        let out = "test_a.py::a PASSED\ntest_b.py::b FAILED\n=== 2 failed, 10 passed in 0.5s ===\n";
        let facts = synthetic().parse("u", "pytest", out, Some(1));
        assert_eq!(build_status(&facts), Some("pass"));
        let t = test_fact(&facts).expect("test_outcome present");
        assert_eq!(t.args, vec!["u", "10", "2", "12"]);
    }

    #[test]
    fn zero_exit_with_summary_reports_counts() {
        let facts = synthetic().parse("u", "pytest", "=== 12 passed in 0.2s ===\n", Some(0));
        assert_eq!(build_status(&facts), Some("pass"));
        let t = test_fact(&facts).expect("test_outcome present");
        assert_eq!(
            t.args,
            vec!["u", "12", "0", "12"],
            "absent failed group counts as 0"
        );
    }

    #[test]
    fn summary_matches_are_summed_across_output() {
        let out = "=== 3 passed in 0.1s ===\n=== 1 failed, 5 passed in 0.2s ===\n";
        let facts = synthetic().parse("u", "pytest", out, Some(1));
        let t = test_fact(&facts).expect("test_outcome present");
        assert_eq!(t.args, vec!["u", "8", "1", "9"]);
    }

    #[test]
    fn alternation_summary_regex_captures_both_counts() {
        // The init-scaffolded pytest def uses an alternation regex
        // (`N failed|M passed`). Each branch matches separately under
        // `captures_iter`, so "1 failed, 10 passed" yields two matches whose
        // counts sum correctly — neither side is dropped.
        let def: ToolchainDef = serde_json::from_str(
            r#"{
                "id": "pytest",
                "matches": "pytest",
                "test_summary": "(?P<failed>\\d+) failed|(?P<passed>\\d+) passed"
            }"#,
        )
        .unwrap();
        let compiled = CompiledDef::compile(def, DefSource::Project).unwrap();
        let out = "=========== 1 failed, 10 passed in 0.52s ===========\n";
        let facts = compiled.parse("u", "pytest", out, Some(1));
        let t = test_fact(&facts).expect("test_outcome present");
        assert_eq!(
            t.args,
            vec!["u", "10", "1", "11"],
            "alternation branches must both be captured and summed"
        );
    }

    #[test]
    fn no_exit_with_test_summary_is_compiled_with_test_result() {
        // Spec Task 4: a valid test summary is explicit success evidence —
        // tests that ran prove the code compiled.
        let facts = synthetic().parse("u", "pytest", "=== 4 passed in 0.1s ===\n", None);
        assert_eq!(build_status(&facts), Some("pass"));
        let t = test_fact(&facts).expect("test_outcome present");
        assert_eq!(t.args, vec!["u", "4", "0", "4"]);
    }

    #[test]
    fn per_test_results_uses_named_groups_and_pass_tokens() {
        let out = "tests/test_x.py::one PASSED\ntests/test_x.py::two FAILED\n";
        let results = synthetic().per_test_results(out);
        assert_eq!(
            results,
            vec![
                ("tests/test_x.py::one".to_string(), true),
                ("tests/test_x.py::two".to_string(), false),
            ]
        );
    }

    #[test]
    fn per_test_results_empty_without_per_test_regex() {
        assert!(exit_only().per_test_results("anything").is_empty());
    }

    // ── cargo fidelity: the retired CargoAdapter test suite, verbatim ─────

    fn cargo_def() -> CompiledDef {
        let def = builtin_defs().into_iter().next().unwrap();
        CompiledDef::compile(def, DefSource::BuiltIn).unwrap()
    }

    #[test]
    fn cargo_successful_build_is_pass() {
        let out = "   Compiling foo v0.1.0\n    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.42s\n";
        let facts = cargo_def().parse("u", "cargo build", out, None);
        assert_eq!(build_status(&facts), Some("pass"));
        assert!(
            test_fact(&facts).is_none(),
            "build command emits no test fact"
        );
    }

    #[test]
    fn cargo_warnings_only_build_is_still_pass() {
        let out = "warning: unused variable: `x`\n   --> src/main.rs:2:9\n    Finished dev profile in 0.3s\n";
        let facts = cargo_def().parse("u", "cargo build", out, None);
        assert_eq!(build_status(&facts), Some("pass"));
    }

    #[test]
    fn cargo_compile_error_is_fail() {
        let out = "error[E0425]: cannot find value `x` in this scope\n --> src/main.rs:2:5\nerror: could not compile `foo` (bin \"foo\") due to 1 previous error\n";
        let facts = cargo_def().parse("u", "cargo build", out, None);
        assert_eq!(build_status(&facts), Some("fail"));
    }

    #[test]
    fn cargo_could_not_compile_without_error_code_is_fail() {
        let out = "error: linking with `cc` failed\nerror: could not compile `foo`\n";
        let facts = cargo_def().parse("u", "cargo build", out, None);
        assert_eq!(build_status(&facts), Some("fail"));
    }

    #[test]
    fn cargo_tests_all_pass() {
        let out = "running 12 tests\ntest a ... ok\ntest result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n";
        let facts = cargo_def().parse("u", "cargo test", out, None);
        assert_eq!(build_status(&facts), Some("pass"));
        let t = test_fact(&facts).expect("test_outcome present");
        assert_eq!(t.args, vec!["u", "12", "0", "12"]);
    }

    #[test]
    fn cargo_tests_with_failures() {
        let out = "test result: FAILED. 10 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s\nerror: test failed, to rerun pass `--lib`\n";
        let facts = cargo_def().parse("u", "cargo test", out, None);
        // A test failure is NOT a compile failure.
        assert_eq!(build_status(&facts), Some("pass"));
        let t = test_fact(&facts).expect("test_outcome present");
        assert_eq!(t.args, vec!["u", "10", "2", "12"]);
    }

    #[test]
    fn cargo_multiple_test_binaries_are_summed() {
        let out = "test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n\
                   test result: ok. 5 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s\n";
        let facts = cargo_def().parse("u", "cargo test", out, None);
        let t = test_fact(&facts).expect("test_outcome present");
        assert_eq!(t.args, vec!["u", "8", "1", "9"]);
    }

    // ── cargo-nextest ───────────────────────────────────────────────────
    //
    // The `matches` pattern accepts `cargo nextest`, so the def claims these
    // commands. nextest never emits libtest's `test result:` line, so a
    // libtest-only summary pattern silently yields no test signal at all —
    // and a project whose gate is `cargo nextest run` can never ground one.

    #[test]
    fn nextest_all_pass_grounds_a_test_signal() {
        let out = "    Starting 234 tests across 21 binaries\n\
                   Summary [  61.425s] 234 tests run: 234 passed, 9 skipped\n";
        let facts = cargo_def().parse("u", "cargo nextest run", out, None);
        let t = test_fact(&facts).expect("nextest output must ground a test_outcome");
        assert_eq!(t.args, vec!["u", "234", "0", "234"]);
    }

    #[test]
    fn nextest_with_failures_counts_them() {
        // On a failing run nextest emits a `failed` token; on a clean run it
        // does not. The pattern has to treat it as optional without letting a
        // clean run report a bogus count.
        let out = "Summary [   2.439s]   2 tests run: 0 passed, 2 failed, 241 skipped\n";
        let facts = cargo_def().parse("u", "cargo nextest run", out, None);
        let t = test_fact(&facts).expect("test_outcome present");
        assert_eq!(t.args, vec!["u", "0", "2", "2"]);
    }

    #[test]
    fn nextest_slow_and_flaky_annotations_do_not_confuse_the_summary() {
        let out = "        SLOW [> 60.000s] phronesis-mcp::journey slow_case\n\
                   Summary [  61.425s] 234 tests run: 233 passed (1 flaky), 9 skipped\n";
        let facts = cargo_def().parse("u", "cargo nextest run", out, None);
        let t = test_fact(&facts).expect("test_outcome present");
        assert_eq!(t.args, vec!["u", "233", "0", "233"]);
    }

    #[test]
    fn nextest_compile_failure_still_has_no_test_fact() {
        let out = "error[E0599]: no method named `frobnicate` found\n\
                   error: could not compile `foo` (test \"it\") due to 1 previous error\n";
        let facts = cargo_def().parse("u", "cargo nextest run", out, None);
        assert_eq!(build_status(&facts), Some("fail"));
        assert!(test_fact(&facts).is_none());
    }

    #[test]
    fn libtest_and_nextest_shapes_do_not_double_count() {
        // Defensive: if output somehow carries both shapes, the counts must
        // come from one format, not the sum of both.
        let out = "test result: ok. 3 passed; 0 failed; 0 ignored\n\
                   Summary [  1.000s] 3 tests run: 3 passed, 0 skipped\n";
        let facts = cargo_def().parse("u", "cargo nextest run", out, None);
        let t = test_fact(&facts).expect("test_outcome present");
        assert_eq!(t.args, vec!["u", "3", "0", "3"]);
    }

    #[test]
    fn cargo_per_test_results_parses_names_and_status() {
        let out = "running 3 tests\ntest mod_a::ok_one ... ok\ntest mod_b::fails ... FAILED\ntest mod_c::ignored_one ... ignored\ntest result: FAILED. 1 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out\n";
        let results = cargo_def().per_test_results(out);
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
    fn cargo_test_command_that_fails_to_compile_has_no_test_fact() {
        let out = "error[E0599]: no method named `frobnicate` found\nerror: could not compile `foo` (test \"it\") due to 1 previous error\n";
        let facts = cargo_def().parse("u", "cargo test", out, None);
        assert_eq!(build_status(&facts), Some("fail"));
        assert!(
            test_fact(&facts).is_none(),
            "no test signal when compilation fails"
        );
    }

    // ── three-state build outcome (evidence-integrity Task 4) ─────────────

    #[test]
    fn no_exit_and_empty_output_is_unknown() {
        let facts = exit_only().parse("u", "make all", "", None);
        assert_eq!(build_status(&facts), Some("unknown"));
        assert!(test_fact(&facts).is_none(), "unknown emits no test fact");
    }

    #[test]
    fn no_exit_with_compile_fail_text_is_fail() {
        let facts = synthetic().parse("u", "pytest", "ImportError: no module named x\n", None);
        assert_eq!(build_status(&facts), Some("fail"));
    }

    #[test]
    fn no_exit_with_explicit_compile_success_text_is_pass() {
        let def: ToolchainDef = serde_json::from_str(
            r#"{"id":"make","matches":"make","compile_success":["Build complete\\."]}"#,
        )
        .unwrap();
        let d = CompiledDef::compile(def, DefSource::Project).unwrap();
        let facts = d.parse(
            "u",
            "make all",
            "cc -o main main.c\nBuild complete.\n",
            None,
        );
        assert_eq!(build_status(&facts), Some("pass"));
        // Same def, output without the success marker → unknown, not pass.
        let facts = d.parse("u", "make all", "cc -o main main.c\n", None);
        assert_eq!(build_status(&facts), Some("unknown"));
    }

    #[test]
    fn truncated_piped_output_without_exit_is_unknown_not_pass() {
        // `cargo build 2>&1 | head` can truncate before the Finished line,
        // and the pipe consumer's exit code hides cargo's: no exit code, no
        // evidence — must not produce a false compile_ok.
        let out = "   Compiling foo v0.1.0\n   Compiling bar v0.";
        let facts = cargo_def().parse("u", "cargo build 2>&1 | head -5", out, None);
        assert_eq!(build_status(&facts), Some("unknown"));
        assert!(test_fact(&facts).is_none());
    }

    #[test]
    fn cargo_finished_profile_lines_are_compile_success_evidence() {
        // Both real cargo forms: with and without backticks around the name.
        for out in [
            "    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.42s\n",
            "    Finished dev profile in 0.3s\n",
        ] {
            let facts = cargo_def().parse("u", "cargo build", out, None);
            assert_eq!(build_status(&facts), Some("pass"), "output: {out}");
        }
    }

    #[test]
    fn nonzero_exit_is_fail_even_with_compile_success_text() {
        // A present exit code stays authoritative (spec table row 2): success
        // text cannot rescue a nonzero exit unless a test summary matched.
        let out = "    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.0s\n";
        let facts = cargo_def().parse("u", "cargo build", out, Some(101));
        assert_eq!(build_status(&facts), Some("fail"));
    }

    #[test]
    fn zero_exit_with_empty_output_is_pass_without_success_matcher() {
        let facts = cargo_def().parse("u", "cargo build", "", Some(0));
        assert_eq!(build_status(&facts), Some("pass"));
    }

    #[test]
    fn compile_success_field_defaults_empty_for_pre_019_defs() {
        // Backward deserialization compatibility (mandatory): a full pre-0.19
        // def with every old field and no `compile_success` must load.
        let d: ToolchainDef = serde_json::from_str(
            r#"{
                "id": "pytest",
                "matches": "pytest",
                "compile_fail": ["SyntaxError"],
                "test_summary": "(?P<passed>\\d+) passed",
                "per_test": "(?m)^(?P<name>\\S+) (?P<status>PASSED|FAILED)",
                "pass_tokens": ["PASSED"]
            }"#,
        )
        .unwrap();
        assert!(d.compile_success.is_empty());
        assert!(CompiledDef::compile(d, DefSource::Project).is_ok());
    }

    #[test]
    fn compile_rejects_bad_compile_success_regex() {
        let d: ToolchainDef =
            serde_json::from_str(r#"{"id":"x","matches":"x","compile_success":["["]}"#).unwrap();
        assert!(CompiledDef::compile(d, DefSource::Project).is_err());
    }

    #[test]
    fn builtin_cargo_pattern_strings_survive_escaping() {
        // Guards regex-escape fidelity: if a backslash is mangled during
        // editing, these literal comparisons fail before any behavior test.
        let def = builtin_defs().into_iter().next().unwrap();
        assert_eq!(def.compile_fail[0], "error\\[E\\d+\\]");
        assert_eq!(def.compile_success, vec!["Finished .* profile"]);
        let summaries = def.test_summary.as_ref().expect("cargo declares summaries");
        assert_eq!(
            summaries.as_slice()[0],
            "test result: \\w+\\. (?P<passed>\\d+) passed; (?P<failed>\\d+) failed",
        );
        assert_eq!(
            summaries.as_slice()[1],
            "Summary \\[[^\\]]*\\]\\s+\\d+ tests run: (?P<passed>\\d+) passed[^,\\n]*(?:, (?P<failed>\\d+) failed)?",
        );
    }

    #[test]
    fn builtin_cargo_compile_fail_regex_matches_real_error_line() {
        let facts = cargo_def().parse("u", "cargo build", "error[E0308]: mismatched types\n", None);
        assert_eq!(build_status(&facts), Some("fail"));
    }

    // ── command-position recognition (evidence-integrity Task 5) ──────────

    #[test]
    fn cargo_recognition_table_positive_and_negative_forms() {
        // The spec's table-driven cases, verbatim, plus real-world forms.
        let cargo = cargo_def();
        let table: &[(&str, bool)] = &[
            ("cargo test", true),
            ("cd repo && cargo test", true),
            ("FOO=1 cargo test", true),
            ("env FOO=1 cargo test", true),
            ("cargo test --workspace 2>&1", true),
            ("cargo build 2>&1 | tee build.log", true),
            ("git pull; cargo check", true),
            ("echo cargo test", false),
            ("printf 'cargo test'", false),
            ("touch cargo-test.log", false),
            ("# cargo test", false),
            ("git commit -m 'fix cargo test flake'", false),
        ];
        for (cmd, expected) in table {
            assert_eq!(cargo.handles(cmd), *expected, "command: {cmd}");
        }
    }

    #[test]
    fn builtin_cargo_matches_pattern_string_survives_escaping() {
        // Escape-fidelity guard for the anchored matcher.
        let def = builtin_defs().into_iter().next().unwrap();
        assert_eq!(def.matches, "^cargo\\s+(build|check|test|nextest)\\b");
    }

    // ── xcodebuild / swift (XCTest + Swift Testing) ───────────────────────
    //
    // An `xcodebuild test` run through the Bash tool used to ground nothing:
    // cargo was the only built-in def and nothing recognized the command, so
    // the 55-pass result never reached the journal and the commit gate kept
    // the subject at `low` on the compile signal alone.

    fn builtin(id: &str) -> CompiledDef {
        let def = builtin_defs().into_iter().find(|d| d.id == id).unwrap();
        CompiledDef::compile(def, DefSource::BuiltIn).unwrap()
    }

    const XCTEST_PASS: &str = "Test Suite 'FooTests' passed at 2026-08-22 10:00:01.000.\n\
        \t Executed 12 tests, with 0 failures (0 unexpected) in 0.101 (0.103) seconds\n\
        Test Suite 'All tests' passed at 2026-08-22 10:00:01.000.\n\
        \t Executed 55 tests, with 0 failures (0 unexpected) in 1.201 (1.210) seconds\n\
        ** TEST SUCCEEDED **\n";

    const XCTEST_FAIL: &str = "/Users/me/App/AppTests/FooTests.swift:42: error: -[AppTests.FooTests testBar] : XCTAssertEqual failed: (\"1\") is not equal to (\"2\")\n\
        Test Suite 'All tests' failed at 2026-08-22 10:00:01.000.\n\
        \t Executed 55 tests, with 1 failure (0 unexpected) in 1.201 (1.210) seconds\n\
        ** TEST FAILED **\n";

    #[test]
    fn xcodebuild_def_recognizes_test_and_build_invocations() {
        let d = builtin("xcodebuild");
        assert!(d.handles("xcodebuild test -scheme App -destination 'platform=macOS'"));
        assert!(d.handles("cd ios && xcodebuild -scheme App build"));
        assert!(d.handles("xcodebuild test-without-building -scheme App"));
        assert!(!d.handles("echo xcodebuild"));
        assert!(!d.handles("cat xcodebuild.log"));
    }

    #[test]
    fn xcodebuild_all_pass_grounds_compile_and_test_signals() {
        let facts =
            builtin("xcodebuild").parse("u", "xcodebuild test -scheme App", XCTEST_PASS, None);
        assert_eq!(build_status(&facts), Some("pass"));
        let t = test_fact(&facts).expect("xcodebuild output must ground a test_outcome");
        assert_eq!(t.args[2], "0", "no failures");
        assert!(t.args[3].parse::<usize>().unwrap() > 0, "tests ran");
    }

    #[test]
    fn xcodebuild_assertion_failure_is_a_test_failure_not_a_compile_failure() {
        let facts =
            builtin("xcodebuild").parse("u", "xcodebuild test -scheme App", XCTEST_FAIL, Some(65));
        assert_eq!(
            build_status(&facts),
            Some("pass"),
            "the suite compiled and ran"
        );
        let t = test_fact(&facts).expect("test_outcome present");
        assert_eq!(t.args[2], "1");
    }

    #[test]
    fn xcodebuild_compile_error_is_a_build_failure_without_test_fact() {
        let out = "/Users/me/App/Sources/Foo.swift:12:9: error: cannot find 'frobnicate' in scope\n\
                   ** BUILD FAILED **\n";
        let facts = builtin("xcodebuild").parse("u", "xcodebuild test -scheme App", out, None);
        assert_eq!(build_status(&facts), Some("fail"));
        assert!(test_fact(&facts).is_none());
    }

    #[test]
    fn xcodebuild_build_succeeded_marker_is_compile_success_without_exit_code() {
        let out = "Build settings from command line:\n** BUILD SUCCEEDED **\n";
        let facts = builtin("xcodebuild").parse("u", "xcodebuild build -scheme App", out, None);
        assert_eq!(build_status(&facts), Some("pass"));
    }

    #[test]
    fn swift_test_xctest_output_grounds_a_test_signal() {
        let d = builtin("swift");
        assert!(d.handles("swift test"));
        assert!(d.handles("swift build -c release"));
        assert!(!d.handles("swift run"));
        let facts = d.parse("u", "swift test", XCTEST_PASS, Some(0));
        let t = test_fact(&facts).expect("test_outcome present");
        assert_eq!(t.args[2], "0");
    }

    #[test]
    fn swift_testing_run_summary_pass_and_fail() {
        let d = builtin("swift");
        let pass = "\u{2714} Test run with 12 tests in 2 suites passed after 0.123 seconds.\n";
        let facts = d.parse("u", "swift test", pass, None);
        let t = test_fact(&facts).expect("passing Swift Testing run grounds a test_outcome");
        assert_eq!(t.args, vec!["u", "12", "0", "12"]);

        let fail = "\u{2718} Test run with 12 tests in 2 suites failed after 0.123 seconds with 2 issues.\n";
        let facts = d.parse("u", "swift test", fail, Some(1));
        let t = test_fact(&facts).expect("failing run still grounds a (failing) test_outcome");
        assert_eq!(t.args[2], "2");
    }
}
