# Governed TDD walkthrough: host acceptance and test selection

> Historical validation record, authored here on 2026-11-25. The live-agent
> evidence below applies to commit `7b23be659ba4e139359ba660c65fb3c8edffc837`.
> The later message/comment fixes and selective-test extension have the
> separate automated evidence described below. Follow the
> [current tutorial](../../tutorials/first-governed-session.md) for instructions.

## Real host walkthrough

An independent Claude Sonnet 5 session ran inside Claude Code 2.1.330. It
received the published tutorial and installation guide, a disposable copy of
the practice fixture, and an acceptance brief, without builder reports or
previous review conclusions. The coordinator supplied a candidate
`phr-mcp 0.41.4` executable and initialized the project's generated hooks.
The host loaded those hook settings explicitly. This was a headless session;
MCP servers were disabled to isolate tool-hook enforcement.

The retained tool transcript and final files establish this sequence:

1. Write the two published tests before changing production source.
2. Run `cargo test --offline -j 1 --test visits`: `parses_valid_count` fails
   with actual `0`, expected `12`; `malformed_count_is_zero` passes. Cargo
   exits 101 because of the assertion, not compilation or setup failure.
3. Propose the `.unwrap()` implementation with a real Edit tool call. The
   Phronesis pre-check hook refuses it. Its log identifies
   `tutorial-no-unwrap-in-src` in `blocked_by` with exit 2.
4. Read the source again and observe the original zero-returning stub.
5. Apply the compliant `match` implementation with Edit; the hook allows it.
6. Run the same Cargo command: `2 passed; 0 failed`. Read back both files.

The coordinator independently checked transcript order, the published
instruction bytes, unchanged test bytes and unchanged rules. There were
three modifying proposals: test creation, denied source edit, and allowed
source edit. Logs corroborated actual host tool results; they were not used
alone to certify enforcement.

The process exited 0 after 50.18 seconds. Reported API cost was $0.2118036
within the authorized $5 / 30-minute envelope. This is harness telemetry,
not an invoice. The supplied binary SHA-256 was
`2d6ebc3b414b4a9fc1a09e2353351531aa1f4c7132a662949f31a7ff6e727cba`.

## Findings and narrow follow-up fixes

No functional blocker appeared in the TDD task. Two presentation issues did:

- The surfaced refusal explained the policy, but the rule ID was present
  only in the log, despite Step 4 saying the hook would name it. The teaching
  rule message now includes `tutorial-no-unwrap-in-src`; the executable
  checker requires that ID in the actual hook output as well as the log.
- Editing only the function left a module comment describing the completed
  code as incomplete. The starter now uses a neutral description.

A compound `export ... && cargo test ...` attempt was denied by the test
harness permission configuration. The published bare Cargo command worked.
The tutorial already exports the variable separately, so this was harness
friction rather than a reproduced documentation defect.

## Automated selective-test extension

The optional extension records a passing three-test Git baseline inside the
disposable copy. Its third test, `package_name_is_stable`, checks package
metadata without calling `visits_today`. It then refactors only the production
function to use `unwrap_or(0)`, leaves the edit unstaged, rebuilds the graph,
and selects tests for the change.

The executed published commands produced:

- Changed function: `fn:src/lib.rs::visits_today`.
- `static_reach_available: true` with no stale-graph note.
- Exactly `parses_valid_count` and `malformed_count_is_zero`, both labeled
  `static_reach`; the package control was excluded.
- Each documented exact Cargo command ran one passing test with two filtered
  out. Tests remained unchanged during the refactor and selection.

An initial execution exposed a real instructions error: redirecting selection
JSON into the project created a new file and immediately made the graph
stale. The corrected instructions save it beside the practice directory.
The passing regression exercises that exact command sequence.

Test-first development for this follow-up started with two expected failures:
the refusal lacked the rule ID, and the selection section's executable
blocks were absent. After the fixes, all **14 tutorial checks passed, none
failed or skipped**, using Rust 1.90. The checker executes published code and
commands; it does not keep an independent hand-written copy of the example.
See [the executable scenarios](../../../examples/first-governed-session/tdd_cases.py).

## Limits

- The new selection extension and follow-up message/comment changes were
  validated automatically, not by another live-agent run. The host evidence
  remains bound to the earlier commit above.
- No dynamic coverage was imported. Static reach selects relevant tests; it
  is not a coverage percentage or proof that testing is sufficient. Rust
  selections returned `command: null`, so the tutorial explains the explicit
  Cargo commands rather than claiming automatic execution.
- Interactive trust prompts, `/hooks` UI, MCP approval and management,
  fresh-clone installation, other hosts and unfamiliar-human usability
  remain unobserved.
- This demonstrates TDD chronology in one observed task. The teaching rule
  itself enforces a coding constraint, not test-first chronology.

Raw transcripts, receipts, test outputs, fixtures and build caches were
preserved locally. No merge, release or deployment formed part of this work.
