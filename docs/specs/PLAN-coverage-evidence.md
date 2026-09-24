# Coverage Evidence Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement `SPEC-coverage-evidence.md` Phase 1 plus the region mapper (Phase 2's first item) — the durable coverage evidence store, normalized importer, demand-gated hook-time hydration, tree-sitter region mapping, and the relevant-test join rule — proven end-to-end on the `safe_divide` fixture with a real `cargo-llvm-cov` per-test export.

**Architecture:** Durable `.phronesis/coverage.jsonl` + `.phronesis/coverage.index` (derived, gitignored, replace-per-import). At hook fire, host-side producers follow the `clock_facts` pattern: `head_revision` from the existing `git_head_probe`; coverage facts assert **demand-gated** (only relations a loaded rule mentions, as `graph/hydrate.rs` does) and **change-scoped** (only the event's edited files). Region identity anchors to tree-sitter elements — `fn:<qualified-name>` and `branch:<fn>:<fnv1a-12>` of the if-condition text — never raw line numbers. No `phr` engine changes.

**Tech Stack:** Rust 2024 edition, `phronesis-mcp` only; `tree-sitter` + `tree-sitter-rust` (already workspace deps), `serde`/`serde_json`, `tempfile` in tests; `cargo-llvm-cov 0.8.7` + `cargo-nextest 0.9.140` for the real fixture export.

**Spec:** `docs/specs/SPEC-coverage-evidence.md`

## Global Constraints

- No changes to the `phr` crate; everything lives in `phronesis-mcp`.
- Facts are `Fact { predicate, args: Vec<String> }` — flat positional strings only; regions are minted ID strings, line numbers are store payload only.
- All minted region/test strings must survive the `security.rs` validators (predicate-name pattern for IDs; reject control chars).
- `.phronesis/coverage.jsonl` and `.phronesis/coverage.index` are derived state: gitignored, rebuildable, never committed — except the fixture's committed sample export, which lives under `tests/fixtures/` and **is** committed.
- Import is all-or-nothing: one invalid record fails the whole import with a clear error (no partial stores).
- `cargo clippy --workspace -- -D warnings` clean; `cargo fmt --all` applied; every task ends with green `cargo test -p phronesis-mcp` for its new tests.
- Conventional-commit messages; each task is one commit.

## Review Focus

Input classes the spec implies but no task's happy path exercises — each pinned by the named test:

1. **Malformed export line** → whole import fails, store untouched — Task 2, `test_import_rejects_malformed_line`.
2. **Re-import of the same revision** → idempotent replace, no duplicate hits — Task 2, `test_import_is_idempotent_per_revision`.
3. **Store revision ≠ HEAD** → `coverage_stale` asserted — Task 5, `test_hydrate_reports_stale_coverage`.
4. **Rule mentions coverage relations but no edited file overlaps the store** → zero coverage facts asserted (scoping) — Task 5, `test_hydrate_scopes_to_edited_files`.
5. **Anchor drift: the §4 edit changes the error message, not the condition** → same branch anchor, changed_region still maps — Task 4, `test_branch_anchor_survives_message_edit` (this is acceptance A2's pin).

---

## File structure

**Created**
- `crates/phronesis-mcp/src/coverage/mod.rs` — public surface + re-exports
- `crates/phronesis-mcp/src/coverage/store.rs` — `HitRecord`, `CoverageIndex`, read/write, replace-per-import
- `crates/phronesis-mcp/src/coverage/import.rs` — export parsing, validation, `ImportSummary`
- `crates/phronesis-mcp/src/coverage/region_map.rs` — tree-sitter spans, branch anchors, line-diff → changed regions
- `crates/phronesis-mcp/src/coverage/hydrate.rs` — demand-gated, change-scoped fact production + `head_revision`
- `crates/phronesis-mcp/tests/fixtures/coverage-sample/Cargo.toml` — the fixture crate (real)
- `crates/phronesis-mcp/tests/fixtures/coverage-sample/src/lib.rs` — `safe_divide` + 3 tests, verbatim from the sketch
- `crates/phronesis-mcp/tests/fixtures/coverage-sample/regenerate-export.sh` — documented real-export command
- `crates/phronesis-mcp/tests/fixtures/coverage-sample/export.jsonl` — committed real normalized export
- `crates/phronesis-mcp/tests/coverage_store.rs`
- `crates/phronesis-mcp/tests/coverage_import.rs`
- `crates/phronesis-mcp/tests/coverage_region_map.rs`
- `crates/phronesis-mcp/tests/coverage_hydrate.rs`
- `crates/phronesis-mcp/tests/coverage_golden.rs`
- `crates/phronesis-mcp/tests/features/coverage-evidence.feature`

**Modified**
- `crates/phronesis-mcp/src/lib.rs` — `pub mod coverage;`
- `crates/phronesis-mcp/src/main.rs` — `Coverage` CLI subcommand (`phr-mcp coverage import <file>`)
- `crates/phronesis-mcp/src/hook_facts.rs` — call `coverage::hydrate::assert_facts` after existing producers
- `.phronesis/rules.json` — rule `log-relevant-test-for-change` (spec §5.1)
- `.gitignore` — `.phronesis/coverage.jsonl`, `.phronesis/coverage.index`
- `crates/phronesis-mcp/CLAUDE.md` — `coverage import` subcommand row
- `AGENTS.md` — one-line coverage-evidence mention in the file-roles table

---

## Task 1: Coverage store — records, index, replace-per-import

**Files:**
- Create: `crates/phronesis-mcp/src/coverage/mod.rs`, `crates/phronesis-mcp/src/coverage/store.rs`
- Modify: `crates/phronesis-mcp/src/lib.rs` (`pub mod coverage;`)
- Test: `crates/phronesis-mcp/tests/coverage_store.rs`

**Interfaces:**
- Produces (later tasks rely on these exact names):

```rust
pub const COVERAGE_FORMAT: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HitRecord {
    pub v: u32,
    pub kind: String,        // "hit"
    pub test: String,
    pub region: String,      // "fn:safe_divide" | "branch:safe_divide:abc123def456"
    pub file: String,        // repo-relative, graph file_rel form
    pub start_line: u64,
    pub end_line: u64,
    pub hit_kind: String,    // "region" | "branch"
    pub revision: String,    // 40-hex
    pub tool: String,        // "cargo-llvm-cov"
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoverageIndex {
    pub format: u32,
    pub revision: String,
    pub imported_at: u64,
    pub tool: String,
}

pub fn store_paths(root: &Path) -> (PathBuf, PathBuf);              // (coverage.jsonl, coverage.index)
pub fn write_store(root: &Path, records: &[HitRecord], index: &CoverageIndex) -> Result<()>;
pub fn load_index(root: &Path) -> Option<CoverageIndex>;
pub fn load_hits(root: &Path) -> Result<Vec<HitRecord>>;
```

- [ ] **Step 1: Write the failing tests** — round-trip, replace-per-import, index read:

```rust
// tests/coverage_store.rs
use phronesis_mcp::coverage::store::{HitRecord, CoverageIndex, write_store, load_index, load_hits, COVERAGE_FORMAT};

fn temp_root() -> tempfile::TempDir { tempfile::tempdir().unwrap() }

fn hit(test: &str, region: &str, rev: &str) -> HitRecord {
    HitRecord { v: COVERAGE_FORMAT, kind: "hit".into(), test: test.into(), region: region.into(),
        file: "src/lib.rs".into(), start_line: 1, end_line: 7, hit_kind: "region".into(),
        revision: rev.into(), tool: "cargo-llvm-cov".into() }
}

#[test]
fn round_trips_records_and_index() {
    let root = temp_root();
    let idx = CoverageIndex { format: COVERAGE_FORMAT, revision: "a".repeat(40), imported_at: 1, tool: "cargo-llvm-cov".into() };
    write_store(root.path(), &[hit("t1", "fn:safe_divide", &"a".repeat(40))], &idx).unwrap();
    assert_eq!(load_index(root.path()), Some(idx));
    assert_eq!(load_hits(root.path()).unwrap().len(), 1);
}

#[test]
fn replace_per_import_drops_prior_revision() {
    let root = temp_root();
    let idx = |rev: &str| CoverageIndex { format: COVERAGE_FORMAT, revision: rev.into(), imported_at: 2, tool: "cargo-llvm-cov".into() };
    write_store(root.path(), &[hit("t1", "fn:safe_divide", &"a".repeat(40))], &idx(&"a".repeat(40))).unwrap();
    write_store(root.path(), &[hit("t2", "fn:safe_divide", &"b".repeat(40))], &idx(&"b".repeat(40))).unwrap();
    let hits = load_hits(root.path()).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].test, "t2"); // old revision fully replaced
}

#[test]
fn missing_store_loads_none_and_empty() {
    let root = temp_root();
    assert_eq!(load_index(root.path()), None);
    assert!(load_hits(root.path()).unwrap().is_empty());
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p phronesis-mcp --test coverage_store` → FAIL (unresolved module).
- [ ] **Step 3: Implement** `store.rs`: serde structs as above; `write_store` writes JSONL + index **atomically** (write to `<name>.tmp`, `fs::rename` into place — same discipline as `rules_file` autosave); `load_*` parse with `anyhow` context; unknown `v` field → error `unsupported coverage format`. Add `pub mod coverage;` to `lib.rs` with `pub mod store;` in `coverage/mod.rs`.
- [ ] **Step 4: Run to verify pass** — `cargo test -p phronesis-mcp --test coverage_store` → PASS.
- [ ] **Step 5: Commit** — `git add -A && git commit -m "feat(coverage): durable hit-record store with replace-per-import"`

---

## Task 2: Importer — validation, all-or-nothing, CLI

**Files:**
- Create: `crates/phronesis-mcp/src/coverage/import.rs`
- Modify: `crates/phronesis-mcp/src/main.rs` (subcommand), `crates/phronesis-mcp/src/coverage/mod.rs`
- Test: `crates/phronesis-mcp/tests/coverage_import.rs`

**Interfaces:**
- Consumes: `store::{HitRecord, CoverageIndex, write_store}` from Task 1; `security.rs` validators.
- Produces:

```rust
pub struct ImportSummary { pub records: usize, pub tests: usize, pub revision: String }
pub fn import_export(root: &Path, export_path: &Path, now_unix: u64) -> Result<ImportSummary>;
pub fn validate_record(rec: &HitRecord) -> Result<()>;   // all-or-nothing: call for every record first
```

Validation rules (each is a test): `v == COVERAGE_FORMAT`; `kind == "hit"`; `test`/`region` non-empty, ≤ 256 bytes, no control chars, `[A-Za-z0-9_:.\/-]` only; `file` repo-relative (no leading `/`, no `..`); `revision` is 40-hex; `hit_kind` ∈ {`region`,`branch`}; `start_line ≤ end_line`; export file ≤ `PHRONESIS_LOG_MAX_BYTES` cap (reuse `security.rs` limit). Mixed-revision export → error `export mixes revisions` (import is per-revision).

- [ ] **Step 1: Failing tests** — happy path writes store+index and returns counts; `test_import_rejects_malformed_line` (bad JSON line → error, store untouched: `load_index` still `None`); `test_import_rejects_absolute_file_path`; `test_import_rejects_mixed_revisions`; `test_import_is_idempotent_per_revision` (import same file twice → identical `load_hits`, count unchanged).
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** `import_export`: read file (size-capped), parse line-by-line into `HitRecord`, collect; if any `validate_record` fails → bail before any write; group-check single revision; write via Task 1. CLI in `main.rs`: `Coverage(coverage_cmd::CoverageArgs)` enum with `Import { export: PathBuf }` → calls `import_export(project_root, &export, now)` and prints `imported {records} hits across {tests} tests at revision {revision}`.
- [ ] **Step 4: Run** → PASS. Also `cargo run -p phronesis-mcp -- coverage import <bad-file>` prints the error, exit 1.
- [ ] **Step 5: Commit** — `feat(coverage): normalized export importer with all-or-nothing validation + CLI`

---

## Task 3: Fixture crate + real export

**Files:**
- Create: `crates/phronesis-mcp/tests/fixtures/coverage-sample/Cargo.toml`, `.../src/lib.rs`, `.../regenerate-export.sh`, `.../export.jsonl`

**Interfaces:**
- Consumes: nothing (standalone real crate).
- Produces: `export.jsonl` — the committed, real, per-test normalized export later tasks import. Fixture `src/lib.rs` is the sketch verbatim (`safe_divide` + 3 tests, spec §1); `regenerate-export.sh` is the documented command that regenerates `export.jsonl`.

- [ ] **Step 1: Create the fixture crate** — `Cargo.toml` (`[package] name = "coverage-sample"`, `[lib] path = "src/lib.rs"`) and `src/lib.rs` copied verbatim from spec §1.
- [ ] **Step 2: Write `regenerate-export.sh`** — runs each test separately under `cargo-llvm-cov` (test identity retained by isolation, spec §2's first option), extracts executed functions/branches from each run's JSON, and emits normalized records:

```sh
#!/bin/sh
# Regenerates export.jsonl from REAL per-test cargo-llvm-cov runs.
# Requires: cargo-llvm-cov 0.8.x. Run from this directory.
set -eu
REV=$(git -C ../.. rev-parse HEAD)   # fixture lives inside the phronesis repo; revision = repo HEAD
: > export.jsonl
for t in divides_positive_values divides_negative_values rejects_zero_denominator; do
  cargo llvm-cov --json --branch --exact "$t" > "/tmp/cov-$t.json"
  # Normalize: for each function executed in this single-test run, emit one
  # "region" hit; for each branch line executed, emit one "branch" hit.
  jq -r --arg t "$t" --arg rev "$REV" '
    .data[0].functions[] | select(.count > 0) |
    {v:1, kind:"hit", test:$t, region:("fn:" + .name), file:(.filenames[0] | sub("^.*coverage-sample/"; "")),
     start_line:.regions[0][0], end_line:.regions[-1][0], hit_kind:"region", revision:$rev, tool:"cargo-llvm-cov"} | tojson
  ' "/tmp/cov-$t.json" >> export.jsonl
done
echo "wrote $(wc -l < export.jsonl) records at $REV"
```

(The jq is the plan's reference shape; Task 3's executor runs it against the real `--json --branch` output and adjusts field paths to the actual llvm-cov JSON — the **command invocations and the record contract are fixed**, the extraction detail is verified empirically and the result committed.)

- [ ] **Step 3: Run it for real** — `cd tests/fixtures/coverage-sample && ./regenerate-export.sh`; verify by hand: `rejects_zero_denominator` must show a `branch:safe_divide:<anchor>` hit on the zero-denominator condition line plus `fn:safe_divide`; the two positive/negative tests must show `fn:safe_divide` and **no** branch hit for the zero branch. Commit the generated `export.jsonl`.
- [ ] **Step 4: Sanity test** — `cargo test -p phronesis-mcp --test coverage_import` with a test that imports the committed `export.jsonl` from `tests/fixtures/coverage-sample/` and asserts: 3 distinct tests, `fn:safe_divide` hit by all 3, and exactly one test hits a `branch:safe_divide:` region.
- [ ] **Step 5: Commit** — `feat(coverage): safe_divide fixture with real cargo-llvm-cov per-test export`

---

## Task 4: Region map — tree-sitter spans, branch anchors, changed regions

**Files:**
- Create: `crates/phronesis-mcp/src/coverage/region_map.rs`
- Test: `crates/phronesis-mcp/tests/coverage_region_map.rs`

**Interfaces:**
- Consumes: `tree-sitter`, `tree-sitter-rust` (workspace deps).
- Produces:

```rust
pub fn function_region_id(f: &str) -> String;                        // "fn:{f}"
pub fn branch_region_id(f: &str, anchor: &str) -> String;            // "branch:{f}:{anchor}"
pub struct BranchSite { pub function: String, pub anchor: String, pub start_line: u64, pub end_line: u64 }
pub fn extract_branch_sites(source: &str) -> Result<Vec<BranchSite>>;
pub struct ChangedRegions { pub functions: Vec<String>, pub branches: Vec<String> }
pub fn changed_regions(old: &str, new: &str) -> Result<ChangedRegions>;
```

Anchor = 12 lowercase hex of FNV-1a over the **if-condition source text** (same hash family as `graph/sync/mod.rs`). Spans are 1-based, inclusive.

- [ ] **Step 1: Failing tests**:

```rust
// tests/coverage_region_map.rs — content from the fixture, verbatim
const OLD: &str = "pub fn safe_divide(numerator: i32, denominator: i32) -> Result<i32, &'static str> {\n    if denominator == 0 {\n        return Err(\"division by zero\");\n    }\n\n    Ok(numerator / denominator)\n}\n";
const NEW: &str = "pub fn safe_divide(numerator: i32, denominator: i32) -> Result<i32, &'static str> {\n    if denominator == 0 {\n        return Err(\"invalid denominator\");\n    }\n\n    Ok(numerator / denominator)\n}\n";

#[test]
fn test_branch_anchor_survives_message_edit() {           // Review Focus #5 / acceptance A2
    let old = extract_branch_sites(OLD).unwrap();
    let new = extract_branch_sites(NEW).unwrap();
    assert_eq!(old.len(), 1);
    assert_eq!(old[0].anchor, new[0].anchor);              // condition unchanged -> same site
    let ch = changed_regions(OLD, NEW).unwrap();
    assert!(ch.branches.contains(&branch_region_id("safe_divide", &old[0].anchor)));
    assert!(ch.functions.contains(&function_region_id("safe_divide")));
}

#[test]
fn test_untouched_function_not_reported() {
    let old = format!("{OLD}pub fn helper() -> i32 {{ 1 }}\n");
    let new = format!("{NEW}pub fn helper() -> i32 {{ 1 }}\n");
    let ch = changed_regions(&old, &new).unwrap();
    assert!(!ch.functions.contains(&function_region_id("helper")));
}

#[test]
fn test_condition_change_moves_anchor() {
    let new_cond = OLD.replace("denominator == 0", "denominator <= 0");
    let a = extract_branch_sites(OLD).unwrap()[0].anchor.clone();
    let b = extract_branch_sites(&new_cond).unwrap()[0].anchor;
    assert_ne!(a, b);
}
```

- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** — tree-sitter parse (reuse the parser setup pattern from `src/syntax/`), walk to `function_item` nodes (name from `name` field child; span = node start/end row) and `if_expression` nodes (condition = `condition` field text; anchor = FNV-1a-12 over it; span = condition node rows). `changed_regions`: split old/new into lines, simple LCS to find changed line numbers in both, then map every function/branch site whose span overlaps any changed line (both sides) into `ChangedRegions` (deduplicated, sorted). Guard: skip branch mapping for files > 100_000 lines (documented cap).
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** — `feat(coverage): tree-sitter region map with stable branch anchors`

---

## Task 5: Hydration — demand-gated, change-scoped, head_revision, staleness

**Files:**
- Create: `crates/phronesis-mcp/src/coverage/hydrate.rs`
- Modify: `crates/phronesis-mcp/src/hook_facts.rs` (call site after existing producers), `crates/phronesis-mcp/src/coverage/mod.rs`
- Test: `crates/phronesis-mcp/tests/coverage_hydrate.rs`

**Interfaces:**
- Consumes: `store::{load_index, load_hits}`, `region_map::changed_regions`, `lifecycle::outcome::git_head_probe`, `Rule` conditions from the loaded rules (the hook already holds them).
- Produces:

```rust
pub const RELATIONS: &[&str] = &["test_hits_region", "test_hits_branch", "coverage_revision",
    "coverage_stale", "changed_region", "changed_function", "head_revision"];

/// Facts for one hook event. `rule_relations` = the set of predicates any loaded
/// rule mentions (demand gate); `edited` = (repo-relative file, old content, new content).
pub fn facts_for_event(root: &Path, rule_relations: &HashSet<String>,
    edited: &[(String, Option<String>, String)], head_sha: Option<String>) -> Result<Vec<Fact>>;
```

Behavior: if `RELATIONS ∩ rule_relations` is empty → return `vec![]` (demand gate — Review Focus #4). Otherwise: `head_revision(head_sha or git_head_probe)`; `coverage_revision(index.revision)`; if they differ → `coverage_stale` (zero-arg); for each edited file, `changed_regions(old,new)` → `changed_region`/`changed_function` facts (args `["head:<short>", "<region>"]`); hits from the store **only for files/regions touched by the edit** (change scope) → `test_hits_region(test, region)` / `test_hits_branch`. All facts get `source: "coverage"`, stable IDs `coverage:<predicate>:<args-hash>` so re-assert replaces (the graph `fact_id()` precedent). Wire the call into `hook_facts.rs` after the existing producers, wrapped in a `PHRONESIS_NO_COVERAGE` env escape hatch (house fail-open pattern).

- [ ] **Step 1: Failing tests** — `test_demand_gate_skips_when_no_rule_mentions` (rule_relations without coverage predicates → no facts, even with a full store); `test_hydrate_scopes_to_edited_files` (store has hits for file A and B; edit touches A → only A's hits asserted); `test_hydrate_reports_stale_coverage` (index revision ≠ head → `coverage_stale` present, and hits still asserted — stale is a marker, enforcement demotion is the hook's existing drift path); `test_hydrate_emits_changed_regions` (the §4 edit → `changed_region` for `fn:safe_divide` + the branch site); `test_head_revision_uses_probe` (head_sha None + a temp git repo → fact carries the probed sha).
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** per the contract above.
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** — `feat(coverage): demand-gated change-scoped hook hydration`

---

## Task 6: Rule 5.1 + golden trace + BDD

**Files:**
- Modify: `.phronesis/rules.json`, `crates/phronesis-mcp/tests/coverage_golden.rs`, `crates/phronesis-mcp/tests/features/coverage-evidence.feature`

**Interfaces:**
- Consumes: everything above. The rule (spec §5.1, verbatim):

```json
{
  "id": "log-relevant-test-for-change",
  "phase": "post",
  "priority": 30,
  "audit": true,
  "when": [
    { "changed_region": ["?change", "?region"] },
    { "test_hits_region": ["?test", "?region"] }
  ],
  "then": { "log": "change ?change touches region ?region exercised by test ?test" }
}
```

- [ ] **Step 1: Failing golden test** — `coverage_golden.rs`: temp project root; commit the fixture's `export.jsonl` into a temp `.phronesis/coverage.jsonl` via `import_export`; build the event (edit `src/lib.rs` OLD→NEW per Task 4 constants); `facts_for_event` with `rule_relations` containing the two predicates; assert facts into a `ReteNetwork` loaded with rule 5.1 (follow the `hook` test harness pattern used by `values_integration.rs`); fire; assert exactly one `log` consequence per matching pair, and that it names `rejects_zero_denominator` with the branch region **and** all three tests at `fn:safe_divide` — while `divides_positive_values`/`divides_negative_values` never pair with the branch region (acceptance A2).
- [ ] **Step 2: Run** → FAIL (rule not yet in rules.json / facts not wired).
- [ ] **Step 3: Add the rule** to `.phronesis/rules.json`; make the golden test load rules from a fixture rules file mirroring it.
- [ ] **Step 4: Run** → PASS. Then the BDD scenario in `coverage-evidence.feature`:

```gherkin
Feature: Coverage evidence
  Scenario: Branch change derives the branch-relevant test
    Given a project with the safe_divide fixture and its real coverage export
    When the zero-denominator error message is edited
    Then rule log-relevant-test-for-change fires for test "rejects_zero_denominator"
    And it does not fire for "divides_positive_values" at the branch region
```

- [ ] **Step 5: Commit** — `feat(coverage): relevant-test join rule with golden trace`

---

## Task 7: Docs, gitignore, demo polish

**Files:**
- Modify: `.gitignore`, `crates/phronesis-mcp/CLAUDE.md`, `AGENTS.md`

- [ ] **Step 1:** `.gitignore` += `.phronesis/coverage.jsonl`, `.phronesis/coverage.index` (derived state, matching `graph.jsonl` treatment).
- [ ] **Step 2:** `CLAUDE.md`: add `coverage import` row to the CLI table — "`phr-mcp coverage import <export.jsonl>` — ingest a per-test coverage export into the evidence store".
- [ ] **Step 3:** `AGENTS.md`: add one row to Key File Roles — `crates/phronesis-mcp/src/coverage/` — "Coverage evidence store, importer, region mapping, demand-gated hydration (SPEC-coverage-evidence.md)".
- [ ] **Step 4:** Full gates: `cargo test -p phronesis-mcp && cargo clippy --workspace -- -D warnings && cargo fmt --all` → green.
- [ ] **Step 5: Commit** — `docs(coverage): wire coverage evidence into project docs`

---

## Self-review

- **Spec coverage:** store/import/hydrate/region facts = spec §3–§5; rule 5.1 = §5.1; A1 = Task 3; A2 = Tasks 4+6 (`test_branch_anchor_survives_message_edit` + golden); A4 = Task 5 demand-gate test; A7 = Task 6 BDD. A3 (gap rule) and A6 (selection) are Phase 2 per the spec — **not** in this plan, deliberately. A5's demotion rides the existing `hook_logged.rs` path; `coverage_stale` (Task 5) is the fact it keys on.
- **Type consistency:** `HitRecord`/`CoverageIndex` names identical in Tasks 1–3; `function_region_id`/`branch_region_id` identical in Tasks 4–6; `facts_for_event` signature fixed in Task 5 and consumed verbatim in Task 6.
- **Placeholders:** the one empirical unknown (llvm-cov JSON field paths in Task 3's jq) is an explicitly marked verification step with a committed real artifact — not a deferred implementation.