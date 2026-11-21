---
id: untested-risky-call
date: 2026-07-29
status: accepted
enforces: [warn-untested-risky-call]
superseded_by: null
tags: [code-graph, structural-rules, test-coverage]
---

# Warn when production code adds a panic-capable call with no direct test

## Context

The structural code graph (SPEC-triple-store-rete.md) can answer "which
functions call a watched panic-introducing API and have no direct test?"
with zero token cost at the hook boundary. But the raw ingredients are
noisy: on this repository `untested` alone flags 695 functions — 61% of
all functions — because the direct-call heuristic deliberately misses
transitive coverage (§4.4). A rule on bare `untested` would mostly report
"covered only transitively", which is true but not actionable.

## Decision

Ship the *composed* signal, not the ingredient: warn only when the edited
production file defines a function that both calls a watched API
(`unwrap`, `expect`, `panic!`, `todo!`, `unimplemented!` — closed
watchlist, `.phronesis/graph.toml`) and has no direct test. Measured on
this repository the composition fires 6 times total, with a hand-audited
true positive (spec §10).

Three scoping choices are part of the decision:

- **Warn, never block**, until a false-positive rate is measured on a
  second corpus (spec §8 task 7). Blocking on an unmeasured heuristic is
  how an enforcement layer loses trust permanently.
- **Scoped to `edited_file`** — graph relations describe the whole
  repository; unscoped, the rule fires the same warnings on every edit.
- **Rust-only** — the rule needs a closed, idiomatic watchlist of
  panic-introducing calls, and only Rust has a defensible one. Python's
  everything-raises semantics would bury the signal (spec §4.2.3).

## Enforcement

- `warn-untested-risky-call` in `.phronesis/rules.json` (structural pack,
  `phr-mcp init --packs structural`). Also auditable tree-wide via
  `phr-mcp audit` (`audit: true`).
- Enforcement degrades honestly: a stale or format-outdated graph demotes
  the rule to warn-with-notice; it never blocks on evidence the harness
  cannot vouch for.

## Consequences

- Direct-call coverage is over-approximated by design (short-name match,
  spec §4.4): the heuristic errs toward silence, not accusation.
- Promotion to `block` is gated on the second-corpus measurement — see
  the phase plan (Phase Two starts only after that measurement).
- Adding a watched API is a config edit; adding a *relation* is a spec
  change (closed relation set, spec §1.2).
