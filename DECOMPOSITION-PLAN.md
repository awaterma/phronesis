# Decomposition Design for `crates/phronesis-mcp/src/audit.rs`

## Module Map

| New module file | Contents |
|-----------------|----------|
| `crates/phronesis-mcp/src/audit/mod.rs` | Module declarations, re-exports, `use` statements |
| `crates/phronesis-mcp/src/audit/types.rs` | Public data types and small helpers |
| `crates/phronesis-mcp/src/audit/engine.rs` | Core rule evaluation and scan logic |
| `crates/phronesis-mcp/src/audit/run.rs` | `run_core`, `run`, `run_profiled`, `discover_files`, `AuditSectionTimes` |
| `crates/phronesis-mcp/src/audit/diagnostics.rs` | `empty_result_diagnostic`, `near_miss_rule_ids` |
| `crates/phronesis-mcp/src/audit/trend.rs` | Trend types and `compute_trend` |
| `crates/phronesis-mcp/src/audit/render.rs` | Table/JSON renderers and date helpers |
| `crates/phronesis-mcp/src/audit/graph.rs` | Structural graph merge helpers |
| `crates/phronesis-mcp/src/audit/tests/engine_tests.rs` | Tests for engine, run, diagnostics, types |
| `crates/phronesis-mcp/src/audit/tests/trend_tests.rs` | Tests for trend computation |
| `crates/phronesis-mcp/src/audit/tests/render_tests.rs` | Tests for renderers |
| `crates/phronesis-mcp/src/audit/tests/graph_tests.rs` | Tests for graph merge |

All production modules are under 800 LOC. Test modules are also under 800 LOC each.

---

## Per-Module Inventory

### `types.rs` (approx. lines 1–120 of original)

| Item | Original line range (approx.) | New visibility |
|------|-------------------------------|----------------|
| `use std::collections::{BTreeMap, HashMap};` | 1–2 | `pub(crate)` |
| `use std::path::{Path, PathBuf};` | 3 | `pub(crate)` |
| `use std::time::Instant;` | 4 | `pub(crate)` |
| `use crate::rules_file::{DiskRule, RulesFile};` | 5 | `pub(crate)` |
| `use crate::syntax;` | 6 | `pub(crate)` |
| `use phr::{Fact, RuleId};` | 7 | `pub(crate)` |
| `pub enum Level` | 11–14 | `pub` |
| `impl Level` (from_action_type, as_str) | 16–30 | `pub` |
| `pub struct AuditOpts` | 32–36 | `pub` |
| `pub fn resolve_scan_root` | 38–52 | `pub` |
| `pub fn audit_snapshot_entry` | 54–78 | `pub` |
| `pub struct AuditReport` | 80–86 | `pub` |
| `pub struct RuleAudit` | 88–93 | `pub` |
| `pub struct FileAudit` | 95–103 | `pub` |
| `struct PerFileHits` | 105–108 | `pub(crate)` |
| `impl PerFileHits` (push_line, push_detail, extend_lines) | 110–120 | `pub(crate)` |

### `engine.rs` (approx. lines 121–700 of original)

| Item | Original line range (approx.) | New visibility |
|------|-------------------------------|----------------|
| `fn rule_applies_to_file` | 121–170 | `pub(crate)` |
| `fn is_ast_predicate` | 171–178 | `pub(crate)` |
| `fn rule_has_ast_predicate` | 179–184 | `pub(crate)` |
| `fn ast_hit_detail` | 185–210 | `pub(crate)` |
| `fn is_whole_file_rule` | 211–216 | `pub(crate)` |
| `fn normalized_relative_path` | 217–224 | `pub(crate)` |
| `fn audit_path_facts` | 225–250 | `pub(crate)` |
| `fn script_body` | 251–256 | `pub(crate)` |
| `fn validate_builtin_script` | 257–264 | `pub(crate)` |
| `pub fn script_diagnostics` | 265–280 | `pub` |
| `fn script_guards_pass` | 281–300 | `pub(crate)` |
| `fn file_exempts_rule` | 301–320 | `pub(crate)` |
| `fn line_preceded_by_doc_comment` | 321–330 | `pub(crate)` |
| `pub fn rule_matches_filter` | 331–345 | `pub` |
| `fn filter_audit_rules` | 346–352 | `pub(crate)` |
| `fn eval_ast_rule` | 353–390 | `pub(crate)` |
| `struct ContentEvalCtx` | 391–396 | `pub(crate)` |
| `fn eval_content_rule` | 397–415 | `pub(crate)` |
| `struct EvalCtx` | 416–425 | `pub(crate)` |
| `fn evaluate_rule_for_file` | 426–490 | `pub(crate)` |
| `struct ScanFileInput` | 491–498 | `pub(crate)` |
| `fn scan_file_into_accum` | 499–540 | `pub(crate)` |
| `fn build_per_rule` | 541–570 | `pub(crate)` |
| `fn rule_report_order` | 571–580 | `pub(crate)` |

### `run.rs` (approx. lines 581–700 of original)

| Item | Original line range (approx.) | New visibility |
|------|-------------------------------|----------------|
| `fn run_core` | 581–650 | `pub(crate)` |
| `pub struct AuditSectionTimes` | 651–670 | `pub` |
| `pub fn run` | 671–675 | `pub` |
| `pub fn run_profiled` | 676–682 | `pub` |
| `pub fn discover_files` | 683–700 | `pub` |

### `diagnostics.rs` (approx. lines 701–800 of original)

| Item | Original line range (approx.) | New visibility |
|------|-------------------------------|----------------|
| `pub fn empty_result_diagnostic` | 701–760 | `pub` |
| `fn near_miss_rule_ids` | 761–775 | `pub(crate)` |

### `trend.rs` (approx. lines 801–1000 of original)

| Item | Original line range (approx.) | New visibility |
|------|-------------------------------|----------------|
| `use crate::action_log::LogEntry;` | 801 | `pub(crate)` |
| `pub struct TrendOpts` | 803–810 | `pub` |
| `pub struct DebtTrend` | 812–820 | `pub` |
| `pub struct RuleTrend` | 822–830 | `pub` |
| `pub struct TrendPoint` | 832–836 | `pub` |
| `fn rule_trends` | 837–880 | `pub(crate)` |
| `pub fn compute_trend` | 881–940 | `pub` |

### `render.rs` (approx. lines 1001–1300 of original)

| Item | Original line range (approx.) | New visibility |
|------|-------------------------------|----------------|
| `use serde_json::json;` | 1001 | `pub(crate)` |
| `pub fn render_table` | 1002–1080 | `pub` |
| `pub fn render_json` | 1081–1130 | `pub` |
| `pub fn render_trend_table` | 1131–1200 | `pub` |
| `pub fn render_trend_json` | 1201–1240 | `pub` |
| `pub(crate) fn short_iso_date` | 1241–1250 | `pub(crate)` |
| `fn days_to_ymd` | 1251–1270 | `pub(crate)` |

### `graph.rs` (approx. lines 1301–1400 of original)

| Item | Original line range (approx.) | New visibility |
|------|-------------------------------|----------------|
| `pub fn graph_scope_prefix` | 1301–1315 | `pub` |
| `fn within_scope` | 1316–1325 | `pub(crate)` |
| `pub fn merge_graph_hits` | 1326–1370 | `pub` |

### Test modules

Original `mod tests` (approx. lines 1401–3500) is split into:

- `tests/engine_tests.rs`: contains all tests from original `mod tests` that exercise `run`, `run_profiled`, `discover_files`, `rule_matches_filter`, `script_diagnostics`, `empty_result_diagnostic`, `doc_excepted`, AST predicates, content rules, etc. (approx. 1200 lines, but split further if needed; we can keep under 800 by moving some to `types_tests.rs` if necessary). For this design, we assume it fits under 800 after removing trend/render/graph tests.
- `tests/trend_tests.rs`: contains `compute_trend_*` tests and helpers `audit_entry`.
- `tests/render_tests.rs`: contains `render_table_*`, `render_json_*`, `render_trend_*` tests and helpers `make_report`, `make_trend`.
- `tests/graph_tests.rs`: contains original `mod graph_merge_tests` content.

Each test file is under 800 LOC.

---

## Wiring Plan

### `audit/mod.rs`

```rust
//! Whole-tree audit: walk the project, run opted-in rules' predicates
//! against full file contents, report per-rule violation counts. Pure
//! functions for the eval/aggregation/render path; I/O lives in `run`.
//!
//! Split into cohesive modules; see individual files.

mod types;
mod engine;
mod run;
mod diagnostics;
mod trend;
mod render;
mod graph;

pub use types::{
    AuditOpts, AuditReport, FileAudit, Level, RuleAudit, resolve_scan_root,
    audit_snapshot_entry,
};
pub use engine::{rule_matches_filter, script_diagnostics};
pub use run::{AuditSectionTimes, discover_files, run, run_profiled};
pub use diagnostics::empty_result_diagnostic;
pub use trend::{DebtTrend, RuleTrend, TrendOpts, TrendPoint, compute_trend};
pub use render::{render_json, render_table, render_trend_json, render_trend_table};
pub use graph::{graph_scope_prefix, merge_graph_hits};

#[cfg(test)]
mod tests {
    mod engine_tests;
    mod trend_tests;
    mod render_tests;
    mod graph_tests;
}
```

### Visibility Adjustments

- All items that were `pub` remain `pub` and are re-exported from `mod.rs`.
- Items that were private but used across modules become `pub(crate)`.
- `PerFileHits` and its impl become `pub(crate)` because `engine.rs` and `graph.rs` both use it.
- `rule_report_order`, `build_per_rule`, `filter_audit_rules`, etc. become `pub(crate)`.
- `short_iso_date` remains `pub(crate)`.
- `days_to_ymd` becomes `pub(crate)` (used only in `render.rs`).
- `within_scope` becomes `pub(crate)` (used only in `graph.rs`).
- `near_miss_rule_ids` becomes `pub(crate)` (used only in `diagnostics.rs`).

### Import Adjustments

- `types.rs` imports `BTreeMap`, `HashMap`, `Path`, `PathBuf`, `Instant`, `DiskRule`, `RulesFile`, `syntax`, `Fact`, `RuleId`.
- `engine.rs` imports from `types` (`Level`, `PerFileHits`, `AuditOpts`, `AuditReport`, `RuleAudit`, `FileAudit`), `crate::rules_file`, `crate::syntax`, `phr`, `std::collections`, `std::path`, `std::time`.
- `run.rs` imports from `types`, `engine`, `std::path`, `std::time`, `ignore`.
- `diagnostics.rs` imports from `types`, `engine`, `std::path`.
- `trend.rs` imports from `types`, `crate::action_log::LogEntry`, `std::collections::BTreeMap`.
- `render.rs` imports from `types`, `trend`, `serde_json::json`.
- `graph.rs` imports from `types`, `engine`, `crate::graph::audit::GraphHit`, `std::collections::BTreeMap`, `std::path::PathBuf`.

No logic changes; only `use` paths and visibility modifiers are adjusted.

---

## Behavior Preservation Notes

- **Code moves only**: Every function body, struct definition, and impl block is copied verbatim. No edits to logic, formatting, or comments.
- **Visibility changes are additive**: Private items become `pub(crate)` only where cross-module access is required. This does not alter external API because `pub(crate)` is not exported outside the crate.
- **Re-exports preserve public API**: All previously `pub` items are re-exported from `audit/mod.rs` with the same names, so external callers (`server.rs`, `main.rs`, etc.) see no change.
- **Test modules are moved unchanged**: Test functions are copied verbatim into the new test files. The `use super::*;` inside each test module now refers to the parent `audit` module, which re-exports all needed items, so tests compile without modification.
- **No behavioral differences**: The split does not change evaluation order, sorting, error handling, or any runtime behavior. The only observable difference is file organization.

---

## Ordered Execution Steps

1. **Create directory structure**:
   ```
   mkdir -p crates/phronesis-mcp/src/audit/tests
   ```

2. **Create `audit/mod.rs`** with the module declarations and re-exports as shown above (initially empty submodule files can be placeholders).

3. **Move `types.rs`**:
   - Copy lines 1–120 of original `audit.rs` into `audit/types.rs`.
   - Adjust imports: remove `use crate::syntax;` if not needed (it is used by `audit_snapshot_entry`? No, that uses `serde_json`; keep only necessary imports).
   - Change `PerFileHits` and its impl to `pub(crate)`.
   - Remove `pub` from items that will be re-exported? No, keep `pub` and re-export.

4. **Move `engine.rs`**:
   - Copy lines 121–580 into `audit/engine.rs`.
   - Add `use crate::audit::types::{...};` for needed types.
   - Change all private functions to `pub(crate)`.
   - Ensure `rule_matches_filter` and `script_diagnostics` remain `pub`.

5. **Move `run.rs`**:
   - Copy lines 581–700 into `audit/run.rs`.
   - Add imports for `types`, `engine`, `std::path`, `std::time`, `ignore`.
   - `run_core` becomes `pub(crate)`; `run`, `run_profiled`, `discover_files`, `AuditSectionTimes` remain `pub`.

6. **Move `diagnostics.rs`**:
   - Copy lines 701–775 into `audit/diagnostics.rs`.
   - Add imports for `types`, `engine`, `std::path`.
   - `empty_result_diagnostic` remains `pub`; `near_miss_rule_ids` becomes `pub(crate)`.

7. **Move `trend.rs`**:
   - Copy lines 801–940 into `audit/trend.rs`.
   - Add imports for `types`, `crate::action_log::LogEntry`, `std::collections::BTreeMap`.
   - All public types remain `pub`; `rule_trends` becomes `pub(crate)`.

8. **Move `render.rs`**:
   - Copy lines 1001–1270 into `audit/render.rs`.
   - Add imports for `types`, `trend`, `serde_json::json`.
   - `render_*` functions remain `pub`; `short_iso_date` remains `pub(crate)`; `days_to_ymd` becomes `pub(crate)`.

9. **Move `graph.rs`**:
   - Copy lines 1301–1370 into `audit/graph.rs`.
   - Add imports for `types`, `engine`, `crate::graph::audit::GraphHit`, `std::collections::BTreeMap`, `std::path::PathBuf`.
   - `graph_scope_prefix` and `merge_graph_hits` remain `pub`; `within_scope` becomes `pub(crate)`.

10. **Move tests**:
    - Split original `mod tests` (lines 1401–3500) into four files under `audit/tests/`:
      - `engine_tests.rs`: all tests except trend, render, graph.
      - `trend_tests.rs`: `compute_trend_*` tests and `audit_entry` helper.
      - `render_tests.rs`: `render_*` tests and helpers `make_report`, `make_trend`.
      - `graph_tests.rs`: original `mod graph_merge_tests` content.
    - In each test file, change `use super::*;` to `use crate::audit::*;` (or keep `use super::*;` if the test module is nested under `audit::tests`, which re-exports everything).
    - Ensure each test file is under 800 LOC; if `engine_tests.rs` exceeds, split further (e.g., `engine_tests2.rs`).

11. **Update `audit/mod.rs`**:
    - Add `#[cfg(test)] mod tests { mod engine_tests; mod trend_tests; mod render_tests; mod graph_tests; }`.
    - Verify all re-exports are present.

12. **Delete original `audit.rs`** (or keep as empty with `include!`? No, remove it).

13. **Run `cargo check -p phronesis-mcp`** to catch missing imports or visibility errors.

14. **Run `cargo test -p phronesis-mcp audit`** to ensure all tests pass.

15. **Run `phr-mcp coverage select`** and follow the targeted testing policy as required.

---

## Risks

- **Visibility mistakes**: Forgetting to mark a cross-module item `pub(crate)` will cause compile errors. Mitigation: run `cargo check` after each module move.
- **Import cycles**: `engine.rs` and `run.rs` may need to import from each other; ensure no circular dependencies by keeping `run.rs` dependent on `engine.rs` only.
- **Test module path changes**: Tests using `super::*` may need adjustment if the nesting changes. Mitigation: use `crate::audit::*` in test files.
- **Line count underestimation**: Some modules may exceed 800 LOC if the original line ranges are off. Mitigation: after moving, run `wc -l` on each file and split further if needed.
- **Behavior drift**: Accidental edits during copy-paste. Mitigation: use `git diff` to verify only moves and visibility changes, no logic changes.