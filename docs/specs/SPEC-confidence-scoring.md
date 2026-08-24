# SPEC: confidence scoring — gating LLM output on grounded outcomes

**Status:** draft
**Authors:** Claude, Andrew Waterman
**Date:** 2026-06-18
**Target release:** phronesis-mcp 0.13.0 (MINOR — new fact family, new outcome
              adapter layer, new MCP tool, gate rules in init pack). No breaking
              change to the `phr` library crate.
**Depends on:** SPEC-journey-facts (0.12.0) — reuses the durable per-subject
              ledger + per-invocation re-derivation. Can ship without it by
              carrying its own minimal ledger, but is cleaner layered on top.
**Affects:** new `crates/phronesis-mcp/src/outcomes/` (mod, `adapter.rs`,
              `cargo.rs`, `facts.rs`, `bugs.rs`, `score.rs`),
              `crates/phronesis-mcp/src/{hook.rs, init.rs, main.rs, server.rs,
              server_params.rs}`. No `phr` change — confidence facts are
              ordinary `Fact`s; gate verdicts are ordinary `Consequence`s.

> **Amended by `SPEC-structural-rule-migration.md` §"Confidence gate
> severity":** the low-confidence band described below as a `block` was
> changed to `warn`. Incomplete or failing confidence evidence is now
> advisory at Git-mutation time — it never blocks the governed `git (commit|
> merge|rebase|cherry-pick|revert|pull)` commands. The design rationale, the
> signal derivation, and the medium/high bands below are otherwise still
> current; read `then: block` in the examples that follow as historical.

## North star (context, not scope)

The end goal is a *participatory* system: the rules engine, the human, and the
LLM collaborate to discover code patterns, suggest new ones, and eventually do
cross-language translation (OO → functional). phronesis is the **coordination
layer** between human intent, LLM suggestion, and pattern discovery — fuzzy
detection + LLM reasoning + human judgment, closing the loop iteratively. It is
explicitly *not* an automated analyzer that tries to extract perfect semantic
intent on its own.

None of that is trustworthy until phronesis can **gate LLM output** — detect
hallucination and score confidence before a suggestion reaches the human (an
invented CLI flag; a translation that silently breaks data flow). Confidence
scoring is the foundation the whole vision rests on. This SPEC builds the
narrow, grounded, verifiable first milestone and is designed so the
human-in-the-loop feedback path can attach later — without implementing that
loop yet.

## The milestone (narrow, objective, no ambiguity)

Score confidence on three **grounded outcome** signals, not on language syntax:

1. Does the suggested code **compile**?
2. Do the **tests pass**?
3. Does it **catch the known-bug case** (a TDD test that should be red on the
   buggy baseline and go green under the suggestion)?

compile + pass + catch-the-bug = high confidence. Failing any signal lowers the
score and flags the work for human review. The verdict gates the agent's
**done-claim / commit** (chosen gate point — see §6), so a low-confidence
suggestion cannot be presented to the human as finished.

## Three engine constraints this design is built around

Verified in the engine, because they determine what "thread a score through
RETE" can actually mean:

1. **`__script__` is not real Rhai.** `script_evaluator.rs` is a hand-rolled DSL
   supporting only `facts_contain('pred',[...])` and
   `facts_count('pred',[...]) >= N`. There is **no general arithmetic** — after
   `?n` substitution, `n >= 3` becomes `3 >= 3`, which is rejected. *(The
   `CLAUDE.md` "Rhai / `rank > 5`" description overstates the current
   capability; see Appendix.)* ⇒ **We do not compute a float in a condition.**
2. **Firing is single-pass; no automatic forward chaining.**
   `fire_all_consequences` drains the agenda built from asserted facts and
   collects `Consequence`s. A rule action does not assert a fact back into
   working memory and re-trigger matching. ⇒ **A confidence verdict must be a
   precomputed fact (host) or a direct count (DSL), matched by gate rules in the
   same pass — not derived rule-to-rule.**
3. **`HookPayload.tool_output` already exists and is unused** (`hook.rs:47`,
   kept deliberately for "a future post-check rule that wants to inspect tool
   output"). And `Consequence` already names `"compile.error"` as a canonical
   predicate. ⇒ **The grounding seam is already present; this SPEC fills it.**

## Architecture

```
  SLOW CLOCK — build/test tool calls (Bash: cargo test, …)
     │  post-check sees command + tool_output
     ▼
  ┌────────────────┐   command pattern → parser
  │ OUTCOME ADAPTER │── neutral facts ──► per-subject LEDGER (journey journal)
  │ cargo/pytest/…  │   build_outcome / test_outcome / bug_check_outcome
  └────────────────┘
                                          ▲ append, keyed by subject
  FAST CLOCK — edits accumulate a WORK UNIT (subject id)
                                          │
  GATE — pre-check on git commit / done-claim
     │  re-derive subject's outcome facts into a fresh ReteNetwork
     ▼
  signal_pass facts ─► confidence band (count DSL or host) ─► gate rule ─► Consequence (verdict + provenance)
```

Two clocks. Syntactic predicates run on the **fast clock** (every edit). Compile
and test are the **slow clock** — they *are* tool calls, and their **output is
the signal**. Confidence is a second subsystem that observes slow-clock output,
ledgers it per subject, and gates at the moment of presentation.

## 1. The subject: work unit (implicit now, explicit later)

Confidence is *about* a unit of work. Decision: **support both, implicit
first.**

- **Implicit work unit (milestone).** A monotonic `subject` id is minted at the
  first edit after a settled (green) state and carried until a build/test cycle
  settles it. Stored in `.phronesis/outcomes/current` (the "open work unit").
  Outcome facts and edits attach to it. Zero new agent behavior — confidence
  just happens. Attribution is coarse (the whole change set since last green),
  which is acceptable for the milestone.
- **Explicit suggestion (milestone step 4 / north star).** The LLM declares a
  unit via `submit_suggestion(subject, summary)`; confidence attaches to that
  id. This is the clean home for translation ("the OO→functional rewrite of
  module X is subject `xlate-7`"). The MCP tool both *opens* a subject and is a
  *gate query* against it.

Subject minting/settling lives in `outcomes/mod.rs`; the id is the key for every
ledger entry and every `*_outcome` / `signal_pass` fact.

## 2. Neutral outcome facts behind a thin adapter layer

The constraint "grounded outcomes, not language-specific syntax" resolves with
the **same shape `syntax/` already uses**: per-language tree-sitter modules emit
*neutral* facts. By analogy:

> **outcome adapters : confidence facts  ::  tree-sitter modules : syntax facts**

A registry maps a command pattern → parser. Each adapter parses one toolchain's
output and emits the **same neutral facts**; rules never name `cargo`.

| Neutral fact | Args | Source |
|---|---|---|
| `build_outcome` | `[subject, status]`  status ∈ `pass`/`fail` | compile step |
| `test_outcome` | `[subject, passed, failed, total]` | test step |
| `bug_check_outcome` | `[subject, bug_id, status]`  status ∈ `fixed`/`open`/`regressed` | known-bug test transition |

Adapter registry — built-in defs (`outcomes::toolchain::builtin_defs`);
anything else is a project def in `.phronesis/toolchains.json`:

| Adapter | Command pattern | Parses |
|---|---|---|
| `cargo` | `cargo (build\|check)` | error/warning summary → `build_outcome` |
| `cargo` | `cargo (test\|nextest)` | `test result: ok. N passed; M failed` / nextest `Summary […]` → `test_outcome` + per-test names for bug matching |
| `xcodebuild` | `xcodebuild …` | `file:line:col: error:` / `** BUILD FAILED **` → build fail; `** BUILD SUCCEEDED **` / `** TEST SUCCEEDED **` → build pass without an exit code; XCTest `Executed N tests, with M failures` and Swift Testing `Test run with N tests … passed/failed … with K issues` → `test_outcome` |
| `swift` | `swift (build\|test)` | same Swift patterns; `Build complete!` → build pass |
| *(project def)* `pytest`, `tsc` | shipped as examples by `init --packs confidence` | summary line → same neutral facts |

A command no def recognizes grounds nothing — it is not an error, it is
simply invisible to confidence. That is the failure mode a user sees as "tests
never registered": the run happened, but nothing parsed it. The remedies, in
order, are a built-in def, a project def, or the explicit `phr-mcp signal`
escape hatch below.

Only the adapter knows toolchain specifics; everything above is
language-neutral, so confidence generalizes the moment a second adapter lands.
Adapters are pure functions `(&str output) -> Vec<OutcomeFact>`, unit-tested
against captured fixtures — no process execution in the parser.

## 3. Representing confidence in *this* engine

Given constraint #1 (no arithmetic) and #2 (single-pass), confidence is a
**discretized band**, never a float in a condition. Three layered approaches:

### (A) Count-of-signals — milestone default, zero new engine primitives

Each passed signal asserts one atomic fact:

```
signal_pass(subject, "compile")
signal_pass(subject, "tests")
signal_pass(subject, "bug:1042")
```

Confidence band = how many passed, expressed with the **existing** `facts_count`
DSL. The gate derivation asserts **only the open subject's** `signal_pass` facts
into the otherwise-fresh network (§5), so the rules use the wildcard subject
`['*','*']` and need no `?s` binding — there is exactly one subject in scope:

```json
{ "id": "confidence-low-blocks-commit", "phase": "pre", "priority": 30,
  "when": [ { "bash_command_matches": "git (commit|merge|rebase|cherry-pick|revert|pull)" },
            { "__script__": "facts_count('signal_pass', ['*','*']) <= 1" } ],
  "then": { "warn": "Low confidence — compile/tests/known-bug evidence is incomplete or failing. Run `phr-mcp confidence` for the per-signal report before presenting this as done." } }

{ "id": "confidence-medium-warns-commit", "phase": "pre", "priority": 29,
  "when": [ { "bash_command_matches": "git (commit|merge|rebase|cherry-pick|revert|pull)" },
            { "__script__": "facts_count('signal_pass', ['*','*']) == 2" } ],
  "then": { "warn": "Medium confidence — one grounded signal is missing. Review before presenting as done." } }
```

3/3 = high, 2/3 = medium, ≤1 = low. At 3/3 neither rule fires and the Git
command proceeds. As of `SPEC-structural-rule-migration.md`, **both bands
`warn`** — the low-confidence rule's id and priority are unchanged from the
milestone-0.13.0 design, but its action changed from `block` to `warn` (see
the amendment note above). Maps 1:1 to the milestone and uses only predicates
that exist today (`bash_command_matches` is the real regex predicate;
`facts_count(...) <op> N` is the real DSL). `signal_pass` carries `[subject,
signal_name]`, so `['*','*']` counts the open subject's passed signals
regardless of name.

The aggregate count cannot distinguish "a signal never ran" from "a signal ran
and failed" — both leave the same `signal_pass` fact absent. Neither gate
message names a specific missing or failing signal for that reason; naming
one would claim more than the asserted facts prove. A future change may add
explicit per-signal status facts (`signal_missing` / `signal_failed`) to
support named messages, but that is a separately reviewed change, not part of
this milestone.

### (B) Host-computed band fact — escape hatch for weighting

When "count of passes" is too crude (a failed compile should dominate a passed
lint), `score.rs` computes the band in Rust and asserts a single fact
`confidence(subject, "low")`. Gate rules become pure equality matches. Scoring
*policy* lives in Rust/config; gating *policy* lives in `rules.json`. Same
discretized-band interface as (A), so gate rules are interchangeable.

### (C) Continuous scoring — deferred

Weighted/continuous scores (the north star's nuanced translation confidence)
need a `script_evaluator` upgrade behind the trait seam its own module doc
anticipates ("a future richer scripting layer … would plug in behind a trait").
Out of scope; noted so the band interface in (A)/(B) is forward-compatible.

**Ship (A); document (B) as the escape hatch; flag (C).** All three reduce
confidence to a fact the gate matches in one pass — respecting constraints #1
and #2.

## 4. Known-bug registry (the TDD signal)

A manifest of tests that *should* be red on the buggy baseline:

```json
// .phronesis/bugs.json
[ { "bug_id": "1042", "test": "auth::tests::rejects_expired_token",
    "status": "open", "baseline": "red" } ]
```

Semantics that make the signal trustworthy:

- A known-bug test is **red on the buggy baseline** (it genuinely detects the
  bug). The `cargo` adapter reads per-test results, and `bugs.rs` matches them
  against the registry.
- The suggestion earns `bug_check_outcome(subject, "1042", "fixed")` →
  `signal_pass(subject, "bug:1042")` **only when** that test goes red→green
  **and** `test_outcome` shows no new failures (no regression).
- `regressed` = a previously-green test now failing ⇒ strong negative signal,
  forces low band regardless of count.

**Honest caveat (documented, not solved in milestone):** a test can go green for
the wrong reason (unrelated change, weakened assertion). Milestone defense =
require zero regressions alongside the fix. Phase-2 hardening = run the bug-test
against *both* baseline and suggestion to confirm the transition, rather than
trusting a single post-suggestion run.

## 5. Persistence + where the gate acts

### Persistence — reuse the journey ledger

The compile fact, the test fact, and the edits happen in **different stateless
hook invocations**. To score them together they must be durable and re-asserted
at gate time — exactly the SPEC-journey-facts mechanism. Outcome adapters append
`*_outcome` events to the journal keyed by `subject`; at the gate-check
invocation the derivation reads the open subject's events and asserts the
`signal_pass` / `confidence` facts into the otherwise-fresh network. Confidence
scoring is the **first serious consumer** of the journey substrate and validates
it. (If journey-facts hasn't shipped, `outcomes/` carries a minimal
`.phronesis/outcomes/<subject>.jsonl` ledger with the same flock discipline as
`action_log.rs`; swap to the shared journal when available.)

### Gate point — advise on the done-claim / commit (amended)

By post-check the edit is already on disk; you cannot un-write it. So the gate
fires at the **moment of presentation**, not the moment of editing:

- **Primary (milestone):** a **pre-check on the governed Git mutations**
  (`git (commit|merge|rebase|cherry-pick|revert|pull)`; and the existing
  "report done" surface). Re-derive the open subject's signals; if the band is
  low (or any `regressed`), **warn** — per
  `SPEC-structural-rule-migration.md` §"Confidence gate severity", incomplete
  or failing confidence evidence is advisory, not a block, so the command
  always proceeds. This dovetails with the existing `llm` pack's "verify
  before done" rules — confidence is the grounded teeth behind that nudge.
- **Explicit:** `submit_suggestion(subject)` returns the structured report and
  flags low-confidence subjects before the human sees them.

Wiring in `hook.rs`:

- **Outcome capture** — in `run_post_check`, when `tool_name` is a command tool
  and the command matches an adapter pattern, parse `payload.tool_output`,
  append outcome facts to the ledger under the open subject, and (approach B)
  recompute the band. Fail-open: a parse miss never blocks.
- **Gate** — in `run_pre_check`, when the command matches the governed Git
  mutation scope (or the done-claim surface), load the open subject,
  re-derive its `signal_pass`/`confidence` facts via `outcomes::derive`, bind
  `?s` = subject, then the existing rule-fire path applies. A low or medium
  verdict exits 1 (warn) without naming a specific signal — the aggregate
  count can't prove which one is missing or failing; run `phr-mcp confidence`
  for that detail.

## 6. The human-in-the-loop seam — built *for*, not built

Design for the feedback path; don't implement the loop. Concretely:

- Every confidence verdict is emitted as a **`Consequence` carrying
  `Provenance::RuleFiring.bound_facts`** — the exact outcome facts that produced
  the band. The verdict is *explainable and traceable* by construction.
- `submit_suggestion` returns `{ subject, band, signals_passed,
  signals_missing, evidence: [fact ids] }` — a structured report, a natural
  `ConsequenceKind::Snapshot`.
- A future correction ("this was actually fine" / "you missed a real break")
  attaches to that provenance and, when the loop is built, tunes scoring weights
  (approach B) or adds a bug-test (the registry). **This SPEC ships the
  traceable report; it does not ship the correction handler.**

## CLI & MCP surface

```
phr-mcp confidence                 # band + signals for the open work unit (table)
phr-mcp confidence --subject <id>  # for a specific subject
phr-mcp confidence --json
phr-mcp signal tests pass|fail     # explicit escape hatch: journal the outcome
phr-mcp signal compile pass|fail   #   for the open unit (minting one if needed)
```

`signal` exists for test runners with no toolchain def and for runs that
happened outside the hook. It writes the identical `outcome:*` journal tag the
post-check hook stamps, so derivation (latest-of-each-kind wins) and the commit
gate cannot distinguish it from a hook-captured run. It refuses when the
`confidence` pack is not enabled, and accepts only `compile` and `tests`
(known-bug signals stay grounded in the registry).

MCP: `submit_suggestion(subject, summary)` (open + gate query) and
`get_confidence(subject?)` (read-only report). No tree-sweep variant —
confidence is about a live work unit's grounded outcomes, not a static scan.

`init` changes (opt-in `--packs confidence`):
- Starter `.phronesis/bugs.json` (empty array + commented example).
- Confidence gate rules in `rules.json` (the `confidence-*` rules above).
- `.phronesis/outcomes/` added to the ignore set (local state); `bugs.json`
  **tracked** (project knowledge, like `rules.json`).

## Module layout

| File | Responsibility |
|---|---|
| `src/outcomes/mod.rs` (new) | `Subject` minting/settling, public `derive` (subject → `signal_pass`/`confidence` facts), errors. |
| `src/outcomes/adapter.rs` (new) | Adapter trait + registry (command pattern → parser); neutral `OutcomeFact`. |
| `src/outcomes/cargo.rs` (new) | cargo build/test/nextest output parser → neutral facts + per-test results. |
| `src/outcomes/bugs.rs` (new) | `bugs.json` registry; match test results → `bug_check_outcome`. |
| `src/outcomes/facts.rs` (new) | Neutral fact constructors; signal-fact derivation; band discretization (approach B). |
| `src/outcomes/score.rs` (new) | Host-side weighting policy (approach B); band thresholds. |
| `src/hook.rs` (modified) | post-check: capture `tool_output` → ledger. pre-check: gate `git commit`/done-claim on derived band. Fail-open. |
| `src/init.rs` (modified) | `--packs confidence`: `bugs.json`, gate rules, gitignore. |
| `src/main.rs` / `server*.rs` (modified) | `confidence` subcommand; `submit_suggestion` + `get_confidence` MCP tools + params. |

No `phr` change: outcome/signal/confidence are ordinary `Fact`s; verdicts are
ordinary `Consequence`s with existing provenance.

## Testing strategy

| Layer | Tests |
|---|---|
| cargo adapter | parse pass/fail summaries; `test result:` counts; per-test names; warning-only build = pass; compile error = fail; captured-fixture round-trips |
| bug registry | open test red→green ⇒ `fixed`; still red ⇒ `open`; previously-green now red ⇒ `regressed`; unknown test ignored |
| band derivation | 3 signals ⇒ high; 2 ⇒ medium; ≤1 ⇒ low; any `regressed` ⇒ low regardless of count (approach B); count DSL path (approach A) end-to-end |
| ledger / subject | mint on first edit after green; carry across invocations; settle on build/test; flock serialization (mirror `action_log_concurrency`) |
| gate (hook integration) | low and medium bands warn on the governed Git mutations (exit 1, no named signal — the aggregate count can't distinguish missing from failed); high band passes clean (exit 0); an unrelated blocking rule still exits 2; fail-open on unparseable output / missing ledger; `PHRONESIS_NO_*` disables |
| provenance | verdict `Consequence` carries the outcome fact ids that produced it |
| MCP | `submit_suggestion` opens subject + returns report; `get_confidence` read-only mirror |

## Commit plan

1. **`feat(outcomes): cargo adapter + neutral outcome facts`** — `adapter.rs`,
   `cargo.rs`, `facts.rs`, fixtures. No hook wiring.
2. **`feat(outcomes): per-subject ledger + signal derivation`** — `mod.rs`
   subject lifecycle, ledger (or journey-journal reuse), `derive`,
   count-based band (approach A). Test-harness driven.
3. **`feat(outcomes): gate git commit / done-claim on confidence band`** —
   `hook.rs` post-check capture + pre-check gate, fail-open, integration tests.
4. **`feat(outcomes): known-bug registry + bug_check signal`** — `bugs.rs`,
   `bugs.json`, red→green/regression semantics, 3/3 high path.
5. **`feat: submit_suggestion + get_confidence MCP tools; confidence CLI; init
   pack; bump 0.13.0`** — explicit-subject path, structured report with
   provenance, CLAUDE.md docs, version bump.
6. **(later)** host-weighted bands (B); second adapter (pytest) to prove
   neutrality; baseline-vs-suggestion bug confirmation; continuous scoring (C).

Commits 1–2 are pure additions (no behavior change). Commit 3 is first
user-visible gating. Commit 5 is the release; commit 6 the generalization toward
the north star.

## Open questions

- **Subject boundaries.** "Since last green" is clean when builds happen
  regularly; a long edit streak with no build leaves a large, coarsely-attributed
  unit. Should a stale open subject expire / force a build prompt? (Ties to a
  journey `journey_since(["build","calls"])` rule.)
- **Done-claim surface beyond `git commit`.** Claude Code has no single "I'm
  done" tool call. `git commit` is the reliable proxy; should we also gate on
  the `llm` pack's completion-claim patterns? Probably yes — reuse
  `collect_bash_command_patterns`.
- **Where weighting policy lives (approach B).** Hardcoded Rust vs. a
  `confidence.json` weights file (mirrors the `journey.json` precedent). Defer
  until (A) proves insufficient.
- **Multi-language suite attribution.** A repo that runs `cargo test` *and*
  `pytest` for one subject needs both adapters' outcomes merged under the same
  subject — the neutral-fact design already supports this; the registry/ledger
  keying must not assume one toolchain.
- **`__script__` honesty.** This SPEC leans on `facts_count(...) >= N`, which is
  real. The broader "Rhai" claim in `CLAUDE.md` should be corrected, and the
  continuous-scoring north star (C) is the trigger to actually add an expression
  layer behind the evaluator's trait seam. Track as its own task.

## Appendix — `__script__` capability (as verified)

`script_evaluator.rs` supports exactly:
- `facts_contain('predicate', ['arg', '*', …])` — existence with `*` wildcard.
- `facts_count('predicate', ['arg', '*', …]) <op> N` — `op ∈ >=,>,==,<=,<`,
  `N` a non-negative integer.
- leading `!` negation; `?var` substitution from bindings.

It does **not** support arbitrary arithmetic, float comparison, boolean
combinators (`&&`/`||`), or string ops. Any design that needs those must either
discretize to facts (this SPEC's approach) or extend the evaluator (deferred).
