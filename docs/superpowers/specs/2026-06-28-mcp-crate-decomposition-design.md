# MCP-crate decomposition — design

- **Date:** 2026-06-28
- **Status:** Approved; in implementation (docs/superpowers/plans/2026-07-03-mcp-crate-decomposition.md)
- **Branch (planned):** `fix/mcp-crate-decomposition`, off `main`
- **Pre-feature anchor:** tag `v0.16.2`
- **Baseline audit:** 59 hits total across 3 rules (39 `audit-rust-let-binding-count-high` + 18 `audit-rust-let-mut-count-high` + 2 `audit-file-loc-high`). Of these, **51 are in scope** (MCP crate: 35 let-binding + 14 let-mut + 2 file-loc) and **8 are deferred** (core engine: 4 let-binding + 4 let-mut). The trend's starting point is the full 59; the target end-state is 8 (engine only)

## Context

A `phr-mcp audit` pass surfaced 92 hits across 6 rules. A first pass
("trivial + config only") cleared 33: the `manual-err-return`,
`allow-dead-code-in-src`, and `newtype-id-string` rules went to zero;
`file-loc-high` dropped 6→2 (existing `//! phronesis-allow` markers now
honored); the two `let`-count rules were scoped to `src/`, dropping
examples/benches/tests noise (50→39, 32→18).

The remaining 59 are **production-src debt** — functions with many
outer-scope `let` bindings (or `let mut` decls) and two files over 800
LOC. They require function decomposition, which was explicitly out of
scope for the first pass. This spec covers the next pass.

The debt splits cleanly by dual-consumer risk:

- **Core engine (`phronesis` crate)** — `network.rs`
  (`assert_fact` 13, `add_rule` 9, `update_agenda` 21),
  `script_evaluator.rs` (12), `beta_network.rs`, `production.rs`. A
  downstream consumer embeds the engine and makes many internal
  accesses to its working-memory manager (`wme_manager`); refactoring
  engine internals can break that consumer. **Deferred to a separate
  spec gated on an embedded-consumer impact check.**
- **MCP crate (`phronesis-mcp`)** — safe; no dual-consumer risk. **In
  scope for this spec.**

## Goal

Eliminate *unjustified* `phronesis-mcp` audit debt. Every
`let-binding`/`let-mut` hit is either decomposed below threshold or
carries a justified `//! phronesis-allow: <rule-id> <reason>` marker;
`hook.rs` and `syntax/rust.rs` are either <800 LOC or marker-justified.

"Drive to justified," not literally zero: markers are allowed for
functions that are genuinely cohesive above threshold — a tight
exhaustive dispatch, a serializer that must stay linear — where
extraction would scatter logic that reads better in one place. Markers
are **not** a license to skip a real refactor.

## Scope

**In:** the `phronesis-mcp` crate only. Work-list (current audit, after
`src/` scoping):

| File | Function | Hits |
|---|---|---|
| `audit.rs` | `run` / `run_profiled` / `compute_trend` / `days_to_ymd` | 26 / 32 / 11 / 10 let; 5 / 6 / 3 mut |
| `main.rs` | `main` | 64 let |
| `hook.rs` | `run_pre_check` / `run_post_check` / `next_seq` / `journey_record_post` | 19 / 19 / 11 / 10 let; +file-loc |
| `syntax/rust.rs` | `extract_result_string_returns` / `collect_derives_from_attr` / `extract_public_functions` | 12 / 8 let; 6 / 4 / 5 mut; +file-loc |
| `rules_file.rs` | `deserialize` ×2 / `unfold_or` / `merge` | 16 / 11 / 8 / 10 let; 6 / 6 mut |
| `server.rs` | `fire_rules` / `extract_rules` / `save_rules` / `load_rules_file` / `get_stats` / `audit_codebase` / `extract_rules_from_markdown` | 10 / 8 / 10 / 12 / 8 / 12 let; 3 / 4 mut |
| `diff_extract.rs` | `rust_test_block_keep_mask` / `count_code_braces` | 13 let + 9 mut; 3 mut |
| `memory_drift.rs` | `parse_memory_file` / `score_entry` | 10 / 12 let; 4 mut |
| `journey/derive.rs` | `scan_script` / `record_pair` / `assert_facts` | 10 / 11 / 8 let |
| `journey/mod.rs` | `current_sid` | 8 let |
| `journey/tagger.rs` | `fire` | 8 let |
| `journey_cli.rs` | `compute` | 8 let |
| `outcomes/cargo.rs` | `extract_from` / `parse` | 9 let; 4 mut |
| `claude_md_drift.rs` | `extract_imperatives` | 8 let |
| `context.rs` | `build_turn_body` | 8 let |
| `wiki.rs` | `parse_decision_file` | 8 let |
| `server_persistence.rs` | `autoload` | 8 let |

**Out (deferred, separate embedded-consumer-gated spec):** `network.rs`,
`script_evaluator.rs`, `beta_network.rs`, `production.rs`. These are
core-engine; they stay untouched in this pass.

## Prerequisites (Phase 0)

1. **Enable marker exemption on the `let` rules.** Add `doc_excepted:
   true` to `audit-rust-let-binding-count-high` and
   `audit-rust-let-mut-count-high` in the shipped pack (`init.rs`) **and**
   the local `.phronesis/rules.json`. Without this, any `//! phronesis-allow`
   marker placed on a `let`-rule hit is dead text — the audit ignores it.
   (Mirror exactly the pattern already used by `audit-file-loc-high` and
   `audit-allow-dead-code-in-src`.)
2. **Confirm the pre-feature anchor:** tag `v0.15.0` exists on `main`.
   Branch `fix/mcp-crate-decomposition` off `main` (not off
   `fix/audit-ast-hit-detail`).
3. **Record the baseline.** The starting point is 59 hits (39 + 18 + 2),
   captured above. The `phr-mcp trend` tool diffs subsequent audit
   snapshots against this, so each commit's effect is visible.

## Decomposition decision tree

For each target function, apply the first matching branch, in order:

1. **File >800 LOC with multiple responsibilities → module split.**
   Extract cohesive groups of functions into submodule files under the
   same module path. Applies to `hook.rs` (1764 LOC: pre-check /
   post-check / journey-record / seq helpers) and `syntax/rust.rs`
   (1622 LOC: multiple independent tree-sitter extractors). Module
   splits also relocate functions out of the over-800 file.
2. **Sequential temporaries, otherwise cohesive (8–13 lets) → block
   pattern.** Scope intermediate `let`s into a block so only the final
   value is visible to the rest of the function:
   `let result = { let raw = …; let parsed = …; … };`. Lowest-risk;
   behavior-preserving by construction.
3. **Multi-step logic / dispatch arms / per-branch scan (15+ lets) →
   extract helper functions.** Pull each sub-logic into a named helper.
   The parent's `let`-count drops because the bindings move into the
   helper.
4. **Genuinely cohesive above threshold, extraction would scatter it →
   `//! phronesis-allow` marker with rationale.** Rare; requires a
   written reason in the marker.

A function may receive more than one treatment (e.g. `hook.rs`
functions get extraction *and* the file gets a module split). The tree
is applied per function; the file-LOC treatment is applied per file.

## Per-file plan (ordered, biggest-impact-first)

| # | File | Targets | Technique |
|---|---|---|---|
| 1 | `main.rs` | `main` (64) | Extract one `handle_<variant>` fn per `Command` arm; `main` becomes a thin dispatch |
| 2 | `audit.rs` | `run` (26), `run_profiled` (32), `compute_trend` (11), `days_to_ymd` (10) | Extract a shared scan core; `run_profiled` wraps it with timing (dedup the two near-duplicate bodies). Preserve the `AuditSectionTimes` boundary |
| 3 | `hook.rs` | `run_pre_check` (19), `run_post_check` (19), `next_seq` (11), `journey_record_post` (10) + >800 LOC | Extract load→evaluate→render stages; module-split the file to <800 |
| 4 | `syntax/rust.rs` | `extract_result_string_returns` (12), `collect_derives_from_attr` (8), `extract_public_functions` (5) + >800 LOC | Extract per-node-kind helpers; module-split the file to <800 |
| 5 | `rules_file.rs` | `deserialize` ×2 (16, 11), `unfold_or` (8), `merge` (10) | Extract sub-helpers for each deserialize/merge stage |
| 6 | `server.rs` | 7 fns (8–12) | Block-pattern + minor extracts |
| 7 | `diff_extract.rs` | `rust_test_block_keep_mask` (13 + 9 mut) | Collapse the 9 `let mut` counters into one state struct; extract per-state helpers |
| 8 | `memory_drift.rs` | `parse_memory_file` (10), `score_entry` (12) | Extract helpers |
| 9 | `journey/*` | `scan_script`, `record_pair`, `assert_facts`, `current_sid`, `fire`, `compute` (8–11) | Block-pattern / minor extract |
| 10 | 8-let singles | `claude_md_drift::extract_imperatives`, `context::build_turn_body`, `wiki::parse_decision_file`, `server_persistence::autoload`, `outcomes/cargo::extract_from` | Block-pattern, batched in one commit |

Ordering is by impact (total `let`-count reduced), with the file-LOC
treatments riding along in steps 3 and 4. Steps can be reordered if a
later step unblocks an earlier one.

## Verification (hybrid: targeted TDD + trend gate)

**Characterization tests** — for the complex/risky entrypoints, confirm
existing coverage locks behavior *before* refactor and add tests where
coverage is thin:

- `main` — integration tests already spawn `phr-mcp` subcommands
  (`hook_integration.rs`, BDD features); confirm the dispatch arms are
  each exercised, add a characterization test for any arm that isn't.
- `audit::run` / `run_profiled` — the `audit.rs` unit-test module
  covers rendering and the scan; extend with a test that asserts
  `run_profiled` returns both `AuditReport` and `AuditSectionTimes`
  for a fixed input (guards the dedup).
- `hook::run_pre_check` / `run_post_check` — integration tests cover
  the hook entrypoints; rely on them as the net.
- `rules_file::deserialize` / `merge`, `diff_extract::rust_test_block_keep_mask`
  — existing unit tests cover these; confirm before refactor.

The small 8–10 `let` cluster (steps 9–10) trusts the existing 517-test
suite as the net; no forced characterization test for trivial
block-pattern scoping.

**Universal trend gate** — after every commit:

- `cargo test --workspace` green; `cargo clippy --workspace --all-targets
  -- -D warnings` clean; `cargo fmt` clean.
- `phr-mcp audit` for the touched rule must not increase (expect a
  decrease); the full audit count must not increase. `phr-mcp trend`
  shows the delta.

**Refactors are behavior-preserving only.** No logic changes. If a
refactor surfaces a real bug, stop, file the bug separately, and resume
the refactor — do not fold the fix into the decomposition commit.

## Branching and commits

One branch, `fix/mcp-crate-decomposition`, off `main`. One commit per
file/cluster (the ten steps above are a reasonable commit granularity).
Each commit is independently green and trend-non-increasing, so any
single commit is a safe rollback point.

PR at the end. If smaller reviews are preferred, the steps can be split
into per-cluster PRs against the same branch.

**Push discipline:** no push without explicit human review/approval.
Weekend/holiday/after-5pm-Pacific timing is satisfied today, but the
push still waits on approval.

## Risks and mitigations

- **Engine excluded → no embedded-consumer risk this pass.** `network.rs` et al.
  are deferred. If a refactor in the MCP crate is found to touch an
  engine type's public surface, stop and re-scope.
- **Markers as skip-license** — the spec restricts markers to the
  cohesive-tight case with a written rationale. Each marker is reviewed;
  a marker without a real reason is rejected.
- **`run`/`run_profiled` dedup** — the profiling boundary
  (`AuditSectionTimes`) must survive. The characterization test above
  guards it.
- **Module splits (`hook.rs`, `syntax/rust.rs`)** — preserve `pub`
  re-exports and fix import sites; `use` paths must still resolve. Run
  the full workspace test suite after each split.
- **Scope creep** — refactors only; no logic changes; no unrelated
  cleanup folded in.

## Done-when

- Every `phronesis-mcp` `let-binding`/`let-mut` hit is either below
  threshold or carries a justified `//! phronesis-allow` marker.
- `hook.rs` and `syntax/rust.rs` are either <800 LOC or
  marker-justified.
- `cargo test --workspace` green; `clippy --all-targets -D warnings`
  clean; `cargo fmt` clean.
- `phr-mcp audit` shows 0 *unjustified* `phronesis-mcp` hits;
  `phr-mcp trend` reflects the drop from the 59-hit baseline.
- Core-engine hits (`network.rs` et al.) untouched.

## Deferred: engine decomposition (separate spec)

A follow-up spec will cover the four core-engine targets
(`network.rs::update_agenda`/`assert_fact`/`add_rule`,
`script_evaluator.rs::evaluate_facts_count_comparison`,
`beta_network.rs::process_token_from_source`,
`production.rs::execute`). It is gated on an **embedded-consumer
impact check**: a read-only audit of the downstream consumer's
internal `wme_manager` accesses against the proposed post-refactor
signatures, plus a parallel-session gate (stay out of engine
internals entirely if a sibling session is driving that consumer).
That spec is not started here.