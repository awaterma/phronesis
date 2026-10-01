# Plan review consolidation — 2026-09-28-phronesis-evidence-gaps

Reviewers: GLM-5.3 (together.ai, text-only) and Codex gpt-6-luna (read-only against the repo at 636ba0b).
Orchestrator verified each factual claim against the code before accepting it.

## GLM-5.3 — APPROVE WITH CHANGES

| # | Finding | Verified | Disposition |
|---|---|---|---|
| C1 | Task C1 redirect parser contradicts its own test (`>&2` → `Some("2")`; quoted spaced target split) | Yes, by reading the plan's code | **Accept.** Replace the whitespace tokenizer with a quote-aware scanner; treat `>&N` / `N>&M` as fd-dups; share the unquote helper with B1. Test stays as written. |
| C2 | "Outside root" negative test lives under the OS temp dir, which is an allowed root; contingency also under /tmp | Yes | **Accept.** Put the outside file under `env!("CARGO_MANIFEST_DIR")`. Add the precedence pin (captured output wins). |
| C3 | A2 before A3 turns the repo audit red; CI runs `audit --fail-on block` | CI does run it (`.github/workflows/ci.yml:29`). The premise (init.rs still present) is stale: init.rs is gone since #112, so no red today. | **Accept the fold:** delete the stale ignore line in A2's commit; markers/doc_excepted in A3 may land in any order. |
| C4 | Excluded files now cost more than scanned ones; no cap | Yes | **Accept.** Structural scan of excluded files skips files over `MAX_FILE_BYTES_DEFAULT` and non-source extensions; document the trade-off. |
| C5 | `git -C` heuristic picks the wrong invocation (`git -C /x diff && git commit`); `git -C .` inside `cd wt` resolves to project root | Yes | **Accept.** Derive the directory from the head-moving segment: its own `-C` (resolved relative to any leading `cd`), else the leading `cd`. Add both compound cases to the B1 test. Log an unresolved-dir note for `sh -c` / subshells. |
| C6 | Redirect file may be stale (`false && cargo test > run.log`) | Yes | **Accept.** `ExtractFromInput.not_before: Option<u64>`; the hook passes the in-flight record's `ts` (`state::Inflight.ts`); a file with mtime < not_before is never read. `signal ingest` is an explicit operator act and passes the file's own mtime. |
| C7 | D1 adds only `properties.json`; MCP default false replays the gap; skip-worktree hides worker writes | Both `properties.json` and `toolchains.json` are tracked | **Accept a/b, accept c as documentation.** Derive the dirty set from `git -C <source> status --porcelain -- .phronesis/`; default `seed_tracked` to true on both CLI and MCP with `--no-seed-tracked` to opt out; document that the source checkout owns tracked `.phronesis` files during a swarm and workers never write them (they cannot promote properties anyway). |
| C8 | Unbound harnesses dropped, including FAILED ones; corrupt store → everything unbound | Yes | **Accept.** Journal unbound harnesses as `outcome:proof_unbound:<harness>:<passed|failed>`; on store load error, keep raw names and warn. |
| C9 | "Any order" is false: E3 uses C3; C3 and D1 both touch `harness-workers.md` | Yes | **Accept.** State the E3→C3 dependency; serialize the agent-skills edits into D1's commit. |
| C10 | `&Vec<PathBuf>` closure param trips `clippy::ptr_arg`; A2 closure type mismatch | Yes | **Accept.** Use `&[PathBuf]`; inline the two scan loops. |
| C11 | Hybrid rules (AST + lexical predicate) run fully on excluded files | Yes | **Accept as policy.** Classification is per rule: any AST predicate makes the rule structural. Pin with a test and document. |
| C12 | A3 test pins text, not behaviour | Yes | **Accept.** Add a binary-run assertion that `audit --rule <id> --json` does not list `rules_rust.rs`. |
| C13 | `^cargo kani` may miss `cd … && cargo kani`; harness charset too narrow | Matching is per segment (`toolchain.rs:255`), so the anchor is fine. Charset point stands. | **Accept charset only:** `(?P<name>\S+?)\.\.\.`. |
| C14 | Pre/post probe roots can diverge | Yes | **Accept.** Persist `probe_root` on `Inflight` at push; pop uses the stored value. |
| Nits | footer list unbounded; `f.args[1]` index; `UnknownSignal` orphan; B3 test unwritten | `UnknownSignal` is still used (`outcomes/mod.rs:94,147`), so no orphan | **Accept** footer truncation (count + first 5), `get_mut`, and writing B3's test in full. |

Answers to GLM's questions: (1) yes, CI runs the blocking audit; (2) per segment; (3) yes, via `Inflight.ts`; (4) the source checkout owns tracked `.phronesis` files during a swarm, workers do not write them, MCP defaults to seeding; (5) no: unbound FAILED results are journaled under `proof_unbound`; (6) D1 lands the agent-skills edit, C3 references it; (7) it is tracked and is now covered by the git-status-derived dirty set.

## Codex gpt-6-luna — (pending)

## Codex gpt-6-luna — APPROVE WITH CHANGES (read-only against 636ba0b)

| Finding | Verified | Disposition |
|---|---|---|
| Critical C2: redirect attribution across pipeline segments (`cargo test \| tee run.log` splits into two segments); whitespace tokenizer unsafe | Yes (`segment.rs:39` splits on `\|`) | **Accept.** Redirect in the handled segment or `tee` in the very next segment; quote-aware scanner (shared with GLM C1). |
| Critical C2: TOCTOU/symlink/FIFO; temp dir too permissive | Yes | **Accept.** Project root only; `symlink_metadata` regular-file check; `security::read_file_capped`; symlink/FIFO/outside-root tests. |
| Critical E1: `(?s).*?` can pair a header without a verdict with the next verdict; fixture synthetic | Yes; `regex` has no lookahead | **Accept.** New `ToolchainDef.section_start`; per-section `per_test`; fixture with an unfinished harness. Real-output claim is from the orchestrator's own Kani log (`kani-merged-add_binding_totality.log`), noted. |
| Major A1: second walk may report `.gitignore`/hidden exclusions | **Rejected**: both walks honour `.gitignore` and hidden defaults, so the set difference is `.phronesisignore` policy only | Pinned anyway with a `.gitignore`-overlap and hidden-file test. `is_none_or` is 1.82, within MSRV. |
| Major A2: mixed rules; unsorted paths; `files_scanned` semantics | Yes | **Accept.** Per-rule classification stated and tested; explicit sort; `files_scanned` counts excluded files and the docs say so. |
| Major A3: `audit-string-concat-with-plus` lacks `doc_excepted`; test never runs the audit | Yes (`rules_rust.rs:280-289`) | **Accept.** Add `doc_excepted`; the test runs the binary per rule. |
| Major B1: `git -C` regex too permissive; `cd` parser incomplete | Yes | **Accept.** Head-moving segment's own `-C`; absolute paths only; `None` on unmodelled syntax; comment/string/subshell/`sh -c` cases tested. |
| Major B2: relative paths against the wrong cwd; `CommitRow` has only `sha`/`band` | Yes | **Accept.** Absolute only; B3 updates every output representation with a written test. |
| Major C3: `ingest` journals with no tags; ambiguous provenance tag; wrong error variant | Yes (`SignalError` = NotEnabled/UnknownSignal/Subject/Journal) | **Accept.** `NoToolchain`, `NoOutcome`; nothing journaled on empty; `outcome:ingested` distinct from `outcome:output_from_file`; tests for nonzero exit, empty, unknown toolchain. |
| Major E2: dropping unbound facts vs `proof_run_outcome` stale-pass retraction; `unwrap_or_default` on the store | Yes (`toolchain.rs:375-407`) | **Accept.** Unbound results become `proof_unbound` facts (never dropped); run-level fact untouched; stateful test mirroring `derive.rs:565-600`; store load errors reported. |
| Major E3: `confidence` may not surface `proof` | **Rejected**: `derive.rs:228` grounds `signal_pass(_, "proof")` and `:526-534` asserts the band lifts | E3 keeps the e2e check and says to file a bug if it ever fails. |
| Major D1: three-way check; `update-index` result ignored; conflicts | Design-only review (repo not visible) | **Accept.** Seed only when worktree == HEAD; conflicts refused and named; `update-index` failures reported; `--no-skip-worktree` documented. |
| Minor A4 wording; C1 tokenization; E1 registry loading; global MSRV/rustfmt notes | Yes | **Accept** all. |

## Part F (added after the reviews)

The user reported the same afternoon that a Python project's coverage never connects to its graph. Verified: the graph is multi-language but coverage is Rust-only (`collect.rs:88-98` `.rs` filter, `region_map.rs:427` tree-sitter Rust only). Part F (Python function regions, lcov-dir import, pytest per-test loop with a devcontainer path, `.py` changed regions, docs) was added to the plan and has **not** been through the two reviewers; it should be before execution.

## Part F review (2026-09-29): GLM-5.3 and Codex, both APPROVE WITH CHANGES

| Finding | Source | Verified | Disposition |
|---|---|---|---|
| Hit rule counts the `def` line coverage.py marks at import time; the plan's own fixture (`save`: `DA:4,1`, `DA:5,0`) could not pass | both | Yes (fixture shape) | **Accept.** Hit = executed body line (`body_start_line..=end_line`); one-line bodies decided by `FNDA`, else reported `unattributable`. `FunctionSite.body_start_line` added for Rust and Python. |
| `LcovSource.function_hits` never read → clippy fails | GLM | Yes | **Accept.** Consumed by the one-liner rule. |
| Unresolved / filtered `SF` paths dropped silently; CHANGELOG claim false | GLM | Yes | **Accept.** Summary counts and names `unresolved_sf`, `ambiguous_sf`, `filtered_sf`, `no_regions`, `unattributable`. |
| `relativize` takes the first existing suffix, not the longest; collisions | both | Yes | **Accept.** Longest suffix; `Ambiguous` refuses; `..` is `Missing`. |
| Host stamps a revision for container-measured lines | both | Yes | **Accept.** Collection script writes `manifest.json` (container HEAD + per-file SHA-256); import refuses on mismatch; `--no-manifest` downgrades the tool label. |
| Parametrized node ids break file-stem ids; shell quoting | both | Yes | **Accept.** `TN:` always written; stems sanitized; ids shell-quoted; parameter suffix dropped for the graph id, kept in a provenance comment. |
| pytest paths untested; CI has no Python | both | Yes (`ci.yml` has no Python step) | **Accept.** Committed `--collect-only -q` fixture; live runs gated on `python3`. |
| `async def` may be skipped | GLM | **Rejected as a bug**: tree-sitter-python 0.25 folds `async def` into `function_definition` and `graph/python.rs` tests it | Test case added anyway. |
| `hit_kind: "function"` rejected by the store; store path wrong in the test | Codex | Yes (`store.rs:440-475`, `:110-113`) | **Accept.** `hit_kind: "region"`; `.phronesis/coverage.jsonl`. |
| Test id shape is `python:<ns>::tests::test_utils::test_load`, not dotted modules | Codex | Yes (`graph/python.rs:543-550`) | **Accept.** Shared `qualified_test_id`; exact-id assertions. |
| Empty import must refuse and leave the store | Codex | Yes | **Accept.** |
| `select` needs a rendering path, not a display tweak | Codex | Yes | **Accept.** `command` in `--json` and the table, from the `defines_test` edge. |
| Ordinals live in `function_sites_by_node:432-458`, not `extract_sites` | Codex | Yes | **Accept.** Factored into `assign_function_ordinals`, shared. |
| `end_position()` column 0 lands on the next row | Codex | Plausible; probe first | **Accept.** `inclusive_end_line` rule; probe step in F1. |
| F4 must also cover `properties/hydrate.rs:68-80` | Codex | Yes | **Accept.** Dispatch inside `region_map::changed_regions`, tests for both consumers. |
| No Python BDD scenario | both | Yes | **Accept.** F5 adds one on the committed fixture. |
| Memoize sites per file; dedupe policy; `tests/` exclusion via the classifier | GLM/Codex | Yes | **Accept.** Cache in `records_from_lcov_dir`; existing exact-duplicate dedupe documented; classifier reused. |
