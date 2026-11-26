# First governed session: validation record

> Historical validation record, authored here on 2026-11-22 for the
> first-governed-session change. This is a point-in-time report, not maintained
> setup guidance. Follow the [current tutorial](../../tutorials/first-governed-session.md).

## Outcome and scope

The change adds a disposable first-session tutorial, `phr-mcp doctor`, an
explicit isolated probe, executable onboarding checks, and two opt-in
embedding-host APIs. Atomic fact deltas, fault injection, and mutation
observers remain [proposed follow-ups](../../specs/SPEC-embedding-host-follow-ups.md).
No merge, release, or deployment was performed as part of this validation.

## Independent walkthrough

A GLM 5.3 reviewer first received only the published candidate instructions,
a fresh scratch project, and a supplied candidate executable. It recorded its
obstacles before reading implementation or builder conclusions. It found a
missing `cd "$SCRATCH/practice"` before initialization. The correction is now
covered by `documented_setup`, which executes the actual published Step 0–3
shell blocks in a miniature checkout and checks that setup affects the copy.

After that initial assessment was preserved, the reviewer inspected the
implementation and reran the walkthrough. Its findings corrected a stale
probe-output quote and a false failure for shared-only Claude settings.
Separately, the coordinator reproduced false `ok` findings for a nonexistent
absolute hook executable, an invalid matcher regex, and a missing hook type;
real CLI regressions now pin those cases. Final independent re-verification
found no remaining blocker. It reproduced **12 passed, 0 failed, 0 skipped**.

This is an agent proxy for an unfamiliar human. Its initial environment used
a supplied debug binary, not a pristine clone or independently installed
release. The coordinator separately ran the documented source installation
with `--locked` into an empty installation prefix and reran all 12 checks
against that installed release-profile binary. Cached dependencies were used.

## Three distinct evidence levels

| Evidence | Observed result | Boundary |
|---|---|---|
| Configuration | Doctor reads effective rules and the generated Claude local hook settings; invalid configuration needs attention | Other host settings scopes are not merged. Shared-only wiring and live trust remain `not_observed`. |
| Isolated probe | Allowed proposal exits 0; blocked proposal exits 2 and names `doctor-probe-no-unwrap` | This invokes this executable's `pre-check` directly, not a host adapter or live session. |
| Actual host | Claude Code executed a safe Write, then refused the marker Write with the teaching rule | A real headless process with generated settings supplied through `--settings`; interactive trust UI was not tested. |

For the final host run, the process had only Read and Write tools, read the
file before writing, and did not retry the denied operation. The coordinator
checked both the hook records and the resulting file. The relevant records,
with machine-specific paths omitted, were:

```json
[
  {"event":"pre_check","tool":"Write","exit":0,"blocked_by":[]},
  {"event":"pre_check","tool":"Write","exit":2,
   "blocked_by":[{"kind":"rule","rule":"no-tutorial-blocked-marker"}]}
]
```

The file retained `Hello, governance.` followed by a newline; it did not
contain `TUTORIAL-BLOCKED-MARKER`. The actual refusal named
`no-tutorial-blocked-marker` and its literal-marker policy. Local log records
are unauthenticated corroborating evidence; the actual host invocation and
retained file were also checked. Doctor does not promote those logs to a
verified host verdict.

Interactive folder trust, project MCP approval, the `/hooks` UI, a GUI host's
PATH, Codex/Gemini integration, and a walkthrough with an unfamiliar human
remain unobserved. No success is claimed for them.

## Engine API review

A separate GLM 5.3 assessment recorded expectations before inspecting the
in-progress core snapshot. It identified stale tests from a nesting redesign,
duplicated guard handling, and missing multi-match/error-path coverage. The
final review checked the repaired implementation independently and found no
remaining blocker.

The coordinator also corrected upfront bulk selection: an error could consume
matches that had not yet executed. Selection now removes one best matching
item at a time under the agenda lock, sharing ordinary guard/refraction
handling. The internal error regression consumes an earlier activation,
encounters a missing production state, and verifies that later selected and
unrelated items retain their original identities and sequence numbers.
No public fault-injection API was added.

Ordered batching uses ordinary incremental processing, rejects mixed-mode
nesting before changing state, permits same-mode guards to close in any order,
and does no final scan. This preserves the equivalent unbatched activation
trace; it deliberately provides **no coalescing speedup**. Existing fast
batching stays available. Action values remain descriptions for the host to
apply, not engine-applied host effects.

## Executed checks

All commands below exited 0 on macOS arm64 with Rust 1.90. Cargo used four
build jobs and offline cached dependencies. Ignored tests retain their
normal ignored status; no blanket claim of exhaustive coverage is made.

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo clippy -p phronesis --all-targets --features embedding-host -- -D warnings`
- `cargo test -p phronesis --features embedding-host`
- `cargo clippy -p phronesis-mcp --all-targets --no-default-features --features rhai -- -D warnings`
- `cargo test -p phronesis-mcp --no-default-features --features rhai`
- `cargo test -p phronesis-mcp --no-default-features --test language_pack_extraction`
- `cargo test --release -p phronesis-mcp --test journey_tagger -- perf_smoke_20_taggers_100_facts --exact`
- Core Rust documentation with `RUSTDOCFLAGS="-D warnings"`, default and `embedding-host`
- `cargo install --locked --path crates/phronesis-mcp --root <isolated-prefix>`
- `python3 examples/first-governed-session/check_tutorial.py --phr-mcp <installed-binary> -v`
- `phr-mcp audit --fail-on block`: **0 blocked**, 355 advisory findings;
  two binding-dependent script rules are unsupported by this audit path.
- Documentation unit tests, strict source checks, catalogue consistency,
  the locked site build, redirects, and strict rendered checks.

The final coverage selection named 890 entries. Test-log reconciliation found
888 executable tests passing, including the release-only performance test.
Two entries were test-only helper functions without test attributes, not
runnable tests. Some direct CLI/probe coverage is invisible to static call
mapping; its subprocess regressions and observed outcomes supply the evidence.

## Collaboration and accounting

Sonnet 5 handled diagnostics and engine implementation; GLM 5.3 Flash handled
the tutorial and fixtures; GLM 5.3 supplied independent walkthrough/code
reviews. Builders exchanged the actual doctor JSON contract before finalizing
examples. Review findings led to concrete implementation changes, and the
coordinator integrated repairs and ran the combined gates. The first headless
host attempt failed because the coordinator omitted the prerequisite Read
capability; its receipt was retained and a fresh bounded repair succeeded.

The approved envelope was eight hours, $50, and at most five concurrent
agent/build jobs. Recorded harness usage, including bids and the unsuccessful
host attempt, was **$25.14426522**. This is harness telemetry, not an independently
reconciled provider invoice; the coordinator chat's billing is not available
in that ledger. No local Ollama lane was used. Worktrees, build caches and raw
receipts were retained locally; credentials and private runtime configuration
are not included in this public report.

## Reproduction anchors

- Tutorial rule SHA-256: `7c57856884834f24c6134fea3d06066b438709212160dc8d2b2d1331ed9cdc45`
- Installed macOS arm64 binary SHA-256: `c1a47dc6f9bbf6501a0ec4a35e0ba9ed0c9f2b55f5bc9d352008f3218ca45e79` (`phr-mcp 0.41.3`;
  the feature check is `phr-mcp doctor --help`, since this is unreleased code).
- Behavioral tests: [doctor CLI](../../../crates/phronesis-mcp/tests/doctor_integration.rs),
  [selected agenda](../../../crates/phronesis/tests/embedding_host_agenda.rs),
  [ordered batches](../../../crates/phronesis/tests/ordered_fact_batch.rs),
  [partial failure](../../../crates/phronesis/src/network/embedding_tests.rs).
