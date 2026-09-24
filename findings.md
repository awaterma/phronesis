# Dogfood Validation Findings — Coverage-Evidence Feature via Phronesis

**Worktree:** `wt/v1-dogfood` (branch from `feat/coverage-evidence`, HEAD = `ff35428`)
**Date:** 2026-09-24
**Mode:** Read-only analysis pass (no fixes applied)

---

## 1. Commands Run and Key Output

### 1.1 `phr-mcp audit`

```
phronesis: no rules configured; run `phr-mcp init` first
```

**Reading:** The worktree has no `.phronesis/rules.json`. The `.phronesis/`
directory exists with state files (context.json, journey.json, toolchains.json,
bugs.json, kernel.md, predicates/, wiki/, nudges/) but **no rules file** and
**no log.jsonl**. `phr-mcp init` was never run in this worktree, so the entire
rule pack (llm, rust, rhai, etc.) is absent. The audit command cannot fire any
rules and produces no per-rule output.

### 1.2 `phr-mcp audit --fail-on block`

```
phronesis: no rules configured; run `phr-mcp init` first
EXIT_CODE=0
```

**Reading:** Exit code 0 — **passed trivially**. With no rules configured,
there are no block-phase rules to fail on, so the exit code is vacuously
clean. This is not evidence that the coverage files are clean; it is evidence
that the governance layer is not installed in this worktree.

### 1.3 `phr-mcp confidence`

```
No open work unit. Run a build/test under the hook first, or record one with
`phr-mcp signal tests pass`.
```

**Reading:** No work unit is open (`phr-mcp unit show` → "no work unit open").
The confidence system has no subject to score. `.phronesis/confidence.json`
contains only `{"version": 1}` with no signals. No confidence band (low/
medium/high) can be computed.

### 1.4 `phr-mcp journey --lifecycle`

```
    SID             SEQ       KIND/MODE               HOST
(no lifecycle records)
```

**Reading:** No lifecycle records exist. The journey journal
(`.phronesis/journey/session` contains `s-2026-09-24-b420c4`, and
`.phronesis/journey/inflight` has 10 in-flight bash tool entries from this
analysis session) has no prior session history. The swarm commits that built
the coverage feature (bbfde6e, 7477585, a771a74, 1e5afbd, 477e66b, ff35428)
left **no phronesis lifecycle trace** — they were committed without phronesis
hooks active in this worktree.

### 1.5 `phr-mcp stats`

```
no phronesis activity recorded yet

lifecycle      counts since log entry (none)
sessions        0
prompts         0   fresh 0   mid_turn 0   correction 0
interventions   0   (mid_turn + correction)
interrupts      0
sub-agents      0   starts, 0 matched
commits         0   (shell tool calls only)
```

**Reading:** Zero activity across all dimensions. No hook has ever fired in
this worktree. The action log (`log.jsonl`) does not exist.

### 1.6 `.phronesis/log.jsonl` tail

```
ls: .phronesis/log.jsonl: No such file or directory
```

**Reading:** No action log exists. No hook decisions (blocks/warns) have ever
been recorded against any file, including the coverage files.

### 1.7 `phr-mcp drift` (supplementary)

```
claude_md  12 scanned, 12 uncovered
memory     not present (no directory)
wiki       28 scanned, 28 uncovered
code       not present (no code graph)

2 present, 2 missing, 0 errored — 40 uncovered total
```

**Reading:** Drift detection ran (it doesn't require rules.json) and reports
40 uncovered items across CLAUDE.md and wiki sources. None of these items
specifically concern the coverage feature — they are pre-existing drift
between guidance docs and rules. The "code not present" line confirms no
code graph has been built.

---

## 2. Warn/Block/Low-Confidence Signals Concerning Coverage Files

Since no rules are configured, no automated warn/block signals were produced.
The following is a **manual pattern scan** of the coverage source files against
the rule patterns defined in `.phronesis/wiki/decisions/` (the ADRs that back
the rust pack rules):

### 2.1 Production source files (`crates/phronesis-mcp/src/coverage/`)

| File | Pattern | Count | Would-Fire Rule | Severity |
|------|---------|-------|-----------------|----------|
| `import.rs:129` | `Result<String>` return | 1 | `enforce-no-result-string-error` | **block** |
| `hydrate.rs` | `.clone()` calls | 4 | `warn-clone-heavy` (3+ per fn) | warn (1 fn has 4 clones in `facts_for_event`) |
| `import.rs` | `.clone()` calls | 3 | `warn-clone-heavy` (3+ per fn) | warn (`import_export` has 3 clones) |
| `region_map.rs` | `let mut` count | 11 across file | `audit-rust-let-mut-count-high` (3+ per fn) | audit (multiple functions qualify) |
| All files | `.unwrap()` / `.expect()` / `panic!` / `todo!` / `dbg!` | 0 | — | clean |
| All files | `#![deny(warnings)]` | 0 | `block-deny-warnings-attribute` | clean |
| All files | `impl Deref` | 0 | `warn-deref-for-non-pointer-type` | clean |

**Key finding on `Result<String>`:** `import.rs:129` — `fn single_revision(records: &[HitRecord]) -> Result<String>`
uses `anyhow::Result<String>`, which is `Result<String, anyhow::Error>`. The
`enforce-no-result-string-error` rule uses the AST predicate
`function_returns_result_string` which checks for `Result<_, String>` (String
in the **error** position). This function has String in the **ok** position,
so it would **not** fire. This is a false alarm on manual scan — the rule is
narrower than grep suggests.

**Clone-heavy in `facts_for_event`** (hydrate.rs): 4 `.clone()` calls —
`sha.clone()`, `idx.revision.clone()`, `change_id.clone()` (×2 in the loop),
`region.clone()` (×2). The `warn-clone-heavy` rule fires at 3+ clones per
function. This function has 6 clone calls, well above threshold. However,
these are necessary clones for building owned `Vec<String>` args in a
loop — the block pattern (closing mutability) doesn't apply to collection
construction.

### 2.2 Test files (`crates/phronesis-mcp/tests/coverage_*.rs`)

| File | `.unwrap()` count | Notes |
|------|-------------------|-------|
| `coverage_hydrate.rs` | 11 | Standard test idioms — exempt from `no-unwrap-in-src` (tests/ dir) |
| `coverage_import.rs` | 18 | Standard test idioms — exempt |
| `coverage_region_map.rs` | 7 | Standard test idioms — exempt |
| `coverage_store.rs` | 7 | Standard test idioms — exempt |

All `.unwrap()` calls in test files are in `tests/` which is exempt from
the `enforce-no-unwrap-in-src` rule per the ADR. **No violations.**

### 2.3 Fixture (`tests/fixtures/coverage-sample/`)

| File | Pattern | Notes |
|------|---------|-------|
| `src/lib.rs` | `Result<i32, &'static str>` return | Not `Result<_, String>` — static str, not String. Clean. |
| `export.jsonl` | Data file | No rule patterns. |
| `regenerate-export.sh` | Shell script | Not scanned by rust rules. |

The fixture's `safe_divide` returns `Result<i32, &'static str>`, which is
acceptable (not `Result<_, String>`). The fixture is a standalone crate
(deliberately not a workspace member) with its own `[workspace]` in Cargo.toml.

---

## 3. Signals for Swarm Grading

### 3.1 Git log evidence

```
ff35428 refactor(coverage): review-hardening wave - expect() statics, LCS walk cleanup, let-chain clippy-clean (verified: 2593 tests green, clippy -D warnings clean)
477e66b test(coverage): hydration suite (T5 follow-up)
1e5afbd feat(coverage): safe_divide fixture with REAL per-test cargo-llvm-cov export + anchor consistency pin (T3, swarm: ollama-mistral-large-3-cloud)
0ef2e37 chore(coverage): gitignore derived coverage state + fixture artifacts
a771a74 feat(coverage): tree-sitter region map + demand-gated change-scoped hydration (T4+T5, swarm: ollama-mistral-large-3-cloud)
df5dea4 docs(coverage): evidence-graph spec decomposition (A/B/C) + implementation plan
7477585 feat(coverage): importer tests + phr-mcp coverage import CLI
bbfde6e feat(coverage): durable store + normalized importer modules (T1+T2, swarm: ollama-qwen3.5-cloud)
```

### 3.2 Phronesis governance trace

**There is no phronesis governance trace for the coverage feature work.**

- `.phronesis/log.jsonl` — does not exist (no hook ever fired)
- `phr-mcp stats` — zero sessions, zero prompts, zero commits, zero interventions
- `phr-mcp journey --lifecycle` — no lifecycle records
- `phr-mcp confidence` — no open work unit, no signals

The swarm commits (bbfde6e by `ollama-qwen3.5-cloud`, a771a74 and 1e5afbd by
`ollama-mistral-large-3-cloud`) were produced without phronesis hooks active.
The commit messages claim verification ("2593 tests green, clippy -D warnings
clean" in ff35428), but this is **unverified by phronesis** — no confidence
signal, no hook decision, no journey record corroborates it.

### 3.3 Commit message claims vs. phronesis evidence

| Commit | Claim | Phronesis Evidence |
|--------|-------|--------------------|
| `ff35428` | "2593 tests green, clippy -D warnings clean" | None — no confidence signal, no test signal recorded |
| `1e5afbd` | "anchor consistency pin" | None — no hook activity |
| `a771a74` | "swarm: ollama-mistral-large-3-cloud" | None — no lifecycle record naming this agent |
| `bbfde6e` | "swarm: ollama-qwen3.5-cloud" | None — no lifecycle record naming this agent |

**Conclusion for swarm grading:** The feature work was **not governed** by
phronesis. The commits exist in git history but left no phronesis trace. This
is expected for a worktree that was never initialized with `phr-mcp init`, but
it means the dogfood validation cannot confirm or deny the verification claims
in the commit messages using phronesis evidence alone.

---

## 4. Audit Debt the New Files Add (Per-Rule, with Counts)

Since `phr-mcp audit` could not run (no rules configured), the following is a
**manual audit** mapping the coverage files against the rules that would fire
if the rust pack were installed:

| Rule ID | Phase | Files Affected | Count | Details |
|---------|-------|---------------|-------|---------|
| `warn-clone-heavy` | pre (warn) | `hydrate.rs` | 1 fn | `facts_for_event`: 6 `.clone()` calls (threshold: 3) |
| `warn-clone-heavy` | pre (warn) | `import.rs` | 1 fn | `import_export`: 3 `.clone()` calls (at threshold) |
| `audit-rust-let-mut-count-high` | audit | `region_map.rs` | 3+ fns | `extract_branch_sites` (3), `changed_regions` (3), `lcs_table` (1), `all_nodes` (2), `find_function_name` (1) — several exceed threshold of 3 outer-scope `let mut` |
| `audit-rust-let-binding-count-high` | audit | `region_map.rs` | 2+ fns | `changed_regions` and `extract_branch_sites` have 4+ outer-scope `let` bindings |
| `enforce-no-unwrap-in-src` | pre (block) | (none) | 0 | All production coverage files are `.unwrap()`-free |
| `enforce-no-panic-in-src` | pre (block) | (none) | 0 | No `panic!`/`todo!`/`unimplemented!` in coverage source |
| `enforce-no-result-string-error` | pre (block) | (none) | 0 | `Result<String>` in import.rs is `anyhow::Result<String>` (String in ok position, not error) |
| `warn-dbg-in-src` | pre (warn) | (none) | 0 | No `dbg!()` calls |
| `block-deny-warnings-attribute` | pre (block) | (none) | 0 | No `#![deny(warnings)]` |
| `warn-deref-for-non-pointer-type` | pre (warn) | (none) | 0 | No `impl Deref` |
| `warn-empty-test` | pre (warn) | (none) | 0 | All test functions contain `assert!` or `.unwrap()` |

### Summary of debt

- **0 blocking violations** in coverage production source
- **2 advisory warnings** (clone-heavy in hydrate.rs and import.rs)
- **5+ audit-only items** (let-mut and let-binding counts in region_map.rs)
- **0 test quality issues** (all tests have assertions)

The coverage feature source code is **clean against all block-phase rules**.
The advisory and audit-phase items are minor and consistent with the patterns
seen in the rest of the codebase (clone-heavy collection builders and
multi-let-mut AST walker functions are common throughout phronesis-mcp).

---

## 5. Summary

| Dimension | Result |
|-----------|--------|
| `audit --fail-on block` | **Passed** (exit 0) — but vacuously: no rules configured |
| Rules installed | **None** — `rules.json` missing, `phr-mcp init` never run in this worktree |
| Hook activity log | **Empty** — `log.jsonl` does not exist |
| Confidence band | **N/A** — no open work unit |
| Journey/lifecycle records | **None** — zero sessions, zero prompts, zero commits |
| Swarm governance evidence | **None** — commits have no phronesis trace |
| Coverage source: block violations | **0** |
| Coverage source: warn violations | **2** (clone-heavy) |
| Coverage source: audit debt | **5+** (let-mut/let-binding counts) |
| Coverage tests: violations | **0** (unwrap in tests/ is exempt) |
| Fixture: violations | **0** |

The coverage-evidence feature code itself is well-disciplined: no panics, no
unwrap in production paths, no `Result<_, String>`, no `deny(warnings)`. The
primary finding is that **phronesis was never activated in this worktree**, so
the dogfood validation could not exercise the governance layer against the
feature. To get real dogfood evidence, the worktree needs `phr-mcp init --packs
rust,llm` followed by a re-run of audit and hook-governed edits.