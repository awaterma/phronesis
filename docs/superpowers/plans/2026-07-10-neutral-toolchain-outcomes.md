# Neutral Toolchain Outcomes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make confidence scoring toolchain-neutral: exit code as the universal pass/fail signal, per-toolchain refinement as declarative data in `.phronesis/toolchains.json`, cargo demoted to a bundled definition, plus `command_exit` capture on journal records and write-side journal compaction.

**Architecture:** Per `docs/superpowers/specs/2026-07-10-neutral-toolchain-outcomes-design.md`. One generic parse engine (`CompiledDef::parse`) applies the two-tier model (exit code authoritative for build, regex refinement for counts/per-test); the registry is built-in defs ∪ project defs with id-override; the hook threads `command_exit` from the tool-response payload into both the adapter and the journal record; `journey::journal::append` gains a size-capped, subject-aware compaction.

**Tech Stack:** Rust (edition 2024), serde/serde_json, regex, thiserror, fs2, clap 4, existing `CARGO_BIN_EXE_phr-mcp` integration-test pattern.

## Global Constraints

- No `.unwrap()` / `.expect()` / `panic!()` / `todo!()` in `crates/*/src/**` production paths (test modules exempt) — enforced by `enforce-no-unwrap-in-src` and friends. Use `let-else`, `ok()`, `?`.
- No `Result<_, String>` returns in src — use `thiserror` (enforced by `enforce-no-result-string-error`).
- All cargo invocations use `--workspace` (enforced by `warn-cargo-build-without-workspace`).
- Never pipe `cargo test` output through `grep`/`head`/`tail` — it destroys the summary lines the confidence gate parses (ADR 2026-07-06-piped-test-output-loses-signal). Run it bare.
- Hook additions are fail-open: toolchain-def loading, exit-code extraction, and journal compaction must never change a hook's exit code or block an append.
- Workspace version bumps one MINOR above whatever the parent branch (`feat/payload-contract-corpus`, plan targets 0.18.0) shipped — **0.19.0** if the corpus landed first, 0.18.0 otherwise. Lockstep: `[workspace.package] version` in root `Cargo.toml` plus the internal path-dep pins on `phr`/`phronesis-rhai` in `crates/phronesis-mcp/Cargo.toml`.
- Commit messages end with `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`. Nothing is pushed; human reviews before any push.

## Design decisions resolved by this plan (per spec §Open questions)

1. **Built-in defs: cargo only.** pytest/tsc ship as *examples written by `init`* into `.phronesis/toolchains.json` for the `confidence` pack (spec offered both options; this keeps the binary's built-in surface minimal and dogfoods the project-def path).
2. **Named capture groups, not positional.** `test_summary` must use `(?P<passed>\d+)` (required) and `(?P<failed>\d+)` (optional); `per_test` must use `(?P<name>...)` and `(?P<status>...)`, with a `pass_tokens` list (default `["ok", "PASSED", "passed"]`) naming which status tokens mean pass. *Deviation from the spec's positional groups, with cause:* real pytest summaries order failed-first (`2 failed, 10 passed`) while cargo orders passed-first — positional group numbers cannot express both; named groups are order-independent and self-documenting. Recorded here so the spec's §2 table is read with this amendment.
3. **Compaction tail `K` = `SUFFIX_HARD_CAP` (10 000)** so compaction never drops inside the read window. Default `max_journal_bytes()` = 16 MiB (`MAX_JOURNAL_BYTES_DEFAULT`), env override `PHRONESIS_MAX_JOURNAL_BYTES`, ceiling 1 GiB — mirrors `action_log`'s shape at a smaller default because journal records are smaller than log entries.
4. **Corpus independence.** The spec pins exit-code payload locations via the payload-contract corpus. That corpus plan is design-complete but may not be implemented when this executes, so Task 4's tests use the existing spawn-the-binary integration pattern directly and do not depend on the corpus runner. If the corpus runner exists at execution time, additionally drop a `post-bash-exit-code.json` fixture into its layout (optional step, noted in Task 4).

## File structure

- **Create** `crates/phronesis-mcp/src/outcomes/toolchain.rs` — `ToolchainDef` (serde), `CompiledDef` (validated regexes + parse engine), built-in cargo def, project-def loading, registry assembly. Cargo fidelity tests live here.
- **Rewrite** `crates/phronesis-mcp/src/outcomes/adapter.rs` — trait gains `command_exit`; registry-driven routing; `extract_from` + tag mapping move here from `cargo.rs`.
- **Delete** `crates/phronesis-mcp/src/outcomes/cargo.rs` — `CargoAdapter` retired; its test suite is ported verbatim to `toolchain.rs` as the fidelity bar.
- **Modify** `crates/phronesis-mcp/src/outcomes/mod.rs` — module list + re-exports.
- **Modify** `crates/phronesis-mcp/src/hook/journey_record.rs` — exit-code accessor, threading into `extract_from` and the journal record.
- **Modify** `crates/phronesis-mcp/src/journey/journal.rs` — `command_exit` field; compaction.
- **Modify** `crates/phronesis-mcp/src/hook/mod.rs`, `hook/pre.rs`, `hook/post.rs` — `log_hook_event` gains `command_exit`.
- **Modify** `crates/phronesis-mcp/src/main.rs` — `toolchains` subcommand.
- **Modify** `crates/phronesis-mcp/src/init.rs` — confidence pack writes `toolchains.json` example + gitignore whitelist.
- **Modify** `crates/phronesis-mcp/CLAUDE.md`, root `Cargo.toml` — docs + version.

---

### Task 1: `ToolchainDef` — deserialize, validate, load, assemble registry

**Files:**
- Create: `crates/phronesis-mcp/src/outcomes/toolchain.rs`
- Modify: `crates/phronesis-mcp/src/outcomes/mod.rs:19-24` (add `pub mod toolchain;`)

**Interfaces:**
- Consumes: nothing project-internal (serde, regex, thiserror).
- Produces:
  - `pub struct ToolchainDef { pub id: String, pub matches: String, pub compile_fail: Vec<String>, pub test_summary: Option<String>, pub per_test: Option<String>, pub pass_tokens: Vec<String> }` (Serialize + Deserialize; `pass_tokens` defaults to `["ok","PASSED","passed"]`)
  - `pub enum DefSource { BuiltIn, Project, Override }` with `pub fn as_str(&self) -> &'static str`
  - `pub struct CompiledDef { pub def: ToolchainDef, pub source: DefSource, ... }` with `pub fn compile(def: ToolchainDef, source: DefSource) -> Result<Self, ToolchainError>` and `pub fn handles(&self, command: &str) -> bool`
  - `pub fn builtin_defs() -> Vec<ToolchainDef>` (cargo only)
  - `pub fn config_path(root: &Path) -> PathBuf` (`.phronesis/toolchains.json`)
  - `pub fn load_project_defs(root: &Path) -> Vec<ToolchainDef>` (fail-open)
  - `pub fn registry(root: &Path) -> Vec<CompiledDef>` (built-ins ∪ project, id-override) — Tasks 2/3/6 consume this.

- [ ] **Step 1: Write the failing tests**

Create `crates/phronesis-mcp/src/outcomes/toolchain.rs` with the test module first (the types referenced don't exist yet, so this fails to compile — that is the failing state):

```rust
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
}
```

- [ ] **Step 2: Register the module and run the tests to verify they fail**

Add `pub mod toolchain;` to the module list in `crates/phronesis-mcp/src/outcomes/mod.rs` (after `pub mod subject;`).

Run: `cargo test --workspace outcomes::toolchain`
Expected: FAIL to compile — `ToolchainDef` etc. not defined.

- [ ] **Step 3: Write the implementation**

Above the test module in `toolchain.rs`:

```rust
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
    let mut out: Vec<CompiledDef> = Vec::new();
    for def in builtin_defs() {
        if let Some(over) = project.iter().find(|p| p.id == def.id) {
            if let Ok(c) = CompiledDef::compile(over.clone(), DefSource::Override) {
                out.push(c);
            }
            continue;
        }
        if let Ok(c) = CompiledDef::compile(def, DefSource::BuiltIn) {
            out.push(c);
        }
    }
    for def in project {
        if builtin_defs().iter().any(|b| b.id == def.id) {
            continue; // already placed as Override
        }
        if let Ok(c) = CompiledDef::compile(def, DefSource::Project) {
            out.push(c);
        }
    }
    out
}
```

- [ ] **Step 4: Run the tests and make sure they pass**

Run: `cargo test --workspace outcomes::toolchain`
Expected: PASS (all Task 1 tests green).

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/outcomes/toolchain.rs crates/phronesis-mcp/src/outcomes/mod.rs
git commit -m "feat(outcomes): declarative ToolchainDef — load, validate, registry with builtin cargo def

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: Two-tier parse engine on `CompiledDef`

**Files:**
- Modify: `crates/phronesis-mcp/src/outcomes/toolchain.rs` (add `parse`, `per_test_results`, `TestCounts` to `impl CompiledDef`)

**Interfaces:**
- Consumes: `OutcomeFact::{build, test}` from `crate::outcomes::facts` (existing).
- Produces (Tasks 3/5 consume):
  - `CompiledDef::parse(&self, subject: &str, command: &str, output: &str, command_exit: Option<i32>) -> Vec<OutcomeFact>`
  - `CompiledDef::per_test_results(&self, output: &str) -> Vec<(String, bool)>`

**Signal precedence implemented (spec §Signal precedence):** `compile_fail` match → build fail always. Otherwise exit 0 → build pass; exit non-zero → build fail *unless* a `test_summary` match is present (non-zero exit attributed to the test failures it reports); exit absent → build pass (Tier-2-only fallback, today's cargo behavior). Test fact emitted only when build passed and a summary matched.

- [ ] **Step 1: Write the failing tests**

Append to the `tests` module in `toolchain.rs`:

```rust
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --workspace outcomes::toolchain`
Expected: FAIL to compile — `parse`/`per_test_results` not defined on `CompiledDef`.

- [ ] **Step 3: Write the implementation**

Add to `toolchain.rs` (below the `CompiledDef::compile`/`handles` impl block, extending it):

```rust
use crate::outcomes::facts::OutcomeFact;

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
```

(Move the `use crate::outcomes::facts::OutcomeFact;` to the top of the file with the other imports.)

- [ ] **Step 4: Run the tests and make sure they pass**

Run: `cargo test --workspace outcomes::toolchain`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/outcomes/toolchain.rs
git commit -m "feat(outcomes): two-tier parse engine — exit code + declarative regex refinement

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: Registry-driven adapter; cargo fidelity; delete `CargoAdapter`

**Files:**
- Rewrite: `crates/phronesis-mcp/src/outcomes/adapter.rs`
- Delete: `crates/phronesis-mcp/src/outcomes/cargo.rs`
- Modify: `crates/phronesis-mcp/src/outcomes/mod.rs` (drop `pub mod cargo;`, keep re-exports working)
- Modify: `crates/phronesis-mcp/src/hook/journey_record.rs:43` (call site)
- Modify: `crates/phronesis-mcp/src/outcomes/toolchain.rs` (cargo fidelity tests ported here)

**Interfaces:**
- Consumes: `toolchain::{registry, CompiledDef}` (Task 1/2), `bugs::{load, check}`, `subject::{open, settle}` (existing).
- Produces (Task 4 and the hook consume):
  - `pub trait OutcomeAdapter { fn handles(&self, command: &str) -> bool; fn parse(&self, subject: &str, command: &str, output: &str, command_exit: Option<i32>) -> Vec<OutcomeFact>; }` — the spec's signature change.
  - `pub struct ConfigAdapter { pub def: CompiledDef }` implementing the trait.
  - `pub fn handles(root: &Path, command: &str) -> bool`
  - `pub fn extract(root: &Path, subject: &str, command: &str, output: &str, command_exit: Option<i32>) -> Vec<OutcomeFact>`
  - `pub fn extract_from(project_root: &Path, tool_name: &str, command_opt: Option<&str>, output: &str, command_exit: Option<i32>) -> (Vec<String>, Option<String>)` — moved from `cargo.rs`, exit param added; behavior otherwise identical (git-commit settle, opt-in guard, subject open, tags).

Note: `handles`/`extract` gain a `root` parameter because the registry now includes project defs. `outcomes/mod.rs`'s `pub use adapter::{extract, handles}` re-export stays.

- [ ] **Step 1: Port the cargo fidelity tests (they must fail first)**

Append to `toolchain.rs`'s test module — this is the *fidelity bar*: every assertion is copied from the retired `cargo.rs` suite, now driven through the built-in def with `command_exit: None` (fixtures carry output only, preserving today's Tier-2 path):

```rust
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
        assert!(test_fact(&facts).is_none(), "build command emits no test fact");
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
        assert!(test_fact(&facts).is_none(), "no test signal when compilation fails");
    }
```

Run: `cargo test --workspace outcomes::toolchain`
Expected: PASS already (the engine from Task 2 + the built-in def satisfy them — that *is* the fidelity check; if any fails, fix the built-in def, not the test).

- [ ] **Step 2: Rewrite `adapter.rs`**

Replace the entire file with:

```rust
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
    toolchain::registry(root).into_iter().find(|d| d.handles(command))
}

/// Does any def recognize this command? Lets callers skip opening a work
/// unit for irrelevant commands (e.g. `ls`).
pub fn handles(root: &Path, command: &str) -> bool {
    matching_def(root, command).is_some()
}

/// Extract neutral outcome facts. Empty when no def recognizes the command —
/// a non-build/test command produces no grounded signal, which is correct.
pub fn extract(
    root: &Path,
    subject: &str,
    command: &str,
    output: &str,
    command_exit: Option<i32>,
) -> Vec<OutcomeFact> {
    matching_def(root, command)
        .map(|d| d.parse(subject, command, output, command_exit))
        .unwrap_or_default()
}
```

Then move these items from `cargo.rs` into `adapter.rs` **unchanged except where noted**: `build_tag`, `test_tag`, `outcome_tags` (verbatim), `bug_caught_tags` (signature becomes `fn bug_caught_tags(project_root: &Path, subject: &str, per_test: &[(String, bool)]) -> Vec<String>` — the caller now supplies per-test results from the matched def instead of the function re-parsing with a cargo-only regex; body drops the `per_test_results(output)` line and uses the parameter), and `extract_from` (gains `command_exit: Option<i32>` as the last parameter; the doc comment and guards are verbatim; the body's adapter calls become):

```rust
    if !crate::outcomes::adapter::handles(project_root, command) {
        return (Vec::new(), None);
    }
    let subject = match subject_mod::open(project_root) {
        Ok(s) => s,
        Err(_) => return (Vec::new(), None),
    };
    let Some(def) = matching_def(project_root, command) else {
        return (Vec::new(), None);
    };
    let outcome_facts = def.parse(&subject, command, output, command_exit);
    let per_test = def.per_test_results(output);
    let tags: Vec<String> = outcome_tags(&outcome_facts)
        .into_iter()
        .chain(bug_caught_tags(project_root, &subject, &per_test))
        .collect();
    (tags, Some(subject))
```

Port `adapter.rs`'s two existing tests, adapted to the new signatures (tempdir for `root`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_command_yields_no_facts() {
        let dir = tempfile::tempdir().unwrap();
        let facts = extract(dir.path(), "u", "ls -la", "total 0\n", None);
        assert!(facts.is_empty());
    }

    #[test]
    fn cargo_command_is_routed_through_the_registry() {
        let dir = tempfile::tempdir().unwrap();
        let facts = extract(
            dir.path(),
            "u",
            "cargo build",
            "   Finished dev profile in 0.4s\n",
            None,
        );
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
        let facts = extract(dir.path(), "u", "pytest -q", "=== 3 passed in 0.1s ===\n", Some(0));
        assert!(facts.iter().any(|f| f.predicate == "build_outcome"));
        assert!(facts.iter().any(|f| f.predicate == "test_outcome"));
    }
}
```

- [ ] **Step 3: Delete `cargo.rs` and update the seams**

1. `git rm crates/phronesis-mcp/src/outcomes/cargo.rs`
2. In `crates/phronesis-mcp/src/outcomes/mod.rs`: remove `pub mod cargo;`; the `pub use adapter::{extract, handles};` line stays. Update the module doc comment's "per-toolchain adapters (`cargo` first)" sentence to "declarative toolchain defs (cargo built-in; project defs in `.phronesis/toolchains.json`)".
3. In `crates/phronesis-mcp/src/hook/journey_record.rs:43`: change

```rust
    outcomes::cargo::extract_from(&root, tool_name, command.as_deref(), &output)
```
to
```rust
    outcomes::adapter::extract_from(&root, tool_name, command.as_deref(), &output, None)
```
(the `None` is temporary — Task 4 threads the real exit code).

- [ ] **Step 4: Build and run the full suite**

Run: `cargo build --workspace` then `cargo test --workspace`
Expected: builds clean (compiler will surface any missed `outcomes::cargo` reference — fix each by pointing at `outcomes::adapter` / `outcomes::toolchain`); all tests PASS, including every ported cargo fidelity test.

- [ ] **Step 5: Commit**

```bash
git add -u crates/phronesis-mcp/src/outcomes crates/phronesis-mcp/src/hook/journey_record.rs
git commit -m "refactor(outcomes): cargo becomes a built-in ToolchainDef; CargoAdapter deleted

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 4: `command_exit` capture — payload accessor, journal field, action-log field

**Files:**
- Modify: `crates/phronesis-mcp/src/hook/journey_record.rs` (accessor + threading)
- Modify: `crates/phronesis-mcp/src/journey/journal.rs:31-56` (`JournalRecord` field)
- Modify: `crates/phronesis-mcp/src/hook/mod.rs:299-320` (`log_hook_event` signature), `hook/pre.rs:132,140,144`, `hook/post.rs:156,170` (call sites)
- Test: unit tests in `journey_record.rs` and `journal.rs`

**Interfaces:**
- Consumes: `HookPayload` (existing), `extract_tool_output_text` (existing in `journey_record.rs`), `adapter::extract_from` (Task 3).
- Produces:
  - `pub(super) fn payload_command_exit(payload: &HookPayload) -> Option<i32>` in `journey_record.rs` (visible to `post.rs`)
  - `JournalRecord.command_exit: Option<i32>` (serde `default` + `skip_serializing_if = "Option::is_none"`; schema `v` stays 1 — the field is optional and additive)
  - `log_hook_event(phase, tool_name, file_path, exit, command_exit: Option<i32>, consequences)` — the log field is named `command_exit`, never `exit` (the entry already has an `exit` field for the hook's own exit code).

- [ ] **Step 1: Write the failing tests**

Append to the `tests` module in `journey_record.rs`:

```rust
    fn payload_with_output(output: serde_json::Value) -> HookPayload {
        HookPayload {
            tool_name: Some("Bash".to_string()),
            tool_input: Some(serde_json::json!({ "command": "cargo build --workspace" })),
            tool_output: Some(output),
        }
    }

    #[test]
    fn command_exit_from_exit_code_key() {
        let p = payload_with_output(serde_json::json!({ "exit_code": 101, "stdout": "boom" }));
        assert_eq!(payload_command_exit(&p), Some(101));
    }

    #[test]
    fn command_exit_tries_alternate_keys_in_order() {
        for key in ["exitCode", "returncode", "code", "status"] {
            let p = payload_with_output(serde_json::json!({ key: 2 }));
            assert_eq!(payload_command_exit(&p), Some(2), "key {key} should be read");
        }
    }

    #[test]
    fn command_exit_falls_back_to_trailing_text_line() {
        let p = payload_with_output(serde_json::json!({
            "stdout": "   Compiling foo\nerror: boom\nexit code: 101"
        }));
        assert_eq!(payload_command_exit(&p), Some(101));
    }

    #[test]
    fn command_exit_absent_is_none() {
        let p = payload_with_output(serde_json::json!({ "stdout": "Finished dev profile" }));
        assert_eq!(payload_command_exit(&p), None);
    }

    #[test]
    fn command_exit_none_without_tool_output() {
        let p = make_payload("Bash", serde_json::json!({ "command": "ls" }));
        assert_eq!(payload_command_exit(&p), None);
    }

    #[test]
    fn command_exit_string_output_uses_text_fallback() {
        let p = payload_with_output(serde_json::Value::String(
            "ran stuff\nexit code: 3".to_string(),
        ));
        assert_eq!(payload_command_exit(&p), Some(3));
    }

    #[test]
    fn command_exit_non_numeric_status_is_none() {
        // Some CLIs send status as a string ("success") — only numeric counts.
        let p = payload_with_output(serde_json::json!({ "status": "success" }));
        assert_eq!(payload_command_exit(&p), None);
    }
```

And to `journal.rs`'s tests (create a `#[cfg(test)] mod tests` at the bottom if none exists):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn record(seq: u64, command_exit: Option<i32>) -> JournalRecord {
        JournalRecord {
            v: 1,
            ts: seq,
            sid: "s".to_string(),
            seq,
            tool: "Bash".to_string(),
            path: "<cmd>".to_string(),
            ext: None,
            module: None,
            tags: vec![],
            subject: None,
            command_exit,
        }
    }

    #[test]
    fn command_exit_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), &record(1, Some(101))).unwrap();
        let recs = read_recent(dir.path(), 10).unwrap();
        assert_eq!(recs[0].command_exit, Some(101));
    }

    #[test]
    fn command_exit_none_is_omitted_from_serialization() {
        let line = serde_json::to_string(&record(1, None)).unwrap();
        assert!(
            !line.contains("command_exit"),
            "absent exit code must be omitted, not null: {line}"
        );
    }

    #[test]
    fn v1_line_without_command_exit_still_parses() {
        let line = r#"{"v":1,"ts":0,"sid":"s","seq":1,"tool":"Bash","path":"<cmd>","tags":[]}"#;
        let rec: JournalRecord = serde_json::from_str(line).unwrap();
        assert_eq!(rec.command_exit, None);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --workspace journey`
Expected: FAIL to compile — `command_exit` field and `payload_command_exit` don't exist.

- [ ] **Step 3: Implement**

1. **`journal.rs`** — add to `JournalRecord` after `subject`:

```rust
    /// The tool call's process exit code, when the CLI's payload carried one
    /// (Bash/shell records only). Named `command_exit`, not `exit` — the
    /// action log's `exit` is the hook's own exit code; the two must not
    /// collide. Absent means the CLI genuinely didn't send one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_exit: Option<i32>,
```

2. **`journey_record.rs`** — add the accessor next to `extract_tool_output_text`:

```rust
/// Trailing `exit code: N` line in captured output text — the last-resort
/// source when the payload has no structured exit field.
fn exit_from_text(text: &str) -> Option<i32> {
    text.lines()
        .rev()
        .find_map(|l| l.trim().strip_prefix("exit code: ").and_then(|n| n.trim().parse().ok()))
}

/// Best-effort exit-code extraction from the tool-response object. The exact
/// location is CLI-specific (pinned by the payload-contract corpus); known
/// candidates are tried in order, then the trailing-text fallback. `None`
/// means the CLI didn't provide one — confidence falls back to Tier 2.
pub(super) fn payload_command_exit(payload: &HookPayload) -> Option<i32> {
    let v = payload.tool_output.as_ref()?;
    if let Some(obj) = v.as_object() {
        for key in ["exit_code", "exitCode", "returncode", "code", "status"] {
            if let Some(n) = obj.get(key).and_then(serde_json::Value::as_i64) {
                return i32::try_from(n).ok();
            }
        }
    }
    exit_from_text(&extract_tool_output_text(payload))
}
```

3. **Thread it through** `journey_record.rs`:

```rust
fn outcomes_for_journal(
    payload: &HookPayload,
    tool_name: &str,
) -> (Vec<String>, Option<String>, Option<i32>) {
    let root = security::project_root();
    let command = super::extract_new_content(payload, tool_name);
    let output = extract_tool_output_text(payload);
    let command_exit = matches!(tool_name, "Bash" | "run_shell_command")
        .then(|| payload_command_exit(payload))
        .flatten();
    let (tags, subject) =
        outcomes::adapter::extract_from(&root, tool_name, command.as_deref(), &output, command_exit);
    (tags, subject, command_exit)
}
```

`build_journal_record` gains a `command_exit: Option<i32>` parameter and stamps it on the record; `journey_record_post` destructures the triple and passes it through.

4. **Action log** — in `hook/mod.rs`, `log_hook_event` gains `command_exit: Option<i32>` (parameter order: after `exit`); body adds:

```rust
    let mut entry = LogEntry::new("hook", event)
        .with("phase", phase.to_string())
        .with("tool", tool_name.to_string())
        .with("file", file_path.to_string())
        .with("exit", exit)
        .with("consequences", consequences_value);
    if let Some(ce) = command_exit {
        entry = entry.with("command_exit", ce);
    }
```

Call sites: `pre.rs:132,140,144` pass `None` (no output exists at pre time). `post.rs:156,170` compute `let command_exit = super::journey_record::payload_command_exit(&payload);` once near the top of the post path and pass it.

5. Add `command_exit: None` to every other `JournalRecord { .. }` struct literal the compiler flags — known sites: `outcomes/mod.rs` (`report_reflects_journal_signals` test), `journey/derive.rs` (test `rec()` helper), plus any in `journey/mod.rs`/`context.rs` tests. Run `cargo build --workspace` and fix each listed error.

- [ ] **Step 4: Run the full suite**

Run: `cargo build --workspace` then `cargo test --workspace`
Expected: PASS.

- [ ] **Step 5 (optional, only if the payload-contract corpus runner exists on this branch):** add fixture `crates/phronesis-mcp/tests/fixtures/payloads/claude-code/post-bash-exit-code.json` — a post-check Bash payload whose `tool_response` carries `exit_code: 101` for a `cargo build` command, with expectations asserting the journal record has `command_exit: 101` and tag `outcome:compile_error`. Follow the corpus `Fixture`/`Expect` shape. Skip this step entirely if `tests/fixtures/payloads/` does not exist.

- [ ] **Step 6: Commit**

```bash
git add -u crates/phronesis-mcp/src
git commit -m "feat(hook): capture command_exit on every shell record — Tier 1 exit-code signal

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 5: Journal compaction — subject-aware, bounded growth

**Files:**
- Modify: `crates/phronesis-mcp/src/journey/journal.rs`

**Interfaces:**
- Consumes: `JournalRecord`, `SUFFIX_HARD_CAP`, `events_path` (existing).
- Produces:
  - `pub const MAX_JOURNAL_BYTES_DEFAULT: u64 = 16 * 1024 * 1024;`
  - `pub const MAX_JOURNAL_BYTES_CEILING: u64 = 1024 * 1024 * 1024;`
  - `pub const COMPACT_TAIL_RECORDS: usize = SUFFIX_HARD_CAP;` (compaction never drops inside the read window)
  - `fn max_journal_bytes() -> u64` (env `PHRONESIS_MAX_JOURNAL_BYTES`, clamped to the ceiling)
  - `pub fn maybe_compact(root: &Path, max_bytes: u64, tail_records: usize) -> Result<bool, JournalError>` — parameterized for testability (env-var behavior can't be unit-tested in-process; the wired call passes the env-derived value)
  - `append` calls `maybe_compact(root, max_journal_bytes(), COMPACT_TAIL_RECORDS)` before writing, **fail-open** (error → stderr, append proceeds).

**Retention rule (spec §5):** keep the most recent `tail_records` records, plus — for every `subject` appearing in the dropped prefix — its most recent `outcome:*`-bearing record, prepended in original order. Rewrite is atomic: temp file in the same directory, then rename. Malformed lines are dropped at compaction (consistent with read behavior, which already skips them).

- [ ] **Step 1: Write the failing tests**

Append to `journal.rs`'s tests (reuse the Task 4 `record` helper; add a tagged variant):

```rust
    fn tagged(seq: u64, subject: &str, tags: &[&str]) -> JournalRecord {
        JournalRecord {
            subject: Some(subject.to_string()),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            ..record(seq, None)
        }
    }

    #[test]
    fn under_cap_is_untouched() {
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), &record(1, None)).unwrap();
        let compacted = maybe_compact(dir.path(), u64::MAX, 2).unwrap();
        assert!(!compacted);
        assert_eq!(read_recent(dir.path(), 100).unwrap().len(), 1);
    }

    #[test]
    fn missing_file_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!maybe_compact(dir.path(), 1, 2).unwrap());
    }

    #[test]
    fn over_cap_keeps_tail_and_latest_outcome_per_prefix_subject() {
        let dir = tempfile::tempdir().unwrap();
        // Prefix: subject "u" has two outcome records (seq 1 stale, seq 2
        // latest) and noise; subject "v" has one; seq 4 is outcome-less noise.
        append(dir.path(), &tagged(1, "u", &["outcome:compile_error"])).unwrap();
        append(dir.path(), &tagged(2, "u", &["outcome:compile_ok"])).unwrap();
        append(dir.path(), &tagged(3, "v", &["outcome:test_pass"])).unwrap();
        append(dir.path(), &record(4, None)).unwrap();
        // Tail of 2:
        append(dir.path(), &record(5, None)).unwrap();
        append(dir.path(), &record(6, None)).unwrap();
        let compacted = maybe_compact(dir.path(), 1, 2).unwrap();
        assert!(compacted);
        let recs = read_recent(dir.path(), 100).unwrap();
        let seqs: Vec<u64> = recs.iter().map(|r| r.seq).collect();
        assert_eq!(
            seqs,
            vec![2, 3, 5, 6],
            "latest outcome per prefix subject (2 for u, 3 for v) + tail, original order"
        );
    }

    #[test]
    fn confidence_read_still_works_after_compaction() {
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), &tagged(1, "u", &["outcome:compile_ok"])).unwrap();
        for seq in 2..=5 {
            append(dir.path(), &record(seq, None)).unwrap();
        }
        maybe_compact(dir.path(), 1, 2).unwrap();
        let for_u = read_recent_subject(dir.path(), "u", 10).unwrap();
        assert_eq!(for_u.len(), 1, "subject u's grounded outcome survives");
        assert!(for_u[0].tags.iter().any(|t| t == "outcome:compile_ok"));
    }

    #[test]
    fn append_still_succeeds_after_compaction() {
        let dir = tempfile::tempdir().unwrap();
        for seq in 1..=4 {
            append(dir.path(), &record(seq, None)).unwrap();
        }
        maybe_compact(dir.path(), 1, 2).unwrap();
        append(dir.path(), &record(5, None)).unwrap();
        let seqs: Vec<u64> = read_recent(dir.path(), 100).unwrap().iter().map(|r| r.seq).collect();
        assert_eq!(seqs, vec![3, 4, 5]);
    }

    #[test]
    fn malformed_lines_are_dropped_at_compaction() {
        let dir = tempfile::tempdir().unwrap();
        for seq in 1..=3 {
            append(dir.path(), &record(seq, None)).unwrap();
        }
        // Torn write in the middle of the prefix.
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(dir.path().join(".phronesis/journey/events.jsonl"))
            .unwrap();
        writeln!(f, "{{torn").unwrap();
        drop(f);
        append(dir.path(), &record(4, None)).unwrap();
        maybe_compact(dir.path(), 1, 2).unwrap();
        let seqs: Vec<u64> = read_recent(dir.path(), 100).unwrap().iter().map(|r| r.seq).collect();
        assert_eq!(seqs, vec![3, 4]);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --workspace journey::journal`
Expected: FAIL to compile — `maybe_compact` not defined.

- [ ] **Step 3: Implement**

Add to `journal.rs`:

```rust
/// Default write-side size cap for the journal. Journal records are small
/// (~200–500 bytes), so 16 MiB comfortably holds several times the
/// `SUFFIX_HARD_CAP` read window.
pub const MAX_JOURNAL_BYTES_DEFAULT: u64 = 16 * 1024 * 1024;
/// Upper bound on the env override, mirroring the action log's ceiling.
pub const MAX_JOURNAL_BYTES_CEILING: u64 = 1024 * 1024 * 1024;
/// Records retained unconditionally at the tail. Equal to `SUFFIX_HARD_CAP`
/// so compaction can never drop a record the readers could still see.
pub const COMPACT_TAIL_RECORDS: usize = SUFFIX_HARD_CAP;

fn max_journal_bytes() -> u64 {
    std::env::var("PHRONESIS_MAX_JOURNAL_BYTES")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(|v| v.min(MAX_JOURNAL_BYTES_CEILING))
        .unwrap_or(MAX_JOURNAL_BYTES_DEFAULT)
}

/// Compact the journal when it exceeds `max_bytes`: retain the most recent
/// `tail_records` records plus, for every subject appearing in the dropped
/// prefix, its most recent `outcome:*`-bearing record (so each work unit's
/// latest grounded build/test result survives for confidence banding).
/// Atomic rewrite (temp file + rename) under the same advisory lock the
/// appenders take; never blind-truncates. Returns whether a compaction ran.
pub fn maybe_compact(
    root: &Path,
    max_bytes: u64,
    tail_records: usize,
) -> Result<bool, JournalError> {
    let path = events_path(root);
    let meta = match std::fs::metadata(&path) {
        Ok(m) => m,
        Err(_) => return Ok(false),
    };
    if meta.len() < max_bytes {
        return Ok(false);
    }
    let io_err = |e: std::io::Error| JournalError::Io {
        path: path.display().to_string(),
        source: e,
    };
    let file = OpenOptions::new().read(true).open(&path).map_err(io_err)?;
    file.lock_exclusive().map_err(io_err)?;
    let result = compact_locked(&path, tail_records);
    let _ = FileExt::unlock(&file);
    result
}

fn compact_locked(path: &Path, tail_records: usize) -> Result<bool, JournalError> {
    let io_err = |e: std::io::Error| JournalError::Io {
        path: path.display().to_string(),
        source: e,
    };
    let content = std::fs::read_to_string(path).map_err(io_err)?;
    let all: Vec<JournalRecord> = content
        .lines()
        .filter_map(|l| serde_json::from_str::<JournalRecord>(l).ok())
        .collect();
    if all.len() <= tail_records {
        return Ok(false);
    }
    let split = all.len() - tail_records;
    let (prefix, tail) = all.split_at(split);
    // Latest outcome-bearing record per subject in the prefix, by index.
    let mut latest: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for (i, r) in prefix.iter().enumerate() {
        if let Some(s) = r.subject.as_deref()
            && r.tags.iter().any(|t| t.starts_with("outcome:"))
        {
            latest.insert(s, i);
        }
    }
    let mut keep: Vec<usize> = latest.into_values().collect();
    keep.sort_unstable();
    let mut out = String::new();
    for i in keep {
        out.push_str(&serde_json::to_string(&prefix[i])?);
        out.push('\n');
    }
    for r in tail {
        out.push_str(&serde_json::to_string(r)?);
        out.push('\n');
    }
    let tmp = path.with_extension("jsonl.tmp");
    let tmp_err = |e: std::io::Error| JournalError::Io {
        path: tmp.display().to_string(),
        source: e,
    };
    std::fs::write(&tmp, out).map_err(tmp_err)?;
    std::fs::rename(&tmp, path).map_err(tmp_err)?;
    Ok(true)
}
```

Wire into `append`, immediately after the `create_dir_all` block:

```rust
    // Write-side retention: best-effort and fail-open — a compaction error
    // must never block the append.
    if let Err(e) = maybe_compact(root, max_journal_bytes(), COMPACT_TAIL_RECORDS) {
        eprintln!("phronesis: journal compaction skipped: {e}");
    }
```

- [ ] **Step 4: Run the full suite**

Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/journey/journal.rs
git commit -m "feat(journey): subject-aware journal compaction — bounded write-side growth

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 6: `phr-mcp toolchains` — list active defs

**Files:**
- Modify: `crates/phronesis-mcp/src/main.rs` (Command variant near line 77-96, dispatch near line 310, handler near `handle_confidence` at line 453)

**Interfaces:**
- Consumes: `toolchain::{registry, DefSource, CompiledDef}` (Task 1), `security::project_root()` (existing — same access `handle_confidence` uses).
- Produces: CLI surface only. Table columns: `ID`, `SOURCE`, `MATCHES`, `SIGNALS` (which refinements are configured, e.g. `exit+summary+per-test+compile-fail`). `--json` emits `[{"id","source","matches","compile_fail","test_summary","per_test","pass_tokens"}]`.

- [ ] **Step 1: Write the failing integration test**

Create `crates/phronesis-mcp/tests/toolchains_cli.rs`:

```rust
use std::process::Command;

fn run_in(dir: &std::path::Path, args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(args)
        .current_dir(dir)
        .env("PHRONESIS_PROJECT_ROOT", dir)
        .output()
        .expect("spawn phr-mcp");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
    )
}

#[test]
fn toolchains_lists_builtin_cargo() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (code, stdout) = run_in(dir.path(), &["toolchains"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("cargo"), "built-in cargo listed: {stdout}");
    assert!(stdout.contains("built-in"), "source column present: {stdout}");
}

#[test]
fn toolchains_json_includes_project_def() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join(".phronesis")).expect("mkdir");
    std::fs::write(
        dir.path().join(".phronesis/toolchains.json"),
        r#"[{"id":"pytest","matches":"pytest"}]"#,
    )
    .expect("write defs");
    let (code, stdout) = run_in(dir.path(), &["toolchains", "--json"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("json output");
    let ids: Vec<&str> = v
        .as_array()
        .expect("array")
        .iter()
        .filter_map(|e| e.get("id").and_then(|i| i.as_str()))
        .collect();
    assert_eq!(ids, vec!["cargo", "pytest"]);
    assert_eq!(v[1]["source"], "project");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --workspace --test toolchains_cli`
Expected: FAIL — `toolchains` is an unrecognized subcommand (exit code ≠ 0).

- [ ] **Step 3: Implement**

Add the variant to the `Command` enum (after `Confidence`):

```rust
    /// List active toolchain definitions (built-in + project)
    Toolchains {
        /// Emit JSON instead of a table
        #[arg(long)]
        json: bool,
    },
```

Dispatch arm (next to `Command::Confidence` at main.rs:310):

```rust
        Command::Toolchains { json } => handle_toolchains(json),
```

Handler (next to `handle_confidence`):

```rust
fn handle_toolchains(json: bool) -> anyhow::Result<()> {
    use phronesis_mcp::outcomes::toolchain;
    let root = security::project_root();
    let defs = toolchain::registry(&root);
    if json {
        let items: Vec<serde_json::Value> = defs
            .iter()
            .map(|d| {
                let mut v = serde_json::to_value(&d.def).unwrap_or_default();
                if let Some(obj) = v.as_object_mut() {
                    obj.insert("source".to_string(), d.source.as_str().into());
                }
                v
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
        return Ok(());
    }
    if defs.is_empty() {
        println!("no toolchain definitions active");
        return Ok(());
    }
    println!("{:<12} {:<10} {:<40} SIGNALS", "ID", "SOURCE", "MATCHES");
    for d in &defs {
        let mut signals = vec!["exit"];
        if !d.def.compile_fail.is_empty() {
            signals.push("compile-fail");
        }
        if d.def.test_summary.is_some() {
            signals.push("summary");
        }
        if d.def.per_test.is_some() {
            signals.push("per-test");
        }
        println!(
            "{:<12} {:<10} {:<40} {}",
            d.def.id,
            d.source.as_str(),
            d.def.matches,
            signals.join("+")
        );
    }
    Ok(())
}
```

Match the existing import style at the top of `main.rs` (if other handlers use `use phronesis_mcp::...` at file scope, import `toolchain` there instead of inside the function; if `security` is referenced as `phronesis_mcp::security`, follow suit — copy exactly what `handle_confidence` does).

- [ ] **Step 4: Run the tests and make sure they pass**

Run: `cargo test --workspace --test toolchains_cli` then `cargo test --workspace`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/main.rs crates/phronesis-mcp/tests/toolchains_cli.rs
git commit -m "feat(cli): phr-mcp toolchains — list active toolchain defs

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 7: `init` writes the toolchains.json example (confidence pack)

**Files:**
- Modify: `crates/phronesis-mcp/src/init.rs` (`write_confidence_scaffold` at line 827-860; gitignore entries near line 940; a `TOOLCHAINS_JSON` const near `CONFIDENCE_JSON` at line 817)

**Interfaces:**
- Consumes: existing `write_confidence_scaffold` file loop and gitignore-entry mechanism.
- Produces: `.phronesis/toolchains.json` (pytest + tsc example defs) written when the `confidence` pack is selected and the file doesn't exist; `!.phronesis/toolchains.json` gitignore whitelist (it's project config, like `confidence.json`/`bugs.json`).

- [ ] **Step 1: Write the failing tests**

Add to `init.rs`'s test module, following the pattern of the existing confidence-scaffold tests (find them with `grep -n "confidence" crates/phronesis-mcp/src/init.rs` in the test region and mirror their setup — they construct `InitOpts` with `packs: vec![Pack::Confidence]` and a tempdir root):

```rust
    #[test]
    fn confidence_pack_writes_toolchains_example() {
        let dir = tempfile::tempdir().unwrap();
        let opts = confidence_opts(); // reuse/adapt the existing helper that selects Pack::Confidence
        run_init(dir.path(), &opts).unwrap();
        let raw = std::fs::read_to_string(dir.path().join(".phronesis/toolchains.json"))
            .expect("toolchains.json written");
        let defs: Vec<serde_json::Value> = serde_json::from_str(&raw).expect("valid JSON array");
        let ids: Vec<&str> = defs.iter().filter_map(|d| d["id"].as_str()).collect();
        assert_eq!(ids, vec!["pytest", "tsc"]);
    }

    #[test]
    fn toolchains_example_left_alone_on_rerun() {
        let dir = tempfile::tempdir().unwrap();
        let opts = confidence_opts();
        run_init(dir.path(), &opts).unwrap();
        let custom = r#"[{"id":"mine","matches":"mine"}]"#;
        std::fs::write(dir.path().join(".phronesis/toolchains.json"), custom).unwrap();
        run_init(dir.path(), &opts).unwrap();
        let raw = std::fs::read_to_string(dir.path().join(".phronesis/toolchains.json")).unwrap();
        assert_eq!(raw, custom, "existing file must be left unchanged");
    }

    #[test]
    fn confidence_pack_unignores_toolchains_json() {
        let dir = tempfile::tempdir().unwrap();
        let opts = confidence_opts();
        run_init(dir.path(), &opts).unwrap();
        let gi = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(gi.contains("!.phronesis/toolchains.json"));
    }
```

(Adjust helper names — `confidence_opts()`/`run_init(...)` — to whatever the existing confidence-scaffold tests in this file actually use; the assertions are the contract.)

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --workspace init`
Expected: FAIL — file not written / gitignore entry absent.

- [ ] **Step 3: Implement**

Add the const next to `CONFIDENCE_JSON` (JSON has no comments, so the example entries carry a `_doc` key serde ignores as an unknown field — but `ToolchainDef` uses default serde which *rejects nothing*; unknown keys are ignored by default, so `_doc` is safe):

```rust
/// Example `.phronesis/toolchains.json` written with the confidence pack:
/// two real project-def examples (pytest, tsc) proving toolchain neutrality.
/// Users edit/extend in place; `init` never overwrites it.
const TOOLCHAINS_JSON: &str = r#"[
  {
    "_doc": "Recognition regex is `matches`; refinement fields are optional. See `phr-mcp toolchains`.",
    "id": "pytest",
    "matches": "pytest",
    "compile_fail": ["SyntaxError", "ImportError"],
    "test_summary": "(?:(?P<failed>\\d+) failed, )?(?P<passed>\\d+) passed",
    "per_test": "(?m)^(?P<name>\\S+) (?P<status>PASSED|FAILED)",
    "pass_tokens": ["PASSED"]
  },
  {
    "id": "tsc",
    "matches": "\\btsc\\b",
    "compile_fail": ["error TS\\d+"]
  }
]
"#;
```

In `write_confidence_scaffold`, extend the file array:

```rust
    for (name, contents) in [
        ("confidence.json", CONFIDENCE_JSON),
        ("bugs.json", CONFIDENCE_BUGS_JSON),
        ("toolchains.json", TOOLCHAINS_JSON),
    ] {
```

In the gitignore block (init.rs:940):

```rust
    if opts.packs.contains(&Pack::Confidence) {
        entries.push("!.phronesis/confidence.json");
        entries.push("!.phronesis/bugs.json");
        entries.push("!.phronesis/toolchains.json");
    }
```

- [ ] **Step 4: Run the tests and make sure they pass**

Run: `cargo test --workspace init` then `cargo test --workspace`
Expected: PASS. (If `pytest`'s `_doc` key trips `ToolchainDef` deserialization in any test, confirm serde's default ignore-unknown-fields behavior is in effect — do not add `deny_unknown_fields`.)

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/init.rs
git commit -m "feat(init): confidence pack scaffolds .phronesis/toolchains.json example

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 8: Docs, version bump, final verification

**Files:**
- Modify: `crates/phronesis-mcp/CLAUDE.md`
- Modify: root `Cargo.toml` (workspace version), `crates/phronesis-mcp/Cargo.toml` (internal dep pins)

- [ ] **Step 1: Update `crates/phronesis-mcp/CLAUDE.md`**

Three edits:
1. Subcommand list (Build & Run block): after the `confidence` line add
   `cargo run -- toolchains        # List active toolchain defs (built-in + project); --json for machine output`
2. `confidence` pack bullet: append — "Also scaffolds `.phronesis/toolchains.json` (pytest/tsc example defs). Confidence signals are toolchain-neutral: any command matched by a toolchain def grounds a `build_outcome` from its exit code (`command_exit`, captured on every shell journal record), with optional per-toolchain regex refinement for test counts and per-test results. Cargo ships as a built-in def; project defs in `toolchains.json` extend or override it. Journal growth is bounded by write-side compaction (`PHRONESIS_MAX_JOURNAL_BYTES`, default 16 MiB) that preserves each subject's latest grounded outcome."
3. Architecture `src/outcomes/` bullet: replace "behind a per-toolchain adapter (`cargo` first)" with "behind declarative toolchain defs (built-in cargo def + `.phronesis/toolchains.json`, one generic parse engine in `outcomes/toolchain.rs`)".

- [ ] **Step 2: Version bump**

Per the Global Constraints rule: set `[workspace.package] version` in the root `Cargo.toml` to one MINOR above the parent branch's release (0.19.0 if the payload-contract corpus shipped 0.18.0; 0.18.0 otherwise), and bump the `phr`/`phronesis-rhai` path-dep version pins in `crates/phronesis-mcp/Cargo.toml` to match. New config surface + subcommand + journal field + env var = MINOR (spec §Versioning).

- [ ] **Step 3: Full verification**

Run each, bare, in order:

```bash
cargo fmt --all
cargo clippy --workspace -- -D warnings
cargo build --workspace
cargo test --workspace
```

Expected: fmt clean, zero clippy warnings, build succeeds, all tests pass. Do not pipe any of these.

- [ ] **Step 4: Commit**

```bash
git add crates/phronesis-mcp/CLAUDE.md Cargo.toml crates/phronesis-mcp/Cargo.toml Cargo.lock
git commit -m "docs+version: neutral toolchain outcomes — MINOR bump, CLAUDE.md surfaces

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Self-review checklist (run after writing, already applied)

- **Spec coverage:** §1 two-tier model → Task 2; §2 toolchains.json → Tasks 1, 7; §3 adapter layer + cargo fidelity → Task 3; §4 exit-code capture (extract/record/honesty, `command_exit` naming, action-log field) → Task 4; §5 compaction → Task 5; §Surfaces (`toolchains` CLI, `init`, unchanged `confidence`) → Tasks 6, 7; §Testing (fidelity/generic/pytest/exit-capture/compaction) → Tasks 3, 2, 2+7, 4, 5; §Versioning → Task 8. Open questions all resolved in the "Design decisions" header.
- **Known deviation, documented:** named capture groups instead of positional (header decision 2).
- **Type consistency:** `parse(subject, command, output, command_exit: Option<i32>)` is identical on the trait, `ConfigAdapter`, and `CompiledDef`; `extract_from` five-arg shape matches between Task 3's definition and Task 4's caller; `payload_command_exit` is `pub(super)` for `post.rs`; `maybe_compact(root, max_bytes, tail_records)` matches all test call sites.
