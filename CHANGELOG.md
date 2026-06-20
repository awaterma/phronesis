# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This project is
pre-1.0: while `0.x`, MINOR versions may carry breaking changes.

## [0.13.2] - 2026-06-20

### Fixed
- **`bash_command_matches` taggers actually fire.** `journey::tagger::tagger_facts`
  built only file/content facts and relied on a "tagger regex pass" implied by a
  misleading comment but never implemented. The default `build` tagger
  (`{ "bash_command_matches": "cargo (build|check|test)" }`) silently no-fired on
  every `cargo` invocation. `tagger_facts` now walks `taggers[*].when[*]`
  (including nested `or` clauses) collecting `bash_command_matches` patterns,
  regex-matches each against the bash command, and asserts one synthetic
  `bash_command_matches:<pattern>` Fact per match — the same pattern
  `check_bash_command_patterns` uses for top-level rules
  (`hook_facts.rs:316`). Surfaced in a live playtest, not in unit tests.
- **`HookPayload.tool_output` accepts `tool_response` as a serde alias.** Claude
  Code's PostToolUse hook delivers Bash output under `tool_response`, not
  `tool_output`. Without the alias, the field was `None` / empty string, so
  `compiled("")` returned true (no error patterns match → spurious
  `outcome:compile_ok`) and `TEST_RESULT.captures_iter("")` returned nothing
  (`outcome:test_pass` never fired). Net effect: confidence-scoring was wedged at
  "low / compile" for every real `cargo` run, even when tests were green —
  the whole gate-by-band feature was non-functional in production. Tests and
  fixtures all passed because they synthesized payloads under `tool_output`;
  only a live hook payload surfaced it. Backward compatible with Gemini and
  existing fixtures.

## [0.13.1] - 2026-06-20

### Fixed
- **Same-day sid fallback collision.** When `.phronesis/journey/session` was
  missing, the journey fallback was the literal placeholder
  `s-YYYY-MM-DD-fallback`, collapsing distinct sessions to the same id. Now
  `journey::current_sid` reads-or-creates atomically in the
  `context::ensure_session_id` format (`s-YYYY-MM-DD-<6 hex>`); the placeholder
  is gone.
- **Triple-duplicated `current_sid` consolidated.** Three independent
  implementations (in `hook`, `main`, and `server::get_journey`) coalesced into
  a single `journey::current_sid(project_root)` helper. Same semantics, one
  source of truth.

### Changed
- **CLAUDE.md packs list now includes `confidence`** alongside `journey`. The
  scaffolded CLAUDE.md previously enumerated `journey` only.
- **`phr-mcp journey` nudges on empty config.** When `.phronesis/journey.json`
  is missing or empty, the CLI emits a stderr suggestion
  ("run `phr-mcp init --packs journey` to scaffold one") before falling back
  to an empty config. The hook stays silent — fail-open is advisory there, not
  user-facing.

### Fixed (engine)
- **Pure-script rules now fire.** Rules whose `when` was entirely `__script__`
  clauses had no alpha state, no terminal id, no p-state — they never reached
  the agenda, because `__script__` clauses are post-filters on activations and
  with no other clause there were no activations to filter. `update_agenda`
  now branches on `real_condition_count == 0` (count of non-`__script__`
  conditions per loaded rule) and, for pure-script rules, evaluates the script
  clauses against the current fact base with empty bindings, emitting an
  activation when every clause passes. Dedupe key is `<rule_id>` —
  fire-once-ever, the right semantics for threshold rules. Alpha/beta network
  and the production network shape are unmodified; mixed-script behaviour is
  unchanged. Surfaced by the journey-facts SPEC's headline
  `auth-churn-without-tests` rule, which is naturally two `__script__` clauses
  (`facts_count(...) >= 5` AND `facts_count(...) == 0`). The
  `journey_seen` anchor leaf added as a workaround is no longer required.

## [0.13.0] - 2026-06-20

### Added
- **Journey facts (new fact family + new hook stage)** — call-window and
  session-scale predicates that summarise *trajectory*, not the current diff.
  Five aggregators over project-defined tags (`journey_occurrence`,
  `journey_count`, `journey_seen`, `journey_since_ge`, `journey_distinct`)
  with windowed selectors (`5c` for last 5 calls, `30m`/`2h`/`7d` wall-clock,
  `s` for session; repo-lifetime `r` is phase 2). Rule-driven derivation;
  the journal is the substrate, the predicates are recomputed each cycle.
  See `docs/specs/SPEC-journey-facts.md`.
  - **Append-only journal** (`.phronesis/journey/events.jsonl`) writes a
    record per post-check with subject + tags + monotonic seq. Tail-read
    for hot queries (`SUFFIX_HARD_CAP = 10_000` lines) and per-subject read
    for outcomes folding.
  - **Taggers reuse the predicate engine** — `taggers[*].when` clauses are the
    same DSL as rule conditions. `bash_command_matches`, `new_content_contains`,
    `file_path_matches` all available. Project-defined via
    `.phronesis/journey.json`.
  - **Derivation pass** runs at every pre-check and post-check via
    `journey::derive::assert_facts`; selector validation rejects malformed
    journey config without exit-2.
  - **Outcomes ledger folded into the journey journal** (the notable storage
    change of 0.13.0). `outcomes/ledger.rs` is gone; `outcomes/cargo.rs` now
    returns `(tags, subject)` and the hook stamps them on a single journal
    record. `outcomes/derive::signals` reads via
    `journey::journal::read_recent_subject`. Confidence-scoring behaviour is
    byte-identical; the storage is unified.
  - **`SessionStart` stamps** `.phronesis/journey/session`; pre/post-check
    read it. `PHRONESIS_NO_JOURNEY=1` disables both paths. Fail-open
    throughout — corrupt `journey.json` or missing journal degrades to "no
    journey facts," never exit 2.
  - **`phr-mcp journey [--json] [--explain <rule-id>]`** renders the
    `journey_*` facts a derivation pass would assert against the current
    journal, with `--explain` filtering to a single rule's dependencies.
  - **MCP tool `get_journey`** mirrors the same table/JSON view so the agent
    can ask "what does my trajectory look like" mid-conversation.
  - **`phr-mcp init --packs journey`** writes a starter `journey.json` and
    ensures it is tracked.

### Changed
- **Workspace bumps to 0.13.0.** `phr` and `phr-mcp` move together; `phr-mcp`'s
  `phr` dep bumps to match.

### Notes
- **Coverage discipline.** The workspace stayed at or above the pre-feature
  baseline of 85.4% lines across the journey-facts merges; the journal and
  tagger modules sit near ~90%.

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
  - `phr-mcp init --packs confidence` — writes the commit-gate rules plus the
    `.phronesis/confidence.json` opt-in marker and `.phronesis/bugs.json`
    registry, and carves both back into `.gitignore` as tracked config.
  - MCP tools `get_confidence` (band/signals report) and `submit_suggestion`
    (declare an explicit work unit, e.g. a translation, and accrue signals to
    it).
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

## Known follow-ups (specs landed, implementation deferred)

- **`SPEC-gate-merge-commits.md`** — broaden the confidence gate's
  `bash_command_matches` pattern from `"git commit"` to
  `"git (commit|merge|rebase|cherry-pick|revert|pull)"`. Five of six
  commit-producing porcelain commands currently bypass the gate. Live-tested
  during the journey-facts merge night. PATCH-shaped change for 0.13.x.
- **`SPEC-pack-opt-in-facts.md`** — pack-level supersession via zero-arg
  marker facts. When `confidence` is opted in, assert `confidence_enabled`
  at hook fire (mirroring `clock_facts`) and condition
  `nudge-verify-before-commit` on its absence via the existing
  `facts_count(...) == 0` form. Removes the double-warn on every `git commit`
  for projects running both `llm` and `confidence` packs. PATCH for 0.13.x.

## Earlier releases

Pre-0.11 history (0.10.0 and earlier) is recorded in the git log and
`docs/specs/`. Notably, 0.10.0 added wiki-drift, the block-pattern rules, and
the v2 rule schema.

[0.13.2]: https://github.com/awaterma/phronesis/releases/tag/v0.13.2
[0.13.1]: https://github.com/awaterma/phronesis/releases/tag/v0.13.1
[0.13.0]: https://github.com/awaterma/phronesis/releases/tag/v0.13.0
[0.12.0]: https://github.com/awaterma/phronesis/releases/tag/v0.12.0
[0.11.0]: https://github.com/awaterma/phronesis/releases/tag/v0.11.0
