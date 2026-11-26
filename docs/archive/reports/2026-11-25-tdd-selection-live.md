# Current TDD tutorial: live host and selective-test acceptance

> Historical validation record, authored here on 2026-11-25. Tested tutorial
> and fixture commit: `12bef8ff939d7aa71b07a644fac60ac853db59d5`.
> This evidence-only publication does not change those inputs. Follow the
> [current tutorial](../../tutorials/first-governed-session.md) for instructions.

## Outcome and identity

The current TDD behavior and optional selective-test behavior were observed
in a real Claude Code session, including both exact Cargo test runs. One
host-permission adaptation was required for saving diagnostic output; this
was not a completely verbatim execution of every published shell command.

The [machine-readable verification](2026-11-25-tdd-selection-live.json)
records SHA-256 hashes of both supplied instruction pages, the starter crate,
source fixture and rule file, plus the binary and raw host transcript hashes.
The coordinator matched those inputs to the tested commit and rechecked them
before publishing this record. Transcript event indices are zero-based and
identify the actual tool invocations; tool-use IDs correlate their results.

The agent was Claude Sonnet 5 inside Claude Code 2.1.330. It received only
public instructions, a disposable fixture and an acceptance brief, without
builder conclusions. The coordinator supplied `phr-mcp 0.41.4`, Rust 1.90 and
generated project hook wiring. Hooks were loaded explicitly; the session was
headless and MCP servers were disabled. No claim of independent installation
or interactive trust acceptance follows from this setup.

## Observed TDD sequence

| Event | Actual result |
|---|---|
| Write the two published tests | Allowed; implementation was still the stub |
| Run the documented Cargo command | Exit 101; valid count fails with actual 0 / expected 12; malformed-input test passes |
| Propose `.unwrap()` using Edit | Phronesis denies the real tool invocation; surfaced refusal now includes `tutorial-no-unwrap-in-src` |
| Read the source after denial | Original stub remains unchanged |
| Edit to the compliant `match` implementation | Allowed |
| Rerun the same Cargo command | Two passed, zero failed; original tests unchanged |

The neutral module comment remained accurate after implementation. The
coordinator checked raw tool results and resulting files, not just the
agent's summary or the local hook log. No rule or hook settings were weakened.

## Observed selective-test sequence

1. The agent added the documented `package_name_is_stable` control test;
   all three tests passed.
2. It created exactly one Git baseline commit in the disposable project with
   the tutorial's command-scoped identity. There were no remotes or pushes.
3. It refactored only `src/lib.rs` to use `unwrap_or(0)`, leaving that edit
   unstaged and all tests unchanged from the baseline.
4. It rebuilt the graph and ran `phr-mcp coverage select --json` through the
   real host's Bash tool. The output named only `fn:src/lib.rs::visits_today`
   as changed and reported `static_reach_available: true` with no stale note.
5. Exactly two tests were selected, both labeled `static_reach`:
   `parses_valid_count` and `malformed_count_is_zero` in target `visits`.
   The unrelated package-metadata test was excluded. Their `command` fields
   were null, as the tutorial explains.
6. Each documented named Cargo invocation with `--exact` ran one passing
   test and filtered out two. The coordinator verified both individual
   command results in the transcript.

The graph reported one unresolved call and zero ambiguous calls. Selection
was therefore not treated as a claim of complete call resolution. No runtime
coverage was imported; static reach is not a coverage percentage or proof
that the selected tests are sufficient for every change.

## Harness restrictions and retained attempts

Three sequential sessions were used within one $5 / 30-minute envelope:

- The first exercised TDD and selection, but its allowlist omitted the
  filtered Cargo commands. Those exact runs were not accepted as verified.
- The second stopped when it added a shell wrapper to the doctor command
  that the harness did not allow.
- The final session used corrected command-prefix permissions and literal
  command instructions. It completed the behavioral sequence and both
  selected test runs.

The final session still encountered a host-level denial for the published
`> ../selected-tests.json` shell redirection. It ran the same selection
command without redirection, inspected the real stdout JSON, and saved it
beside the practice directory with Write. The coordinator verified that the
saved JSON equals the actual command output. No synthetic selection data
was substituted. The published redirection itself remains unverified in
this restricted host configuration; it had passed the separate automated
shell check recorded in the [earlier report](2026-11-25-tdd-walkthrough.md).

The completed session took 85.31 seconds and reported $0.3066367 in API cost.
All three attempts together reported **$0.6981896**, before the original
30-minute deadline. These are harness receipts, not invoices. Partial runs,
transcripts, outputs, fixtures and build caches were retained.

## Remaining boundaries

Interactive folder trust, `/hooks` UI, MCP approval and management,
fresh-clone installation, other hosts, and unfamiliar-human usability remain
unobserved. The walkthrough demonstrates test-first chronology in this task;
the teaching rule itself does not enforce that chronology.

This record supersedes the earlier evidence gap for live-agent execution of
the selective-test extension, with the output-saving adaptation stated above.
It does not retroactively broaden the scope of the previous historical
records. No tutorial, fixture, engine or checker changes were needed to
publish this evidence; no merge, release or deployment was performed.
