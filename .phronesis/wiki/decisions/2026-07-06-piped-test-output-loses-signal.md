---
id: piped-test-output-loses-signal
date: 2026-07-06
status: accepted
enforces: []
superseded_by: null
tags: [confidence, outcomes, hooks]
---

# piped-test-output-loses-signal

## Context

Observed live during the 0.17.1 release session (PR #7). The agent ran the
full test suite before committing, but as

```
cargo test --workspace --quiet 2>&1 | grep -cE "test result: ok"
```

whose captured output is the single line `46`. The confidence gate then
blocked the commit at `low` with only a `compile` signal on record. A
subsequent unpiped `cargo test --workspace` produced the `tests` signal and
the commit went through at `medium`.

Mechanism (`src/outcomes/cargo.rs`): `CargoAdapter::handles` /
`is_test_command` match on the *command string* (`contains("cargo test")`),
so the piped invocation was correctly recognized as a test run. But
`test_outcome` is derived by `sum_test_results`, which parses the *captured
output* for cargo's `test result: ok. N passed; M failed` summary lines
(unanchored `TEST_RESULT` regex). Any shell pipeline that consumes or
filters those lines — `grep -c`, `wc -l`, a `grep -v` chain that happens to
drop them, `| tail` past them — yields `None`, and no test signal is
recorded even though the tests ran and passed.

This is an agent-shaped failure mode: piping `cargo test` through `grep` to
shorten output is one of the most natural commands for an LLM to write, and
the signal loss is silent — the gap only surfaces later as a confusing
commit block. (Pipelines that *preserve* the summary lines, e.g.
`| grep -E "test result"` or `| uniq -c`, still match fine because the
regex is unanchored.)

## Decision

Accept output-parsing as the capture mechanism — it is what keeps the
adapter execution-free and the signal grounded in evidence the human can
also see. The gate blocking here was *correct behavior*: "I ran the tests"
without on-the-record output is exactly the claim phronesis exists to
reject.

Mitigate on two fronts instead:

1. **Guidance (this page + durable.md candidate):** when running tests for
   confidence-gate credit, run `cargo test` (or `cargo nextest run`)
   unpiped, or ensure the `test result:` summary lines survive the
   pipeline. Filter with `--quiet` rather than `grep -c`/`wc -l`.
2. **Possible future rule (not yet landed):** a `warn`-level
   `bash_command_matches` rule for `cargo (test|nextest)[^|]*\|\s*(grep -c|wc -l)`
   — the two filters that provably destroy the summary lines — telling the
   agent the run won't count toward confidence. Deliberately narrow: most
   pipes are harmless and a broad "no piping cargo test" rule would trip
   constantly.

## Enforcement

- Prose guidance only, by explicit choice (2026-07-06): one occurrence by
  one agent is not yet a pattern. If the friction recurs, land the narrow
  `bash_command_matches` rule drafted above and wire its id into
  `enforces:`.

## Consequences

- Agents that pipe test output through count/summarize filters will keep
  hitting low-confidence commit blocks until they re-run unpiped; that is
  annoying but safe. The failure direction is false-negative (missing
  signal), never false-positive (phantom green).
- If the rule lands, it fires at pre-check before the wasted test run,
  converting a late confusing block into an early actionable warning.
- An alternative considered and rejected for now: having the hook re-run
  tests itself or trust exit codes. Both break the "parsing only — no
  process execution" contract in `outcomes/cargo.rs` and would let a
  `grep -c` exit code stand in for test evidence.
