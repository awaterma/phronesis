# Decomposition Design for `crates/phronesis-mcp/src/init.rs`

## Overview

`init.rs` (5228 LOC) is split into a directory module `init/` with cohesive submodules, each under 800 LOC. The split is purely mechanical: items are moved verbatim, with only visibility and import adjustments required for cross-module access. Public API is preserved via re-exports from `init/mod.rs`.

Line ranges below are approximate, derived from the original file order. The implementing engineer should locate items by name; ranges are provided for orientation.

---

## Module Inventory

### 1. `init/mod.rs` (≈100 LOC)

**Contents:**
- `mod rule_sync;` and `use rule_sync::write_rules_file;` (original lines 1-10)
- `pub fn run(opts: InitOpts) -> Result<InitReport, InitError>` (original lines ~330-370)
- `fn canonicalize_root(p: &Path) -> Result<PathBuf, InitError>` (original lines ~380-400)
- `fn binary_on_path(name: &str) -> bool` (original lines ~402-410)
- Module declarations and re-exports (see Wiring Plan)

**Notes:** `run` calls writer functions from `writers_hooks` and `writers_scaffold`; import them with `use super::writers_hooks::*;` and `use super::writers_scaffold::*;`.

---

### 2. `init/types.rs` (≈250 LOC)

**Contents:**
- `pub enum Pack` (original lines ~30-70)
- `impl Pack` including `ALL`, `parse`, `rules`, `label` (original lines ~72-150)
- `pub const BASE_PACKS: &[Pack]` (original lines ~152-160)
- `pub fn parse_packs(s: &str) -> Result<Vec<Pack>, InitError>` (original lines ~162-200)
- `pub fn compose_packs(packs: &[Pack]) -> Value` (original lines ~202-230)
- `pub enum InitError` (original lines ~232-260)
- `pub struct InitOpts` (original lines ~262-280)
- `pub struct InitReport` (original lines ~282-290)

**Notes:** `impl Pack::rules` calls rule functions from `rules_core`, `rules_rust`, `rules_python`, `rules_other`. Add `use super::rules_core::*; use super::rules_rust::*; use super::rules_python::*; use super::rules_other::*;` at the top of `types.rs`.

---

### 3. `init/global_install.rs` (≈200 LOC)

**Contents:**
- `pub fn user_claude_config_path() -> Option<PathBuf>` (original lines ~300-310)
- `pub fn user_gemini_config_path() -> Option<PathBuf>` (original lines ~312-320)
- `struct McpTarget<'a>` (original lines ~322-330)
- `fn install_one_target(...)` (original lines ~332-380)
- `fn uninstall_one_target(...)` (original lines ~382-420)
- `pub fn install_globally(dry_run: bool) -> Result<InitReport, InitError>` (original lines ~422-440)
- `pub fn install_globally_with_home(home: &Path, dry_run: bool) -> Result<InitReport, InitError>` (original lines ~442-470)
- `pub fn uninstall_globally(dry_run: bool) -> Result<InitReport, InitError>` (original lines ~472-490)
- `pub fn uninstall_globally_with_home(home: &Path, dry_run: bool) -> Result<InitReport, InitError>` (original lines ~492-520)

**Notes:** Uses `read_json`, `write_json`, `with_extension`, `ensure_parent` from `json_helpers`. Add `use super::json_helpers::*;` and `use super::types::*;`.

---

### 4. `init/writers_hooks.rs` (≈300 LOC)

**Contents:**
- `fn write_settings(...)` (original lines ~530-600)
- `fn write_mcp_json(...)` (original lines ~602-640)
- `fn write_gemini_settings(...)` (original lines ~642-720)
- `fn write_codex_hooks(...)` (original lines ~722-780)
- `fn write_codex_config(...)` (original lines ~782-830)

**Notes:** Uses `read_json`, `write_json`, `ensure_parent`, `with_extension`, `upsert_hook`, `upsert_hook_by_command`, `upsert_codex_hook` from `json_helpers`. Add `use super::json_helpers::*;` and `use super::types::*;`.

---

### 5. `init/writers_scaffold.rs` (≈700 LOC)

**Contents:**
- `pub(crate) const DEFAULT_DURABLE_MD: &str` (original lines ~840-900)
- `const DEFAULT_CONTEXT_KERNEL: &str` (original lines ~902-920)
- `const CONTEXT_NUDGES_README: &str` (original lines ~922-1020)
- `const WIKI_DECISIONS_README: &str` (original lines ~1022-1050)
- `const CONFIDENCE_JSON: &str` (original lines ~1052-1060)
- `const CONFIDENCE_BUGS_JSON: &str` (original lines ~1062-1070)
- `const TOOLCHAINS_JSON: &str` (original lines ~1072-1110)
- `const JOURNEY_JSON: &str` (original lines ~1112-1130)
- `fn write_durable_md(...)` (original lines ~1132-1180)
- `fn write_context_scaffold(...)` (original lines ~1182-1260)
- `fn write_wiki_scaffold(...)` (original lines ~1262-1300)
- `fn write_confidence_scaffold(...)` (original lines ~1302-1350)
- `fn write_journey_scaffold(...)` (original lines ~1352-1400)
- `fn build_structural_graph(...)` (original lines ~1402-1440)
- `fn update_gitignore(...)` (original lines ~1442-1560)

**Notes:** Uses `crate::durable_migrate`, `crate::context::config`, `crate::graph::sync`, `crate::rules_file` (absolute paths, no change). Uses `read_json`, `write_json`, `ensure_parent`, `with_extension` from `json_helpers`. Add `use super::json_helpers::*;` and `use super::types::*;`.

---

### 6. `init/json_helpers.rs` (≈200 LOC)

**Contents:**
- `fn read_json(path: &Path) -> Result<Option<Value>, InitError>` (original lines ~1570-1600)
- `fn write_json(...)` (original lines ~1602-1640)
- `fn with_extension(path: &Path, ext: &str) -> PathBuf` (original lines ~1642-1650)
- `fn ensure_parent(path: &Path) -> Result<(), InitError>` (original lines ~1652-1660)
- `fn upsert_hook(settings: &mut Value, event: &str, new_entry: Value)` (original lines ~1662-1690)
- `const PHRONESIS_HOOK_SUBCOMMANDS: [&str; 5]` (original lines ~1692-1700)
- `fn is_phronesis_hook_command(command: &str) -> bool` (original lines ~1702-1730)
- `fn upsert_hook_by_command(...)` (original lines ~1732-1770)
- `fn upsert_codex_hook(...)` (original lines ~1772-1800)

**Notes:** Uses `InitError` from `types`. Add `use super::types::InitError;`.

---

### 7. `init/rules_core.rs` (≈300 LOC)

**Contents:**
- `fn git_invocation_prefix() -> &'static str` (original lines ~1810-1850)
- `fn git_subcommand_gate(subcommands: &str) -> String` (original lines ~1852-1870)
- `fn confidence_rules() -> Value` (original lines ~1872-1930)
- `fn trust_anchor_shell_pattern() -> String` (original lines ~1932-2010)
- `fn deflection_rules() -> Value` (original lines ~2012-2140)

**Notes:** No external dependencies beyond `serde_json::json`.

---

### 8. `init/rules_rust.rs` (≈400 LOC)

**Contents:**
- `fn rust_rules() -> Value` (original lines ~2142-2550)

**Notes:** Standalone; no imports needed beyond `serde_json::json`.

---

### 9. `init/rules_python.rs` (≈320 LOC)

**Contents:**
- `fn python_rules() -> Value` (original lines ~2552-2680)
- `fn python_patterns_rules() -> Value` (original lines ~2682-2900)

**Notes:** No external dependencies.

---

### 10. `init/rules_other.rs` (≈480 LOC)

**Contents:**
- `fn rhai_rules() -> Value` (original lines ~2902-2960)
- `fn structural_rules() -> Value` (original lines ~2962-3100)
- `fn typescript_rules() -> Value` (original lines ~3102-3160)
- `fn swift_rules() -> Value` (original lines ~3162-3250)
- `fn lua_rules() -> Value` (original lines ~3252-3290)
- `fn cue_rules() -> Value` (original lines ~3292-3330)
- `fn json_rules() -> Value` (original lines ~3332-3370)
- `fn yaml_rules() -> Value` (original lines ~3372-3430)
- `fn helm3_rules() -> Value` (original lines ~3432-3480)

**Notes:** No external dependencies.

---

### 11. `init/tests/mod.rs` (≈20 LOC)

**Contents:**
- `#[cfg(test)] mod pack_tests;`
- `#[cfg(test)] mod run_tests;`
- `#[cfg(test)] mod global_install_tests;`
- `#[cfg(test)] mod hook_helpers_tests;`
- `#[cfg(test)] mod risky_call_coverage_tests;`

**Notes:** The original `mod tests` and `mod risky_call_coverage_tests` are replaced by this directory module. The original `mod tests` contents are distributed among the submodules below.

---

### 12. `init/tests/pack_tests.rs` (≈400 LOC)

**Contents:** Tests from original `mod tests` related to `Pack`, `parse_packs`, `compose_packs`, `BASE_PACKS`, pack validation, and rule shape assertions. Includes:
- `user_config_paths_are_based_on_home`
- `every_starter_pack_passes_load_time_validation`
- `this_repos_rules_pass_load_time_validation`
- `pack_parse_accepts_aliases`
- `parses_swift_pack`
- `parses_new_language_packs`
- `parses_structural_pack`
- `structural_pack_ships_the_two_measured_rules`
- `structural_rules_cover_typescript`
- `structural_rules_only_warn`
- `structural_rules_opt_into_audit`
- `structural_rules_reference_only_graph_relations`
- `rust_pack_does_not_carry_structural_rules`
- `rhai_pack_carries_rhai_rules_and_rust_does_not`
- `rhai_pack_messages_are_project_neutral`
- `swift_pack_yields_rules`
- `pack_parse_rejects_unknown`
- `parse_packs_default_is_the_complete_base`
- `parse_packs_handles_comma_separated_list`
- `parse_packs_dedupes_duplicates`
- `parse_packs_rejects_none_combined_with_another_pack`
- `default_platform_blocks_trust_anchor_writes`
- `llm_pack_is_only_deflection_rules`
- `rust_pack_carries_only_rust_rules`
- `rust_pack_includes_new_predicate_rules`
- `rust_pack_includes_block_pattern_rules`
- `let_count_audit_rules_are_doc_excepted`
- `rust_pack_audit_only_rules_have_consistent_shape`
- `rust_pack_includes_patterns_book_rules`
- `rust_pack_includes_tier_1_rules`
- `compose_packs_llm_plus_rust_merges_both`
- `rust_pack_includes_runtime_hazard_rules`
- `rust_pack_includes_panic_in_drop_rule`
- `python_pack_includes_patterns_guide_rules`
- `rust_pack_includes_pub_fn_doc_rule`
- `swift_pack_includes_throws_force_unwrap_rule`
- `typescript_pack_includes_param_count_rule`
- `compose_packs_dedupes_by_rule_id`
- `none_pack_is_empty`
- `base_expands_to_every_language_agnostic_pack`
- `base_contains_no_language_pack`
- `base_composes_with_a_language_pack`
- `base_is_order_preserving_and_deduped`
- `base_is_case_insensitive_and_whitespace_tolerant`
- `an_unknown_pack_still_errors_and_names_base`

**Notes:** Use `use crate::init::*;` at top to access private items.

---

### 13. `init/tests/run_tests.rs` (≈500 LOC)

**Contents:** Tests from original `mod tests` related to `run`, file writers, gitignore, context pack, structural graph, and idempotency. Includes:
- `run_dry_run_writes_nothing`
- `run_hooks_only_skips_rules_and_gitignore`
- `run_dry_run_does_not_write_gemini_settings`
- `run_writes_all_four_files_on_fresh_project`
- `run_preserves_existing_permissions_in_settings`
- `run_does_not_overwrite_rules_without_force`
- `run_overwrites_rules_with_force`
- `init_with_base_scaffolds_every_subsystem`
- `base_ships_the_compact_kernel_within_its_own_ceiling`
- `context_pack_scaffolds_config_kernel_and_readme`
- `context_pack_leaves_an_existing_durable_file_as_the_session_document`
- `context_pack_dry_run_writes_nothing`
- `context_pack_preserves_existing_files`
- `context_pack_is_idempotent`
- `context_pack_carves_its_files_out_of_the_broad_gitignore`
- `without_the_context_pack_no_context_files_or_carveouts_appear`
- `gitignore_appends_only_missing_entries`
- `second_run_is_idempotent`
- `errors_when_path_missing`
- `run_writes_gemini_settings_with_mcp_and_hooks`
- `write_settings_includes_session_start_and_user_prompt_submit_hooks`
- `write_gemini_settings_includes_session_and_before_agent_hooks`
- `init_builds_the_graph_when_the_structural_pack_is_selected`
- `init_reports_the_graph_build`
- `graph_is_built_even_when_structural_rules_are_not_selected`
- `dry_run_builds_no_graph`
- `a_failed_graph_build_warns_rather_than_failing_init`

**Notes:** Use `use crate::init::*;`.

---

### 14. `init/tests/global_install_tests.rs` (≈300 LOC)

**Contents:** Tests from original `mod tests` related to global install/uninstall. Includes:
- `install_globally_with_home_writes_gemini_settings`
- `install_globally_with_home_writes_claude_json`
- `install_globally_with_home_preserves_other_gemini_settings`
- `install_globally_with_home_idempotent`
- `uninstall_globally_with_home_removes_from_gemini`
- `uninstall_globally_with_home_removes_from_claude`
- `uninstall_globally_with_home_idempotent_when_nothing_installed`
- `install_globally_with_home_dry_run_writes_nothing`

**Notes:** Use `use crate::init::*;`.

---

### 15. `init/tests/hook_helpers_tests.rs` (≈200 LOC)

**Contents:** Tests from original `mod tests` related to hook helpers. Includes:
- `upsert_hook_replaces_matching_matcher`
- `upsert_hook_by_command_replaces_ours_and_keeps_foreign`
- `is_phronesis_hook_command_recognizes_ours_and_only_ours`
- `upsert_hook_by_command_creates_missing_event_array`
- `upsert_hook_appends_when_no_matching_matcher`

**Notes:** Use `use crate::init::*;`.

---

### 16. `init/tests/risky_call_coverage_tests.rs` (≈300 LOC)

**Contents:** Entire original `mod risky_call_coverage_tests` (original lines ~5000-5228). Includes all tests for `install_one_target`, `write_mcp_json`, `write_gemini_settings`, `upsert_codex_hook`.

**Notes:** Use `use crate::init::*;`.

---

## Wiring Plan

### `init/mod.rs`

```rust
mod rule_sync;
use rule_sync::write_rules_file;

mod types;
pub use types::{Pack, BASE_PACKS, parse_packs, compose_packs, InitError, InitOpts, InitReport};

mod global_install;
pub use global_install::{
    user_claude_config_path, user_gemini_config_path,
    install_globally, install_globally_with_home,
    uninstall_globally, uninstall_globally_with_home,
};

mod writers_hooks;
mod writers_scaffold;
mod json_helpers;
mod rules_core;
mod rules_rust;
mod rules_python;
mod rules_other;

use writers_hooks::*;
use writers_scaffold::*;

pub fn run(opts: InitOpts) -> Result<InitReport, InitError> {
    // body unchanged
}

fn canonicalize_root(p: &Path) -> Result<PathBuf, InitError> {
    // body unchanged
}

fn binary_on_path(name: &str) -> bool {
    // body unchanged
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod risky_call_coverage_tests;
```

### `lib.rs`

No change required if it already declares `mod init;` or `pub mod init;`. If it re-exports items from `init`, keep those re-exports. If it does not, add:

```rust
pub use init::{
    Pack, BASE_PACKS, parse_packs, compose_packs, InitError, InitOpts, InitReport,
    user_claude_config_path, user_gemini_config_path,
    install_globally, install_globally_with_home,
    uninstall_globally, uninstall_globally_with_home,
    run,
};
```

---

## Public API and Behavior Preservation

- All public items (`Pack`, `BASE_PACKS`, `parse_packs`, `compose_packs`, `InitError`, `InitOpts`, `InitReport`, `user_claude_config_path`, `user_gemini_config_path`, `install_globally`, `install_globally_with_home`, `uninstall_globally`, `uninstall_globally_with_home`, `run`) are re-exported from `init/mod.rs` with identical signatures.
- `DEFAULT_DURABLE_MD` remains `pub(crate)`; no external visibility change.
- Private items moved to submodules must be made visible to siblings. Use `pub(crate)` or `pub(super)` on moved functions/structs/constants as needed. This is a visibility adjustment, not a behavior change.
- Absolute paths (`crate::durable_migrate`, `crate::context::config`, `crate::graph::sync`, `crate::rules_file`) remain unchanged.
- Test modules use `use crate::init::*;` instead of `use super::*;` to access private items from the parent module. This is a mechanical import fix.
- No logic is edited; all function bodies, constants, and test assertions are moved verbatim.

---

## Ordered Execution Steps

1. **Create directory structure**
   ```sh
   mkdir -p crates/phronesis-mcp/src/init/tests
   ```

2. **Create `init/mod.rs`**
   - Copy the original `init.rs` to `init/mod.rs`.
   - Remove all items except `mod rule_sync;`, `use rule_sync::write_rules_file;`, `run`, `canonicalize_root`, `binary_on_path`.
   - Add module declarations and re-exports as shown in Wiring Plan.
   - Add `use writers_hooks::*; use writers_scaffold::*;` inside `run` or at top.

3. **Create `init/types.rs`**
   - Move `Pack`, `impl Pack`, `BASE_PACKS`, `parse_packs`, `compose_packs`, `InitError`, `InitOpts`, `InitReport` from `init/mod.rs` into this file.
   - Add `use super::rules_core::*; use super::rules_rust::*; use super::rules_python::*; use super::rules_other::*;` at top.

4. **Create `init/global_install.rs`**
   - Move `user_claude_config_path`, `user_gemini_config_path`, `McpTarget`, `install_one_target`, `uninstall_one_target`, `install_globally`, `install_globally_with_home`, `uninstall_globally`, `uninstall_globally_with_home`.
   - Add `use super::json_helpers::*; use super::types::*;`.

5. **Create `init/writers_hooks.rs`**
   - Move `write_settings`, `write_mcp_json`, `write_gemini_settings`, `write_codex_hooks`, `write_codex_config`.
   - Add `use super::json_helpers::*; use super::types::*;`.

6. **Create `init/writers_scaffold.rs`**
   - Move all scaffold constants and functions listed above.
   - Add `use super::json_helpers::*; use super::types::*;`.

7. **Create `init/json_helpers.rs`**
   - Move `read_json`, `write_json`, `with_extension`, `ensure_parent`, `upsert_hook`, `PHRONESIS_HOOK_SUBCOMMANDS`, `is_phronesis_hook_command`, `upsert_hook_by_command`, `upsert_codex_hook`.
   - Add `use super::types::InitError;`.

8. **Create rule modules**
   - `init/rules_core.rs`: move `git_invocation_prefix`, `git_subcommand_gate`, `confidence_rules`, `trust_anchor_shell_pattern`, `deflection_rules`.
   - `init/rules_rust.rs`: move `rust_rules`.
   - `init/rules_python.rs`: move `python_rules`, `python_patterns_rules`.
   - `init/rules_other.rs`: move `rhai_rules`, `structural_rules`, `typescript_rules`, `swift_rules`, `lua_rules`, `cue_rules`, `json_rules`, `yaml_rules`, `helm3_rules`.

9. **Create test modules**
   - `init/tests/mod.rs` with declarations.
   - Distribute original `mod tests` contents into `pack_tests.rs`, `run_tests.rs`, `global_install_tests.rs`, `hook_helpers_tests.rs` as listed.
   - Move original `mod risky_call_coverage_tests` into `init/tests/risky_call_coverage_tests.rs`.
   - In each test file, replace `use super::*;` with `use crate::init::*;`.

10. **Update `lib.rs` if necessary**
    - Ensure `mod init;` (or `pub mod init;`) is present.
    - Add re-exports if not already present.

11. **Compile and fix visibility/import errors**
    - Run `cargo check -p phronesis-mcp`.
    - Adjust `pub(crate)`/`pub(super)` on moved items as needed.

12. **Run targeted tests**
    - Follow project policy: `phr-mcp coverage select`, rebuild graph if stale, run listed tests.
    - Run full suite: `cargo test --workspace -p phronesis-mcp`.

13. **Verify line counts**
    - Ensure each new module file is under 800 LOC (excluding test files if policy allows; but design keeps all under 800).

---

## Risks

- **Visibility errors:** Private items moved to submodules are no longer visible to siblings. Mitigation: add `pub(crate)` or `pub(super)` as needed; this is mechanical.
- **Test import breakage:** `use super::*` in test submodules no longer refers to `init`. Mitigation: use `use crate::init::*;`.
- **Circular dependencies:** `types.rs` depends on rule modules; rule modules do not depend on `types`. No cycles expected.
- **Line count drift:** Some modules (e.g., `writers_scaffold.rs`) are near 700 LOC. If future additions push over 800, further split may be needed.
- **Absolute path references:** Ensure `crate::durable_migrate`, `crate::context::config`, `crate::graph::sync`, `crate::rules_file` remain unchanged; they are absolute and unaffected by module moves.