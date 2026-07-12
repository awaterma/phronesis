//! Declarative toolchain definitions — the data that replaces hand-written
//! adapters (design: docs/superpowers/specs/2026-07-10-neutral-toolchain-outcomes-design.md).
//!
//! A `ToolchainDef` says how to *recognize* a build/test command (`matches`)
//! and, optionally, how to refine the exit-code signal: `compile_fail`
//! patterns force a build failure, `test_summary` extracts passed/failed
//! counts via named groups `(?P<passed>)`/`(?P<failed>)`, and `per_test`
//! extracts per-test results via `(?P<name>)`/`(?P<status>)` + `pass_tokens`.
//! Cargo ships as a built-in def and runs through the exact same engine.
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
    /// Regex with named groups `(?P<passed>\d+)` (required) and optional
    /// `(?P<failed>\d+)`. All matches in the output are summed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_summary: Option<String>,
    /// Regex with named groups `(?P<name>)` and `(?P<status>)`; feeds the
    /// known-bug registry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_test: Option<String>,
    /// Which `status` tokens mean "pass".
    #[serde(default = "default_pass_tokens")]
    pub pass_tokens: Vec<String>,
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
    test_summary: Option<Regex>,
    per_test: Option<Regex>,
}

fn compile_field(id: &str, field: &'static str, pattern: &str) -> Result<Regex, ToolchainError> {
    Regex::new(pattern).map_err(|source| ToolchainError::BadRegex {
        id: id.to_string(),
        field,
        source,
    })
}

fn require_groups(
    id: &str,
    field: &'static str,
    re: &Regex,
    groups: &[&str],
    label: &'static str,
) -> Result<(), ToolchainError> {
    let names: Vec<&str> = re.capture_names().flatten().collect();
    if groups.iter().all(|g| names.contains(g)) {
        Ok(())
    } else {
        Err(ToolchainError::MissingGroup {
            id: id.to_string(),
            field,
            groups: label,
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
        let test_summary = def
            .test_summary
            .as_deref()
            .map(|p| {
                let re = compile_field(&def.id, "test_summary", p)?;
                require_groups(&def.id, "test_summary", &re, &["passed"], "(?P<passed>)")?;
                Ok::<_, ToolchainError>(re)
            })
            .transpose()?;
        let per_test = def
            .per_test
            .as_deref()
            .map(|p| {
                let re = compile_field(&def.id, "per_test", p)?;
                require_groups(
                    &def.id,
                    "per_test",
                    &re,
                    &["name", "status"],
                    "(?P<name>) and (?P<status>)",
                )?;
                Ok::<_, ToolchainError>(re)
            })
            .transpose()?;
        Ok(Self {
            def,
            source,
            matches,
            compile_fail,
            test_summary,
            per_test,
        })
    }

    /// Does this def recognize the command as a build/test invocation?
    pub fn handles(&self, command: &str) -> bool {
        self.matches.is_match(command)
    }
}

/// Summed test counts extracted from `test_summary` matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TestCounts {
    passed: usize,
    failed: usize,
}

impl CompiledDef {
    fn compile_fail_hit(&self, output: &str) -> bool {
        self.compile_fail.iter().any(|re| re.is_match(output))
    }

    /// Sum every `test_summary` match (multi-binary runs). `None` when the
    /// def has no summary regex or nothing matched.
    fn test_counts(&self, output: &str) -> Option<TestCounts> {
        let re = self.test_summary.as_ref()?;
        let mut found = false;
        let mut counts = TestCounts { passed: 0, failed: 0 };
        for caps in re.captures_iter(output) {
            found = true;
            counts.passed += caps
                .name("passed")
                .and_then(|m| m.as_str().parse::<usize>().ok())
                .unwrap_or(0);
            counts.failed += caps
                .name("failed")
                .and_then(|m| m.as_str().parse::<usize>().ok())
                .unwrap_or(0);
        }
        found.then_some(counts)
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

    /// The two-tier model. Build: `compile_fail` match → fail; else a present
    /// exit code is authoritative — except that a non-zero exit accompanied
    /// by a test summary is attributed to the test failures it reports (a
    /// test failure is not a compile failure); no exit code → Tier 2 alone
    /// (no compile_fail match means compiled). Tests: summed summary counts,
    /// only when the build passed.
    pub fn parse(
        &self,
        subject: &str,
        _command: &str,
        output: &str,
        command_exit: Option<i32>,
    ) -> Vec<OutcomeFact> {
        let counts = self.test_counts(output);
        let build_pass = if self.compile_fail_hit(output) {
            false
        } else {
            match command_exit {
                Some(0) | None => true,
                Some(_) => counts.is_some(),
            }
        };
        let mut facts = vec![OutcomeFact::build(subject, build_pass)];
        if build_pass && let Some(c) = counts {
            facts.push(OutcomeFact::test(subject, c.passed, c.failed));
        }
        facts
    }
}

/// The bundled defs. Cargo is the only built-in — pytest/tsc examples ship
/// via `phr-mcp init` as project defs so the built-in surface stays minimal.
/// This def must keep the retired `CargoAdapter`'s exact semantics; the
/// fidelity tests below are the acceptance bar.
pub fn builtin_defs() -> Vec<ToolchainDef> {
    vec![ToolchainDef {
        id: "cargo".to_string(),
        matches: r"cargo (build|check|test|nextest)".to_string(),
        compile_fail: vec![r"error\[E\d+\]".to_string(), "could not compile".to_string()],
        test_summary: Some(
            r"test result: \w+\. (?P<passed>\d+) passed; (?P<failed>\d+) failed".to_string(),
        ),
        per_test: Some(r"(?m)^test (?P<name>\S+) \.\.\. (?P<status>ok|FAILED)".to_string()),
        pass_tokens: default_pass_tokens(),
    }]
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
        assert_eq!(defs.len(), 1);
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
        std::fs::write(config_path(dir.path()), r#"[{"id":"pytest","matches":"pytest"}]"#).unwrap();
        let reg = registry(dir.path());
        assert_eq!(reg.len(), 2);
        assert_eq!(reg[0].def.id, "cargo");
        assert_eq!(reg[0].source, DefSource::BuiltIn);
        assert_eq!(reg[1].def.id, "pytest");
        assert_eq!(reg[1].source, DefSource::Project);
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
        assert_eq!(reg.len(), 1, "override replaces, not appends");
        assert_eq!(reg[0].source, DefSource::Override);
        assert!(reg[0].handles("cargo-custom build"));
        assert!(!reg[0].handles("cargo build"));
    }

    #[test]
    fn registry_with_no_project_file_is_builtins_only() {
        let dir = tempfile::tempdir().unwrap();
        let reg = registry(dir.path());
        assert_eq!(reg.len(), 1);
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
        assert!(test_fact(&facts).is_none(), "no test signal on a compile failure");
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
        assert_eq!(t.args, vec!["u", "12", "0", "12"], "absent failed group counts as 0");
    }

    #[test]
    fn summary_matches_are_summed_across_output() {
        let out = "=== 3 passed in 0.1s ===\n=== 1 failed, 5 passed in 0.2s ===\n";
        let facts = synthetic().parse("u", "pytest", out, Some(1));
        let t = test_fact(&facts).expect("test_outcome present");
        assert_eq!(t.args, vec!["u", "8", "1", "9"]);
    }

    #[test]
    fn no_exit_and_no_compile_fail_is_tier2_pass() {
        // Fallback: payload carried no exit code — today's behavior preserved.
        let facts = synthetic().parse("u", "pytest", "=== 4 passed in 0.1s ===\n", None);
        assert_eq!(build_status(&facts), Some("pass"));
        assert!(test_fact(&facts).is_some());
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
}
