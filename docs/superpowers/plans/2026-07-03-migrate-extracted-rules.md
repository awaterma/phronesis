# migrate-extracted-rules Command Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `phr-mcp migrate-extracted-rules <path> [--dry-run]`, which rewrites pre-0.14.0 `extract_rules` output in a rules.json: strips bracketed metadata prefixes from messages, demotes `block` to `warn`, and demotes to `log` any rule duplicating a structural Rust-pack rule.

**Architecture:** A new pure-logic module `migrate_extracted.rs` operates on `Vec<SourceRule>` (no I/O), so unit tests need no filesystem. A thin CLI arm in `main.rs` wires it to `rules_file::read_source`/`write_source`, which provide `.json.bak` backup and atomic write for free. Detection of "extracted" rules keys on the `markdown_rule` predicate — the one signal unique to extractor output — not on rule-id naming.

**Tech Stack:** Rust, clap (derive), serde_json; existing `rules_file` API.

**Source specs:** `docs/specs/SPEC-extract-rules-defaults.md` §"Salvage path" and §"Rollout plan" step 1; deferred-work note in `CHANGELOG.md` 0.14.0.

## Global Constraints

- Branch: `fix/migrate-extracted-rules` off `main` (pre-feature anchor `v0.16.1` already tags `main` HEAD `b477a0f`). Use an isolated worktree (superpowers:using-git-worktrees) at execution time.
- Release: phr-mcp **0.16.2** (PATCH — the project's own CHANGELOG classifies this command as "a follow-up PATCH").
- Every commit: `cargo test --workspace` green, `cargo clippy --workspace --all-targets -- -D warnings` clean, `cargo fmt --all -- --check` clean.
- `phr-mcp audit` total must not increase (currently 59). New code must stay under 8 outer-scope `let`s per function — do not add audit debt while the decomposition plan is in flight.
- **No push, ever, without explicit human approval** (durable project rule).
- Strip **five** prefixes, not the spec's four: pre-0.14.0 extraction also emitted `[directive]` (verified against `git show 7883865`).
- The five prefixes always end `"] "` (old code: `format!("[{}] {}", kind, text)`).
- Demotion is monotonic: never raise an action's severity. Ranks: `constraint_violation` (block) = 2, `constraint_warning` (warn) = 1, anything else (`log`, …) = 0. Target = `log` if the message matches the structural keyword table, else `warn`; apply only when current rank exceeds target rank.
- Structural keyword table (SPEC Problem 4a, matched case-insensitively against the message): `unwrap`, `clone`, `deref`, `&string`, `&vec`, `thiserror`. These correspond to shipped rules `enforce-no-unwrap-in-src`, `warn-clone-heavy`, `warn-deref-for-non-pointer-type`, `warn-rust-public-fn-takes-string-ref`, `warn-rust-public-fn-takes-vec-ref`, `enforce-no-result-string-error`.

---

### Task 1: `migrate_extracted` core module (pure logic, TDD)

**Files:**
- Create: `crates/phronesis-mcp/src/migrate_extracted.rs`
- Modify: `crates/phronesis-mcp/src/lib.rs` (add `pub mod migrate_extracted;` in alphabetical order, after `pub mod memory_drift;` at line 13)

**Interfaces:**
- Consumes: `crate::rules_file::{SourceRule, WhenClause, DiskAction}` (see `rules_file.rs:92` for `SourceRule` fields, `:16` for `WhenClause`, `:393` for `DiskAction { action_type: String, params: Vec<String> }`).
- Produces: `pub fn migrate_extracted(sources: &mut [SourceRule]) -> MigrationSummary` and `pub struct MigrationSummary { pub examined: usize, pub changed: usize, pub prefixes_stripped: usize, pub demoted_to_warn: usize, pub demoted_to_log: usize }` (all fields pub; derive `Debug, Default, PartialEq, serde::Serialize`).

- [ ] **Step 1: Write the failing tests**

Create `crates/phronesis-mcp/src/migrate_extracted.rs` with an empty implementation surface and this test module (the `then` verb key is the v2 on-disk form; `SourceRule`'s custom `Deserialize` parses it):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules_file::SourceRule;

    fn rule(verb: &str, msg: &str, extracted: bool) -> SourceRule {
        let when = if extracted {
            serde_json::json!([{ "markdown_rule": ["docs/RUST-PATTERNS-GUIDE.md", "Anti-Patterns"] }])
        } else {
            serde_json::json!([{ "new_content_contains": ".unwrap()" }])
        };
        // json! doesn't accept computed keys, so insert the verb after the fact
        let mut obj = serde_json::json!({
            "id": "rust-patterns-guide-anti-patterns-12",
            "phase": "pre",
            "priority": 5,
            "when": when,
            "then": {}
        });
        obj["then"][verb] = serde_json::Value::String(msg.to_string());
        serde_json::from_value(obj).unwrap()
    }

    fn msg(r: &SourceRule) -> &str {
        &r.then.params[0]
    }

    #[test]
    fn strips_all_five_prefixes() {
        for prefix in ["pattern", "anti_pattern", "context", "problem", "directive"] {
            let mut rules = vec![rule("warn", &format!("[{prefix}] Use the thing."), true)];
            let summary = migrate_extracted(&mut rules);
            assert_eq!(msg(&rules[0]), "Use the thing.", "prefix [{prefix}] not stripped");
            assert_eq!(summary.prefixes_stripped, 1);
        }
    }

    #[test]
    fn demotes_block_to_warn() {
        let mut rules = vec![rule("block", "Prefer iterators over index loops.", true)];
        let summary = migrate_extracted(&mut rules);
        assert_eq!(rules[0].then.action_type, "constraint_warning");
        assert_eq!(summary.demoted_to_warn, 1);
        assert_eq!(summary.demoted_to_log, 0);
    }

    #[test]
    fn demotes_structural_duplicate_block_to_log() {
        let mut rules = vec![rule("block", "[anti_pattern] Avoid: Clone to Satisfy Borrow Checker", true)];
        let summary = migrate_extracted(&mut rules);
        assert_eq!(rules[0].then.action_type, "log");
        assert_eq!(msg(&rules[0]), "Avoid: Clone to Satisfy Borrow Checker");
        assert_eq!(summary.demoted_to_log, 1);
    }

    #[test]
    fn demotes_structural_duplicate_warn_to_log() {
        let mut rules = vec![rule("warn", "Never call .unwrap() in src.", true)];
        let summary = migrate_extracted(&mut rules);
        assert_eq!(rules[0].then.action_type, "log");
        assert_eq!(summary.demoted_to_log, 1);
    }

    #[test]
    fn plain_warn_stays_warn_and_log_stays_log() {
        let mut rules = vec![
            rule("warn", "Prefer builders for many-arg constructors.", true),
            rule("log", "Reference note.", true),
        ];
        let summary = migrate_extracted(&mut rules);
        assert_eq!(rules[0].then.action_type, "constraint_warning");
        assert_eq!(rules[1].then.action_type, "log");
        assert_eq!(summary.changed, 0);
    }

    #[test]
    fn non_extracted_rules_are_untouched() {
        let mut rules = vec![rule("block", "[pattern] Should not change.", false)];
        let summary = migrate_extracted(&mut rules);
        assert_eq!(rules[0].then.action_type, "constraint_violation");
        assert_eq!(msg(&rules[0]), "[pattern] Should not change.");
        assert_eq!(summary.examined, 0);
        assert_eq!(summary.changed, 0);
    }

    #[test]
    fn idempotent_second_run_changes_nothing() {
        let mut rules = vec![rule("block", "[problem] Overuse of unwrap panics in prod.", true)];
        let first = migrate_extracted(&mut rules);
        assert_eq!(first.changed, 1);
        let second = migrate_extracted(&mut rules);
        assert_eq!(second.changed, 0);
        assert_eq!(second.prefixes_stripped, 0);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p phronesis-mcp migrate_extracted`
Expected: compile error — `migrate_extracted` / `MigrationSummary` not defined.

- [ ] **Step 3: Write the implementation**

Above the test module in the same file:

```rust
//! Salvage migration for rules produced by `extract_rules` before 0.14.0
//! (block-action, bracket-prefixed messages). Implements the "Salvage
//! path" in docs/specs/SPEC-extract-rules-defaults.md.

use crate::rules_file::{SourceRule, WhenClause};

/// Message keywords covered by structural Rust-pack rules
/// (SPEC-extract-rules-defaults Problem 4a). Matched case-insensitively.
const STRUCTURAL_KEYWORDS: &[&str] = &["unwrap", "clone", "deref", "&string", "&vec", "thiserror"];

/// Extraction-time discriminators the old extractor prefixed onto messages.
const PREFIXES: &[&str] = &[
    "[pattern] ",
    "[anti_pattern] ",
    "[context] ",
    "[problem] ",
    "[directive] ",
];

#[derive(Debug, Default, PartialEq, serde::Serialize)]
pub struct MigrationSummary {
    pub examined: usize,
    pub changed: usize,
    pub prefixes_stripped: usize,
    pub demoted_to_warn: usize,
    pub demoted_to_log: usize,
}

/// A rule is "extracted" iff any `when` leaf uses the `markdown_rule`
/// predicate — the one condition shape unique to extractor output.
fn is_extracted(rule: &SourceRule) -> bool {
    rule.when.iter().any(clause_has_markdown_rule)
}

fn clause_has_markdown_rule(clause: &WhenClause) -> bool {
    match clause {
        WhenClause::Leaf(cond) => cond.predicate == "markdown_rule",
        WhenClause::Or(alts) => alts.iter().any(clause_has_markdown_rule),
    }
}

fn severity_rank(action_type: &str) -> u8 {
    match action_type {
        "constraint_violation" => 2,
        "constraint_warning" => 1,
        _ => 0,
    }
}

fn duplicates_structural_rule(message: &str) -> bool {
    let lower = message.to_lowercase();
    STRUCTURAL_KEYWORDS.iter().any(|k| lower.contains(k))
}

/// Rewrite extracted rules in place. Non-extracted rules are untouched.
pub fn migrate_extracted(sources: &mut [SourceRule]) -> MigrationSummary {
    let mut summary = MigrationSummary::default();
    for rule in sources.iter_mut().filter(|r| is_extracted(r)) {
        summary.examined += 1;
        let mut rule_changed = false;

        if let Some(message) = rule.then.params.first_mut() {
            if let Some(stripped) = PREFIXES.iter().find_map(|p| message.strip_prefix(p)) {
                *message = stripped.to_string();
                summary.prefixes_stripped += 1;
                rule_changed = true;
            }
        }

        let message = rule.then.params.first().map(String::as_str).unwrap_or("");
        let (target, is_log) = if duplicates_structural_rule(message) {
            ("log", true)
        } else {
            ("constraint_warning", false)
        };
        if severity_rank(&rule.then.action_type) > severity_rank(target) {
            rule.then.action_type = target.to_string();
            if is_log {
                summary.demoted_to_log += 1;
            } else {
                summary.demoted_to_warn += 1;
            }
            rule_changed = true;
        }

        if rule_changed {
            summary.changed += 1;
        }
    }
    summary
}
```

Add `pub mod migrate_extracted;` to `crates/phronesis-mcp/src/lib.rs` after line 13 (`pub mod memory_drift;`).

Note: if `SourceRule.then` or `DiskAction` fields are not visible from the new module, check `rules_file.rs:92`/`393` — both are `pub` per the current source; do not widen anything without confirming it's actually needed.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p phronesis-mcp migrate_extracted`
Expected: 7 passed.

- [ ] **Step 5: Quality gate and commit**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: all green.

```bash
git add crates/phronesis-mcp/src/migrate_extracted.rs crates/phronesis-mcp/src/lib.rs
git commit -m "feat(migrate): add extracted-rules salvage core (SPEC-extract-rules-defaults)"
```

---

### Task 2: CLI wiring + integration test

**Files:**
- Modify: `crates/phronesis-mcp/src/main.rs` — new `Command` variant after `MigrateRules` (ends line 161), new match arm after the `MigrateRules` arm (ends line 671), extend the `use phronesis_mcp::` import at line 16.
- Create: `crates/phronesis-mcp/tests/migrate_extracted_integration.rs`
- Model to copy: `crates/phronesis-mcp/tests/migrate_integration.rs` (the `CARGO_BIN_EXE_phr-mcp` spawn pattern and its `run_migrate` helper).

**Interfaces:**
- Consumes: `migrate_extracted::migrate_extracted` (Task 1), `rules_file::{read_source, write_source}` (`rules_file.rs:446`, `:566` — `write_source` backs up to `.json.bak` and writes atomically).
- Produces: the `phr-mcp migrate-extracted-rules` subcommand; exit 0 on success (including "nothing to do"), exit 1 with `error:` on read failure.

- [ ] **Step 1: Write the failing integration test**

Create `crates/phronesis-mcp/tests/migrate_extracted_integration.rs`:

```rust
use std::fs;
use std::process::Command;

fn run_cmd(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .arg("migrate-extracted-rules")
        .args(args)
        .output()
        .expect("failed to spawn phr-mcp")
}

const OLD_SHAPE: &str = r#"{
  "rules": [
    {
      "id": "rust-patterns-guide-anti-patterns-12",
      "phase": "pre",
      "priority": 5,
      "when": [ { "markdown_rule": ["docs/RUST-PATTERNS-GUIDE.md", "Anti-Patterns"] } ],
      "then": { "block": "[anti_pattern] Overuse of unwrap() panics in production." }
    },
    {
      "id": "rust-patterns-guide-idioms-1",
      "phase": "pre",
      "priority": 5,
      "when": [ { "markdown_rule": ["docs/RUST-PATTERNS-GUIDE.md", "Idioms"] } ],
      "then": { "block": "[pattern] Prefer iterators over index loops." }
    },
    {
      "id": "enforce-no-unwrap-in-src",
      "phase": "pre",
      "priority": 8,
      "when": [ { "new_content_contains": ".unwrap()" }, { "file_path_matches": "src" } ],
      "then": { "block": "No .unwrap() in src/." }
    }
  ]
}"#;

#[test]
fn migrates_extracted_rules_in_place_with_backup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.json");
    fs::write(&path, OLD_SHAPE).unwrap();

    let out = run_cmd(&[path.to_str().unwrap()]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

    let migrated = fs::read_to_string(&path).unwrap();
    // unwrap-keyword rule → log, prefix gone
    assert!(migrated.contains(r#""log": "Overuse of unwrap() panics in production.""#));
    // plain pattern rule → warn, prefix gone
    assert!(migrated.contains(r#""warn": "Prefer iterators over index loops.""#));
    // structural rule untouched (still block, message intact)
    assert!(migrated.contains(r#""block": "No .unwrap() in src/.""#));
    assert!(!migrated.contains("[pattern]"));
    assert!(!migrated.contains("[anti_pattern]"));
    // backup written
    assert!(path.with_extension("json.bak").exists());
}

#[test]
fn dry_run_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.json");
    fs::write(&path, OLD_SHAPE).unwrap();

    let out = run_cmd(&["--dry-run", path.to_str().unwrap()]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(r#""warn": "Prefer iterators over index loops.""#));
    assert_eq!(fs::read_to_string(&path).unwrap(), OLD_SHAPE);
    assert!(!path.with_extension("json.bak").exists());
}

#[test]
fn nothing_to_migrate_reports_and_exits_zero() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.json");
    fs::write(
        &path,
        r#"{"rules":[{"id":"x","phase":"pre","priority":5,"when":[{"new_content_contains":"todo!"}],"then":{"warn":"No todo!"}}]}"#,
    )
    .unwrap();

    let out = run_cmd(&[path.to_str().unwrap()]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("no extracted rules"));
    assert!(!path.with_extension("json.bak").exists());
}
```

Check first whether `tempfile` is already a dev-dependency of phronesis-mcp (`grep tempfile crates/phronesis-mcp/Cargo.toml`); `migrate_integration.rs` almost certainly uses it — mirror whatever it does.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p phronesis-mcp --test migrate_extracted_integration`
Expected: FAIL — clap rejects the unknown subcommand (`error: unrecognized subcommand`).

- [ ] **Step 3: Add the CLI variant and match arm**

In the `Command` enum, directly after the `MigrateRules` variant (after line 161):

```rust
    /// Rewrite pre-0.14.0 `extract_rules` output in a rules.json: strip
    /// bracketed metadata prefixes ([pattern], [anti_pattern], ...), demote
    /// `block` to `warn`, and demote to `log` rules the Rust pack already
    /// enforces structurally. Idempotent. Backs up to rules.json.bak.
    #[command(name = "migrate-extracted-rules")]
    MigrateExtractedRules {
        /// Path to the rules.json file to rewrite.
        path: PathBuf,
        /// Print the migrated JSON to stdout; write nothing.
        #[arg(long)]
        dry_run: bool,
    },
```

In `main()`, directly after the `MigrateRules` arm (after line 671), mirroring its dry-run wrapper exactly (see main.rs:652–662):

```rust
        Command::MigrateExtractedRules { path, dry_run } => {
            let mut sources: Vec<SourceRule> =
                rules_file::read_source(&path).map_err(|e| anyhow::anyhow!("{}", e))?;
            let summary = migrate_extracted::migrate_extracted(&mut sources);

            if summary.changed == 0 {
                println!("no extracted rules to migrate in {}", path.display());
                return Ok(());
            }

            if dry_run {
                #[derive(serde::Serialize)]
                struct Wrapper<'a> {
                    rules: &'a [SourceRule],
                }
                println!(
                    "{}",
                    serde_json::to_string_pretty(&Wrapper { rules: &sources })?
                );
                return Ok(());
            }

            rules_file::write_source(&path, &sources).map_err(|e| anyhow::anyhow!("{}", e))?;
            println!(
                "migrated {} extracted rule(s) in {} ({} prefix(es) stripped, {} demoted to warn, {} demoted to log)",
                summary.changed,
                path.display(),
                summary.prefixes_stripped,
                summary.demoted_to_warn,
                summary.demoted_to_log
            );
            Ok(())
        }
```

Extend the import at main.rs:16 to include `migrate_extracted` (and confirm `rules_file` + `SourceRule` are already imported for the `MigrateRules` arm — reuse whatever paths that arm uses).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p phronesis-mcp --test migrate_extracted_integration`
Expected: 3 passed.

- [ ] **Step 5: Quality gate and commit**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all -- --check && phr-mcp audit | tail -2`
Expected: all green; audit total still 59.

```bash
git add crates/phronesis-mcp/src/main.rs crates/phronesis-mcp/tests/migrate_extracted_integration.rs
git commit -m "feat(cli): add migrate-extracted-rules salvage command"
```

---

### Task 3: Release chores (0.16.2)

**Files:**
- Modify: `crates/phronesis-mcp/Cargo.toml` (version → `0.16.2`), `Cargo.lock` (via `cargo build`), `CHANGELOG.md`, `docs/specs/SPEC-extract-rules-defaults.md` (status line), crate docs if they enumerate CLI commands (`grep -rn "migrate-rules" crates/phronesis-mcp/CLAUDE.md README.md crates/phronesis-mcp/README.md` and mirror the new command wherever migrate-rules is documented).

- [ ] **Step 1: Bump version and update docs**

Set `version = "0.16.2"` in `crates/phronesis-mcp/Cargo.toml`; run `cargo build -p phronesis-mcp` to refresh `Cargo.lock`.

Add to `CHANGELOG.md` above the 0.16.1 entry:

```markdown
## [0.16.2] - <today's date>

### Added
- **`phr-mcp migrate-extracted-rules <path> [--dry-run]`** — the salvage
  command deferred from 0.14.0. Rewrites pre-0.14.0 `extract_rules` output
  in place (with a `.bak` backup): strips the bracketed extraction-time
  prefixes (`[pattern]`, `[anti_pattern]`, `[context]`, `[problem]`,
  `[directive]`) from messages, demotes `block` actions to `warn`, and
  demotes to `log` any extracted rule duplicating a structural Rust-pack
  rule (the SPEC's static keyword table: unwrap, clone, Deref, &String,
  &Vec, thiserror). Extracted rules are detected by their `markdown_rule`
  condition, so hand-written rules are never touched. Idempotent.
  Implements the salvage path in `docs/specs/SPEC-extract-rules-defaults.md`.
```

(Add the matching `[0.16.2]: https://github.com/awaterma/phronesis/releases/tag/v0.16.2` link line at the bottom.)

In `docs/specs/SPEC-extract-rules-defaults.md`, update the salvage-path paragraph (lines ~193–195) to note the command shipped in 0.16.2, and note in the rollout plan that step 1 is now complete.

- [ ] **Step 2: Final verification**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: green. Then a live smoke test against a scratch copy:

```bash
cp .phronesis/rules.json /tmp/scratch-rules.json 2>/dev/null || true
cargo run -p phronesis-mcp --bin phr-mcp -- migrate-extracted-rules --dry-run /tmp/scratch-rules.json
```

Expected: either "no extracted rules to migrate" (local file was hand-salvaged already) or a sensible dry-run dump. Never write to the real `.phronesis/rules.json` during verification.

- [ ] **Step 3: Commit**

```bash
git add crates/phronesis-mcp/Cargo.toml Cargo.lock CHANGELOG.md docs/specs/SPEC-extract-rules-defaults.md
git commit -m "chore(release): phr-mcp 0.16.2 — migrate-extracted-rules"
```

- [ ] **Step 4: STOP — human review**

Do not push, do not merge. Present the branch diff to the human for review (superpowers:finishing-a-development-branch). The decomposition plan branches off `main` *after* this merges.
