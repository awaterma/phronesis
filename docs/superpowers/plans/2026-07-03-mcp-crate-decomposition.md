# MCP-Crate Decomposition Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Drive `phronesis-mcp` audit debt from 51 in-scope hits to 0 unjustified hits (workspace total 59 → 8 engine-only), per the approved design `docs/superpowers/specs/2026-06-28-mcp-crate-decomposition-design.md`.

**Architecture:** Behavior-preserving refactors only, in three shapes: (1) module splits for the two >800-LOC files (`hook.rs`, `syntax/rust.rs`) with `pub use` re-exports so no external path changes; (2) helper-function extraction for multi-step logic; (3) the block pattern (`let x = { ... };`) for sequential temporaries. One state struct (`diff_extract.rs`). The engine crate (`crates/phronesis/src/network.rs` et al.) is untouched — its 8 hits are the deliberate end-state.

**Tech Stack:** Rust; verification via the existing 517-test workspace suite + `phr-mcp audit` trend gate.

## Global Constraints

- Branch: `fix/mcp-crate-decomposition` off `main`, **after** the 0.16.2 migrate-extracted-rules PATCH merges. Pre-feature anchor: tag `v0.16.1` (the design doc's `v0.15.0` reference is stale). Use an isolated worktree (superpowers:using-git-worktrees).
- **Line numbers in this plan reference v0.16.1 coordinates (`b477a0f`).** `main.rs` shifts after the 0.16.2 merge — always re-locate by item name; line numbers are hints, not addresses.
- **Refactors are behavior-preserving only.** No logic changes, no drive-by cleanup. If a refactor surfaces a real bug: stop, record it (scratch note or decision page), leave the behavior as-is, resume the refactor.
- **Per-commit gate (every task):** `cargo test --workspace` green; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt --all -- --check` clean; audit non-increasing, checked with the branch build: `cargo run -q -p phronesis-mcp --bin phr-mcp -- audit | tail -4` (record the three rule counts before and after; touched-file rows must disappear from `... -- audit --rule audit-rust-let-binding-count-high` / `...let-mut...`).
- **Audit threshold is 8**: any function you write or touch must end with ≤7 outer-scope `let` bindings and ≤7 `let mut`, or you've moved debt, not removed it.
- **`//! phronesis-allow` markers are file-level** — a marker for a let-rule exempts *every* function in that file for that rule. Treat markers as a last resort requiring a written rationale in the marker text; prefer decomposition. Expected marker count for this plan: zero.
- **No push, ever, without explicit human approval.** Commits on the branch are fine; PR at the end.
- Commit trailers per harness rules (Co-Authored-By etc.) apply to all commits.

## Baseline (recorded 2026-07-03, v0.16.1)

| Rule | Hits | Files |
|---|---|---|
| audit-rust-let-binding-count-high | 39 | 19 |
| audit-rust-let-mut-count-high | 18 | 10 |
| audit-file-loc-high | 2 (`hook.rs` 1764 LOC, `syntax/rust.rs` 1622 LOC) | 2 |

In scope: 51 (MCP crate). Out of scope: 8 (engine: `network.rs` `assert_fact`/`add_rule`/`update_agenda`, `script_evaluator.rs`, `beta_network.rs`, `production.rs` — 4 let + 4 mut).

---

### Task 1: Phase 0 — enable `doc_excepted` on the let rules, fix the stale anchor

**Files:**
- Modify: `crates/phronesis-mcp/src/init.rs` — `audit-rust-let-binding-count-high` (object at lines 1570–1580) and `audit-rust-let-mut-count-high` (1581–1591) in the shipped pack.
- Modify: `.phronesis/rules.json` — same two rules (objects starting at lines 592 and 613).
- Modify: `docs/superpowers/specs/2026-06-28-mcp-crate-decomposition-design.md` — anchor line 6: `v0.15.0` → `v0.16.1`; status line 4 → `Approved; in implementation (docs/superpowers/plans/2026-07-03-mcp-crate-decomposition.md)`.

**Interfaces:**
- Produces: `//! phronesis-allow: audit-rust-let-binding-count-high <reason>` (and the let-mut twin) become honorable markers. The audit-side skip already exists (`audit.rs:357` checks `rule.doc_excepted.unwrap_or(false) && file_exempts_rule(...)` *before* the AST branch, so it works for AST-predicate rules); only the rule data lacks the flag.

- [ ] **Step 1: Write the failing test**

In `crates/phronesis-mcp/src/init.rs`'s test module (near the existing let-rule assertions at lines 2006–2012), add:

```rust
#[test]
fn let_count_audit_rules_are_doc_excepted() {
    let rules = compose_packs("rust").expect("rust pack");
    for id in [
        "audit-rust-let-binding-count-high",
        "audit-rust-let-mut-count-high",
    ] {
        let rule = rules
            .iter()
            .find(|r| r["id"] == id)
            .unwrap_or_else(|| panic!("{id} missing from rust pack"));
        assert_eq!(
            rule["doc_excepted"], true,
            "{id} must honor //! phronesis-allow markers"
        );
    }
}
```

(Adapt the lookup to how the neighboring tests at init.rs:2006–2012 actually access pack rules — match their idiom exactly.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p phronesis-mcp let_count_audit_rules_are_doc_excepted`
Expected: FAIL — `doc_excepted` is null/missing.

- [ ] **Step 3: Add the flag in both places**

In each of the two init.rs rule objects, add `"doc_excepted": true,` directly after `"audit": true,` — mirroring the key order used by `audit-file-loc-high` (see init.rs:1356/1450/1552 for the existing pattern). Make the identical edit to the two rules in `.phronesis/rules.json` (after the `"audit": true` line inside objects at 592/613 — note the local copies serialize keys in the pinned order `id, phase, priority, [audit, silent, doc_excepted], when, then` per `rules_file.rs:340–352`).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p phronesis-mcp init`
Expected: PASS including the new test.

- [ ] **Step 5: Record baseline, gate, commit**

Run: `cargo run -q -p phronesis-mcp --bin phr-mcp -- audit | tail -4`
Expected: unchanged 39/18/2 (the flag alone changes nothing — no markers exist yet). Full gate, then:

```bash
git add crates/phronesis-mcp/src/init.rs .phronesis/rules.json docs/superpowers/specs/2026-06-28-mcp-crate-decomposition-design.md
git commit -m "feat(audit): honor phronesis-allow markers on let-count rules (decomposition phase 0)"
```

---

### Task 2: `main.rs` — one `handle_<variant>` per Command arm (−1 let hit, the biggest single function: 64 lets)

**Files:**
- Modify: `crates/phronesis-mcp/src/main.rs` only. The file keeps its `phronesis-allow: audit-file-loc-high` marker (line 9) and its declarations-plus-dispatch layout; handlers stay in this file.

**Interfaces:**
- Consumes: nothing new. Produces: private `async fn handle_<variant>(...) -> anyhow::Result<()>` per extracted arm; `main()` becomes a thin `match` of one-line delegations.

- [ ] **Step 1: Baseline the integration suite**

Run: `cargo test -p phronesis-mcp --tests`
Expected: green. These tests spawn the binary per subcommand (`migrate_integration.rs`, `init_integration.rs`, `journey_cli_integration.rs`, `hook_integration.rs`, `decision_new_integration.rs`, `confidence_cli_integration.rs`, `wiki_drift_integration.rs`, …) and are the characterization net. Spot-check that every *large* arm has at least one spawning test: Audit (`action_log_integration.rs` spawns audit? — verify with `grep -rln '"audit"' crates/phronesis-mcp/tests/`), Stats, Trend, MemoryDrift, WikiDrift, Decision, Init, MigrateRules, MigrateExtractedRules. For any large arm with **zero** coverage, add one smoke test (spawn with `--help`-adjacent minimal args in a tempdir, assert exit status and one stdout marker string) in `crates/phronesis-mcp/tests/cli_smoke.rs` before refactoring.

- [ ] **Step 2: Extract handlers arm by arm**

For each variant with a multi-line arm — `Serve`, `SessionContext`, `TurnContext`, `Stats`, `Confidence`, `Journey`, `Audit`, `Trend`, `ClaudeMdDrift`, `MigrateRules`, `MigrateExtractedRules`, `MemoryDrift`, `WikiDrift`, `Decision`, `Init`, `Install`, `Uninstall` — cut the arm body into a private fn below `main()`:

```rust
async fn handle_audit(
    rule: Option<String>,
    path: Option<PathBuf>,
    json: bool,
    fail_on: Option<String>,
) -> anyhow::Result<()> {
    // former arm body, verbatim
}
```

and the arm becomes `Command::Audit { rule, path, json, fail_on } => handle_audit(rule, path, json, fail_on).await,`. Rules of the extraction:
- `async fn` only where the body `.await`s (Serve, Journey, and any arm calling async APIs); the rest are plain `fn` called without `.await`.
- `std::process::exit(...)` calls move verbatim — they are the behavior contract (Audit's `fail_on` exit codes, MigrateRules' `--check` exit 1). Never convert an `exit` into a returned error.
- `PreCheck`/`PostCheck` stay as the existing one-liners.
- The `Serve` handler needs `use rmcp::ServiceExt;` in scope (currently imported at main.rs:23 — file-level import already covers it).
- `today_iso()` (lines 18–22) stays; the Decision handler keeps using it.

- [ ] **Step 3: Gate and audit check**

Run the full per-commit gate. Then:
Run: `cargo run -q -p phronesis-mcp --bin phr-mcp -- audit --rule audit-rust-let-binding-count-high | grep main.rs`
Expected: no output (the `main` hit is gone; no handler re-trips the rule — if one does, apply the block pattern inside that handler until it's ≤7).

- [ ] **Step 4: Commit**

```bash
git add crates/phronesis-mcp/src/main.rs crates/phronesis-mcp/tests/cli_smoke.rs
git commit -m "refactor(cli): extract handle_<variant> per Command arm; main is thin dispatch"
```

---

### Task 3: `audit.rs` — characterization test, then dedup `run`/`run_profiled` (−4 let, −3 mut hits)

**Files:**
- Modify: `crates/phronesis-mcp/src/audit.rs` (`run` 287–511, `run_profiled` 541–731, `compute_trend` 868–984, `days_to_ymd` 1259–1271; test module 1273–3075).

**Interfaces:**
- Consumes: nothing new. Produces (all private to audit.rs): `fn run_core(rules: &RulesFile, opts: &AuditOpts, times: Option<&mut AuditSectionTimes>) -> AuditReport`; scan-stage helpers per Step 3. Public signatures of `run`, `run_profiled`, `compute_trend` are **frozen** — callers at main.rs (`run` in the audit handler), server.rs:768 (`run`), server.rs:847 (`compute_trend`), examples/profile_audit.rs:63/66 (`run_profiled`) must not change.

- [ ] **Step 1: Write the missing characterization test (currently ZERO coverage of `run_profiled`)**

Add to the audit.rs test module, using the same fixture helpers the neighboring `run_*` tests use (see `run_finds_content_matches_in_files` at 1299 for the tempdir/rules idiom):

```rust
#[test]
fn run_profiled_matches_run_and_populates_section_times() {
    // reuse the fixture from run_finds_content_matches_in_files:
    // a tempdir with one matching file and one audit rule
    let (rules, opts) = /* same setup as run_finds_content_matches_in_files */;

    let plain = run(&rules, &opts);
    let (profiled, times) = run_profiled(&rules, &opts);

    // identical audit semantics
    assert_eq!(plain.files_scanned, profiled.files_scanned);
    assert_eq!(plain.per_rule.len(), profiled.per_rule.len());
    for (a, b) in plain.per_rule.iter().zip(profiled.per_rule.iter()) {
        assert_eq!(a.rule_id, b.rule_id);
        assert_eq!(a.hits, b.hits);
        assert_eq!(a.files, b.files);
    }
    // instrumentation populated
    assert_eq!(times.files_scanned, profiled.files_scanned);
    assert!(times.audit_rules >= 1);
    assert!(times.total >= times.match_loop);
}
```

(Adapt field names to the real `RuleAudit`/`AuditReport` shapes — copy assertions from `render_json_shape` at 2959 if simpler. The point pinned: same report from both entrypoints, `AuditSectionTimes` populated.)

Run: `cargo test -p phronesis-mcp run_profiled_matches_run`
Expected: PASS against the current duplicated implementations. This is the guard for the dedup — commit it before refactoring:

```bash
git add crates/phronesis-mcp/src/audit.rs
git commit -m "test(audit): characterize run_profiled against run before dedup"
```

- [ ] **Step 2: Dedup into `run_core`**

Replace the twin bodies with one core: `fn run_core(rules, opts, mut times: Option<&mut AuditSectionTimes>) -> AuditReport`. Timing capture points (the nine deltas, from the current run_profiled at 545–546, 554, 556/562, 568–576, 581/587, 597/684, 661, 687/717, 718–720) become guarded blocks:

```rust
let t = Instant::now();
let files = discover_files(&opts.scan_root, &["*"]);
if let Some(times) = times.as_deref_mut() {
    times.discover = t.elapsed();
    times.files_scanned = files.len() as u32;
}
```

(`Instant::now()` unconditionally — it's nanoseconds; only the stores are guarded. `line_matches_evaluated += 1` likewise guarded in the line loop.) Then:

```rust
pub fn run(rules: &RulesFile, opts: &AuditOpts) -> AuditReport {
    run_core(rules, opts, None)
}

pub fn run_profiled(rules: &RulesFile, opts: &AuditOpts) -> (AuditReport, AuditSectionTimes) {
    let mut times = AuditSectionTimes::default();
    let report = run_core(rules, opts, Some(&mut times));
    (report, times)
}
```

`scan_duration_ms`: in the core, derive from one `total_start.elapsed()` used for both the report field and `times.total` (semantically identical to both current paths). Preserve the mixed-AST-short-circuit caveat comment (currently duplicated at 374–379 and its twin) once in the core.

- [ ] **Step 3: Decompose `run_core` below threshold**

`run_core` inherits ~26+ lets; extract its stages into private helpers so the core reads as a pipeline (each helper ≤7 lets — sub-split if not):

- `fn filter_audit_rules<'a>(rules: &'a RulesFile, rule_filter: Option<&str>) -> Vec<&'a DiskRule>` — lines 294–299.
- `fn scan_file_into_accum(path: &Path, rules: &[&DiskRule], accum: &mut BTreeMap<...>, times: Option<&mut AuditSectionTimes>)` — the per-file body 314–466 (read, lines, keep_mask, effective LOC, lazy ast_facts, per-rule loop). Inside it:
  - `fn eval_ast_rule(...) -> Option<PerFileHits>` — the AST branch 380–412,
  - `fn eval_whole_file_rule(...) -> Option<PerFileHits>` — 416–422,
  - `fn eval_content_rule(...) -> Option<PerFileHits>` — the line loop 429–463.
- `fn build_per_rule(accum: BTreeMap<...>) -> Vec<RuleAudit>` — accum→sorted vec, 468–498.

Exact signatures will fall out of what the loop actually threads (keep_mask, lines, ast_facts cache); the constraint that matters: behavior identical, each fn ≤7 outer lets, `PerFileHits`/`details` handling (0.16.1's named-AST-hit feature) untouched.

- [ ] **Step 4: Decompose `compute_trend` (11 lets, 3 mut) and `days_to_ymd` (10 lets)**

`compute_trend`: block-pattern the now/window resolution (`let window = { ... };` for 871–896) and extract `fn rule_trends(snapshots: &[&LogEntry]) -> Vec<RuleTrend>` for 918–968.
`days_to_ymd`: the Hinnant civil-from-days temporaries collapse with one block: `let (y, mp, d) = { let era = ...; let doe = ...; let yoe = ...; ...; (y, mp, d) };` then the final month/year adjustment. Keep the algorithm-reference comment.

- [ ] **Step 5: Gate, audit check, commit**

Full gate. Then: `cargo run -q -p phronesis-mcp --bin phr-mcp -- audit --rule audit-rust-let-binding-count-high | grep audit.rs` → no output; same for `let-mut`. All ~40 existing audit tests plus the new characterization test green.

```bash
git add crates/phronesis-mcp/src/audit.rs
git commit -m "refactor(audit): dedup run/run_profiled via run_core; decompose scan stages"
```

---

### Task 4: `hook.rs` — module split (pure moves) (file-loc hit 1 of 2)

**Files:**
- Create: `crates/phronesis-mcp/src/hook/mod.rs`, `hook/pre.rs`, `hook/post.rs`, `hook/journey_record.rs`, `hook/seq.rs`
- Delete: `crates/phronesis-mcp/src/hook.rs`
- Unchanged: `lib.rs:7` (`pub mod hook;` resolves to the directory), `main.rs`, `hook_facts.rs` (its `use crate::hook::HookError;` at line 13 must keep resolving).

**Interfaces:**
- Frozen external surface: `pub async fn run_pre_check`, `pub async fn run_post_check` (re-exported from mod.rs), `pub(crate) enum HookError` (stays in mod.rs). Verified single external consumers: main.rs:303–304, hook_facts.rs:13.

- [ ] **Step 1: Split with zero code changes (moves only)**

Destination map (v0.16.1 line ranges):

| Destination | Items (source lines) |
|---|---|
| `hook/mod.rs` | module doc; `mod pre; mod post; mod journey_record; mod seq;` + `pub use pre::run_pre_check; pub use post::run_post_check;`; `RulesLoadError` (17–21); `HookError` + `From<String>` (23–39, stays `pub(crate)`); `HookPayload` (41–52, → `pub(super)` fields as needed); `exit_ok` (54–63); `read_payload` (524–528); `extract_tool_output_text` (530–550); `outcomes_for_journal` (552–565) — or move these two to journey_record.rs, their only caller; `unix_secs_now` (567–575, shared: journey_record + `assert_journey_facts_into`); `load_rules` (903–934); `extract_file_path`/`extract_new_content`/`extract_old_content`/`extract_multiedit_field` (936–1008); `assert_pack_marker_facts` (807–830); `assert_journey_facts_into` (832–873); `assert_confidence_signals` (875–901); `log_hook_event` (1018–1042); the hook_logged `pub(crate) use` (1044); payload-extraction tests (1062–1123, 1269–1318, 1651–1763) and the hook_facts/hook_logged passthrough tests (1320–1649) |
| `hook/pre.rs` | `run_pre_check` (65–280) |
| `hook/post.rs` | `run_post_check` (282–522) |
| `hook/journey_record.rs` | `journey_record_post` (619–669, → `pub(super)`); `tagger_facts` (671–752); `collect_tagger_bash_patterns` (754–768); `collect_bash_patterns_from_value` (770–797); `sanitize_pattern` (799–805); tagger tests (1125–1266) |
| `hook/seq.rs` | `next_seq` (577–617, → `pub(super)`) |

Visibility: children reach parent-private items via `super::`; only cross-sibling calls need `pub(super)` (`journey_record_post` called from post.rs, `next_seq` called from journey_record.rs). pre.rs/post.rs import `crate::hook_facts::{...}` directly (don't thread through mod.rs).

- [ ] **Step 2: Gate — the split alone must be invisible**

Run: `cargo test --workspace` (hook_integration.rs 666 lines, journey_hook_integration.rs, syntax_integration.rs, BDD hooks.feature all spawn the binary — split-proof) + clippy + fmt.
Then: `cargo run -q -p phronesis-mcp --bin phr-mcp -- audit --rule audit-file-loc-high`
Expected: only `syntax/rust.rs` remains (every hook/ file lands well under 800: mod ~330, pre ~230, post ~250, journey_record ~260, seq ~60).

- [ ] **Step 3: Commit**

```bash
git add crates/phronesis-mcp/src/hook crates/phronesis-mcp/src/hook.rs
git commit -m "refactor(hook): split hook.rs into pre/post/journey_record/seq modules (<800 LOC each)"
```

---

### Task 5: `hook/` — function decomposition (−4 let hits)

**Files:**
- Modify: `crates/phronesis-mcp/src/hook/{mod,pre,post,journey_record,seq}.rs`

**Interfaces:**
- Produces in mod.rs, `pub(super)`: `fn assert_cargo_workspace_facts(network: &ReteNetwork, content: &str)` (dedup of the verbatim-duplicated scanner loops, pre 186–197 ≡ post 440–451); `fn collect_logged(consequences: Vec<Consequence>) -> (Vec<LoggedConsequence>, Vec<LoggedConsequence>)` (dedup of pre 250–254 ≡ post 495–499, returning violations/warnings split).

- [ ] **Step 1: Dedup the shared blocks**

Extract `assert_cargo_workspace_facts` and `collect_logged` into mod.rs; replace both call sites of each. Exact bodies move verbatim from the pre-check copies.

- [ ] **Step 2: Stage-extract `run_pre_check` (19 lets) and `run_post_check` (19 lets)**

Keep every `process::exit`/`exit_ok()` **in the entrypoint fn** — stages return values, the entrypoint decides exits (fail-closed exit 2 in pre / fail-open exit 1 in post is the contract; do not bury exits in helpers). Extract per entrypoint:
- pre.rs: `fn assert_pre_content_facts(network: &ReteNetwork, payload: &HookPayload, rules: &[Rule], new_content: &str, file_path: Option<&str>) -> ...` (the content block, 152–236 — includes the pre-only old-disk-content read; adjust the parameter list to what the block actually consumes) and a small `let (rules, patterns) = { ... };` block for the load/collect prologue (96–111).
- post.rs: `fn read_disk_content(...) -> Result<Option<String>, PathViolation>` (375–397; the outside-root case returns the Err variant, post.rs maps it to exit 1) and `fn assert_post_content_facts(...)` (412–481, includes the post-only `check_missing_patterns`).

- [ ] **Step 3: Decompose `next_seq` (11 lets) and `journey_record_post` (10 lets)**

seq.rs: inner `fn bump_seq_file(path: &Path) -> std::io::Result<u64>` (open/lock/read/increment/write, source 594–615); `next_seq` becomes the create-dir + `bump_seq_file(...).unwrap_or(0)` wrapper — best-effort zero-default behavior identical.
journey_record.rs: `fn load_tagger_config(root: &Path) -> TaggerConfig` (628–635 with its default fallback) and `fn build_journal_record(...) -> JournalRecord` (647–667).

- [ ] **Step 4: Gate, audit check, commit**

Full gate; `... -- audit --rule audit-rust-let-binding-count-high | grep hook` → no output.

```bash
git add crates/phronesis-mcp/src/hook
git commit -m "refactor(hook): dedup cargo-scanner/consequence blocks; stage-extract pre/post; decompose next_seq, journey_record_post"
```

---

### Task 6: `syntax/rust.rs` — module split + per-node-kind helpers (file-loc hit 2 of 2; −2 let, −3 mut hits)

**Files:**
- Create: `crates/phronesis-mcp/src/syntax/rust/{mod,walk,derives,counts,signatures,docs,assertions,eval}.rs`
- Delete: `crates/phronesis-mcp/src/syntax/rust.rs`
- Unchanged: `syntax/mod.rs:10` (`pub mod rust;`) and its sole call `rust::extract(content)` at `syntax/mod.rs:24`.

**Interfaces:**
- Frozen: `pub fn extract(content: &str) -> SyntaxFacts` in `rust/mod.rs`. The six current `pub(crate)` extractors have zero external callers (verified) — they become `pub(super)`; optionally keep `pub(crate) use` re-exports in mod.rs for zero risk.

- [ ] **Step 1: Split with zero code changes**

Destination map (source lines):

| Destination | Items |
|---|---|
| `rust/mod.rs` | module doc (1–2); `mod` decls; `pub fn extract` (38–78) |
| `rust/walk.rs` | `walk_function_items` (474–502), `walk_struct_items` (183–209), `function_name` (823–828), `is_test_fn` (606–627), `is_pub_fn_node` (643–654) — all `pub(super)` |
| `rust/derives.rs` | `extract_struct_derives` (80–128), `collect_derives_from_attr` (130–181); tests 1280–1340 |
| `rust/counts.rs` | clone group (211–269) + let group (271–404); tests 1137–1233, 1441–1621 |
| `rust/signatures.rs` | `PUB_FN_QUERY` (12–22), `FN_QUERY` (24–36), `extract_function_param_types` (406–449), `extract_async_functions` (451–472), `extract_public_functions` (504–531), `extract_result_string_returns` (830–877); tests 883–1135 |
| `rust/docs.rs` | 533–604 + `is_inside_trait_impl` (629–641); tests 1235–1278 |
| `rust/assertions.rs` | `ASSERTION_MACROS` (656–667), 669–734; tests 1342–1372, 1429–1439 |
| `rust/eval.rs` | 736–821; tests 1374–1427 |

Every test calls the public `extract()` — each submodule's test mod imports `use crate::syntax::rust::extract;`, zero assertion changes. Distribute tests as mapped so no file approaches 800 LOC.

Gate (split must be invisible: `cargo test --workspace`, syntax_integration.rs 26 tests), then audit check: `--rule audit-file-loc-high` → **no rows at all** (both file-loc hits now cleared). Commit:

```bash
git add crates/phronesis-mcp/src/syntax
git commit -m "refactor(syntax): split rust.rs into per-extractor modules (<800 LOC each)"
```

- [ ] **Step 2: Decompose the three flagged functions**

- `signatures.rs::extract_result_string_returns` (12 let, 6 mut): extract `fn result_string_offender(m: &QueryMatch, source: &[u8]) -> Option<String>` — the capture-unpack + filter ladder (source 840–874); outer fn becomes query-iterate-collect (≤3 lets).
- `derives.rs::collect_derives_from_attr` (8 let, 4 mut): extract `fn attr_is_derive(inner: Node, source: &[u8]) -> bool` (145–161) and `fn push_token_tree_idents(args: Node, source: &[u8], struct_name: &str, out: &mut Vec<(String, String)>)` (172–180 — match the real accumulator type).
- `signatures.rs::extract_public_functions` (5 mut): extract `fn pub_fn_name(m: &QueryMatch, source: &[u8]) -> Option<String>` (513–528).

- [ ] **Step 3: Gate, audit check, commit**

Full gate; `grep 'syntax'` on both let-rule audits → no output. All 744 lines of rust.rs tests still passing in their new homes.

```bash
git add crates/phronesis-mcp/src/syntax
git commit -m "refactor(syntax): per-node-kind helpers for result-string/derives/pub-fn extractors"
```

---

### Task 7: `rules_file.rs` (−4 let, −2 mut hits)

**Files:**
- Modify: `crates/phronesis-mcp/src/rules_file.rs`

Per-function treatment (helpers private; existing unit tests named below are the net — run them before and after):

| Target | Treatment |
|---|---|
| `WhenClause::deserialize` (22–87, 16 lets) | extract `fn parse_or_clause(val: &Value) -> Result<WhenClause, String>` and `fn parse_leaf_clause(key: &str, val: &Value) -> Result<WhenClause, String>` (the 50–86 arm); deserialize maps `String` errors to `D::Error` at the boundary. Tests: `v2_leaf_condition_*`, `v2_script_condition`, `v2_or_clause`, `v2_clause_rejects_malformed_inputs`. |
| `SourceRule::deserialize` (182–295, 11 lets) | extract `fn parse_when_field(obj: &Map<String, Value>) -> Result<Vec<WhenClause>, String>` (207–250, both v1/v2 arms) and `fn parse_then_field(obj: &Map<String, Value>) -> Result<DiskAction, String>` (253–283). Tests: `source_rule_parses_v2`, `source_rule_parses_v1_legacy`, round-trip. |
| `unfold_or` (472–556, 8 lets) | block-pattern the per-position phase (`let (position_alts, is_or_position) = { ... };`) + extract `fn cartesian_product(...)` (505–520). Tests: the six `unfold_*`. |
| `merge` (686–750, 10 lets, 6 mut) | extract `fn apply_in_memory(...) -> (BTreeMap<...>, usize, usize)` (692–714) and `fn ordered_merge(existing: &RulesFile, by_id: ...) -> Vec<DiskRule>` (724–740). Tests: the four `merge_*`. |

Gate; `grep rules_file` on both let-rule audits → empty. Commit:

```bash
git add crates/phronesis-mcp/src/rules_file.rs
git commit -m "refactor(rules_file): extract deserialize/unfold/merge stage helpers"
```

---

### Task 8: `server.rs` + `server_persistence.rs` (−7 let, −2 mut hits, incl. the `autoload` single)

**Files:**
- Modify: `crates/phronesis-mcp/src/server.rs`, `crates/phronesis-mcp/src/server_persistence.rs`

**Caution:** server.rs has **no in-file unit tests**; coverage is integration-only (`tests/extraction.rs`, `tests/save_rules_integration.rs`, `tests/bdd.rs`). Prefer the block pattern (behavior-preserving by construction) over helper extraction wherever it suffices.

| Target | Treatment |
|---|---|
| `fire_rules` (324–368) | block-pattern the extend+evict step (the existing `action_types` block at 341–353 is the house style to copy) |
| `extract_rules` (458–500) | block-pattern the validate/resolve/read prologue: `let content = { ... };` (462–470) |
| `save_rules` (505–549) | extract `fn validate_default_phase(phase: &str) -> Result<&str, McpError>` (509–515); block-pattern the in-memory snapshot (526–528) |
| `load_rules_file` (554–594) + `autoload` (server_persistence.rs 22–41) | one shared helper — `pub(crate) fn hydrate_rules(network: &mut ReteNetwork, phase_map: &mut HashMap<String, String>, rules: &[DiskRule], existing_ids: &HashSet<String>) -> (usize, usize)` (loaded, skipped) in server_persistence.rs; `load_rules_file` reports both counts, `autoload` discards. Match the real types at server.rs:570–583. |
| `get_stats` (684–727) | block-pattern the log-read: `let entries = { ... };` (700–706) |
| `audit_codebase` (732–815) | extract `fn resolve_scan_root(param: Option<&str>, project_root: &Path) -> PathBuf` (751–761) and `fn audit_snapshot_entry(report: &AuditReport) -> LogEntry` (the 774–801 closure content) |
| `extract_rules_from_markdown` (1086–1169) | extract per-line classification `fn classify_md_line(trimmed: &str, in_section: bool) -> Option<MdLineKind>` so the scanner loop only pushes; fence/section state stays in the loop (inherent scanner state, 3 muts → ≤2). Tests: `tests/extraction.rs` (405 lines) pins this thoroughly. |

Gate; `grep -E 'server(_persistence)?\.rs'` on both let-rule audits → empty. Commit:

```bash
git add crates/phronesis-mcp/src/server.rs crates/phronesis-mcp/src/server_persistence.rs
git commit -m "refactor(server): block-pattern MCP tool prologs; share rule hydration with autoload"
```

---

### Task 9: `diff_extract.rs` — the state-struct case (−1 let, −2 mut hits)

**Files:**
- Modify: `crates/phronesis-mcp/src/diff_extract.rs` (`rust_test_block_keep_mask` 175–242: 13 lets + 9 muts; `count_code_braces` 277–307: 3 muts)

**Interfaces:**
- Produces (private): `struct BraceScan { depth: i32, started: bool, in_str: bool }` with `fn consume_line(&mut self, line: &str) -> bool` (true when the block closes — wraps the existing `count_code_braces` mut-threading); `fn starts_in_str_table(lines: &[&str]) -> Vec<bool>` (the precompute pass 181–186, absorbing muts 2–3); `fn find_block_end(lines: &[&str], starts_in_str: &[bool], j: usize) -> Option<usize>` (absorbing muts 6–9: depth/block_started/k/in_str).

- [ ] **Step 1: Confirm the safety net, then refactor**

Run: `cargo test -p phronesis-mcp strip_` — the eight `strip_*`/`duplicate_function_names_deduped` tests pin this scanner, including the unbalanced-input and nested-brace edge cases. Then restructure: outer loop keeps only `keep: Vec<bool>` and cursor `i`; the attribute-skip cursor becomes `let j = skip_attrs(lines, marker_idx);` (extract the existing j-loop); block ranges come from `find_block_end`. `count_code_braces`'s own 3 muts are irreducible scanner state — fold `opens`/`closes` into `BraceScan::consume_line` and the standalone fn disappears (or stays as the struct's private core; either way the mut hit moves inside a ≤7-mut method).

Behavior note: `audit.rs` consumes this via `rust_test_block_keep_mask_for` (audit.rs:324–325) — signature frozen.

- [ ] **Step 2: Gate, audit check, commit**

```bash
git add crates/phronesis-mcp/src/diff_extract.rs
git commit -m "refactor(diff_extract): BraceScan state struct; precompute string-start table"
```

---

### Task 10: `memory_drift.rs` (−2 let, −1 mut hits)

**Files:**
- Modify: `crates/phronesis-mcp/src/memory_drift.rs`

| Target | Treatment |
|---|---|
| `parse_memory_file` (194–242) | extract `fn parse_frontmatter_fields(frontmatter: &str) -> (String, String, String)` (name, description, memory_type — lines 200–229; the 4 muts become helper-internal). Tests: `parses_well_formed_memory_file`, `returns_none_for_file_without_frontmatter`. |
| `score_entry` (324–413) | extract `fn best_rule_match(tokens: &..., rules: &RulesFile) -> Option<(f32, MatchedTarget)>` (351–368) and `fn best_durable_match(tokens: &..., durable_md: &str) -> Option<(f32, MatchedTarget)>` (376–397); add `impl DriftItem { fn unmatched(entry: MemoryEntry, bucket: ...) -> Self }` collapsing the three duplicate no-match constructions (330–335, 340–345, 406–411). Tests: the four drift tests via `run`. |

Gate; audit grep empty for the file; commit:

```bash
git add crates/phronesis-mcp/src/memory_drift.rs
git commit -m "refactor(memory_drift): extract frontmatter parse + scoring helpers"
```

---

### Task 11: journey cluster (−6 let hits)

**Files:**
- Modify: `crates/phronesis-mcp/src/journey/derive.rs`, `journey/mod.rs`, `journey/tagger.rs`, `crates/phronesis-mcp/src/journey_cli.rs`

| Target | Treatment |
|---|---|
| `derive.rs::scan_script` (242–289, 10 lets) | extract `fn parse_facts_call(script: &str) -> Option<(&str, Vec<String>)>` (the whole Option-chain parse); `scan_script` becomes parse → `record_pair` (≤3 lets) |
| `derive.rs::record_pair` (313–373, 11 lets) | extract `fn take_sel_k(args: &[String]) -> Option<(String, u32)>` (330–343) and `fn take_two_sel_k(args: &[String]) -> Option<(String, String, u32)>` (345–363) for the duplicated max-k arms |
| `derive.rs::assert_facts` (475–517, 8 lets) | block-pattern the read-bound derivation: `let read_n = { ... };` (490–506) |
| `mod.rs::current_sid` (33–81, 8 lets) | block-pattern sid generation: `let sid = { let ts = ...; let hex = ...; let date = ...; format!(...) };` (50–56) |
| `tagger.rs::fire` (151–210, 8 lets) | extract `fn tags_from_consequences(consequences: ...) -> Vec<String>` (194–208, takes both muts with it) |
| `journey_cli.rs::compute` (61–151, 8 lets) | extract `fn load_config_or_default(project_root: &Path) -> Result<TaggerConfig, JourneyCliError>` (75–92) and `fn rows_from_facts(facts: &[Fact], attribution: &...) -> Vec<JourneyRow>` (112–127) |

Net: existing unit tests (`scan_rules_*`, `current_sid_*`, `compute_surfaces_malformed_journey_config_as_config_error`) + integration (`tests/journey_derive.rs`, `tests/journey_tagger.rs`, `tests/journey_cli_integration.rs`, `tests/journey_hook_integration.rs`). Gate; audit grep empty for `journey`; commit:

```bash
git add crates/phronesis-mcp/src/journey crates/phronesis-mcp/src/journey_cli.rs
git commit -m "refactor(journey): extract parse/emit helpers across derive, tagger, cli"
```

---

### Task 12: the 8-let singles, batched (−5 let, −1 mut hits)

**Files:**
- Modify: `crates/phronesis-mcp/src/claude_md_drift.rs`, `context.rs`, `wiki.rs`, `outcomes/cargo.rs` (server_persistence.rs already handled in Task 8)

| Target | Treatment |
|---|---|
| `claude_md_drift.rs::extract_imperatives` (100–151) | extract `fn bullet_body(trimmed: &str) -> Option<String>` (handles `- `/`* `/numbered forms) + `fn is_imperative(body: &str) -> bool`; the loop becomes a filter_map chain — this also removes the admitted duplication at 133–148. Tests: `extracts_basic_imperative_bullets`, `extracts_numbered_list_imperatives`. |
| `context.rs::build_turn_body` (207–247) | extract `fn render_entry_bullets(entry: &LogEntry, now_secs: u64) -> Vec<String>` (211–235); assemble with `join`. Tests: the five `turn_body_*`. |
| `wiki.rs::parse_decision_file` (86–142) | extract `fn split_frontmatter(rest: &str) -> Result<(&str, &str), String>` (the fence-search loop, 107–128 — this is the 0.16.1-hardened fence logic; its 12 parser tests pin every edge). |
| `outcomes/cargo.rs::extract_from` (135–204, 9 let + 4 mut in `parse`) | `extract_from`: extract `fn outcome_tags(facts: &[OutcomeFact]) -> Vec<String>` (164–179) and `fn bug_caught_tags(project_root: &Path, subject: &str, output: &str) -> Vec<String>` (186–201). `parse` (72–93): extract `fn sum_test_results(output: &str) -> Option<(usize, usize)>` (79–89; `None` replaces the `saw_result` flag). Tests: the eight cargo adapter tests. |

Gate; commit:

```bash
git add crates/phronesis-mcp/src/claude_md_drift.rs crates/phronesis-mcp/src/context.rs crates/phronesis-mcp/src/wiki.rs crates/phronesis-mcp/src/outcomes/cargo.rs
git commit -m "refactor(mcp): block-pattern/helper cleanup for the 8-let singles"
```

---

### Task 13: Done-when verification + CHANGELOG

- [ ] **Step 1: Verify the design's done-when list, item by item**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
cargo run -q -p phronesis-mcp --bin phr-mcp -- audit
cargo run -q -p phronesis-mcp --bin phr-mcp -- trend
```

Expected audit end-state: `audit-file-loc-high` 0 hits; let-binding + let-mut hits total **8, all in `crates/phronesis/src/`** (`network.rs`, `script_evaluator.rs`, `beta_network.rs`, `production.rs`). Zero `//! phronesis-allow` markers added by this plan (if any were added, each must carry a written rationale and be listed in the PR description). `git diff v0.16.1 -- crates/phronesis/src/` must be empty (engine untouched). Trend shows the 59→8 drop.

- [ ] **Step 2: CHANGELOG + design-doc status**

Add under a new `## [Unreleased]` heading (version bump decided with the human at merge — behavior-preserving refactor, PATCH-shaped):

```markdown
### Changed
- **MCP-crate decomposition.** `hook.rs` (1764 LOC) and `syntax/rust.rs`
  (1622 LOC) split into focused submodules; `main`, `audit::run`/`run_profiled`
  (deduped via a shared core), and ~30 further functions decomposed below the
  let-count audit thresholds. Audit debt drops 59 → 8 hits; the remaining 8
  are core-engine functions deferred to the embedded-consumer-gated engine
  spec. Behavior-preserving; no public API changes. Implements
  `docs/superpowers/specs/2026-06-28-mcp-crate-decomposition-design.md`.
```

Update the design doc status line to `Implemented (plan: docs/superpowers/plans/2026-07-03-mcp-crate-decomposition.md)`, and mark `docs/specs/SPEC-god-file-decomposition.md` superseded for `server.rs`/`audit.rs` (add a status note pointing here; its `network.rs` guidance stays live for the deferred engine spec).

```bash
git add CHANGELOG.md docs/superpowers/specs/2026-06-28-mcp-crate-decomposition-design.md docs/specs/SPEC-god-file-decomposition.md
git commit -m "docs: changelog + spec status for MCP-crate decomposition"
```

- [ ] **Step 3: STOP — human review**

Present the branch (per-commit audit deltas make a good PR narrative: each commit is independently green and trend-non-increasing, so any commit is a rollback point). **No push without explicit approval**; if approved, PR per superpowers:finishing-a-development-branch.
