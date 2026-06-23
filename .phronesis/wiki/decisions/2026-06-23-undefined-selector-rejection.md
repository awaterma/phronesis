---
id: undefined-selector-rejection
date: 2026-06-23
status: accepted
enforces: []
superseded_by: null
tags: [journey, safety-by-default, error-handling]
---

# undefined-selector-rejection

## Context

A journey rule references a tag (or `module:<name>`) via `journey_*`
predicates. The tag must be defined in `.phronesis/journey.json` for
derivation to produce facts. The journey module's docstring (in
`crates/phronesis-mcp/src/journey/derive.rs`) explicitly states the design
intent:

> Validate every referenced tag / `module:<name>` selector against the
> project's `TaggerConfig` — silent-typo guard. A rule referencing
> `['testz','s']` when the project defines `tests` is rejected here
> rather than silently always-firing on `== 0`.

The validation logic is implemented (`validate_selectors` in `derive.rs`)
and a typed error variant exists (`DeriveError::UndefinedSelector { rule,
selector }`). But the **caller** in `crates/phronesis-mcp/src/hook.rs`
(`assert_journey_facts_into`) treats every `DeriveError` uniformly as
fail-open: log the error to stderr, continue without journey facts.

The user-visible consequence is the opposite of the docstring's claim:

- A `facts_count(...) >= N` rule with a typo'd selector silently never
  fires (count stays zero — never crosses the threshold). Mild failure.
- A `facts_count(...) == 0` *absence* rule with a typo'd selector fires
  **constantly** (the missing tagger looks like zero occurrences,
  satisfying `== 0` on every call). Loud failure with no clear cause —
  the only diagnostic is a stderr line easily missed in a long loop.

Discovered while validating the loop-programming guide
(`docs/loop-programming-guide.md`); the doc's reassurance to readers
that "a rule referencing a tag the project's `journey.json` doesn't
define is **rejected at load time**" turned out to be aspirational —
the engine emits the warning but loads the rule.

## Decision

**Fail closed on configuration errors from journey derivation.**

Differentiate `DeriveError` variants by category in the hook:

- `BadWindow` and `UndefinedSelector` are **configuration errors** — the
  user has a typo in `rules.json` or `journey.json` that no amount of
  retrying will fix. These must cause the hook to exit non-zero with a
  clear `BLOCKED — ...` message (matching the existing pattern for rule
  load failures, malformed payloads, etc.).
- `Journal` errors stay **fail-open**. A missing or corrupt journal is
  transient, the rest of the rule pack should still run, and blocking
  every edit on a journal hiccup would be worse than the silent miss.

The hook surfaces config errors via `process::exit(2)` (block) with the
same `phronesis: BLOCKED — ...` prefix used elsewhere in the file, so
the agent sees a clear pointer to the misconfigured rule.

## Enforcement

This is an **engine-level invariant**, not a phronesis rule (rules
govern user code; this governs the engine's own behavior). No
`enforces: [rule-id]` link is appropriate. The protection lives in
`assert_journey_facts_into` in `crates/phronesis-mcp/src/hook.rs` and
the contract is pinned by two regression tests in
`crates/phronesis-mcp/tests/journey_hook_integration.rs`:
`undefined_selector_fails_closed_at_pre_check` (exit 2, BLOCKED message
names rule + selector) and
`undefined_selector_fails_closed_at_post_check_as_warn` (exit 1,
WARNING — the action already happened, so the next pre-check is
where it lands as a block).

## Consequences

- Users who have shipped journey rules referencing undefined selectors
  will see hook failures on their next tool call after upgrading. The
  error message names the rule id and the missing selector, so the fix
  is a single-line edit (add the tagger to `journey.json` or fix the
  typo in `rules.json`). This is the intended noisiness — it surfaces
  a class of bug that previously hid behind a stderr line.
- The `docs/loop-programming-guide.md` §4 callout reverts to its
  original "rejected at load time" wording — accurately, this time.
- The fail-open / fail-closed boundary becomes explicit at the call
  site, which is the right place for it. Other modules can follow the
  same pattern.
