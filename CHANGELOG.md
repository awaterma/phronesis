# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This project is
pre-1.0: while `0.x`, MINOR versions may carry breaking changes.

## [0.12.0] - 2026-06-19

### Added
- **Confidence scoring (first milestone)** — gate LLM output on three grounded
  outcomes before a `git commit`: does it compile, do the tests pass, does it
  catch a known bug (a TDD test red on the buggy baseline that goes green).
  See `docs/specs/SPEC-confidence-scoring.md`.
  - Domain-neutral outcome facts (`build_outcome`, `test_outcome`,
    `bug_check_outcome`) behind a per-toolchain **adapter** layer (`cargo`
    first; pytest/tsc/go later emit the same neutral facts).
  - A per-subject **ledger** (`.phronesis/outcomes/<subject>.jsonl`) bridges the
    stateless hook invocations; the pre-check re-derives `signal_pass` facts and
    gate rules count them with the existing `facts_count(...)` DSL
    (`<=1` blocks, `==2` warns, 3 passes clean).
  - Post-check parses a build/test command's captured output into the ledger;
    a `git commit` settles the open work unit.
  - Known-bug registry in `.phronesis/bugs.json`.
  - `phr-mcp confidence [--subject <id>] [--json]` — read-only band/signals
    report for the open work unit.
  - **Opt-in per project** via `.phronesis/confidence.json`; fail-open
    throughout, so projects that haven't enabled it are unaffected.

## [0.11.0] - 2026-06-13

### Added
- **Public fact-query API** on `ReteNetwork` — `facts_snapshot`,
  `facts_matching_predicate`, `facts_matching_predicates` (predicate-set
  membership), `facts_matching` (positional-arg filters), `fact_ids_matching`,
  `get_fact_by_id`, `fact_count`. Sync, owned results sorted by fact id, so
  embedding hosts need not reach into `wme_manager`.
- **Richer `list_facts` MCP tool** — the existing `predicate` filter plus new
  `predicates` (set membership) and `arg_filters` (positional `arg = value`)
  params, backed by the fact-query API. Lets coding agents query working
  memory by predicate set or argument, not just list-all.
- **`bash_command_matches` predicate** — regex rules over Bash/command-tool
  text, gated to command tools (file content quoting the same text never
  fires). Ships two LLM-pack guard rules (stage-explicitly, don't-kill-build).
- **Tree-sitter AST predicates for Python and TypeScript** — Python:
  `python_bare_except`, `python_mutable_default_arg`,
  `python_function_param_count_high`, `python_function_missing_docstring`;
  TypeScript (TSX grammar included): `ts_explicit_any`,
  `ts_non_null_assertion`, `ts_suppression_comment`,
  `ts_function_param_count_high`.
- **Silent zero-result audit diagnostics** — `phr-mcp audit` and the
  `audit_codebase` tool now explain a no-hits result when the cause is
  recoverable (no rules carry `audit: true`, or the walker scanned 0 files)
  instead of returning an empty shape indistinguishable from a failure.
- **CI** — GitHub Actions workflow (fmt + clippy `-D warnings` + tests, on
  MSRV 1.90 and stable).
- Typed-error, retraction-semantics, and salience-order test suites.

### Changed (breaking)
- **`Result<_, ReteError>` replaces `Result<_, String>`** across the engine
  crate. `ReteError` is a matchable enum (`FactNotFound`, `LockPoisoned`,
  `DuplicateFactId`, `BindingConflict`, …) implementing `std::error::Error`;
  `From<ReteError> for String` eases migration for string-carrying hosts.
- **Duplicate fact ids are rejected.** Asserting an id already present with
  *different* content errors (`DuplicateFactId`); an identical re-assert is an
  idempotent no-op. Previously a duplicate silently corrupted the predicate
  index (the same fact was returned twice from `get_by_predicate`).
- **Same-salience agenda items fire in FIFO (insertion) order.** Previously
  tie order was `BinaryHeap`-arbitrary; firing order is now deterministic.

### Deprecated
- **`ReteNetwork::get_persistent_facts`** — it hardcodes consumer-specific
  predicates, which don't belong in a domain-neutral engine. Define your own
  predicate set and call `facts_matching_predicates(&YOUR_SET)`. Slated for
  removal in 0.12.

### Fixed
- **Retraction purges stale agenda items** referencing the retracted fact, so
  a pending rule can no longer fire against a fact that is no longer true.
- **Refraction keys compare exact WME ids** — retracting `f1` no longer
  clobbers the refraction state of `f10` (was a substring match).
- **`get_memory_drift`** marks guidance *actionable* only when it maps to an
  expressible predicate (named command, file/path/code shape, or function
  shape); operational prose is bucketed *ambient*. Actionable entries now also
  register coverage from `durable.md`, so the drift list converges.

## Earlier releases

Pre-0.11 history (0.10.0 and earlier) is recorded in the git log and
`docs/specs/`. Notably, 0.10.0 added wiki-drift, the block-pattern rules, and
the v2 rule schema.

[0.11.0]: https://github.com/awaterma/phronesis/releases/tag/v0.11.0
