# Audit God-File Decomposition — Split Report

**Work unit:** `issue-godsplit-audit`
**Branch:** `godsplit/audit`
**Date:** 2026-09-27

## Summary

Decomposed the monolithic `crates/phronesis-mcp/src/audit.rs` (3926 LOC) into 8 production modules and 7 test modules under `crates/phronesis-mcp/src/audit/`. No logic edits — only code moves, visibility adjustments, and import wiring.

## Modules Created

### Production modules (`crates/phronesis-mcp/src/audit/`)

| Module | LOC | Contents |
|--------|-----|----------|
| `mod.rs` | 35 | Module declarations and re-exports |
| `types.rs` | 154 | `Level`, `AuditOpts`, `AuditReport`, `RuleAudit`, `FileAudit`, `PerFileHits`, `resolve_scan_root`, `audit_snapshot_entry` |
| `engine.rs` | 612 | Core engine: rule evaluation, scan logic, AST predicates, script guards |
| `run.rs` | 172 | `run`, `run_core`, `run_profiled`, `discover_files`, `AuditSectionTimes` |
| `diagnostics.rs` | 103 | `empty_result_diagnostic`, `near_miss_rule_ids` |
| `trend.rs` | 178 | `TrendOpts`, `DebtTrend`, `RuleTrend`, `TrendPoint`, `compute_trend`, `rule_trends` |
| `render.rs` | 296 | `render_table`, `render_json`, `render_trend_table`, `render_trend_json`, `short_iso_date`, `days_to_ymd` |
| `graph.rs` | 85 | `graph_scope_prefix`, `within_scope`, `merge_graph_hits` |

### Test modules (`crates/phronesis-mcp/src/audit/tests/`)

| Module | LOC | Contents |
|--------|-----|----------|
| `engine_tests.rs` | 707 | Core engine tests: content matching, rule filters, file discovery, diagnostics, profiled run |
| `doc_excepted_tests.rs` | 337 | Doc-exemption tests: doc comment preceding, stacked attributes, whole-file rules |
| `script_tests.rs` | 204 | Builtin script guard tests: path exclusion, AST scoping, evaluation order, unsupported scripts |
| `ast_tests.rs` | 543 | AST predicate tests: Python/TS evaluation, let-binding counts, let-mut counts, mixed rules |
| `trend_tests.rs` | 146 | Trend computation tests |
| `render_tests.rs` | 296 | Table/JSON render tests |
| `graph_tests.rs` | 176 | Graph merge helper tests |

## Gate Evidence

### (1) `cargo build` clean

```
$ cargo build -p phronesis-mcp
   Compiling phronesis-mcp v0.35.0
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 29s
```

### (2) `cargo clippy` clean

```
$ cargo clippy --workspace --all-targets -- -D warnings
   Checking phronesis-mcp v0.35.0
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2m 21s
```

### (3) `audit.rs` removed; every module under 800 LOC

```
$ ls crates/phronesis-mcp/src/audit.rs
ls: crates/phronesis-mcp/src/audit.rs: No such file or directory

$ wc -l crates/phronesis-mcp/src/audit/*.rs crates/phronesis-mcp/src/audit/tests/*.rs
     103 diagnostics.rs
     612 engine.rs
      85 graph.rs
      35 mod.rs
     296 render.rs
     172 run.rs
     178 trend.rs
     154 types.rs
     543 tests/ast_tests.rs
     337 tests/doc_excepted_tests.rs
     707 tests/engine_tests.rs
     176 tests/graph_tests.rs
     296 tests/render_tests.rs
     204 tests/script_tests.rs
     146 tests/trend_tests.rs
    4044 total
```

All modules under 800 LOC.

### (4) `phr-mcp audit` — no `audit-file-loc-high` hit for any audit module

```
$ phr-mcp audit --rule audit-file-loc-high
audit-file-loc-high  warn     13     13
    .../src/capsule.rs
    .../src/codex_hook.rs
    .../src/context/render.rs
    .../src/graph/derive.rs
    .../src/graph/extract.rs
    .../src/graph/ownership/extract.rs
    .../src/graph/yaml.rs
    .../src/hook_facts.rs
    .../src/journey/derive.rs
    .../src/properties/validate.rs
    .../src/rules_file.rs
    .../src/syntax/facts.rs
    .../src/syntax/python.rs
```

No audit module file appears in the hit list. All 13 hits are pre-existing files outside `audit/`.

## Test Results

### Targeted tests

`phr-mcp coverage select` returned no tests (coverage store empty). Per policy, ran the tests of the code touched:

```
$ cargo test -p phronesis-mcp --lib -- audit
test result: ok. 93 passed; 0 failed; 0 ignored; 0 measured; 2052 filtered out
```

### Full suite

```
$ cargo test --workspace
test result: ok. 2145 passed; 0 failed (lib tests)
test result: ok. 0 failed (all integration + doctests)
```

All tests pass across the entire workspace.

## Deviations from Plan

1. **engine_tests.rs split into additional files**: The plan mentioned "we can keep under 800 by moving some to `types_tests.rs` if necessary." `engine_tests.rs` was 1768 LOC after extraction, so it was split into 4 files: `engine_tests.rs` (707), `doc_excepted_tests.rs` (337), `script_tests.rs` (204), `ast_tests.rs` (543). The `rule()` helper was made `pub(super)` so sibling test modules can import it via `use super::engine_tests::rule`.

2. **`short_iso_date` made `pub` and re-exported**: The plan had it as `pub(crate)`, but `journey/mod.rs` calls `crate::audit::short_iso_date(ts)`, which requires it to be `pub` and re-exported from `mod.rs`.

3. **`audit_path_facts` imported in script_tests**: The `builtin_script_path_facts_are_fresh_in_either_evaluation_order` test directly calls `audit_path_facts`, a `pub(crate)` function in `engine.rs`. Added `use super::super::engine::audit_path_facts` to `script_tests.rs`.

4. **`graph_tests.rs` trailing brace removed**: The original `mod graph_merge_tests { ... }` block's closing `}` was included in the extraction. Removed since the file is now a standalone module declared in `mod.rs`.

## Visibility Changes

- All private items in `engine.rs` became `pub(crate)` for cross-module access within the `audit` module
- `Level::from_action_type` changed from private to `pub(crate)`
- `rule()` test helper changed to `pub(super)` for cross-test-module access
- `short_iso_date` changed from `pub(crate)` to `pub` (external caller in `journey/mod.rs`)

## Failure Notes

None. All four gate conditions pass.