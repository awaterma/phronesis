# Phronesis Evidence Gaps Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Revision 2 (2026-09-28).** Revised after two independent reviews, GLM-5.3 (text-only) and Codex gpt-6-luna (read-only against the repo at `636ba0b`). Their findings and the orchestrator's dispositions are in `docs/superpowers/plans/reviews/2026-09-28-phronesis-evidence-gaps/consolidated.md`. Every accepted finding is folded in below; where a reviewer's premise was wrong, the task says why.

**Goal:** Close the five places where Phronesis fell short during the 2026-09-27 god-file-split swarm, so the next swarm's audit, lifecycle, confidence, worktree seeding, and Kani verification produce real evidence instead of silence.

**Architecture:** Six parts, each shippable as its own PR with a failing-first test. Parts A, B, C and E change `crates/phronesis-mcp`. Part D changes the swarming tool in the `agent-skills` repository. Dependencies: **E3 uses C3's `signal ingest`**, so C ships before E3 (E1 and E2 are independent); **D1 owns the `harness-workers.md` edit** in agent-skills, and C3 only references it, so the skill version is bumped once. A and B have no dependencies.

**Tech Stack:** Rust (workspace edition 2024, MSRV 1.90), `regex`, `ignore`, `serde_json`, `clap`, `tempfile`/`assert_cmd` in tests. `phr-mcp` hooks are active in this checkout.

**Spec:** GitHub issue #114 (the `.phronesisignore` blind spot) and the 2026-09-28 swarm retrospective recorded in `/Volumes/Data/Git/phronesis-wt/swarm-godsplit-1/README.md`; there is no separate spec document. Each part's "Shortfall" paragraph is the requirement it implements.

## Global Constraints

- Every fix is one PR: failing-first regression test, `cargo build --workspace`, `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, CI green, independent review, a `CHANGELOG.md` entry under `## [Unreleased]`. CI (`.github/workflows/ci.yml:12-29`) runs exactly that gate plus `phr-mcp audit --fail-on block` on Rust 1.90.
- Run `phr-mcp coverage select` before `git add` and run every test it lists; if it reports a stale graph, run `phr-mcp graph rebuild` first.
- No `.unwrap()` in production paths under `src/` (`enforce-no-unwrap-in-src` blocks at hook time); use `?` or `expect("<invariant>")`. Never turn a data-load error into "empty" with `unwrap_or_default()`: report it.
- No `unsafe` block without a `// SAFETY:` line.
- MSRV 1.90: `Option::is_none_or` (1.82), `is_some_and` (1.70), `OnceLock` (1.70), let-else, `if let` chains (edition 2024) are all allowed. Run `cargo fmt --all` on pasted snippets before clippy; the plan's snippets are dense on purpose.
- Conventional-commit messages; PR titles conventional-commit shaped (squash merges).
- This repository is public: never write the downstream consumer's project name into any file.
- Commit messages end with `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>` when Claude authors them.
- Part D lives in `/Volumes/Data/Git/agent-skills/skills/swarming/tool`; after editing that skill, repackage per the agent-skills sync procedure (`package_codex.py`, prune removed files, bump the skill version) once, in Task D1.

## Review Focus

1. **A file excluded by both `.gitignore` and `.phronesisignore`** must not be reported as a policy exclusion, and a hidden file the walker skips by default must not either. Pinned in Task A1 step 1.
2. **A `git -C <dir>` that is not the head-moving invocation** (`git -C /x diff && git commit`) must not redirect the HEAD probe to `/x`; a relative or ambiguous directory must fall back to the project root, never to a guessed one. Pinned in Task B1 step 1.
3. **A `git -C <sibling-repo> commit`** stays a non-commit for this project even though HEAD moved there. Pinned in Task B2 step 1 (the existing `dry_run_heredoc_and_sibling_repo_are_not_commits` stays green).
4. **A redirect target that is stale, a symlink out of the project, a FIFO, or outside the project root** is never read as evidence. Pinned in Task C2 step 1.
5. **A Kani run where a harness header has no verdict** (killed run, compiler error mid-way) must not pair that header with the next harness's verdict. Pinned in Task E1 step 1.

---

## Part A — `.phronesisignore` scopes lexical rules only, and is visible (issue #114)

**Shortfall.** `crates/phronesis-mcp/.phronesisignore` excluded `src/init.rs` wholesale so the pack rules' inline JSON would not trip their own string matchers. The exclusion also removed the file from every structural (AST) rule, hiding nine production `.unwrap()` calls for the life of the entry, with nothing in the audit output saying so. Proven: identical bytes at `src/init.rs` → 0 hits; renamed → 9 hits.

**Decisions.** (1) An ignore entry exempts a file from **lexical** rules only. (2) Classification is **per rule, not per predicate**: a rule with any AST predicate (`rule_has_ast_predicate`, `audit/engine.rs:95`) is structural and runs its whole `when` list on excluded files; a rule with no AST predicate is lexical and skips them. Mixed rules are rare in the packs and their lexical predicates narrow, never widen, the structural hit. (3) The report names excluded files. (4) Reviewer premise corrected: `src/init.rs` no longer exists (#112), so removing the stale ignore line cannot turn CI red; it is folded into Task A2's commit anyway so no intermediate state exists.

### Task A1: Discovery returns the excluded set

**Files:**
- Modify: `crates/phronesis-mcp/src/audit/run.rs` (add `Discovery` + `discover_files_with_excluded` next to `discover_files` at ~line 144)
- Test: `crates/phronesis-mcp/src/audit/tests/engine_tests.rs` (append)

**Interfaces:**
- Produces: `pub struct Discovery { pub scanned: Vec<PathBuf>, pub excluded: Vec<PathBuf> }` (both sorted) and `pub fn discover_files_with_excluded(root: &Path, extensions: &[&str]) -> Discovery`. `discover_files` keeps its signature and returns `.scanned`.

- [ ] **Step 1: Write the failing test**

Append to `crates/phronesis-mcp/src/audit/tests/engine_tests.rs`:

```rust
#[test]
fn discovery_reports_only_phronesisignore_exclusions_at_any_level() {
    use crate::audit::run::discover_files_with_excluded;
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::create_dir_all(root.join("src/deep")).expect("mkdir");
    std::fs::write(root.join("src/kept.rs"), "fn a() {}\n").expect("write");
    std::fs::write(root.join("src/dropped.rs"), "fn b() {}\n").expect("write");
    std::fs::write(root.join("src/deep/nested.rs"), "fn c() {}\n").expect("write");
    // Excluded by .gitignore AND .phronesisignore: not a policy exclusion.
    std::fs::write(root.join("src/generated.rs"), "fn g() {}\n").expect("write");
    // Hidden file: the walker skips it by default; never reported either.
    std::fs::write(root.join("src/.hidden.rs"), "fn h() {}\n").expect("write");
    std::fs::write(root.join(".gitignore"), "src/generated.rs\n").expect("write");
    std::fs::write(root.join(".phronesisignore"), "src/dropped.rs\nsrc/generated.rs\n").expect("write");
    std::fs::write(root.join("src/deep/.phronesisignore"), "nested.rs\n").expect("write");

    let d = discover_files_with_excluded(root, &["rs"]);
    let names = |v: &[std::path::PathBuf]| -> Vec<String> {
        v.iter()
            .map(|p| p.strip_prefix(root).expect("under root").to_string_lossy().to_string())
            .collect()
    };
    assert_eq!(names(&d.scanned), vec!["src/kept.rs"]);
    assert_eq!(names(&d.excluded), vec!["src/deep/nested.rs", "src/dropped.rs"]);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p phronesis-mcp --lib -- audit::tests::engine_tests::discovery_reports_only_phronesisignore_exclusions_at_any_level --exact`
Expected: FAIL with `cannot find function discover_files_with_excluded`.

- [ ] **Step 3: Write minimal implementation**

In `crates/phronesis-mcp/src/audit/run.rs`, above `discover_files`:

```rust
/// Files the walker accepted, split by whether a `.phronesisignore` entry
/// excluded them. Both walks honour `.gitignore`, hidden-file defaults and
/// walker errors identically, so a file that only `.gitignore` drops is in
/// neither set: `excluded` is `.phronesisignore` policy and nothing else.
#[derive(Debug, Default)]
pub struct Discovery {
    /// Sorted.
    pub scanned: Vec<PathBuf>,
    /// Excluded by `.phronesisignore` (at any directory level). Sorted.
    /// Structural rules still scan these; only lexical rules skip them.
    pub excluded: Vec<PathBuf>,
}

/// Like [`discover_files`], but also returns the files a `.phronesisignore`
/// excluded: one walk honours the custom ignore file, one does not, and the
/// difference is the policy exclusions. `.phronesisignore` files themselves
/// are never reported.
pub fn discover_files_with_excluded(root: &Path, extensions: &[&str]) -> Discovery {
    use ignore::WalkBuilder;
    use std::collections::BTreeSet;

    fn walk(root: &Path, extensions: &[&str], honour_phronesisignore: bool) -> BTreeSet<PathBuf> {
        let wildcard = extensions.contains(&"*");
        let mut builder = WalkBuilder::new(root);
        builder.follow_links(false);
        if honour_phronesisignore {
            builder.add_custom_ignore_filename(".phronesisignore");
        }
        let mut out = BTreeSet::new();
        for entry in builder.build().flatten() {
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let path = entry.into_path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if wildcard || extensions.contains(&ext) {
                out.insert(path);
            }
        }
        out
    }

    let with_ignore = walk(root, extensions, true);
    let without_ignore = walk(root, extensions, false);
    Discovery {
        excluded: without_ignore
            .difference(&with_ignore)
            .filter(|p| p.file_name().is_none_or(|n| n != ".phronesisignore"))
            .cloned()
            .collect(),
        scanned: with_ignore.into_iter().collect(),
    }
}
```

Replace the body of `discover_files` with `discover_files_with_excluded(root, extensions).scanned` (keep its doc comment).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p phronesis-mcp --lib -- audit::tests 2>&1 | tail -3`
Expected: the new test passes and every pre-existing `audit::tests` test still passes.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/audit/run.rs crates/phronesis-mcp/src/audit/tests/engine_tests.rs
git commit -m "feat(audit): discovery reports the files .phronesisignore excludes"
```

### Task A2: Structural rules run on excluded files; the report says so; the stale entry goes

**Files:**
- Modify: `crates/phronesis-mcp/src/audit/types.rs:102-108` (`AuditReport`)
- Modify: `crates/phronesis-mcp/src/audit/run.rs:32-93` (`run_core`)
- Modify: `crates/phronesis-mcp/src/audit/render.rs:13-20, 90-100, 113-` (`render_table`, `render_json`)
- Modify: `crates/phronesis-mcp/.phronesisignore` (delete the `src/init.rs` line and its two comment lines)
- Test: `crates/phronesis-mcp/src/audit/tests/engine_tests.rs` (append)

**Interfaces:**
- Consumes: `Discovery` (A1); `rule_has_ast_predicate(&DiskRule) -> bool` (`audit/engine.rs:95`, `pub(super)` since #113, visible to `run.rs`); `crate::security::max_file_bytes()` (the runtime-configurable cap; see `security.rs:37-62`).
- Produces: `AuditReport.lexical_excluded: Vec<PathBuf>` (relative to `scan_root`, sorted). `files_scanned` counts every file offered to a scan, scanned and excluded alike; the docs say so. `render_json` emits `"lexical_excluded": [...]`; `render_table` appends one footer line when non-empty: `N file(s) excluded from lexical rules by .phronesisignore (structural rules still ran): a, b, c, d, e, +K more`.

- [ ] **Step 1: Write the failing test**

Append to `crates/phronesis-mcp/src/audit/tests/engine_tests.rs`:

```rust
/// Policy pinned here: classification is per rule. A rule with any AST
/// predicate is structural and runs its whole `when` list on an excluded
/// file (its lexical predicates only narrow the hit); a rule with no AST
/// predicate is lexical and skips excluded files.
#[test]
fn a_phronesisignored_file_is_scanned_by_structural_rules_only() {
    use crate::audit::run::run_core;
    use crate::audit::types::AuditOpts;
    use crate::rules_file::RulesFile;

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::create_dir_all(root.join("src")).expect("mkdir");
    std::fs::write(
        root.join("src/hidden.rs"),
        "pub fn f(x: Option<u8>) -> u8 {\n    // TODO tidy\n    x.unwrap()\n}\n",
    )
    .expect("write");
    std::fs::write(root.join(".phronesisignore"), "src/hidden.rs\n").expect("write");
    let rules: RulesFile = serde_json::from_str(
        r#"{"rules": [
            {"id": "no-unwrap", "phase": "pre", "priority": 1, "audit": true,
             "when": [{"rust_governed_invocation": ["?file", "?fn", "unwrap"]},
                      {"file_path_matches": "src"}],
             "then": {"block": "no unwrap"}},
            {"id": "mixed-unwrap-near-todo", "phase": "post", "priority": 1, "audit": true,
             "when": [{"rust_governed_invocation": ["?file", "?fn", "unwrap"]},
                      {"new_content_contains": "TODO"}],
             "then": {"warn": "structural rule with a lexical narrowing predicate"}},
            {"id": "no-todo", "phase": "post", "priority": 1, "audit": true,
             "when": [{"new_content_contains": "TODO"}, {"file_path_matches": "src"}],
             "then": {"warn": "no todo"}}
        ]}"#,
    )
    .expect("rules parse");
    let opts = AuditOpts {
        project_root: root.to_path_buf(),
        scan_root: root.to_path_buf(),
        rule_filter: None,
    };

    let report = run_core(&opts, &rules, None);

    let ids: Vec<&str> = report.per_rule.iter().map(|r| r.rule_id.as_str()).collect();
    assert!(ids.contains(&"no-unwrap"), "structural rule fires on an ignored file: {ids:?}");
    assert!(ids.contains(&"mixed-unwrap-near-todo"), "a rule with an AST predicate is structural even with a lexical narrowing predicate: {ids:?}");
    assert!(!ids.contains(&"no-todo"), "a purely lexical rule skips an ignored file: {ids:?}");
    assert_eq!(report.lexical_excluded, vec![std::path::PathBuf::from("src/hidden.rs")]);
    assert_eq!(report.files_scanned, 1, "files offered to any scan, excluded included");
    let table = crate::audit::render_table(&report, false);
    assert!(
        table.contains("1 file(s) excluded from lexical rules by .phronesisignore"),
        "table footer names the exclusion: {table}"
    );
    let json: serde_json::Value =
        serde_json::from_str(&crate::audit::render_json(&report)).expect("json");
    assert_eq!(json["lexical_excluded"], serde_json::json!(["src/hidden.rs"]));
}

#[test]
fn an_excluded_file_over_the_size_cap_is_reported_but_not_structurally_scanned() {
    use crate::audit::run::run_core;
    use crate::audit::types::AuditOpts;
    use crate::rules_file::RulesFile;

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::create_dir_all(root.join("src")).expect("mkdir");
    let body = format!("pub fn f(x: Option<u8>) -> u8 {{ x.unwrap() }}\n// {}\n", "x".repeat(2048));
    std::fs::write(root.join("src/big.rs"), &body).expect("write");
    std::fs::write(root.join(".phronesisignore"), "src/big.rs\n").expect("write");
    let rules: RulesFile = serde_json::from_str(
        r#"{"rules": [{"id": "no-unwrap", "phase": "pre", "priority": 1, "audit": true,
             "when": [{"rust_governed_invocation": ["?file", "?fn", "unwrap"]}],
             "then": {"block": "no unwrap"}}]}"#,
    )
    .expect("rules parse");
    let opts = AuditOpts { project_root: root.to_path_buf(), scan_root: root.to_path_buf(), rule_filter: None };

    // Cap below the file size via the documented runtime override.
    // SAFETY note for reviewers: set_var in tests is process-global; this test
    // must not run in parallel with other tests reading the same variable, so
    // it takes the crate's env-test lock if one exists (grep `ENV_LOCK`).
    temp_env::with_var("PHRONESIS_MAX_FILE_BYTES", Some("1024"), || {
        let report = run_core(&opts, &rules, None);
        assert!(report.per_rule.is_empty(), "no structural scan over the cap: {:?}", report.per_rule);
        assert_eq!(report.lexical_excluded, vec![std::path::PathBuf::from("src/big.rs")]);
    });
}
```

If the crate does not already depend on `temp-env`, use whatever the existing tests in `security.rs` use to set `PHRONESIS_MAX_FILE_BYTES` (grep `PHRONESIS_MAX_FILE_BYTES` under `src/`), and copy that pattern instead. If `AuditOpts` has fields beyond `project_root`, `scan_root`, `rule_filter` (check `types.rs:36-41`), add them with zero values.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p phronesis-mcp --lib -- audit::tests::engine_tests::a_phronesisignored_file --nocapture`
Expected: FAIL with `no field lexical_excluded on type AuditReport`.

- [ ] **Step 3: Write minimal implementation**

`crates/phronesis-mcp/src/audit/types.rs`, in `AuditReport` after `per_rule`:

```rust
    /// Files a `.phronesisignore` entry excluded from lexical rules. They
    /// were still offered to structural rules (unless over the size cap).
    /// Relative to the scan root, sorted. Empty when nothing was excluded.
    pub lexical_excluded: Vec<PathBuf>,
```

`crates/phronesis-mcp/src/audit/run.rs`, in `run_core`: replace the discovery block and scan loop. No closure (a closure over `accum` and `times` does not type-check with `times: Option<&mut AuditSectionTimes>`); the loop body is written twice:

```rust
    let (discovery, files_scanned) = {
        let t = Instant::now();
        let d = if audit_rules.is_empty() {
            Discovery::default()
        } else {
            discover_files_with_excluded(&opts.scan_root, &["*"])
        };
        // Every file offered to any scan, excluded ones included.
        let n = (d.scanned.len() + d.excluded.len()) as u32;
        if let Some(ref mut t2) = times {
            t2.discover = t.elapsed();
            t2.files_scanned = n;
        }
        (d, n)
    };

    let structural_rules: Vec<&DiskRule> = audit_rules
        .iter()
        .copied()
        .filter(|r| super::engine::rule_has_ast_predicate(r))
        .collect();
    let size_cap = crate::security::max_file_bytes();

    let mut accum: BTreeMap<String, (Level, BTreeMap<PathBuf, PerFileHits>)> = BTreeMap::new();

    for path in &discovery.scanned {
        let t = Instant::now();
        let Ok(content) = std::fs::read_to_string(path) else {
            if let Some(ref mut t2) = times {
                t2.read_files += t.elapsed();
            }
            continue;
        };
        if let Some(ref mut t2) = times {
            t2.read_files += t.elapsed();
        }
        scan_file_into_accum(ScanFileInput {
            project_root: &opts.project_root,
            path,
            content: &content,
            rules: &audit_rules,
            accum: &mut accum,
            times: times.as_deref_mut(),
        });
    }

    // Excluded files: structural rules only, and only under the size cap —
    // an ignored vendored tree must not cost more than scanning it would.
    if !structural_rules.is_empty() {
        for path in &discovery.excluded {
            let over_cap = std::fs::metadata(path).map(|m| m.len() > size_cap).unwrap_or(true);
            if over_cap {
                continue;
            }
            let t = Instant::now();
            let Ok(content) = std::fs::read_to_string(path) else {
                if let Some(ref mut t2) = times {
                    t2.read_files += t.elapsed();
                }
                continue;
            };
            if let Some(ref mut t2) = times {
                t2.read_files += t.elapsed();
            }
            scan_file_into_accum(ScanFileInput {
                project_root: &opts.project_root,
                path,
                content: &content,
                rules: &structural_rules,
                accum: &mut accum,
                times: times.as_deref_mut(),
            });
        }
    }

    let mut lexical_excluded: Vec<PathBuf> = discovery
        .excluded
        .iter()
        .map(|p| p.strip_prefix(&opts.scan_root).map(Path::to_path_buf).unwrap_or_else(|_| p.clone()))
        .collect();
    lexical_excluded.sort();
```

Add `lexical_excluded,` to the `AuditReport { .. }` literal. Import `crate::rules_file::DiskRule` as the compiler asks. Check `security::max_file_bytes()` exists (`security.rs:37-62` documents the runtime override); if the function has another name, use that.

`crates/phronesis-mcp/src/audit/render.rs`: add

```rust
const EXCLUDED_NAMES_SHOWN: usize = 5;

fn excluded_line(report: &AuditReport) -> String {
    if report.lexical_excluded.is_empty() {
        return String::new();
    }
    let total = report.lexical_excluded.len();
    let mut names: Vec<String> = report
        .lexical_excluded
        .iter()
        .take(EXCLUDED_NAMES_SHOWN)
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    if total > EXCLUDED_NAMES_SHOWN {
        names.push(format!("+{} more", total - EXCLUDED_NAMES_SHOWN));
    }
    format!(
        "{total} file(s) excluded from lexical rules by .phronesisignore (structural rules still ran): {}\n",
        names.join(", ")
    )
}
```

In `render_table`'s empty branch return `format!("no audit violations found ({} files scanned in {}ms)\n{}", report.files_scanned, report.scan_duration_ms, excluded_line(report))`; in the non-empty branch `out.push_str(&excluded_line(report));` after the `Total:` line. In `render_json`, add `"lexical_excluded": report.lexical_excluded.iter().map(|p| p.to_string_lossy().to_string()).collect::<Vec<_>>()` to the top-level object. Fix every other `AuditReport { .. }` constructor the compiler reports (`render_tests.rs`, `trend_tests.rs`) with `lexical_excluded: Vec::new()`.

`crates/phronesis-mcp/.phronesisignore`: delete the `src/init.rs` line and the two comment lines above it. Keep the header comment.

- [ ] **Step 4: Run tests to verify they pass**

Run each on its own:
```
cargo test -p phronesis-mcp --lib -- audit
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p phronesis-mcp -- audit --fail-on block
```
Expected: `test result: ok.` including the two new tests; clippy exit 0; the audit exits 0 and its footer names no excluded file.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/audit crates/phronesis-mcp/.phronesisignore
git commit -m "feat(audit): .phronesisignore exempts lexical rules only, is reported, and loses its stale init.rs entry"
```

### Task A3: Exempt the self-referential rule text per rule, and prove the exemption works

**Files:**
- Modify: `crates/phronesis-mcp/src/init/rules_rust.rs:1-3` (file doc-comment markers) and the `audit-string-concat-with-plus` definition at ~line 280 (add `"doc_excepted": true`)
- Test: `crates/phronesis-mcp/tests/cli_smoke.rs` (append)

**Interfaces:**
- Consumes: `file_exempts_rule(lines, rule_id)` (`audit/engine.rs:256`), consulted only when `rule.doc_excepted == Some(true)` (`engine.rs:446`). Verified: `audit-newtype-id-string` and `audit-allow-dead-code-in-src` already set `doc_excepted`; `audit-string-concat-with-plus` (`rules_rust.rs:280-289`) does not, so a marker alone cannot exempt it.

- [ ] **Step 1: Write the failing test**

Append to `crates/phronesis-mcp/tests/cli_smoke.rs`:

```rust
/// The pack rule definitions in `src/init/rules_rust.rs` embed their own
/// trigger strings. That self-reference is exempted per rule with
/// `//! phronesis-allow:` markers, never by hiding the file from the audit
/// (issue #114: a whole-file `.phronesisignore` entry hid nine real
/// production unwraps for as long as it existed). This test runs the real
/// audit: a marker that the engine does not honour fails here.
#[test]
fn pack_rule_self_reference_is_exempted_per_rule_and_the_audit_proves_it() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root");
    let ignore = std::fs::read_to_string(repo.join("crates/phronesis-mcp/.phronesisignore"))
        .unwrap_or_default();
    for line in ignore.lines().map(str::trim) {
        assert!(
            line.is_empty() || line.starts_with('#') || !line.contains("src/"),
            ".phronesisignore must not hide source files from the audit: `{line}`"
        );
    }
    for rule in [
        "audit-newtype-id-string",
        "audit-allow-dead-code-in-src",
        "audit-string-concat-with-plus",
    ] {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
            .current_dir(repo)
            .args(["audit", "--rule", rule, "--json"])
            .output()
            .expect("run audit");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            !text.contains("src/init/rules_rust.rs"),
            "{rule} still fires on its own definition text:\n{text}"
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p phronesis-mcp --test cli_smoke -- pack_rule_self_reference_is_exempted_per_rule_and_the_audit_proves_it --exact`
Expected: FAIL: at least one of the three rules lists `src/init/rules_rust.rs` (all three do on `main` today: 233 warned vs 221 before #112).

- [ ] **Step 3: Make the change**

Prepend to `crates/phronesis-mcp/src/init/rules_rust.rs` (before any existing `//!` or `use`):

```rust
//! Rust starter-pack rule definitions.
//!
//! The rule bodies below embed the very strings the rules match, so three
//! lexical rules would fire on this file's own text. Each is exempted by
//! name here (all three set `doc_excepted`); structural rules still run.
//!
//! phronesis-allow: audit-newtype-id-string (rule text embeds `_id: String`)
//! phronesis-allow: audit-allow-dead-code-in-src (rule text embeds the allow attribute)
//! phronesis-allow: audit-string-concat-with-plus (rule text embeds the pattern)
```

In the `audit-string-concat-with-plus` definition (~line 280), add `"doc_excepted": true,` after `"audit": true,`. Then run `cargo run -p phronesis-mcp -- init --rules-only --packs llm,rust` in this checkout so `.phronesis/rules.json` picks up the changed pack rule (it is gitignored here; CI regenerates its own).

- [ ] **Step 4: Verify**

Run: `cargo test -p phronesis-mcp --test cli_smoke` → `test result: ok.`; then `cargo run -p phronesis-mcp -- audit` → the three rules no longer list `rules_rust.rs` and the warned total is back near 221.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/init/rules_rust.rs crates/phronesis-mcp/tests/cli_smoke.rs
git commit -m "fix(audit): exempt the Rust pack's self-referential rule text per rule"
```

### Task A4: Document the semantics and add the CHANGELOG entry

**Files:**
- Modify: `crates/phronesis-mcp/CLAUDE.md` (grep `phronesisignore`), `AGENTS.md` (Security Constraints bullet), `CHANGELOG.md`

- [ ] **Step 1: Write the docs**

`crates/phronesis-mcp/CLAUDE.md`, replacing the `.phronesisignore` sentence:

> `.phronesisignore` (gitignore syntax, honoured at any directory level) exempts matching files from **lexical** rules only: rules with no AST predicate. Rules with an AST predicate are structural and keep running on excluded files (below the `PHRONESIS_MAX_FILE_BYTES` cap), and `phr-mcp audit` lists every excluded file in its footer and under `lexical_excluded` in `--json`. `files_scanned` counts excluded files too. Excluding a large tree therefore still costs a structural pass over its source files. To silence one rule for one file, use a `//! phronesis-allow: <rule-id> <reason>` line in the file's leading doc comment; this works only for rules that set `doc_excepted`.

`AGENTS.md`: `.phronesisignore` exempts files from lexical rules only; excluded files are reported.

`CHANGELOG.md` under `## [Unreleased]` / `### Fixed`:

```markdown
- **`.phronesisignore` no longer hides files from structural rules.** An ignore
  entry now exempts a file from lexical rules only (rules with no AST
  predicate); structural rules keep running on it below the file-size cap, and
  `phr-mcp audit` names every excluded file in its footer and in `--json`
  (`lexical_excluded`). The whole-file entry for `src/init.rs` had silenced
  `enforce-no-unwrap-in-src` on nine production `.unwrap()` calls for as long
  as it existed (#114). The Rust pack's self-referential rule text is now
  exempted per rule with `//! phronesis-allow:` markers.
```

- [ ] **Step 2: Verify and commit**

Run: `cargo test -p phronesis-mcp --test cli_smoke` → `test result: ok.`

```bash
git add crates/phronesis-mcp/CLAUDE.md AGENTS.md CHANGELOG.md
git commit -m "docs: .phronesisignore exempts lexical rules only"
```

---

## Part B — Commits made in another worktree of the same repository are recorded

**Shortfall.** Every worker commit during the swarm ran as `cd /path/to/worktree && git commit …` or `git -C <worktree> commit …` from a session rooted in the main checkout. `lifecycle::inflight::push` and `pop_and_detect` probe `HEAD` at the project root, so HEAD never moved where they looked and `kalpa show` reported `commits 0` everywhere.

**Decisions.** (1) The directory comes from the **head-moving invocation itself**: the segment that matches `command_may_move_head`, its own `-C <dir>`; else a leading `cd <dir>` segment that hands off with `&&` or `;`. (2) **Absolute paths only.** The shell tool's cwd is not known to the hook, so a relative path is ambiguous and falls back to the project root. (3) Anything the parser does not understand (`$VAR`, backticks, backslash escapes, `sh -c`, subshells, comments) returns `None`: undercount, never mis-attribute. (4) A candidate counts only when it shares the project's `git rev-parse --git-common-dir`; a sibling repository stays a non-commit. (5) The probe root is decided once at `push` and stored on the in-flight record, so pre and post probe the same place.

### Task B1: Derive the repository directory a head-moving command names

**Files:**
- Modify: `crates/phronesis-mcp/src/lifecycle/outcome.rs` (add `command_repo_dir` after `command_may_move_head` at ~line 23)
- Test: `crates/phronesis-mcp/tests/lifecycle_outcome.rs` (append)

**Interfaces:**
- Consumes: `crate::outcomes::segment::command_heads(command) -> Vec<String>` (`outcomes/segment.rs:30`; returns normalized segments split on `&&`, `||`, `|`, `;`, newline, with leading `NAME=value`/`env` stripped) and `prefilter()` (`outcome.rs:14`).
- Produces: `pub fn command_repo_dir(command: &str) -> Option<PathBuf>` — an absolute directory, or `None`.

- [ ] **Step 1: Write the failing test**

Append to `crates/phronesis-mcp/tests/lifecycle_outcome.rs`:

```rust
#[test]
fn command_repo_dir_reads_the_head_moving_invocation_conservatively() {
    use std::path::PathBuf;
    let p = |s: &str| Some(PathBuf::from(s));
    assert_eq!(command_repo_dir("git commit -m x"), None);
    assert_eq!(command_repo_dir("cargo test && git commit -am done"), None);
    assert_eq!(command_repo_dir("cd /wt/a && git commit -q -m x"), p("/wt/a"));
    assert_eq!(command_repo_dir("cd /wt/a; git commit -q -m x"), p("/wt/a"));
    assert_eq!(command_repo_dir("git -C /wt/b commit -am x"), p("/wt/b"));
    assert_eq!(command_repo_dir("cd '/wt/my a' && cargo fmt && git commit -am x"), p("/wt/my a"));
    assert_eq!(command_repo_dir("cd \"/wt/other a\"; git commit -am x"), p("/wt/other a"));
    // The commit's own -C wins over an earlier cd.
    assert_eq!(command_repo_dir("cd /a && git -C /b commit -am x"), p("/b"));
    // A -C on a NON head-moving invocation is not the commit's directory.
    assert_eq!(command_repo_dir("git -C /x diff && git commit -am x"), None);
    // Relative paths are ambiguous (the shell's cwd is unknown): fall back.
    assert_eq!(command_repo_dir("cd wt && git commit -am x"), None);
    assert_eq!(command_repo_dir("cd /wt && cargo test && git -C . commit -am x"), None);
    // Forms the parser does not understand return None rather than a guess.
    assert_eq!(command_repo_dir("cd $WT && git commit -am x"), None);
    assert_eq!(command_repo_dir("cd /wt/a\\ b && git commit -am x"), None);
    assert_eq!(command_repo_dir("sh -c 'cd /wt && git commit -am x'"), None);
    assert_eq!(command_repo_dir("(cd /wt && git commit -am x)"), None);
    assert_eq!(command_repo_dir("echo 'git -C /wt commit' # not a commit"), None);
    assert_eq!(command_repo_dir("cd && git commit -am x"), None);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p phronesis-mcp --test lifecycle_outcome -- command_repo_dir_reads_the_head_moving_invocation_conservatively --exact`
Expected: FAIL with `cannot find function command_repo_dir`.

- [ ] **Step 3: Write minimal implementation**

In `crates/phronesis-mcp/src/lifecycle/outcome.rs`:

```rust
use std::path::PathBuf;

/// One shell word from the start of `rest`: a `'…'` or `"…"` quoted run, or
/// a bare run up to whitespace / `;` / `&` / `|`. `None` when the word is
/// empty or contains shell syntax this parser does not model (`$`, backtick,
/// backslash, `(`, `)`, `#`), because a guessed directory is worse than none.
fn shell_word(rest: &str) -> Option<String> {
    let rest = rest.trim_start();
    let first = rest.chars().next()?;
    let word = if first == '\'' || first == '"' {
        let end = rest[1..].find(first)?;
        &rest[1..end + 1]
    } else {
        let end = rest
            .find(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|'))
            .unwrap_or(rest.len());
        &rest[..end]
    };
    if word.is_empty() || word.contains(['$', '`', '\\', '(', ')', '#']) {
        return None;
    }
    Some(word.to_string())
}

/// The absolute directory the head-moving invocation in `command` acts in,
/// or `None`. Decided from the segment that matches the head-moving
/// prefilter: its own `-C <dir>` first; else a leading `cd <dir>` segment
/// that hands off to the rest of the command. Relative paths and unmodelled
/// shell syntax return `None`: the caller then probes the project root, so
/// commits are undercounted, never mis-attributed.
pub fn command_repo_dir(command: &str) -> Option<PathBuf> {
    if command.contains(['$', '`', '(', ')']) || command.contains("sh -c") {
        return None;
    }
    let segments = crate::outcomes::segment::command_heads(command);
    let mover = segments.iter().find(|s| prefilter().is_match(s))?;

    let dir = if let Some(idx) = mover.find(" -C ") {
        shell_word(&mover[idx + 4..])?
    } else {
        let first = segments.first()?;
        let rest = first.strip_prefix("cd ")?;
        if segments.len() < 2 {
            return None;
        }
        shell_word(rest)?
    };
    let path = PathBuf::from(dir);
    path.is_absolute().then_some(path)
}
```

If `command_heads` strips more than leading env assignments (check `segment.rs:30-100`), adjust the `" -C "` search to the segment shape it returns; the test is the contract.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p phronesis-mcp --test lifecycle_outcome`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/lifecycle/outcome.rs crates/phronesis-mcp/tests/lifecycle_outcome.rs
git commit -m "feat(lifecycle): derive the directory a head-moving command names, conservatively"
```

### Task B2: Decide the probe root once, at push; accept only worktrees of this repository

**Files:**
- Modify: `crates/phronesis-mcp/src/lifecycle/outcome.rs` (add `git_common_dir`, `probe_root_for`)
- Modify: `crates/phronesis-mcp/src/lifecycle/state.rs:238-251` (`Inflight`: add `probe_root: Option<String>`)
- Modify: `crates/phronesis-mcp/src/lifecycle/inflight.rs:49-80` (`push`), `:95-150` (`pop_and_detect`)
- Test: `crates/phronesis-mcp/tests/lifecycle_outcome.rs` (append)

**Interfaces:**
- Produces: `pub fn probe_root_for(project_root: &Path, command: &str) -> PathBuf` — `project_root` unless `command_repo_dir` names an existing directory whose `git rev-parse --git-common-dir` canonicalizes to the same path as `project_root`'s. `Inflight.probe_root: Option<String>` (serde default, skipped when `None`) is set at `push` when it differs from the project root; `pop_and_detect` probes `probe_root` when present and stamps the `commit` event with `"repo_dir": "<path>"`.

- [ ] **Step 1: Write the failing test**

Append to `crates/phronesis-mcp/tests/lifecycle_outcome.rs`:

```rust
#[test]
fn a_commit_in_a_linked_worktree_is_detected_but_a_sibling_repo_still_is_not() {
    let d = repo();
    let wt = d.path().join("wt");
    git(d.path(), &["worktree", "add", "-q", "-b", "feature", wt.to_str().unwrap()]);
    let cmd = format!("cd {} && git commit -q -am x", wt.display());
    let root = probe_root_for(d.path(), &cmd);
    assert_eq!(
        std::fs::canonicalize(&root).unwrap(),
        std::fs::canonicalize(&wt).unwrap(),
        "a worktree sharing the common dir is probed directly"
    );
    let before = git_head(&root).unwrap();
    std::fs::write(wt.join("a"), "2").unwrap();
    git(&wt, &["commit", "-q", "-am", "in worktree"]);
    assert!(detect_commit(&root, Some(&before), &cmd, Some(0)).is_some());

    let other = repo();
    let cmd2 = format!("git -C {} commit -am x", other.path().display());
    assert_eq!(
        std::fs::canonicalize(probe_root_for(d.path(), &cmd2)).unwrap(),
        std::fs::canonicalize(d.path()).unwrap(),
        "a sibling repository falls back to the project root"
    );
    let missing = probe_root_for(d.path(), "cd /definitely/not/here && git commit -am x");
    assert_eq!(std::fs::canonicalize(missing).unwrap(), std::fs::canonicalize(d.path()).unwrap());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p phronesis-mcp --test lifecycle_outcome -- a_commit_in_a_linked_worktree_is_detected_but_a_sibling_repo_still_is_not --exact`
Expected: FAIL with `cannot find function probe_root_for`.

- [ ] **Step 3: Write minimal implementation**

`outcome.rs`:

```rust
/// `git rev-parse --git-common-dir` for `dir`, canonicalized. `None` outside
/// a repository or when git is unavailable.
fn git_common_dir(dir: &Path) -> Option<PathBuf> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(dir)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let path = if Path::new(&raw).is_absolute() { PathBuf::from(raw) } else { dir.join(raw) };
    std::fs::canonicalize(path).ok()
}

/// Where to probe HEAD for `command`: the absolute directory it names when
/// that directory exists and is a worktree of the same repository as
/// `project_root`; otherwise `project_root`. A sibling repository therefore
/// still reads as "HEAD did not move here" (spec §Non-goals).
pub fn probe_root_for(project_root: &Path, command: &str) -> PathBuf {
    let Some(candidate) = command_repo_dir(command) else {
        return project_root.to_path_buf();
    };
    if !candidate.is_dir() {
        return project_root.to_path_buf();
    }
    match (git_common_dir(project_root), git_common_dir(&candidate)) {
        (Some(a), Some(b)) if a == b => candidate,
        _ => project_root.to_path_buf(),
    }
}
```

`state.rs`, in `Inflight` after `head_before`:

```rust
    /// The directory HEAD was probed in when it was not the project root: a
    /// linked worktree the command named. Post probes the same place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe_root: Option<String>,
```

Fix every `Inflight { .. }` literal the compiler reports with `probe_root: None`.

`inflight.rs`, in `push`: compute `let probe_root = outcome::probe_root_for(root, call.command);` before the probe; call `outcome::git_head_probe(&probe_root)`; set the field to `(probe_root != root).then(|| probe_root.to_string_lossy().to_string())`. In `pop_and_detect`: `let probe_root: PathBuf = entry.probe_root.as_deref().map(PathBuf::from).unwrap_or_else(|| root.to_path_buf());` and pass `&probe_root` to `detect_commit`; after building `ev`, add `if let Some(dir) = &entry.probe_root { ev = ev.with_extra("repo_dir", dir.clone()); }`. Note `entry` must be bound before the early `return`s that consume it; move the `probe_root` extraction to right after `let Some(entry) = entry else { return };`.

- [ ] **Step 4: Run tests to verify they pass**

Run each on its own:
```
cargo test -p phronesis-mcp --test lifecycle_outcome
cargo test -p phronesis-mcp --test lifecycle_state
cargo test -p phronesis-mcp --test lifecycle_concurrency
cargo test -p phronesis-mcp --lib -- lifecycle
```
Expected: all green; `dry_run_heredoc_and_sibling_repo_are_not_commits` unchanged and green.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/lifecycle crates/phronesis-mcp/tests/lifecycle_outcome.rs
git commit -m "feat(lifecycle): record commits made in a linked worktree of this repository"
```

### Task B3: Surface `repo_dir` in the unit report and document

**Files:**
- Modify: `crates/phronesis-mcp/src/lifecycle/unit_report.rs:24-29` (`CommitRow` has `sha` and `band` today), `:142`, `:273-320`
- Modify: `crates/phronesis-mcp/CLAUDE.md`, `AGENTS.md` (lifecycle bullets), `CHANGELOG.md`
- Test: `crates/phronesis-mcp/tests/unit_cli_integration.rs` (append)

- [ ] **Step 1: Write the failing test**

First read how `unit_cli_integration.rs` seeds a project and how `lifecycle_outcome.rs`/`lifecycle_state.rs` write a `commit` record (grep `"event":"commit"` and `Kind::Commit` under `tests/` and `src/lifecycle/record.rs`). Then append, reusing that file's project-setup helper for `root`:

```rust
#[test]
fn unit_show_reports_the_worktree_a_commit_landed_in() {
    let (dir, root) = governed_project(); // the file's existing helper; rename to match
    run_phr(&root, &["unit", "start", "issue-9"]);
    // One commit record with repo_dir, in the shape record.rs writes.
    let line = serde_json::json!({
        "ts": 1_790_000_000u64, "kind": "lifecycle", "event": "commit", "host": "cli",
        "seq": 7, "sid": "s-test", "subject": "issue-9", "unit_id": "issue-9",
        "sha": "0123456789abcdef0123456789abcdef01234567",
        "head_before": "89abcdef0123456789abcdef0123456789abcdef",
        "repo_dir": "/wt/feature"
    });
    let log = root.join(".phronesis/log.jsonl");
    let mut existing = std::fs::read_to_string(&log).unwrap_or_default();
    existing.push_str(&line.to_string());
    existing.push('\n');
    std::fs::write(&log, existing).expect("append log");

    let json: serde_json::Value = serde_json::from_slice(&run_phr(&root, &["unit", "show", "issue-9", "--json"]).stdout).expect("json");
    assert_eq!(json["commits"][0]["repo_dir"], "/wt/feature", "{json}");
    let table = String::from_utf8_lossy(&run_phr(&root, &["unit", "show", "issue-9"]).stdout).to_string();
    assert!(table.contains("/wt/feature"), "{table}");
    drop(dir);
}
```

Match the record's field names to what `record.rs` actually writes for a commit (the test must not invent a shape the reader ignores); if the log reader filters by `unit_id`/`subject` differently, copy the fields from an existing commit line in the fixtures.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p phronesis-mcp --test unit_cli_integration -- unit_show_reports_the_worktree_a_commit_landed_in --exact`
Expected: FAIL: `commits[0].repo_dir` is null.

- [ ] **Step 3: Implement**

In `unit_report.rs`: add `pub repo_dir: Option<String>` to `CommitRow`; at the `("lifecycle", "commit")` arm populate `repo_dir: e.data.get("repo_dir").and_then(|v| v.as_str()).map(String::from)`; in the table render append ` (in <repo_dir>)` when present; in the JSON add `"repo_dir": c.repo_dir`.

- [ ] **Step 4: Verify**

Run: `cargo test -p phronesis-mcp --test unit_cli_integration` → `test result: ok.`

- [ ] **Step 5: Docs and commit**

`CHANGELOG.md` → `### Fixed`:

```markdown
- **Commits made from another worktree of the same repository are recorded.**
  A `cd <worktree> && git commit …` or `git -C <worktree> commit …` issued from
  a session rooted in the main checkout used to be missed, because the hook
  probed `HEAD` only at the project root; every worker commit in a swarm
  vanished from `kalpa show` and `unit show`. The hook now probes the absolute
  directory the head-moving invocation names when it shares this repository's
  common git dir, decides that once at pre-check, and stamps the record with
  `repo_dir`. Relative paths, unrelated repositories, and shell forms the
  parser does not model still fall back to the project root.
```

`crates/phronesis-mcp/CLAUDE.md` and `AGENTS.md` lifecycle bullets: one sentence each on `repo_dir`.

```bash
git add crates/phronesis-mcp/src/lifecycle/unit_report.rs crates/phronesis-mcp/tests/unit_cli_integration.rs crates/phronesis-mcp/CLAUDE.md AGENTS.md CHANGELOG.md
git commit -m "feat(lifecycle): show the worktree a recorded commit landed in"
```

---

## Part C — Confidence signals survive redirected output and hand-run gates

**Shortfall.** The post-check adapter parses the Bash tool's captured output. Every gate I ran was `cargo test … > log 2>&1`, so the captured output was empty, no summary matched, and the band stayed empty while 3,295 tests passed. Harness workers (Crush) have no post-hook. `phr-mcp signal <compile|tests> <pass|fail>` records a bare verdict, not parsed evidence.

**Decisions.** (1) The redirect target is attributed to the handled invocation only: a `>`/`>>`/`&>` **in its own segment**, or a `| tee <file>` **in the segment immediately after it**. (2) Only files under the **project root** are read. The OS temp dir is not allowed (it is shared and world-writable; both reviewers flagged it). (3) **Freshness:** the file's mtime must be at or after the in-flight record's `ts` (`state::Inflight.ts`, set at pre-check), or it is not read. (4) The read is TOCTOU-safe: open, then check the open handle's metadata is a regular file with the expected device/inode under the root, via `security::read_file_capped` semantics. (5) Explicit ingestion (`signal ingest`) is a distinct provenance, `outcome:ingested`, and never journals an empty result.

### Task C1: Find a segment's stdout redirect target with a quote-aware scanner

**Files:**
- Modify: `crates/phronesis-mcp/src/outcomes/segment.rs` (add `shell_word`, `stdout_redirect_target`, `tee_target`)
- Test: same file, `#[cfg(test)] mod tests` (append)

**Interfaces:**
- Produces: `pub fn stdout_redirect_target(segment: &str) -> Option<String>` — the target of the last `>`, `>>`, `&>`, `1>`, `1>>` in this segment, attached (`>file`) or spaced, quoted or bare; descriptor duplications (`2>&1`, `>&2`, `1>&2`, `&>&1`) are never targets. `pub fn tee_target(segment: &str) -> Option<String>` — for a segment whose first word is `tee`, its first non-option argument (options are words starting with `-`; a `--` ends options). `pub(crate) fn shell_word(rest: &str) -> Option<String>` — shared quote-aware word reader (same rules as B1's; if B1 has landed, move that helper here and have `outcome.rs` call this one).

- [ ] **Step 1: Write the failing test**

Append inside `mod tests` in `segment.rs`:

```rust
    #[test]
    fn stdout_redirect_target_is_quote_aware_and_skips_fd_dups() {
        let t = stdout_redirect_target;
        assert_eq!(t("cargo test --workspace"), None);
        assert_eq!(t("cargo test --workspace > /tmp/t.log 2>&1"), Some("/tmp/t.log".into()));
        assert_eq!(t("cargo test >> logs/run.txt"), Some("logs/run.txt".into()));
        assert_eq!(t("cargo test &> 'out dir/all.log'"), Some("out dir/all.log".into()));
        assert_eq!(t("cargo test >\"out dir/all.log\""), Some("out dir/all.log".into()));
        assert_eq!(t("cargo test >run.log 2>&1"), Some("run.log".into()));
        assert_eq!(t("cargo test 1>run.log"), Some("run.log".into()));
        assert_eq!(t("cargo test > a.log > b.log"), Some("b.log".into()), "the last wins");
        assert_eq!(t("cargo test 2>&1"), None);
        assert_eq!(t("echo x >&2"), None);
        assert_eq!(t("echo x 1>&2"), None);
        assert_eq!(t("echo x &>&1"), None);
        assert_eq!(t("echo x > $OUT"), None, "unmodelled shell syntax is not a target");
        assert_eq!(t("echo x > out\\ dir/log"), None);
    }

    #[test]
    fn tee_target_reads_the_first_file_argument() {
        assert_eq!(tee_target("tee run.log"), Some("run.log".into()));
        assert_eq!(tee_target("tee -a run.log"), Some("run.log".into()));
        assert_eq!(tee_target("tee -- -dashed.log"), Some("-dashed.log".into()));
        assert_eq!(tee_target("tee 'my run.log'"), Some("my run.log".into()));
        assert_eq!(tee_target("cargo test"), None);
        assert_eq!(tee_target("tee"), None);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p phronesis-mcp --lib -- outcomes::segment::tests`
Expected: FAIL with `cannot find function stdout_redirect_target` / `tee_target`.

- [ ] **Step 3: Write minimal implementation**

In `segment.rs`:

```rust
/// One shell word from the start of `rest`: a `'…'`/`"…"` run, or a bare
/// run up to whitespace or an operator character. `None` for an empty word
/// or one carrying shell syntax this scanner does not model (`$`, backtick,
/// backslash, parentheses, `#`): a guessed path is worse than none.
pub(crate) fn shell_word(rest: &str) -> Option<String> {
    let rest = rest.trim_start();
    let first = rest.chars().next()?;
    let word = if first == '\'' || first == '"' {
        let end = rest[1..].find(first)?;
        &rest[1..end + 1]
    } else {
        let end = rest
            .find(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '<' | '>'))
            .unwrap_or(rest.len());
        &rest[..end]
    };
    if word.is_empty() || word.contains(['$', '`', '\\', '(', ')', '#']) {
        return None;
    }
    Some(word.to_string())
}

/// The file this segment sends stdout to, when it does: the target of the
/// last `>`, `>>`, `&>`, `1>` or `1>>`, spaced or attached, quoted or bare.
/// Descriptor duplications (`2>&1`, `>&2`, `&>&1`) are not files. `None`
/// when stdout stays on the terminal or the target is not a plain word.
pub fn stdout_redirect_target(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut target: Option<String> = None;
    let mut i = 0;
    let mut in_quote: Option<u8> = None;
    while i < bytes.len() {
        let c = bytes[i];
        if let Some(q) = in_quote {
            if c == q {
                in_quote = None;
            }
            i += 1;
            continue;
        }
        if c == b'\'' || c == b'"' {
            in_quote = Some(c);
            i += 1;
            continue;
        }
        if c == b'>' {
            // Which descriptor? A digit right before `>` names it; `&>` is both.
            let fd_is_stdout = match i.checked_sub(1).map(|j| bytes[j]) {
                Some(b'&') => true,
                Some(d) if d.is_ascii_digit() => d == b'1',
                _ => true,
            };
            let mut j = i + 1;
            if j < bytes.len() && bytes[j] == b'>' {
                j += 1; // `>>`
            }
            if j < bytes.len() && bytes[j] == b'&' {
                // `>&2`, `2>&1`, `&>&1`: a descriptor, not a file.
                i = j + 1;
                continue;
            }
            if fd_is_stdout {
                match shell_word(&segment[j..]) {
                    Some(w) => target = Some(w),
                    None => return None,
                }
            }
            i = j;
            continue;
        }
        i += 1;
    }
    target
}

/// For a `tee` segment, the first file argument: options (`-a`, `-i`) are
/// skipped and `--` ends them. `None` when the segment is not `tee` or
/// names no file.
pub fn tee_target(segment: &str) -> Option<String> {
    let rest = segment.trim_start().strip_prefix("tee")?;
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let mut rest = rest.trim_start();
    loop {
        let word = shell_word(rest)?;
        let consumed = rest.trim_start().find(&word).map(|k| k + word.len()).unwrap_or(rest.len());
        let quoted_extra = if rest.trim_start().starts_with(['\'', '"']) { 2 } else { 0 };
        rest = &rest.trim_start()[(consumed + quoted_extra).min(rest.trim_start().len())..];
        if word == "--" {
            return shell_word(rest);
        }
        if word.starts_with('-') {
            continue;
        }
        return Some(word);
    }
}
```

The `tee_target` word-advance arithmetic is the fiddly part; if it fights, tokenize the `tee` segment with a small loop that calls `shell_word` and then skips past the word's raw length plus quotes explicitly. The tests are the contract.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p phronesis-mcp --lib -- outcomes::segment` → all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/outcomes/segment.rs
git commit -m "feat(outcomes): quote-aware detection of a command's stdout redirect target"
```

### Task C2: The adapter reads a fresh, in-project redirect target when captured output is empty

**Files:**
- Modify: `crates/phronesis-mcp/src/outcomes/adapter.rs:196-262` (`ExtractFromInput`, `extract_from`, `extract_handled`)
- Modify: `crates/phronesis-mcp/src/hook/journey_record.rs:65-83` (`outcomes_for_journal`: pass `not_before`)
- Modify: `crates/phronesis-mcp/src/lifecycle/state.rs` (add `peek_inflight(root, key) -> Option<Inflight>`, read-only sibling of `pop_inflight` at :276)
- Modify: `crates/phronesis-mcp/src/codex_hook.rs:1557` (add `not_before: None` to the literal; Codex has no in-flight record there)
- Test: `crates/phronesis-mcp/src/outcomes/adapter.rs` tests (append)

**Interfaces:**
- Consumes: `command_heads`, `stdout_redirect_target`, `tee_target` (C1); `security::read_file_capped(&Path) -> Result<String, SecurityError>` (`security.rs:261`); `state::Inflight.ts`.
- Produces: `ExtractFromInput.not_before: Option<u64>` (unix seconds). `fn redirect_output(project_root: &Path, command: &str, not_before: Option<u64>) -> Option<String>`: the handled segment's own redirect target, or the next segment's `tee` target; resolved against `project_root`; read only when `not_before` is `Some`, the canonical path is under the canonical project root, the opened file is a regular file (not FIFO, not a symlink escaping the root), and its mtime ≥ `not_before`. `extract_from` uses it only when the captured output is whitespace-only, and appends `outcome:output_from_file` to the tags when it did.

- [ ] **Step 1: Write the failing tests**

Append inside `mod tests` in `adapter.rs`:

```rust
    fn enabled_project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join(".phronesis")).expect("mkdir");
        std::fs::write(dir.path().join(".phronesis/confidence.json"), "{}").expect("enable");
        dir
    }
    const PASS_LOG: &str = "running 3 tests\ntest a ... ok\ntest result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n";
    fn now_secs() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
    }
    fn extract(root: &std::path::Path, command: &str, output: &str, not_before: Option<u64>) -> Vec<String> {
        extract_from(ExtractFromInput { project_root: root, tool_name: "Bash", command: Some(command), output, command_exit: Some(0), not_before }).0
    }

    #[test]
    fn redirected_output_is_read_from_a_fresh_file_inside_the_project() {
        let dir = enabled_project();
        let root = dir.path();
        std::fs::write(root.join("run.log"), PASS_LOG).expect("log");
        let tags = extract(root, "cargo test --workspace > run.log 2>&1", "", Some(now_secs() - 60));
        assert!(tags.iter().any(|t| t == "outcome:test_pass"), "{tags:?}");
        assert!(tags.iter().any(|t| t == "outcome:output_from_file"), "{tags:?}");
    }

    #[test]
    fn a_tee_in_the_next_segment_is_the_handled_command_s_output() {
        let dir = enabled_project();
        let root = dir.path();
        std::fs::write(root.join("run.log"), PASS_LOG).expect("log");
        let tags = extract(root, "cargo test 2>&1 | tee run.log", "", Some(now_secs() - 60));
        assert!(tags.iter().any(|t| t == "outcome:test_pass"), "{tags:?}");
    }

    #[test]
    fn captured_output_wins_over_the_redirect_file() {
        let dir = enabled_project();
        let root = dir.path();
        std::fs::write(root.join("run.log"), PASS_LOG).expect("stale pass on disk");
        let captured = "test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out\n";
        let tags = extract(root, "cargo test > run.log", captured, Some(now_secs() - 60));
        assert!(tags.iter().any(|t| t == "outcome:test_fail"), "{tags:?}");
        assert!(!tags.iter().any(|t| t == "outcome:output_from_file"), "{tags:?}");
    }

    #[test]
    fn stale_out_of_root_symlinked_fifo_and_unstamped_targets_are_never_evidence() {
        let dir = enabled_project();
        let root = dir.path();
        // Stale: written before the command started.
        std::fs::write(root.join("old.log"), PASS_LOG).expect("log");
        assert!(extract(root, "cargo test > old.log", "", Some(now_secs() + 3600)).iter().all(|t| t != "outcome:test_pass"));
        // No in-flight timestamp at all: refuse.
        assert!(extract(root, "cargo test > old.log", "", None).iter().all(|t| t != "outcome:test_pass"));
        // Outside the project root (a sibling directory under the crate, not under /tmp).
        let outside = tempfile::tempdir_in(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))).expect("outside");
        std::fs::write(outside.path().join("o.log"), PASS_LOG).expect("log");
        let cmd = format!("cargo test > {}", outside.path().join("o.log").display());
        assert!(extract(root, &cmd, "", Some(0)).iter().all(|t| t != "outcome:test_pass"));
        // A symlink inside the root pointing outside it.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path().join("o.log"), root.join("link.log")).expect("symlink");
            assert!(extract(root, "cargo test > link.log", "", Some(0)).iter().all(|t| t != "outcome:test_pass"));
            // A FIFO is never read (it would block forever).
            let fifo = root.join("pipe.log");
            let status = std::process::Command::new("mkfifo").arg(&fifo).status().expect("mkfifo");
            assert!(status.success());
            assert!(extract(root, "cargo test > pipe.log", "", Some(0)).iter().all(|t| t != "outcome:test_pass"));
        }
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p phronesis-mcp --lib -- outcomes::adapter::tests`
Expected: FAIL to compile: `ExtractFromInput` has no field `not_before`.

- [ ] **Step 3: Write minimal implementation**

`adapter.rs`: add `pub not_before: Option<u64>,` to `ExtractFromInput` with a doc comment ("unix seconds the command started at, from the in-flight record; a redirect file older than this is never read"). Add:

```rust
/// The redirect target attributable to the handled invocation: a `>`-style
/// target in its own segment, else a `tee` file in the very next segment.
fn redirect_target_for(project_root: &Path, command: &str) -> Option<String> {
    let segments = crate::outcomes::segment::command_heads(command);
    let idx = segments.iter().position(|s| handles(project_root, s))?;
    if let Some(t) = crate::outcomes::segment::stdout_redirect_target(&segments[idx]) {
        return Some(t);
    }
    segments
        .get(idx + 1)
        .and_then(|next| crate::outcomes::segment::tee_target(next))
}

/// Read the handled command's redirect file as its output, under the rules
/// the module docs state: in-project, regular file, fresh, capped.
fn redirect_output(project_root: &Path, command: &str, not_before: Option<u64>) -> Option<String> {
    let not_before = not_before?;
    let target = redirect_target_for(project_root, command)?;
    let path = {
        let p = Path::new(&target);
        if p.is_absolute() { p.to_path_buf() } else { project_root.join(p) }
    };
    let root_canon = std::fs::canonicalize(project_root).ok()?;
    let canon = std::fs::canonicalize(&path).ok()?;
    if !canon.starts_with(&root_canon) {
        return None;
    }
    // Refuse anything but a regular file before opening (a FIFO would block).
    let meta = std::fs::symlink_metadata(&canon).ok()?;
    if !meta.is_file() {
        return None;
    }
    let mtime = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    if mtime < not_before {
        return None;
    }
    crate::security::read_file_capped(&canon).ok()
}
```

Check `security::read_file_capped` (security.rs:261-274): it documents FIFO and size protection; if it does not itself re-check the file type on the open handle, add that check there rather than here (one place). In `extract_from`, after the `handles` check:

```rust
    let (text, from_file) = if output.trim().is_empty() {
        match redirect_output(project_root, command, not_before) {
            Some(t) => (t, true),
            None => (output.to_string(), false),
        }
    } else {
        (output.to_string(), false)
    };
    let (mut tags, subject) = extract_handled(project_root, command, &text, command_exit);
    if from_file && subject.is_some() && !tags.is_empty() {
        tags.push("outcome:output_from_file".to_string());
    }
    (tags, subject)
```

`state.rs`: add

```rust
/// Read-only look at the newest live in-flight record for `key`, for callers
/// that need its `ts` or `probe_root` before the lifecycle tail pops it.
pub fn peek_inflight(root: &Path, key: &str) -> Option<Inflight> { /* same locking and filters as pop_inflight, without removal */ }
```

`hook/journey_record.rs::outcomes_for_journal`: compute the call key the same way `lifecycle_wiring` does (grep `Call::key` / `key()` in `hook/lifecycle_wiring.rs`), then `let not_before = state::peek_inflight(&root, &key).map(|e| e.ts);` and pass it. Confirm the order of `outcomes_for_journal` versus the lifecycle pop in the hook tail (`hook.rs`); if the pop runs first, have the wiring hand the popped `Inflight.ts` to `outcomes_for_journal` instead of peeking. `codex_hook.rs:1557`: `not_before: None`.

- [ ] **Step 4: Run tests to verify they pass**

Run each on its own:
```
cargo test -p phronesis-mcp --lib -- outcomes
cargo test -p phronesis-mcp --lib -- hook
cargo test -p phronesis-mcp --test hook_integration
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/outcomes crates/phronesis-mcp/src/hook crates/phronesis-mcp/src/lifecycle/state.rs crates/phronesis-mcp/src/codex_hook.rs
git commit -m "feat(outcomes): parse a fresh in-project redirect file when the host captured no output"
```

### Task C3: `phr-mcp signal ingest` records parsed evidence from a saved output file

**Files:**
- Modify: `crates/phronesis-mcp/src/main.rs:115` (`Command::Signal`), `:815` (dispatch), `:1148` (`handle_signal`)
- Modify: `crates/phronesis-mcp/src/outcomes/mod.rs:66-77` (`SignalError`: add `NoToolchain(String)`, `NoOutcome`), add `record_from_output`
- Test: `crates/phronesis-mcp/tests/cli_smoke.rs` (append)

**Interfaces:**
- Produces: `phr-mcp signal ingest --command "<cmd>" --output <file> [--exit <n>]`. `pub fn record_from_output(root: &Path, command: &str, output: &str, exit: Option<i32>) -> Result<(String, Vec<String>), SignalError>`: runs `adapter::extract_from` with `not_before: None` (the operator named the file; no redirect following happens because `output` is non-empty), errors `NoToolchain` when no def handles the command, errors `NoOutcome` when parsing yields no tags (nothing is journaled), otherwise appends one journal record whose tags are the parsed tags plus `outcome:ingested`, and returns `(subject, tags)`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/phronesis-mcp/tests/cli_smoke.rs`:

```rust
fn ingest_project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join(".phronesis")).expect("mkdir");
    std::fs::write(dir.path().join(".phronesis/rules.json"), r#"{"rules": []}"#).expect("rules");
    std::fs::write(dir.path().join(".phronesis/confidence.json"), "{}").expect("enable");
    dir
}
fn phr(root: &std::path::Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_phr-mcp")).current_dir(root).args(args).output().expect("run phr-mcp")
}

#[test]
fn signal_ingest_records_parsed_evidence_and_refuses_empty_or_unknown_output() {
    let dir = ingest_project();
    let root = dir.path();
    std::fs::write(root.join("gate.log"), "test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n").expect("log");
    let out = phr(root, &["signal", "ingest", "--command", "cargo test --workspace", "--output", "gate.log", "--exit", "0"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("outcome:test_pass") && stdout.contains("outcome:ingested"), "{stdout}");
    let conf: serde_json::Value = serde_json::from_slice(&phr(root, &["confidence", "--json"]).stdout).expect("json");
    assert!(conf["signals"].to_string().contains("tests"), "{conf}");

    // A non-zero exit with a failing summary journals a failure, not a pass.
    std::fs::write(root.join("bad.log"), "test result: FAILED. 11 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out\n").expect("log");
    let out = phr(root, &["signal", "ingest", "--command", "cargo test", "--output", "bad.log", "--exit", "101"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("outcome:test_fail"));

    // Empty output: nothing journaled, non-zero exit, clear message.
    std::fs::write(root.join("empty.log"), "").expect("log");
    let out = phr(root, &["signal", "ingest", "--command", "cargo test", "--output", "empty.log"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("no outcome"), "{}", String::from_utf8_lossy(&out.stderr));

    // Unknown toolchain: refused by name.
    let out = phr(root, &["signal", "ingest", "--command", "make check", "--output", "gate.log"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("no toolchain definition handles"), "{}", String::from_utf8_lossy(&out.stderr));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p phronesis-mcp --test cli_smoke -- signal_ingest_records_parsed_evidence_and_refuses_empty_or_unknown_output --exact`
Expected: FAIL (clap rejects `ingest`).

- [ ] **Step 3: Implement**

`outcomes/mod.rs`, in `SignalError` (variants today: `NotEnabled`, `UnknownSignal`, `Subject`, `Journal`):

```rust
    #[error("no toolchain definition handles `{0}`; see `phr-mcp toolchains`")]
    NoToolchain(String),
    #[error("no outcome could be parsed from that output; nothing was journaled")]
    NoOutcome,
```

and

```rust
/// Record the outcome of a command whose output was saved to a file: the
/// hand-run and harness-worker escape hatch. Runs the parse the post-check
/// hook runs (`adapter::extract_from`) and journals its tags on the open work
/// unit's subject with the provenance tag `outcome:ingested`. Refuses, and
/// journals nothing, when no toolchain handles the command or the output
/// yields no outcome.
pub fn record_from_output(root: &Path, command: &str, output: &str, command_exit: Option<i32>) -> Result<(String, Vec<String>), SignalError> {
    if !enabled(root) {
        return Err(SignalError::NotEnabled);
    }
    if !adapter::handles(root, command) {
        return Err(SignalError::NoToolchain(command.to_string()));
    }
    let (mut tags, subject) = adapter::extract_from(adapter::ExtractFromInput {
        project_root: root,
        tool_name: "Bash",
        command: Some(command),
        output,
        command_exit,
        not_before: None,
    });
    let subject_id = subject.ok_or_else(|| SignalError::NoToolchain(command.to_string()))?;
    if tags.is_empty() {
        return Err(SignalError::NoOutcome);
    }
    tags.push("outcome:ingested".to_string());
    let record = crate::journey::journal::JournalRecord {
        v: 1,
        ts: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        sid: crate::journey::current_sid(root),
        seq: crate::hook::seq::next_seq(root),
        tool: "phr-mcp".to_string(),
        path: "<signal>".to_string(),
        ext: None,
        module: None,
        tags: tags.clone(),
        subject: Some(subject_id.clone()),
        command_exit,
        kind: None,
        mode: None,
        host: None,
        turn: None,
        agent: None,
        agent_type: None,
        kalpa: None,
    };
    crate::journey::journal::append(root, &record)?;
    Ok((subject_id, tags))
}
```

`main.rs`: change the `Signal` variant to carry an optional subcommand:

```rust
    Signal {
        #[command(subcommand)]
        action: Option<SignalAction>,
        /// Which signal: `compile` or `tests` (bare form).
        name: Option<String>,
        /// The outcome: `pass` or `fail` (bare form).
        #[arg(value_parser = ["pass", "fail"])]
        outcome: Option<String>,
    },
```

```rust
#[derive(clap::Subcommand)]
enum SignalAction {
    /// Parse a saved command output through the toolchain definitions and
    /// journal the result, as the post-check hook would have.
    Ingest {
        /// The command whose output this is, e.g. "cargo test --workspace".
        #[arg(long)]
        command: String,
        /// File holding the command's stdout and stderr.
        #[arg(long)]
        output: PathBuf,
        /// The command's exit code, when known.
        #[arg(long)]
        exit: Option<i32>,
    },
}
```

Dispatch:

```rust
        Command::Signal { action: Some(SignalAction::Ingest { command, output, exit }), .. } => handle_signal_ingest(&command, &output, exit),
        Command::Signal { action: None, name: Some(name), outcome: Some(outcome), .. } => handle_signal(&name, outcome == "pass"),
        Command::Signal { .. } => anyhow::bail!("usage: phr-mcp signal <compile|tests> <pass|fail>  or  phr-mcp signal ingest --command <cmd> --output <file> [--exit N]"),
```

```rust
fn handle_signal_ingest(command: &str, output: &Path, exit: Option<i32>) -> anyhow::Result<()> {
    let root = phronesis_mcp::security::project_root();
    let text = phronesis_mcp::security::read_file_capped(output)?;
    let (subject, tags) = phronesis_mcp::outcomes::record_from_output(&root, command, &text, exit)?;
    println!("recorded for subject {subject}: {}", tags.join(" "));
    Ok(())
}
```

- [ ] **Step 4: Verify**

Run each on its own: `cargo test -p phronesis-mcp --test cli_smoke`, `cargo test -p phronesis-mcp --lib -- outcomes`, `cargo clippy --workspace --all-targets -- -D warnings`. Expected: all green.

- [ ] **Step 5: Docs and commit**

`crates/phronesis-mcp/CLAUDE.md`, next to `signal`: "`phr-mcp signal ingest --command '<cmd>' --output <file> [--exit N]` parses a saved output through the toolchain definitions and journals the result with `outcome:ingested`; use it after `cargo test … > file` in a shell the hook did not see, or from a harness with no post-hook (the swarming skill's hand-off gate lists it)." `AGENTS.md` CLI table row for `signal`: append the `ingest` form. `CHANGELOG.md` → `### Added`:

```markdown
- **Confidence signals survive redirected output.** When a handled command
  sends stdout to a file inside the project and the host captured nothing,
  the post-check adapter parses that file instead, provided it is a regular
  file written after the command started; the journal record carries
  `outcome:output_from_file`. `phr-mcp signal ingest --command <cmd>
  --output <file>` does the same for a gate run by hand or by a harness
  without a post-hook, journaling parsed evidence tagged `outcome:ingested`,
  and refuses when no toolchain handles the command or nothing parses.
```

The `harness-workers.md` hand-off gate line is added by Task D1 (single agent-skills commit).

```bash
git add crates/phronesis-mcp/src/main.rs crates/phronesis-mcp/src/outcomes/mod.rs crates/phronesis-mcp/tests/cli_smoke.rs crates/phronesis-mcp/CLAUDE.md AGENTS.md CHANGELOG.md
git commit -m "feat(cli): phr-mcp signal ingest parses a saved gate output into confidence evidence"
```

---

## Part D — `govern-worktree` seeds uncommitted tracked `.phronesis` files (swarming tool)

**Shortfall.** Worker worktrees are cut from `HEAD`. `.phronesis/properties.json` and `.phronesis/toolchains.json` are both tracked, and the main checkout had ten uncommitted property records. `seed()` skips any tracked `COPY_FILES` entry, and `properties.json` was not in the list at all, so every worker saw the committed stub. The first loader-verifier attempt died on "property not found".

**Decisions.** (1) The dirty set is derived from `git -C <source> status --porcelain -- .phronesis/`, not from a hard-coded list, so the next tracked file is covered. (2) **Three-way check:** a tracked file is seeded only when the worktree's copy equals its `HEAD` version (the worker has not touched it) and the source's working copy differs; a worker-modified file is a conflict, reported and left alone. (3) Seeding is the **default** (CLI `--no-seed-tracked` and MCP `seed_tracked: false` opt out), because a swarm driver that never reads warnings is exactly the failure this fixes. (4) During a swarm the **source checkout owns** tracked `.phronesis` files; workers do not write them (they cannot promote properties anyway). The seeded copy is marked `skip-worktree`; `git update-index --no-skip-worktree -- <file>` releases it, and the docs say so. (5) `update-index` failures are reported, not ignored.

**Repository:** `/Volumes/Data/Git/agent-skills/skills/swarming/tool` (crate `swarmctl`). Tests in `tests/govern_worktree.rs` use a `phr-mcp` stub via `SWARMCTL_PHR_MCP`.

### Task D1: Seed dirty tracked `.phronesis` files by default, with a three-way conflict check

**Files:**
- Modify: `src/govern.rs:85-100` (`SeedOptions`: add `seed_tracked: bool`), `:300-345` (the copy loop), add `dirty_tracked_phronesis(source) -> Result<Vec<String>>`
- Modify: `src/main.rs:250-275` (`--no-seed-tracked`), the `SeedOptions` construction (~line 598); `src/mcp.rs` (`swarm_govern_worktree`: optional `seed_tracked`, default true)
- Modify: `../SKILL.md` (step 5), `../reference/harness-workers.md` ("Worktree setup" and hand-off gate step 3: add the `phr-mcp signal ingest` line for C3)
- Test: `tests/govern_worktree.rs` (append)

**Interfaces:**
- Produces: `SeedOptions.seed_tracked: bool` (default true from both CLI and MCP). Report actions: `seeded tracked .phronesis/<f> from <source> (uncommitted source changes; skip-worktree set)`; warnings: `tracked .phronesis/<f>: source has uncommitted changes but the worktree copy differs from HEAD too; left alone (worker-modified)`, `tracked .phronesis/<f>: source has uncommitted changes; not seeded (--no-seed-tracked)`, `git update-index --skip-worktree failed for .phronesis/<f>: <stderr>`.

- [ ] **Step 1: Write the failing test**

Read `tests/govern_worktree.rs:25-75` for the `fixture`, `git` and `run` helpers and their field names, then append:

```rust
#[test]
fn dirty_tracked_phronesis_files_are_seeded_by_default_unless_the_worker_changed_them() {
    let f = fixture(true);
    let main = &f.main;
    fs::write(main.join(".phronesis/properties.json"), r#"{"version":1,"properties":[]}"#).unwrap();
    fs::write(main.join(".phronesis/toolchains.json"), "[]").unwrap();
    git(main, &["add", "-f", ".phronesis/properties.json", ".phronesis/toolchains.json"]);
    git(main, &["commit", "-q", "-m", "track phronesis files"]);
    let dirty_props = r#"{"version":1,"properties":[{"id":"x.y","subject":"x","kind":"totality","depends_on":[],"source":"explicit_spec","status":"observed","corroborated_by":[],"encodings":[]}]}"#;
    fs::write(main.join(".phronesis/properties.json"), dirty_props).unwrap();
    fs::write(main.join(".phronesis/toolchains.json"), r#"[{"id":"kani","matches":"^cargo kani"}]"#).unwrap();

    let wt = f.tmp.path().join("wt");
    git(main, &["worktree", "add", "-q", "-b", "w", wt.to_str().unwrap()]);
    // The worker already edited toolchains.json: that one must not be overwritten.
    fs::write(wt.join(".phronesis/toolchains.json"), r#"[{"id":"worker-local","matches":"^make"}]"#).unwrap();

    let out = run(&f.tmp, &["govern-worktree", wt.to_str().unwrap(), "--source", main.to_str().unwrap()]);
    let report: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    let actions = report["actions"].to_string();
    let warnings = report["warnings"].to_string();
    assert!(actions.contains("seeded tracked .phronesis/properties.json"), "{report}");
    assert_eq!(fs::read_to_string(wt.join(".phronesis/properties.json")).unwrap(), dirty_props);
    assert!(warnings.contains("toolchains.json") && warnings.contains("worker-modified"), "{report}");
    assert!(fs::read_to_string(wt.join(".phronesis/toolchains.json")).unwrap().contains("worker-local"));
    let ls = Command::new("git").args(["-C", wt.to_str().unwrap(), "ls-files", "-v", ".phronesis/properties.json"]).output().unwrap();
    assert!(String::from_utf8_lossy(&ls.stdout).starts_with('S'), "skip-worktree set: {:?}", ls.stdout);

    // Opt out: warn only.
    let wt2 = f.tmp.path().join("wt2");
    git(main, &["worktree", "add", "-q", "-b", "w2", wt2.to_str().unwrap()]);
    let out = run(&f.tmp, &["govern-worktree", wt2.to_str().unwrap(), "--source", main.to_str().unwrap(), "--no-seed-tracked"]);
    let report: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert!(report["warnings"].to_string().contains("not seeded (--no-seed-tracked)"), "{report}");
    assert_ne!(fs::read_to_string(wt2.join(".phronesis/properties.json")).unwrap(), dirty_props);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --manifest-path /Volumes/Data/Git/agent-skills/skills/swarming/tool/Cargo.toml --test govern_worktree -- dirty_tracked_phronesis_files_are_seeded_by_default_unless_the_worker_changed_them --exact`
Expected: FAIL (clap rejects `--no-seed-tracked`; and no `seeded tracked` action).

- [ ] **Step 3: Implement**

`src/govern.rs`: add `pub seed_tracked: bool,` to `SeedOptions`. Add:

```rust
/// Tracked files under `.phronesis/` whose working copy in `source` differs
/// from `HEAD` (modified, not untracked): the state a worktree cut from HEAD
/// does not see.
fn dirty_tracked_phronesis(source: &Path) -> Result<Vec<String>> {
    let out = git(source, &["status", "--porcelain", "--untracked-files=no", "--", ".phronesis/"])?;
    anyhow::ensure!(out.status.success(), "git status in {}: {}", source.display(), String::from_utf8_lossy(&out.stderr));
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.get(3..).map(str::trim).filter(|p| !p.is_empty()).map(str::to_string))
        .collect())
}

/// The worktree's `HEAD` bytes for `rel`, or `None` when HEAD has no such file.
fn head_bytes(worktree: &Path, rel: &str) -> Option<Vec<u8>> {
    git(worktree, &["show", &format!("HEAD:{rel}")]).ok().filter(|o| o.status.success()).map(|o| o.stdout)
}
```

After the `COPY_FILES`/`COPY_DIRS` loops (inside the `(Some(src), Some(_))` arm), add:

```rust
            for rel in dirty_tracked_phronesis(src)? {
                let src_bytes = fs::read(src.join(&rel)).ok();
                let wt_path = wt.join(&rel);
                let wt_bytes = fs::read(&wt_path).ok();
                if src_bytes.is_none() || src_bytes == wt_bytes {
                    continue; // deleted in source, or already identical
                }
                if !opts.seed_tracked {
                    warnings.push(format!("tracked {rel}: source has uncommitted changes; not seeded (--no-seed-tracked)"));
                    continue;
                }
                if wt_bytes.is_some() && wt_bytes != head_bytes(wt, &rel) {
                    warnings.push(format!("tracked {rel}: source has uncommitted changes but the worktree copy differs from HEAD too; left alone (worker-modified)"));
                    continue;
                }
                copy_tree(&src.join(&rel), &wt_path)?;
                match git(wt, &["update-index", "--skip-worktree", "--", &rel]) {
                    Ok(o) if o.status.success() => actions.push(format!(
                        "seeded tracked {rel} from {} (uncommitted source changes; skip-worktree set)",
                        src.display()
                    )),
                    Ok(o) => warnings.push(format!("git update-index --skip-worktree failed for {rel}: {}", String::from_utf8_lossy(&o.stderr).trim())),
                    Err(e) => warnings.push(format!("git update-index --skip-worktree failed for {rel}: {e}")),
                }
            }
```

Leave the existing `COPY_FILES` tracked branch as is (its `kept` action still applies to files that are clean in the source). `src/main.rs`: add `#[arg(long)] no_seed_tracked: bool,` to `GovernWorktree` and pass `seed_tracked: !no_seed_tracked`. `src/mcp.rs`: optional `seed_tracked: Option<bool>` on `swarm_govern_worktree`, passed as `.unwrap_or(true)`. Fix other `SeedOptions { .. }` literals with `seed_tracked: true`.

- [ ] **Step 4: Verify**

From the tool directory, each on its own: `cargo test --test govern_worktree`, `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo build --release`, then `cargo install --path .` so `~/.cargo/bin/swarmctl` is current.

- [ ] **Step 5: Docs, package, commit**

`skills/swarming/SKILL.md` step 5: "…run `swarm_govern_worktree` on the worker's worktree first (it seeds the main checkout's uncommitted edits to tracked `.phronesis/` files by default and refuses to overwrite a worker-modified copy; pass `seed_tracked: false` to opt out)…". `reference/harness-workers.md` "Worktree setup": add the ownership sentence ("during a swarm the main checkout owns tracked `.phronesis/` files; workers never write them; `git update-index --no-skip-worktree -- <file>` releases a seeded copy"); hand-off gate step 3: add "`phr-mcp signal ingest --command '<gate cmd>' --output <log> --exit <code>` journals a gate you ran outside the hook (phr-mcp ≥ the version that ships it)". Repackage per the agent-skills sync procedure and bump the skill version once.

```bash
git add skills/swarming
git commit -m "feat(swarmctl): govern-worktree seeds dirty tracked .phronesis files by default with a three-way check"
```

---

## Part E — Kani results flow through the outcomes seam

**Shortfall.** The Kani runs were hand-driven. No toolchain definition recognises `cargo kani`, so no `proof_outcome` fact, journey tag, or `proof` confidence signal was produced, and a harness name cannot be tied to a property. The seam itself works for a mock verifier (`outcomes/derive.rs:456-534`), and `confidence` does surface the `proof` signal (`derive.rs:228`, band-lift test at `:526-534`).

**Decisions.** (1) Real Kani 0.68 output pairs `Checking harness <path>...` with a later `VERIFICATION:- SUCCESSFUL|FAILED`, with hundreds of check lines between and only failures named in the summary. A single regex with `(?s).*?` can pair a header that never got a verdict (killed run, mid-run compiler error) with the next harness's verdict, and the `regex` crate has no lookahead to forbid it. So `ToolchainDef` gains an optional `section_start` regex: when set, `per_test_results` splits the output at each match and applies `per_test` inside each section. (2) Harness → property binding lives in the property's `encodings` as `artifact: "harness:<module::path::name>"`. (3) An **unbound** harness result is not dropped: it is journaled as `proof_unbound` with its status, so a failing unregistered proof is visible; it never counts for any property. (4) A properties store that fails to load is reported and treated as "no bindings" with a stderr line, never silently. (5) Out of scope: bound `verification_result` records (SPEC-property-ontology §2, D9) through `properties::execute`.

### Task E1: `section_start` for per-test parsing, and a Kani definition that uses it

**Files:**
- Modify: `crates/phronesis-mcp/src/outcomes/toolchain.rs:32-80` (`ToolchainDef`: add `section_start: Option<String>`), `:150-245` (`CompiledDef`: compile it), `:322-335` (`per_test_results`)
- Modify: `.phronesis/toolchains.json` (append the def)
- Test: `crates/phronesis-mcp/src/outcomes/toolchain.rs` tests (append)

**Interfaces:**
- Produces: `ToolchainDef.section_start: Option<String>` (serde default, skipped when `None`); `CompiledDef::per_test_results` applies `per_test` per section when `section_start` is set (each section runs from one match to the next). Project def `id: "kani"`, `matches: "^cargo kani"`, `compile_fail: ["error\\[E\\d+\\]", "internal compiler error"]`, `compile_success: ["Manual Harness Summary"]`, `section_start: "(?m)^Checking harness "`, `per_test: "(?s)Checking harness (?P<name>\\S+?)\\.\\.\\..*?VERIFICATION:- (?P<status>SUCCESSFUL|FAILED)"`, `pass_tokens: ["SUCCESSFUL"]`, `outcome_kind: "proof"`. Matching is per command segment (`toolchain.rs:255-259`), so `cd <wt> && cargo kani …` is recognised.

- [ ] **Step 1: Write the failing tests**

Append to the test module in `toolchain.rs`:

```rust
    fn kani_project_def() -> ToolchainDef {
        serde_json::from_str(
            r#"{
              "id": "kani",
              "matches": "^cargo kani",
              "compile_fail": ["error\\[E\\d+\\]", "internal compiler error"],
              "compile_success": ["Manual Harness Summary"],
              "section_start": "(?m)^Checking harness ",
              "per_test": "(?s)Checking harness (?P<name>\\S+?)\\.\\.\\..*?VERIFICATION:- (?P<status>SUCCESSFUL|FAILED)",
              "pass_tokens": ["SUCCESSFUL"],
              "outcome_kind": "proof"
            }"#,
        )
        .expect("def parses")
    }

    #[test]
    fn kani_sections_pair_each_header_with_its_own_verdict_and_leave_unfinished_ones_out() {
        let compiled = CompiledDef::compile(kani_project_def(), DefSource::Project).expect("compiles");
        assert!(compiled.handles("cd /wt && cargo kani -p phronesis --harness add_binding_totality"));
        let output = "Checking harness variable_binding::kani_harness::add_binding_totality...\n\
                      CBMC 6.11\nCheck 385: core::ub_checks::is_valid_allocation_size.division-by-zero.1\n\
                      SUMMARY:\n ** 0 of 653 failed (10 unreachable)\nVERIFICATION:- SUCCESSFUL\n\n\
                      Checking harness variable_binding::kani_harness::merge_totality...\n\
                      Running propositional reduction\n\
                      Checking harness variable_binding::kani_harness::can_bind_totality...\n\
                      SUMMARY:\n ** 1 of 900 failed\nVERIFICATION:- FAILED\n\n\
                      Manual Harness Summary:\nVerification failed for - variable_binding::kani_harness::can_bind_totality\n\
                      Complete - 1 successfully verified harnesses, 1 failures, 2 total.\n";
        assert_eq!(
            compiled.per_test_results(output),
            vec![
                ("variable_binding::kani_harness::add_binding_totality".to_string(), true),
                ("variable_binding::kani_harness::can_bind_totality".to_string(), false),
            ],
            "merge_totality had no verdict and must not borrow can_bind's"
        );
        let facts = compiled.parse("u", "cargo kani -p phronesis", output, Some(1));
        let proofs: Vec<&Vec<String>> = facts.iter().filter(|f| f.predicate == "proof_outcome").map(|f| &f.args).collect();
        assert_eq!(proofs.len(), 2, "{facts:?}");
        assert_eq!(proofs[0][2], "passed");
        assert_eq!(proofs[1][2], "failed");
    }

    #[test]
    fn a_def_without_section_start_parses_as_before() {
        let mut def = kani_project_def();
        def.section_start = None;
        let compiled = CompiledDef::compile(def, DefSource::Project).expect("compiles");
        let output = "Checking harness a::b...\nVERIFICATION:- SUCCESSFUL\n";
        assert_eq!(compiled.per_test_results(output), vec![("a::b".to_string(), true)]);
    }

    #[test]
    fn the_project_registry_carries_a_compiling_kani_def() {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(|p| p.parent()).expect("repo root");
        let defs = registry(repo);
        let kani = defs.iter().find(|d| d.def.id == "kani").expect("a kani def is registered in .phronesis/toolchains.json");
        assert_eq!(kani.source, DefSource::Project);
        assert!(kani.is_proof);
        assert!(kani.handles("cargo kani -p phronesis --harness x"));
    }
```

(`registry(root)` is the loader `main.rs:1158` uses; it already copes with the `_doc` key on the first entry of `.phronesis/toolchains.json`. Adjust the field paths `d.def.id`/`d.source` to the compiled-def struct's real names.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p phronesis-mcp --lib -- outcomes::toolchain::tests::kani`
Expected: FAIL to compile (`section_start` unknown) or the pairing assertion fails.

- [ ] **Step 3: Implement**

`ToolchainDef`: add after `per_test`:

```rust
    /// Optional regex marking the start of each per-test section. When set,
    /// `per_test` is applied inside each section (from one match to the
    /// next) rather than across the whole output, so a multi-line `per_test`
    /// can never pair one item's header with a later item's verdict (Kani
    /// prints `Checking harness X...` and, hundreds of lines later,
    /// `VERIFICATION:- …`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section_start: Option<String>,
```

`CompiledDef`: add `section_start: Option<Regex>`, compiled in `compile` with the same error path as `per_test`. In `per_test_results`:

```rust
    pub fn per_test_results(&self, output: &str) -> Vec<(String, bool)> {
        let Some(re) = &self.per_test else { return Vec::new() };
        let sections: Vec<&str> = match &self.section_start {
            None => vec![output],
            Some(start) => {
                let mut bounds: Vec<usize> = start.find_iter(output).map(|m| m.start()).collect();
                bounds.push(output.len());
                bounds.windows(2).map(|w| &output[w[0]..w[1]]).collect()
            }
        };
        sections
            .iter()
            .flat_map(|section| {
                re.captures_iter(section).map(|caps| {
                    let name = caps.name("name").map(|m| m.as_str().to_string()).unwrap_or_default();
                    let status = caps.name("status").map(|m| m.as_str()).unwrap_or("");
                    (name, self.pass_tokens.iter().any(|t| t == status))
                })
            })
            .collect()
    }
```

Keep whatever the existing body does for `pass_tokens` matching; the shape above is the change. Append the Kani def (fields as in `kani_project_def`) to `.phronesis/toolchains.json`.

- [ ] **Step 4: Verify**

Run: `cargo test -p phronesis-mcp --lib -- outcomes::toolchain` → all pass; `phr-mcp toolchains` lists `kani` with source `project`.

- [ ] **Step 5: Commit**

```bash
git add .phronesis/toolchains.json crates/phronesis-mcp/src/outcomes/toolchain.rs
git commit -m "feat(outcomes): section-scoped per-test parsing and a Kani toolchain definition"
```

### Task E2: Bind harness names to properties; keep unbound results visible

**Files:**
- Modify: `crates/phronesis-mcp/src/outcomes/facts.rs:60-80` (add `OutcomeFact::proof_unbound`)
- Modify: `crates/phronesis-mcp/src/outcomes/adapter.rs:117-150` (`proof_tag` for the new predicate), `:232-262` (`extract_handled`)
- Modify: `crates/phronesis-mcp/src/predicate_provider.rs:114` (add `proof_unbound` to the reserved predicates)
- Modify: `crates/phronesis-mcp/src/properties/store.rs:58-62` (doc on `Encoding.artifact`)
- Test: `crates/phronesis-mcp/src/outcomes/adapter.rs` tests (append)

**Interfaces:**
- Consumes: `properties::store::load_properties(root) -> Result<Vec<Property>, PropertyStoreError>`; `Property.id`, `Property.encodings[].{verifier, artifact}`; `CompiledDef.is_proof` and its def id.
- Produces: `OutcomeFact::proof_unbound(subject, harness, passed)` → predicate `proof_unbound`, args `[subject, harness, "passed"|"failed"]`, tag `outcome:proof_unbound:<harness>:<passed|failed>`. For a proof def, each `proof_outcome` whose name is a property id stays; one whose name maps through an encoding `{verifier: <def id>, artifact: "harness:<name>"}` is rewritten to that property id; anything else becomes `proof_unbound`. `proof_run_outcome` facts are untouched, so the existing stale-pass retraction still works. A store load error prints `phronesis: properties.json could not be loaded (<error>); Kani results are journaled as unbound` once and treats every harness as unbound.

- [ ] **Step 1: Write the failing tests**

Append to `adapter.rs` tests (reuse `enabled_project()` from C2; if Part C has not landed, copy that helper here):

```rust
    const KANI_DEF: &str = r#"[{"id":"kani","matches":"^cargo kani","compile_success":["Manual Harness Summary"],
        "section_start":"(?m)^Checking harness ",
        "per_test":"(?s)Checking harness (?P<name>\\S+?)\\.\\.\\..*?VERIFICATION:- (?P<status>SUCCESSFUL|FAILED)",
        "pass_tokens":["SUCCESSFUL"],"outcome_kind":"proof"}]"#;
    const BOUND_PROPERTY: &str = r#"{"version":1,"properties":[{"id":"variable_binding.substitution_totality","subject":"variable_binding",
        "kind":"totality","depends_on":[],"source":"code_inference","status":"observed","corroborated_by":[],
        "encodings":[{"language":"rust","verifier":"kani","artifact":"harness:variable_binding::kani_harness::add_binding_totality"}]}]}"#;
    const TWO_HARNESSES: &str = "Checking harness variable_binding::kani_harness::add_binding_totality...\nVERIFICATION:- SUCCESSFUL\n\
        Checking harness variable_binding::kani_harness::unbound...\nVERIFICATION:- FAILED\n\
        Manual Harness Summary:\nVerification failed for - variable_binding::kani_harness::unbound\nComplete - 1 successfully verified harnesses, 1 failures, 2 total.\n";

    #[test]
    fn proof_names_map_through_harness_encodings_and_unbound_results_stay_visible() {
        let dir = enabled_project();
        let root = dir.path();
        std::fs::write(root.join(".phronesis/toolchains.json"), KANI_DEF).expect("toolchains");
        std::fs::write(root.join(".phronesis/properties.json"), BOUND_PROPERTY).expect("properties");
        let (tags, subject) = extract_from(ExtractFromInput { project_root: root, tool_name: "Bash", command: Some("cargo kani -p phronesis"), output: TWO_HARNESSES, command_exit: Some(1), not_before: None });
        assert!(subject.is_some());
        assert!(tags.iter().any(|t| t == "outcome:proof_fail:variable_binding.substitution_totality" || t == "outcome:proof_pass:variable_binding.substitution_totality"),
            "the bound harness journals under the property id (a failed run journals per-property failures only): {tags:?}");
        assert!(tags.iter().any(|t| t == "outcome:proof_unbound:variable_binding::kani_harness::unbound:failed"), "{tags:?}");
        assert!(!tags.iter().any(|t| t.starts_with("outcome:proof_pass:variable_binding::kani_harness")), "raw harness paths never masquerade as property ids: {tags:?}");
    }

    #[test]
    fn a_corrupt_properties_store_makes_every_harness_unbound_and_says_so() {
        let dir = enabled_project();
        let root = dir.path();
        std::fs::write(root.join(".phronesis/toolchains.json"), KANI_DEF).expect("toolchains");
        std::fs::write(root.join(".phronesis/properties.json"), "{ not json").expect("corrupt");
        let (tags, _) = extract_from(ExtractFromInput { project_root: root, tool_name: "Bash", command: Some("cargo kani -p phronesis"), output: TWO_HARNESSES, command_exit: Some(1), not_before: None });
        assert!(tags.iter().any(|t| t.starts_with("outcome:proof_unbound:variable_binding::kani_harness::add_binding_totality:")), "{tags:?}");
        assert!(!tags.iter().any(|t| t.contains("substitution_totality")), "{tags:?}");
    }

    /// The run-level `proof_run_outcome` still retracts an earlier pass when
    /// a later run is failed or unbound: same discipline as
    /// `derive::proof_tests::a_failing_proof_run_retracts_an_earlier_proof_signal`,
    /// exercised through the adapter's rewrite.
    #[test]
    fn an_unbound_or_failed_later_run_does_not_leave_an_earlier_bound_pass_active() {
        // Build two journal record sets with derive's helpers exactly as that
        // test does (copy its fixture code), replacing the second run's output
        // with TWO_HARNESSES where the bound harness FAILED, and assert the
        // derived signals no longer contain signal_pass(_, "proof").
        // Read derive.rs:565-600 and mirror it here; the assertion is the same.
    }
```

Write the third test's body by mirroring `derive.rs:565-600` before running (it is a copy, not a design); the plan leaves it as a directive because that fixture code is 30 lines of existing test scaffolding.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p phronesis-mcp --lib -- outcomes::adapter::tests::proof`
Expected: FAIL (tags carry raw harness paths as `proof_pass`/`proof_fail`; no `proof_unbound`).

- [ ] **Step 3: Implement**

`facts.rs`, next to `proof`:

```rust
    /// `proof_unbound(subject, harness, "passed" | "failed")` — a verifier
    /// result for a harness no property claims (no encoding
    /// `{verifier, artifact: "harness:<name>"}`). Visible in the journal so a
    /// failing unregistered proof is never silent; evidence for no property.
    pub fn proof_unbound(subject: &str, harness: &str, passed: bool) -> Self {
        Self {
            predicate: "proof_unbound",
            args: vec![subject.to_string(), harness.to_string(), if passed { "passed" } else { "failed" }.to_string()],
        }
    }
```

`adapter.rs`: in `proof_tag`/`outcome_tags`, map `proof_unbound` → `format!("outcome:proof_unbound:{}:{}", args[1], args[2])`. Add:

```rust
/// The property a proof result speaks for: the name itself when it is a
/// property id, else the property whose encoding for `verifier` names
/// `harness:<name>`.
fn property_for_harness(properties: &[crate::properties::store::Property], verifier: &str, name: &str) -> Option<String> {
    if properties.iter().any(|p| p.id == name) {
        return Some(name.to_string());
    }
    let wanted = format!("harness:{name}");
    properties
        .iter()
        .find(|p| p.encodings.iter().any(|e| e.verifier == verifier && e.artifact == wanted))
        .map(|p| p.id.clone())
}
```

In `extract_handled`, when `def.is_proof`, load the store once and rewrite the facts:

```rust
    let outcome_facts = if def.is_proof {
        let properties = match crate::properties::store::load_properties(project_root) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("phronesis: properties.json could not be loaded ({e}); Kani results are journaled as unbound");
                Vec::new()
            }
        };
        let verifier = def.def.id.as_str();
        outcome_facts
            .into_iter()
            .map(|f| {
                if f.predicate != "proof_outcome" {
                    return f;
                }
                let (Some(name), Some(status)) = (f.args.get(1), f.args.get(2)) else { return f };
                match property_for_harness(&properties, verifier, name) {
                    Some(id) => OutcomeFact::proof(&subject, &id, status == "passed"),
                    None => OutcomeFact::proof_unbound(&subject, name, status == "passed"),
                }
            })
            .collect()
    } else {
        outcome_facts
    };
```

(`def.def.id`: use the real path to the def id on `CompiledDef`.) `predicate_provider.rs:114`: add `"proof_unbound"` beside `"proof_outcome"` so providers cannot forge it. `properties/store.rs` doc on `Encoding.artifact`: "A file path for a standalone artifact (Verus), or `harness:<module::path::name>` for an in-crate Kani proof harness."

- [ ] **Step 4: Verify**

Run: `cargo test -p phronesis-mcp --lib -- outcomes`, `cargo test -p phronesis-mcp --lib -- predicate_provider`, `cargo test -p phronesis-mcp --lib -- properties`; clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/outcomes crates/phronesis-mcp/src/predicate_provider.rs crates/phronesis-mcp/src/properties/store.rs
git commit -m "feat(outcomes): bind Kani harness results to properties; keep unbound results visible"
```

### Task E3: Bind the shipped harness to its property; document; end-to-end check

**Depends on Task C3** (`signal ingest`).

**Files:**
- Modify: `.phronesis/properties.json` (the `variable_binding.substitution_totality` entry's `encodings`). **Coordinate first:** this tracked file has uncommitted edits in the main checkout from the property-ontology work. Put the encoding on top of them and commit in the series the user chooses; do not commit their edits without their say-so.
- Modify: `docs/specs/SPEC-property-ontology.md` §3, `crates/phronesis-mcp/CLAUDE.md`, `CHANGELOG.md`

- [ ] **Step 1: Add the encoding**

```json
      "encodings": [
        {"language": "rust", "verifier": "kani", "artifact": "harness:variable_binding::kani_harness::add_binding_totality"}
      ]
```

- [ ] **Step 2: End-to-end check with the real harness**

Run each on its own in the main checkout:
```
phr-mcp unit start kani-e2e
cargo kani -p phronesis --harness add_binding_totality > /tmp/kani.log 2>&1; echo $?
phr-mcp signal ingest --command "cargo kani -p phronesis --harness add_binding_totality" --output /tmp/kani.log --exit 0
phr-mcp confidence --json
phr-mcp journey --json | grep -c "proof_pass:variable_binding.substitution_totality"
phr-mcp unit end
```
Expected: `ingest` prints `outcome:proof_pass:variable_binding.substitution_totality` and `outcome:ingested`; `confidence --json` lists `proof` among `signals` (derive.rs:228 grounds it; `Band::from_signal_count` counts it); the journey grep is ≥ 1. If `confidence` does not list `proof`, that is a bug in the seam, not in this plan: stop and file it.

- [ ] **Step 3: Docs**

`SPEC-property-ontology.md` §3 after the ToolchainDef sentence: "Kani is registered as a project def (`.phronesis/toolchains.json`, `id: kani`) with `section_start` scoping each harness; a harness is bound to its property by an encoding `{"verifier": "kani", "artifact": "harness:<module::path::name>"}`. An unbound harness result is journaled as `proof_unbound(subject, harness, status)` and counts for no property."

`CHANGELOG.md` → `### Added`:

```markdown
- **Kani results ground the `proof` signal.** A project toolchain definition
  recognises `cargo kani` and pairs each `Checking harness …` header with its
  own `VERIFICATION:-` verdict (new `section_start` field); a harness is bound
  to its property by an `encodings` entry
  `{"verifier":"kani","artifact":"harness:<path>"}`, so a passing run journals
  `outcome:proof_pass:<property>` and lifts the confidence band. Results for
  unregistered harnesses are journaled as `proof_unbound` rather than
  dropped. Bound `verification_result` records (SPEC-property-ontology §2)
  are still produced only by the artifact pipeline.
```

- [ ] **Step 4: Commit**

```bash
git add .phronesis/properties.json docs/specs/SPEC-property-ontology.md crates/phronesis-mcp/CLAUDE.md CHANGELOG.md
git commit -m "feat(properties): bind the add_binding_totality Kani harness to substitution_totality"
```

---

## Part F — Coverage evidence for Python (and any lcov-producing language)

**Revision 2 of this part (2026-09-29)** after GLM-5.3 and Codex reviewed it (`docs/superpowers/plans/reviews/2026-09-28-phronesis-evidence-gaps/review-f-*.md`). Their findings and dispositions are in that directory's `consolidated.md`, Part F section.

**Shortfall (user report, 2026-09-28).** A downstream Python project runs pytest with coverage in a devcontainer and has a `pytest` toolchain definition, yet `coverage select` and the evidence-gap rules never connect its tests to its code. Root cause, verified in this checkout: the code graph is multi-language (`graph/python.rs` emits `defines_test` and the derived `tested_by` edges), but **coverage is Rust-only** at three points: the collector wraps `cargo-llvm-cov` and keeps only `.rs` files under `src/` (`coverage/collect.rs:88-98`); the region map parses with tree-sitter Rust alone (`coverage/region_map.rs:427`); and the import accepts only records whose region ids that Rust map produced. Properties are already language-neutral (`encodings[].language`, Python-aware body validator), so this part is about coverage.

**Decisions.**
1. **lcov is the interchange format**, with its limits stated: line hits (`DA`) always; function counts (`FNDA`) only when the producer emits them; branch data (`BRDA`) is not consumed in this part, so Python gets no branch regions yet. coverage.py, c8/Istanbul, gcov and llvm-cov all emit lcov.
2. **A hit is an executed body line.** coverage.py marks a function's `def` line executed when the module is imported, so "any positive line in the definition span" would attribute every function in an imported module to every test. A function is hit when a `DA` line with count > 0 lies within its **body** (first body line through the end line). One-line bodies (`def f(): return x`) share the `def` line: for those, `FNDA` decides when present; otherwise the site is reported as `unattributable`, never counted.
3. **Store contract unchanged.** Records use `kind: "hit"`, `hit_kind: "region"`, region ids `fn:<file-segment>::<item-path>` (`store.rs:440-475` accepts only `region`/`branch`), store at `.phronesis/coverage.jsonl` + `.phronesis/coverage.index` (`store.rs:110-113`). Import stays all-or-nothing (`import.rs:70-93`); an import that yields zero records refuses and leaves the store untouched.
4. **Python item paths** are `Class::method`, `function`, `outer::inner`, with the same ordinal rule as Rust (`.2`, `.3` on the last segment for duplicate paths, source order). Nested-def and class-method collisions (`Store::load::inner` versus a class `load` with method `inner`) are accepted and documented; ordinals separate them.
5. **Test ids are exactly the graph's:** `python:<ns>::<path>::<segments>::<test_fn>` with every path segment joined by `::` (a concrete id is `python:pyside::tests::test_utils::test_load`, `graph/python.rs:41-58, 144-164, 543-550`), built by one shared helper. Every lcov file carries `TN:<graph-test-id>`; the file name is a sanitized encoding, never the source of the id.
6. **Container/host:** the collection loop runs where Python lives and writes a `manifest.json` beside the lcov files with the container's `git rev-parse HEAD` and a SHA-256 per covered source file; the host import refuses when its HEAD differs or any digest differs from the host file. `SF:` paths are relativized deterministically: relative and existing → as is; absolute under root → stripped; otherwise the **longest** suffix that exists under root, and **ambiguity (two candidates) refuses**. Every `SF:` that does not resolve, or is filtered as test code, is counted and named in the summary.
7. **Per-test isolation:** one fresh `coverage run --data-file=<n>.cov -m pytest <node-id>` per collected test. Module-import contamination is what decision 2 removes; fixture and setup code that a test genuinely executes is that test's evidence.
8. **CI has no Python** (`.github/workflows/ci.yml` runs Rust only). Parser, mapping, and import are tested from committed lcov fixtures; live `pytest`/`coverage` tests are gated on `python3` being on PATH and skip otherwise, saying so.
9. Delivered here: Python regions, lcov-dir import for any language with regions (Rust and Python today), and pytest collection. Other languages get regions in later parts.

### Task F1: Region map dispatches on language; Python function sites with body ranges

**Files:**
- Modify: `crates/phronesis-mcp/src/coverage/region_map.rs` (factor the function-path ordinal step out of `function_sites_by_node` at `:432-458` into `assign_function_ordinals(Vec<RawSite>) -> Vec<FunctionSite>`; add `body_start_line` to `FunctionSite`; add `extract_function_sites_for`, `python_function_sites`)
- Modify: `crates/phronesis-mcp/src/graph/python.rs:18-32` (expose the test-file classifier as `pub(crate) fn classify_python_file(file_path: &str) -> &'static str`)
- Modify: `crates/phronesis-mcp/src/coverage/collect.rs:88-98` (`is_wanted_source` per language; stays private, used by `collect` and, after F2, by `lcov.rs` via `pub(super)`)
- Test: `region_map.rs` and `collect.rs` test modules (append)

**Interfaces:**
- `FunctionSite` gains `pub body_start_line: u64` (for Rust: the line after the signature's `{`, computed from the `body` field's start row; for Python: the `body` field's start row). Existing consumers ignore it.
- `pub fn extract_function_sites_for(rel_path: &str, source: &str) -> Result<Vec<FunctionSite>>`: `rs` → `extract_function_sites`; `py` → `python_function_sites`; else `Ok(Vec::new())`.
- `pub fn python_function_sites(source: &str) -> Result<Vec<FunctionSite>>`: walks `function_definition` (tree-sitter-python 0.25 folds `async def` into it; the graph already relies on that, `graph/python.rs` test `an_async_def_is_a_defined_function`) nested in `class_definition`/`function_definition`; `start_line` is the `def` line (decorators excluded), `end_line` is inclusive and computed as `end_position().row + 1` **minus one when `end_position().column == 0`** (a node ending at the start of the next row does not include that row); `body_start_line` from the `body` field.
- `is_wanted_source(rel)`: `.rs` → existing rule; `.py` → `classify_python_file(rel) != "test"` (which already covers `tests/`, `test_*.py`, `*_test.py`) and file name not `conftest.py`; else `false`.

- [ ] **Step 1: Write the failing tests**

Append to the `region_map.rs` test module:

```rust
    #[test]
    fn python_function_sites_cover_module_class_nested_async_and_duplicate_definitions() {
        let src = "import os\n\
                   \n\
                   def helper(x):\n\
                   \x20   return x + 1\n\
                   \n\
                   class Store:\n\
                   \x20   def load(self):\n\
                   \x20       def inner():\n\
                   \x20           return 1\n\
                   \x20       return inner()\n\
                   \n\
                   \x20   def load(self, again):\n\
                   \x20       return again\n\
                   \n\
                   async def fetch():\n\
                   \x20   return 2\n\
                   \n\
                   def one_liner(): return 3\n";
        let sites = python_function_sites(src).expect("parses");
        let rows: Vec<(&str, u64, u64, u64)> = sites
            .iter()
            .map(|s| (s.item_path.as_str(), s.start_line, s.body_start_line, s.end_line))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("helper", 3, 4, 4),
                ("Store::load", 7, 8, 10),
                ("Store::load::inner", 8, 9, 9),
                ("Store::load.2", 12, 13, 13),
                ("fetch", 15, 16, 16),
                ("one_liner", 18, 18, 18),
            ],
            "{sites:?}"
        );
        assert_eq!(sites[1].region_id("pkg/store.py"), format!("fn:{}::Store::load", file_segment("pkg/store.py")));
        assert_eq!(sites[3].name(), "load");
    }

    #[test]
    fn rust_sites_now_carry_a_body_start_line() {
        let sites = extract_function_sites("fn a(\n    x: u8,\n) -> u8 {\n    x\n}\n").expect("rust");
        assert_eq!((sites[0].start_line, sites[0].body_start_line, sites[0].end_line), (1, 3, 5));
    }

    #[test]
    fn extract_function_sites_for_dispatches_on_extension() {
        assert_eq!(extract_function_sites_for("a/b.py", "def f():\n    pass\n").expect("py").len(), 1);
        assert_eq!(extract_function_sites_for("a/b.rs", "fn f() {}\n").expect("rs").len(), 1);
        assert!(extract_function_sites_for("a/b.txt", "fn f() {}\n").expect("other").is_empty());
    }
```

Before running, probe one real parse (`cargo test -- python_function_sites --nocapture` with a `dbg!` of `end_position()`) and, if the grammar puts a multi-line function's end at column 0 of the next row, keep the `-1` rule; if it ends at the last character of the last body line, drop it. Fix the rule, not the expected lines.

In `collect.rs` tests:

```rust
    #[test]
    fn wanted_sources_are_per_language() {
        assert!(is_wanted_source("crates/x/src/lib.rs"));
        assert!(!is_wanted_source("crates/x/src/bin/main.rs"));
        assert!(is_wanted_source("pkg/store.py"));
        assert!(is_wanted_source("setup.py"));
        assert!(!is_wanted_source("tests/test_store.py"));
        assert!(!is_wanted_source("pkg/tests/helpers.py"));
        assert!(!is_wanted_source("pkg/store_test.py"));
        assert!(!is_wanted_source("test_root.py"));
        assert!(!is_wanted_source("pkg/conftest.py"));
        assert!(!is_wanted_source("README.md"));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p phronesis-mcp --lib -- coverage::region_map::tests::python_function_sites --exact`; `cargo test -p phronesis-mcp --lib -- coverage::collect::tests::wanted_sources_are_per_language --exact`.
Expected: FAIL (`python_function_sites` and `body_start_line` do not exist; `pkg/store.py` rejected).

- [ ] **Step 3: Write minimal implementation**

`region_map.rs`: add `pub body_start_line: u64` to `FunctionSite` and set it in `function_sites_by_node` from the Rust `function_item`'s `body` child (`node.child_by_field_name("body").map(|b| b.start_position().row as u64 + 1).unwrap_or(start_line)`). Move the ordinal-suffix loop at `:432-458` into:

```rust
/// A function site before ordinals: item path without suffix plus its lines.
struct RawSite { path: String, start_line: u64, body_start_line: u64, end_line: u64 }

/// Duplicate item paths get `.2`, `.3`, ... on the last segment in source
/// order (SPEC-coverage-evidence §3.2); the first keeps its bare path.
fn assign_function_ordinals(raw: Vec<RawSite>) -> Vec<FunctionSite> {
    let mut seen: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    raw.into_iter()
        .map(|r| {
            let n = seen.entry(r.path.clone()).and_modify(|c| *c += 1).or_insert(1);
            let item_path = if *n > 1 { format!("{}.{n}", r.path) } else { r.path };
            FunctionSite { item_path, start_line: r.start_line, body_start_line: r.body_start_line, end_line: r.end_line }
        })
        .collect()
}
```

and have the Rust path call it (behaviour unchanged; the existing Rust region tests pin it). Then:

```rust
pub fn extract_function_sites_for(rel_path: &str, source: &str) -> Result<Vec<FunctionSite>> {
    match Path::new(rel_path).extension().and_then(|e| e.to_str()) {
        Some("rs") => extract_function_sites(source),
        Some("py") => python_function_sites(source),
        _ => Ok(Vec::new()),
    }
}

/// Inclusive 1-based end line of `node`: a node whose end sits at column 0
/// of a row ends on the previous row.
fn inclusive_end_line(node: tree_sitter::Node) -> u64 {
    let end = node.end_position();
    let row = end.row as u64 + 1;
    if end.column == 0 { row.saturating_sub(1).max(node.start_position().row as u64 + 1) } else { row }
}

pub fn python_function_sites(source: &str) -> Result<Vec<FunctionSite>> {
    let parsed = crate::syntax::parsed::ParsedFile::parse_python(source)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter failed to parse Python source"))?;
    let crate::syntax::parsed::ParsedFile::Python { tree, source: text } = &parsed else {
        anyhow::bail!("parse_python returned a non-Python tree");
    };
    let bytes = text.as_bytes();
    fn name_of(node: tree_sitter::Node, src: &[u8]) -> Option<String> {
        node.child_by_field_name("name").and_then(|n| n.utf8_text(src).ok()).map(str::to_string)
    }
    fn walk(node: tree_sitter::Node, src: &[u8], scope: &mut Vec<String>, out: &mut Vec<RawSite>) {
        let kind = node.kind();
        let (is_fn, is_class) = (kind == "function_definition", kind == "class_definition");
        let mut pushed = false;
        if (is_fn || is_class) && let Some(name) = name_of(node, src) {
            scope.push(name);
            pushed = true;
            if is_fn {
                let start_line = node.start_position().row as u64 + 1;
                let body_start_line = node
                    .child_by_field_name("body")
                    .map(|b| b.start_position().row as u64 + 1)
                    .unwrap_or(start_line);
                out.push(RawSite { path: scope.join("::"), start_line, body_start_line, end_line: inclusive_end_line(node) });
            }
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            walk(child, src, scope, out);
        }
        if pushed {
            scope.pop();
        }
    }
    let mut raw = Vec::new();
    walk(tree.root_node(), bytes, &mut Vec::new(), &mut raw);
    Ok(assign_function_ordinals(raw))
}
```

(Match `ParsedFile::Python`'s field names to `syntax/parsed.rs:51-60`.) `graph/python.rs`: make the classifier at `:18-32` `pub(crate) fn classify_python_file`. `collect.rs`:

```rust
/// Which source files a record may target, per language. Test code, benches
/// and build scripts are not production evidence.
pub(super) fn is_wanted_source(rel: &str) -> bool {
    if rel.ends_with(".rs") {
        return rel.contains("/src/") && !rel.contains("/src/bin/") && !rel.ends_with("build.rs");
    }
    if rel.ends_with(".py") {
        let name = rel.rsplit('/').next().unwrap_or(rel);
        return name != "conftest.py" && crate::graph::python::classify_python_file(rel) != "test";
    }
    false
}
```

At `collect.rs:215/228` call `extract_function_sites_for(rel, &src)`; branch sites remain Rust-only behind `rel.ends_with(".rs")`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p phronesis-mcp --lib -- coverage` and `cargo test -p phronesis-mcp --lib -- graph::python` → all pass; `cargo clippy --workspace --all-targets -- -D warnings` → exit 0.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/coverage crates/phronesis-mcp/src/graph/python.rs
git commit -m "feat(coverage): per-language region sites with body ranges; Python function regions"
```

### Task F2: `coverage import --format lcov-dir` maps per-test lcov files to body-line hits

**Files:**
- Create: `crates/phronesis-mcp/src/coverage/lcov.rs`
- Create: `crates/phronesis-mcp/tests/fixtures/lcov/python-store/` (committed fixture: `pkg/store.py`, `tests/test_store.py`, `cov/python__pkg__tests__test_store__test_load.lcov`, `cov/manifest.json`, and a `README.md` naming the coverage.py version that produced the lcov and the exact commands)
- Modify: `crates/phronesis-mcp/src/coverage/mod.rs` (`pub mod lcov;`), `crates/phronesis-mcp/src/coverage/import.rs` (factor `import_records(root, records, now) -> Result<ImportSummary>` out of `import_export`; both keep all-or-nothing and single-tool/revision validation)
- Modify: `crates/phronesis-mcp/src/main.rs` (`coverage import`: `--format <jsonl|lcov-dir>` default `jsonl`; `--tool <name>` required for `lcov-dir`; `--allow-dirty` as `collect` has)
- Test: `crates/phronesis-mcp/tests/coverage_lcov_import.rs` (create); `lcov.rs` unit tests

**Interfaces:**
- `pub fn parse_lcov(text: &str) -> Result<LcovFile>`; `pub struct LcovFile { pub test_name: Option<String>, pub files: Vec<LcovSource> }`; `pub struct LcovSource { pub path: String, pub function_hits: Vec<(String, u64)>, pub line_hits: Vec<(u64, u64)> }`. Both vectors are consumed by the hit rule, so neither is dead.
- `pub fn relativize(root: &Path, sf: &str) -> Relativized` with `pub enum Relativized { Path(String), Missing, Ambiguous(Vec<String>) }`: relative and existing → `Path`; absolute under root → `Path` (stripped); else the **longest** existing suffix; two candidates at the same length or any second candidate → `Ambiguous`. Paths containing `..` after normalization are `Missing`.
- `pub struct Manifest { pub revision: String, pub files: BTreeMap<String, String> }` read from `<dir>/manifest.json` (rel path → sha256 hex, as the collection script writes). Import refuses when the manifest is absent (unless `--no-manifest`, which downgrades the record's `tool` to `<tool>+unverified` and prints a warning), when its revision differs from the host HEAD, or when any covered file's host digest differs.
- `pub fn hit_sites<'a>(sites: &'a [FunctionSite], src: &LcovSource) -> (Vec<&'a FunctionSite>, Vec<&'a FunctionSite>)` → `(hit, unattributable)`: a site is hit when some `DA` line with count > 0 satisfies `body_start_line <= line <= end_line`, except one-line bodies (`body_start_line == start_line`), which are hit iff `FNDA` for the site's `name()` has count > 0 and are `unattributable` when no `FNDA` record exists.
- `pub fn records_from_lcov_dir(root: &Path, dir: &Path, tool: &str, revision: &str) -> Result<(Vec<HitRecord>, LcovDirSummary)>`; `LcovDirSummary { files, tests, records, unresolved_sf: Vec<String>, ambiguous_sf: Vec<String>, filtered_sf: Vec<String>, no_regions: Vec<String>, unattributable: Vec<String> }`. Sites per file are memoized. A `HitRecord` is `{ v: COVERAGE_FORMAT, kind: "hit", test, region: site.region_id(rel), file: rel, start_line, end_line, hit_kind: "region", revision, tool }` (the strings `collect.rs:158-168` uses). Zero records → `Err` naming the summary counts; the store is untouched.

- [ ] **Step 1: Write the failing tests**

Unit tests in `lcov.rs`:

```rust
    #[test]
    fn hits_require_an_executed_body_line_and_fnda_for_one_liners() {
        let sites = vec![
            FunctionSite { item_path: "load".into(), start_line: 1, body_start_line: 2, end_line: 2 },
            FunctionSite { item_path: "save".into(), start_line: 4, body_start_line: 5, end_line: 5 },
            FunctionSite { item_path: "one".into(), start_line: 7, body_start_line: 7, end_line: 7 },
            FunctionSite { item_path: "two".into(), start_line: 8, body_start_line: 8, end_line: 8 },
        ];
        // coverage.py marks def lines 1, 4, 7, 8 at import; only load's body ran.
        let src = LcovSource {
            path: "pkg/store.py".into(),
            function_hits: vec![("one".into(), 3)],
            line_hits: vec![(1, 1), (2, 1), (4, 1), (5, 0), (7, 1), (8, 1)],
        };
        let (hit, unattributable) = hit_sites(&sites, &src);
        assert_eq!(hit.iter().map(|s| s.item_path.as_str()).collect::<Vec<_>>(), vec!["load", "one"]);
        assert_eq!(unattributable.iter().map(|s| s.item_path.as_str()).collect::<Vec<_>>(), vec!["two"]);
    }

    #[test]
    fn relativize_prefers_the_longest_suffix_and_refuses_ambiguity() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("pkg")).expect("mkdir");
        std::fs::create_dir_all(root.join("app/pkg")).expect("mkdir");
        std::fs::write(root.join("pkg/x.py"), "").expect("w");
        std::fs::write(root.join("app/pkg/x.py"), "").expect("w");
        std::fs::write(root.join("pkg/only.py"), "").expect("w");
        assert_eq!(relativize(root, "/workspaces/app/pkg/x.py"), Relativized::Path("app/pkg/x.py".into()), "longest existing suffix wins");
        assert_eq!(relativize(root, "/elsewhere/pkg/x.py"), Relativized::Path("pkg/x.py".into()));
        assert!(matches!(relativize(root, "/w/x.py"), Relativized::Ambiguous(_)), "two files named x.py, no longer suffix decides");
        assert_eq!(relativize(root, "pkg/only.py"), Relativized::Path("pkg/only.py".into()));
        assert_eq!(relativize(root, "../pkg/only.py"), Relativized::Missing);
        assert_eq!(relativize(root, "/nope/zzz.py"), Relativized::Missing);
    }

    #[test]
    fn parse_lcov_reads_tn_sf_fn_fnda_da_and_end_of_record() {
        let f = parse_lcov("TN:python:pkg::tests::test_store::test_load\nSF:/w/pkg/store.py\nFN:1,load\nFNDA:1,load\nDA:1,1\nDA:2,1\nend_of_record\nSF:/w/pkg/other.py\nDA:3,0\nend_of_record\n").expect("parses");
        assert_eq!(f.test_name.as_deref(), Some("python:pkg::tests::test_store::test_load"));
        assert_eq!(f.files.len(), 2);
        assert_eq!(f.files[0].function_hits, vec![("load".to_string(), 1)]);
        assert_eq!(f.files[0].line_hits, vec![(1, 1), (2, 1)]);
    }
```

Integration test `crates/phronesis-mcp/tests/coverage_lcov_import.rs` using the committed fixture (copy the fixture dir into a temp git repo, commit it so the manifest's revision can be set to that HEAD by the test, write `manifest.json` with the test-computed digests, then run the CLI):

```rust
#[test]
fn per_test_lcov_files_import_as_python_body_hits_through_the_store() {
    let fx = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lcov/python-store");
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    copy_dir(&fx, root); // helper: recursive copy of pkg/, tests/, cov/
    std::fs::create_dir_all(root.join(".phronesis")).expect("mkdir");
    std::fs::write(root.join(".phronesis/rules.json"), r#"{"rules": []}"#).expect("rules");
    git(root, &["init", "-q"]); git(root, &["add", "."]); git(root, &["commit", "-q", "-m", "fixture"]);
    let head = String::from_utf8(std::process::Command::new("git").args(["rev-parse", "HEAD"]).current_dir(root).output().unwrap().stdout).unwrap().trim().to_string();
    let digest = sha256_hex(&std::fs::read(root.join("pkg/store.py")).unwrap());
    std::fs::write(root.join("cov/manifest.json"), format!(r#"{{"revision":"{head}","files":{{"pkg/store.py":"{digest}"}}}}"#)).unwrap();

    let out = phr(root, &["coverage", "import", "--format", "lcov-dir", "--tool", "coverage.py", "cov"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let store = std::fs::read_to_string(root.join(".phronesis/coverage.jsonl")).expect("store at .phronesis/coverage.jsonl");
    assert!(store.contains(r#""region":"fn:pkg/store.py::load""#), "{store}");
    assert!(!store.contains("::save\""), "save's body never ran; its def line was marked at import: {store}");
    assert!(store.contains(r#""test":"python:pkg::tests::test_store::test_load""#), "{store}");
    assert!(store.contains(r#""hit_kind":"region""#), "{store}");

    // A manifest revision mismatch refuses and leaves the store untouched.
    let before = store.clone();
    std::fs::write(root.join("cov/manifest.json"), r#"{"revision":"0000000000000000000000000000000000000000","files":{}}"#).unwrap();
    let out = phr(root, &["coverage", "import", "--format", "lcov-dir", "--tool", "coverage.py", "cov"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("revision"), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(std::fs::read_to_string(root.join(".phronesis/coverage.jsonl")).unwrap(), before);
}
```

`copy_dir`, `git`, `phr`, `sha256_hex` are small helpers in the test file (`sha2` is already a workspace dependency if `properties::execute::artifact_sha256` uses it; otherwise call `phr-mcp`'s own hashing through a tiny `--print-digest` is not available, so compute with `sha2` behind `[dev-dependencies]`). The fixture lcov must be produced by a real `coverage lcov` run (coverage.py ≥ 7.0 emits `TN`, `SF`, `FN`, `FNDA`, `DA`, `BRDA` when branch coverage is on, and absolute `SF` paths); commit the raw file and the README with the version and commands.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p phronesis-mcp --lib -- coverage::lcov` (module missing) and `cargo test -p phronesis-mcp --test coverage_lcov_import` (clap rejects `--format`).

- [ ] **Step 3: Implement**

`coverage/lcov.rs` with `parse_lcov`, `Relativized`, `relativize` (collect **all** suffixes that exist as files under root, pick the longest, `Ambiguous` when more than one candidate shares the maximum length or when a shorter candidate also exists **and** no longer one is unique; normalize with `Path::components` and refuse `..`), `Manifest::read(dir)`, `hit_sites`, and:

```rust
pub fn records_from_lcov_dir(root: &Path, dir: &Path, tool: &str, revision: &str) -> Result<(Vec<HitRecord>, LcovDirSummary)> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)?.flatten().map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "lcov" || e == "info")).collect();
    paths.sort();
    let mut sites_cache: BTreeMap<String, Vec<FunctionSite>> = BTreeMap::new();
    let mut summary = LcovDirSummary { files: paths.len(), ..Default::default() };
    let mut tests = BTreeSet::new();
    let mut records = Vec::new();
    for path in &paths {
        let lcov = parse_lcov(&std::fs::read_to_string(path)?).with_context(|| format!("parsing {}", path.display()))?;
        let test = lcov.test_name.clone().with_context(|| format!("{}: no TN: record; the collection script always writes one", path.display()))?;
        tests.insert(test.clone());
        for src in &lcov.files {
            let rel = match relativize(root, &src.path) {
                Relativized::Path(p) => p,
                Relativized::Missing => { summary.unresolved_sf.push(src.path.clone()); continue }
                Relativized::Ambiguous(c) => { summary.ambiguous_sf.push(format!("{} -> {}", src.path, c.join(" | "))); continue }
            };
            if !super::collect::is_wanted_source(&rel) { summary.filtered_sf.push(rel); continue }
            if !sites_cache.contains_key(&rel) {
                let source = std::fs::read_to_string(root.join(&rel))?;
                sites_cache.insert(rel.clone(), extract_function_sites_for(&rel, &source)?);
            }
            let sites = &sites_cache[&rel];
            if sites.is_empty() { summary.no_regions.push(rel.clone()); continue }
            let (hit, unattributable) = hit_sites(sites, src);
            summary.unattributable.extend(unattributable.iter().map(|s| s.region_id(&rel)));
            for site in hit {
                records.push(HitRecord { v: COVERAGE_FORMAT, kind: "hit".into(), test: test.clone(), region: site.region_id(&rel), file: rel.clone(),
                    start_line: site.start_line, end_line: site.end_line, hit_kind: "region".into(), revision: revision.to_string(), tool: tool.to_string() });
            }
        }
    }
    summary.tests = tests.len();
    summary.records = records.len();
    for v in [&mut summary.unresolved_sf, &mut summary.ambiguous_sf, &mut summary.filtered_sf, &mut summary.no_regions, &mut summary.unattributable] { v.sort(); v.dedup(); }
    if records.is_empty() {
        anyhow::bail!("no coverage records could be built from {}: {} lcov file(s), {} unresolved SF, {} ambiguous, {} filtered as test code, {} without regions; the store was not changed",
            dir.display(), summary.files, summary.unresolved_sf.len(), summary.ambiguous_sf.len(), summary.filtered_sf.len(), summary.no_regions.len());
    }
    Ok((records, summary))
}
```

`main.rs`: for `lcov-dir`, read the manifest (refuse on absence unless `--no-manifest`), compare its `revision` with host HEAD and each digest with the host file (`sha2`, the same helper `properties::execute::artifact_sha256` uses), then stamp `revision`, call `records_from_lcov_dir`, then `import_records`; print the summary, each non-empty list as `<label>: <n> (<first 5>)`. Existing `import_export` dedupes exact duplicate records already (`import.rs:82`), so re-imports of the same run collapse; a re-import at a new revision replaces the store as today.

- [ ] **Step 4: Verify**

Run each on its own: `cargo test -p phronesis-mcp --lib -- coverage`, `cargo test -p phronesis-mcp --test coverage_lcov_import`, `cargo test -p phronesis-mcp --test bdd` (F5 adds the Python scenario; this run confirms nothing existing broke), `cargo clippy --workspace --all-targets -- -D warnings`.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/coverage crates/phronesis-mcp/src/main.rs crates/phronesis-mcp/tests/coverage_lcov_import.rs crates/phronesis-mcp/tests/fixtures/lcov
git commit -m "feat(coverage): import per-test lcov directories with body-line hits and a container manifest"
```

### Task F3: pytest collection loop (container-friendly) and runnable Python tests in `select`

**Files:**
- Create: `crates/phronesis-mcp/src/coverage/pytest.rs`
- Create: `crates/phronesis-mcp/tests/fixtures/pytest/collect-only.txt` (a real `python -m pytest --collect-only -q` output including a parametrized id, a class-qualified id, warning lines, and the `N tests collected in …` trailer; README with the pytest version)
- Modify: `crates/phronesis-mcp/src/main.rs` (`coverage collect --tool pytest-cov [--emit-script] [--out <dir>]`), `crates/phronesis-mcp/src/coverage/select.rs` (`python:` ids render to `python -m pytest <file>::<name>` in the human table and as `command` in `--json`, from the `defines_test` edge's file path)
- Modify: `crates/phronesis-mcp/src/graph/python.rs` (expose `pub(crate) fn qualified_test_id(namespace: &str, file_path: &str, name_segments: &[&str]) -> String` built from the same `qualify` the emitter uses at `:144-164`)
- Test: `pytest.rs` tests; `select.rs` tests

**Interfaces:**
- `pub fn parse_collect_only(output: &str) -> Vec<String>`: node ids only (`path::name`, `path::Class::name`, `path::name[param]`), skipping blank lines, warnings and the trailer.
- `pub fn graph_test_id(namespace: &str, node_id: &str) -> Option<String>`: `tests/test_store.py::TestSave::test_it[a-b]` → `python:<ns>::tests::test_store::TestSave::test_it` (the parameter suffix is dropped: the graph indexes the function; a parametrized case is one execution of it, and its lcov file carries the full node id in a second `TN:`-adjacent comment line `# node: <node id>` for provenance). Returns `None` when the last segment does not start with `test_` (the graph would not have emitted it) and the collector reports it.
- `pub fn file_stem_for(graph_id: &str) -> String`: replaces every character outside `[A-Za-z0-9._-]` with `_` (so `python:pkg::tests::test_store::test_load` → `python_pkg__tests__test_store__test_load`); a counter suffix disambiguates collisions within one run.
- `pub fn collection_script(entries: &[(String, String)], out_dir: &Path) -> String` (pairs of node id and graph id): POSIX sh, `set -eu`, checks `python3 -m coverage --version` and `python3 -m pytest --version` up front, then per entry `n=$((n+1))`, `python3 -m coverage run --data-file="$OUT/$n.cov" -m pytest -q -- '<node id shell-quoted>'` (`'` inside ids escaped as `'\''`), `python3 -m coverage lcov --data-file="$OUT/$n.cov" -o "$OUT/<stem>.lcov"`, then prepends `TN:<graph id>` (coverage.py writes `TN:` empty) with `printf '%s\n' 'TN:<graph id>' '# node: <node id>' | cat - file > tmp && mv`, removes `$n.cov`; at the end writes `manifest.json` with `git rev-parse HEAD` and `sha256sum` (or `shasum -a 256`) of every `SF` file seen; last line echoes the host command `phr-mcp coverage import --format lcov-dir --tool coverage.py <out_dir>`.
- `coverage collect --tool pytest-cov` runs `parse_collect_only` on a live `--collect-only -q` and executes the script locally when `python3` is on PATH; `--emit-script` prints it instead (the devcontainer path). Namespace `<ns>` comes from the graph's Python unit config the emitter uses (read how `graph/python.rs:402` derives `python:pyside`).

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn collect_only_output_yields_node_ids_only() {
        let out = include_str!("../../tests/fixtures/pytest/collect-only.txt");
        let ids = parse_collect_only(out);
        assert!(ids.contains(&"tests/test_store.py::test_load".to_string()));
        assert!(ids.iter().any(|i| i.contains("::TestSave::test_it")));
        assert!(ids.iter().any(|i| i.ends_with(']')), "a parametrized id is kept whole: {ids:?}");
        assert!(ids.iter().all(|i| i.contains("::")), "no warnings or trailer lines: {ids:?}");
    }

    #[test]
    fn graph_ids_match_the_graph_and_stems_are_filesystem_safe() {
        assert_eq!(graph_test_id("pkg", "tests/test_store.py::test_load").as_deref(), Some("python:pkg::tests::test_store::test_load"));
        assert_eq!(graph_test_id("pkg", "tests/test_store.py::TestSave::test_it[a-b]").as_deref(), Some("python:pkg::tests::test_store::TestSave::test_it"));
        assert_eq!(graph_test_id("pkg", "tests/test_store.py::helper"), None, "not a test_ function: the graph has no id for it");
        assert_eq!(file_stem_for("python:pkg::tests::test_store::test_load"), "python_pkg__tests__test_store__test_load");
    }

    #[test]
    fn the_script_isolates_each_test_quotes_ids_and_writes_a_manifest() {
        let entries = vec![("tests/test_store.py::test_it['x y']".to_string(), "python:pkg::tests::test_store::test_it".to_string())];
        let s = collection_script(&entries, std::path::Path::new("/tmp/cov"));
        assert!(s.starts_with("#!/bin/sh\nset -eu\n"));
        assert!(s.contains("python3 -m coverage --version"));
        assert!(s.contains("--data-file=\"$OUT/1.cov\" -m pytest -q -- 'tests/test_store.py::test_it['\\''x y'\\'']'"), "{s}");
        assert!(s.contains("-o \"$OUT/python_pkg__tests__test_store__test_it.lcov\""), "{s}");
        assert!(s.contains("TN:python:pkg::tests::test_store::test_it"), "{s}");
        assert!(s.contains("manifest.json") && s.contains("rev-parse HEAD"), "{s}");
        assert!(s.contains("phr-mcp coverage import --format lcov-dir --tool coverage.py"), "{s}");
    }
```

In `select.rs` tests: with a graph containing `defines_test("tests/test_store.py", "python:pkg::tests::test_store::test_load")` and a stored hit for that test, `select` renders the row's command as `python -m pytest tests/test_store.py::test_load` in the table and `"command": "python -m pytest tests/test_store.py::test_load"` in `--json`; without the edge, the id is printed and the JSON `command` is `null`.

- [ ] **Step 2: Run to verify they fail**, **Step 3: implement** (tighten the module separator assertion to the graph's `::` shape confirmed at `graph/python.rs:543-550`; use the graph's own `qualify`), **Step 4: verify** with `cargo test -p phronesis-mcp --lib -- coverage` and `cargo clippy --workspace --all-targets -- -D warnings`; live `pytest` runs are behind `if which("python3").is_none() { eprintln!("skipping: python3 not on PATH"); return; }`. **Step 5: commit** `feat(coverage): pytest per-test collection script with manifest; runnable Python tests in select`.

### Task F4: Changed regions for `.py` edits at hook time

**Files:**
- Modify: `crates/phronesis-mcp/src/coverage/region_map.rs::changed_regions` (called by `coverage/hydrate.rs:163-185` and `properties/hydrate.rs:68-80`): take the file path and dispatch through `extract_function_sites_for` instead of the Rust extractor, so both consumers get Python regions.
- Test: `crates/phronesis-mcp/tests/hook_integration.rs` (append), mirroring the existing Rust changed-region test in that file (grep `changed_region` there for the payload builder): a `post-check` payload editing `pkg/store.py`'s `load` body in a governed temp project with the coverage rules yields `changed_region` `fn:pkg/store.py::load` and `changed_function`; and a property test in `properties/hydrate.rs` tests showing a property whose `depends_on` names `fn:pkg/store.py::load` gets its obligation when that Python function changes.

- [ ] **Steps:** write both tests, run them to see them fail (no regions for `.py`), change `changed_regions` to dispatch on the path, run green, commit `feat(coverage): changed regions for Python edits reach the hook and property obligations`.

### Task F5: BDD scenario, docs, spec amendment, CHANGELOG

**Files:**
- Modify: `crates/phronesis-mcp/tests/features/coverage-evidence.feature` (one Python scenario: import the F2 fixture's lcov dir, then `coverage select` for an edit to `pkg/store.py::load` lists `python:pkg::tests::test_store::test_load` under `coverage_observation` and renders the pytest command) with its steps in `tests/bdd/coverage_steps.rs`
- Modify: `docs/specs/SPEC-coverage-evidence.md` ("Languages" subsection: per-language region sites; body-line hit rule and why; lcov as interchange with its limits; manifest cross-check; test-id shape; Python branch regions deferred), `crates/phronesis-mcp/CLAUDE.md`, `AGENTS.md` (coverage workflow incl. the devcontainer flow), `CHANGELOG.md` → `### Added`:

```markdown
- **Coverage evidence for Python.** The region map produces function
  regions for `.py` files; `phr-mcp coverage import --format lcov-dir --tool
  coverage.py <dir>` imports one lcov file per test (coverage.py, or any lcov
  producer) after checking the collection manifest's revision and file
  digests against the host tree; `coverage collect --tool pytest-cov
  [--emit-script]` prints or runs the per-test collection loop, so a
  devcontainer can produce the files and the host can import them; and
  `coverage select` renders Python tests as `python -m pytest <file>::<name>`.
  A function counts as hit only when one of its body lines executed, because
  coverage.py marks `def` lines at import time. Unresolved, ambiguous and
  filtered paths, files without regions, and one-line functions without
  `FNDA` data are reported, never treated as covered. Python branch regions
  are not yet produced. CI runs no Python; parsing and import are tested from
  committed fixtures.
```

- [ ] **Steps:** write the scenario and steps, run `cargo test -p phronesis-mcp --test bdd` (strict runner: undefined steps fail), write the docs, run `cargo test -p phronesis-mcp --test cli_smoke`, commit `docs(coverage): Python coverage workflow, lcov import, and BDD scenario`.

---

## Self-review notes (revision 2)

- **Coverage:** A (blind spot) → A1–A4; B (commits 0) → B1–B3; C (empty confidence) → C1–C3; D (stale properties in worktrees) → D1; E (Kani outside the seam) → E1–E3; F (Rust-only coverage, user report) → F1–F5, added after the first external review and then reviewed by the same two reviewers (Part F revision 2). Kani convergence (ICE, unbounded CBMC) is not in this plan; it belongs to the deferred investigation on the preserved patches.
- **Reviewer findings folded in:** GLM C1–C14 and nits; Codex Critical ×3, Major ×9, Minor ×4. Rejected: Codex A1's `.gitignore` overlap (both walks honour `.gitignore`, so the difference is `.phronesisignore` policy only; pinned by a test anyway) and Codex E3's doubt that `confidence` surfaces `proof` (it does: `derive.rs:228` and the band-lift test at `:526-534`). GLM C3's premise (init.rs still present) is stale but its fold is adopted.
- **Type consistency:** `Discovery` (A1) ↔ A2; `command_repo_dir` (B1) ↔ `probe_root_for` (B2) ↔ `Inflight.probe_root` (B2); `shell_word`/`stdout_redirect_target`/`tee_target` (C1) ↔ `redirect_target_for`/`redirect_output` (C2); `ExtractFromInput.not_before` (C2) ↔ `record_from_output` (C3); `section_start` (E1) ↔ `KANI_DEF` (E2); `proof_unbound` (E2) ↔ docs (E3).
- **Points to confirm against the code while executing:** `AuditOpts`' full field list (A2); the name of the runtime file-size cap accessor (A2); how `command_heads` normalizes segments (B1, C2); the hook tail's order of lifecycle pop versus `outcomes_for_journal` (C2); `read_file_capped`'s own file-type check (C2); the `fixture` helper's field names in `govern_worktree.rs` (D1); the def-id field path on `CompiledDef` and `registry`'s item struct (E1, E2); the exact fixture code to mirror for E2's third test (`derive.rs:565-600`).
