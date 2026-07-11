# Neutral toolchain outcomes — design

**Status:** draft (awaiting review)
**Authors:** Andrew Waterman, Claude
**Date:** 2026-07-10
**Branch:** `feat/neutral-toolchain-outcomes` (off `feat/payload-contract-corpus`)
**Affects:** `crates/phronesis-mcp/src/outcomes/{adapter,cargo,toolchain,facts}.rs`,
            `crates/phronesis-mcp/src/hook/{mod,journey_record}.rs`,
            `crates/phronesis-mcp/src/journey/journal.rs`,
            `crates/phronesis-mcp/src/init.rs`,
            `crates/phronesis-mcp/CLAUDE.md`

## Problem

Confidence scoring grounds a commit gate on real build/test signals — but
only for **cargo**. The `OutcomeAdapter` registry ships a single
hand-written `CargoAdapter` that recognizes `cargo build/check/test` by
substring and parses cargo's human output with cargo-specific regexes. A
Python, Go, or TypeScript project gets no grounded signal at all, so the
confidence gate is inert outside Rust.

Two things are wanted:

1. **Work for other toolchains besides cargo** — pytest, `go test`, tsc,
   gradle, make, anything.
2. **Be more abstract than detecting a specific compiler or test
   runner** — adding a toolchain should not require a Rust code change.

A third gap surfaced while scoping: the **journey journal grows without
bound on write**. Confidence's outcome history and (per this design) the
new exit-code records both live in `events.jsonl`, which has no write-side
retention — only a read-side line cap.

## Design

Four components. The unifying idea: the **unix exit code is the
toolchain-neutral pass/fail primitive**, and everything a specific
toolchain adds on top is declarative data, not compiled code.

### 1. Two-tier signal model

- **Tier 1 — exit code (universal, zero config).** A process that touched
  the OS has an exit code. `command_exit == 0` → success (compile ok; if a
  test command, tests passed). `command_exit != 0` → failure. This grounds
  a `build_outcome` for *any* recognized command with no per-toolchain
  parsing.
- **Tier 2 — regex refinement (per toolchain, optional).** Extracts what
  the exit code can't: passed/failed *counts* (`test_summary`), per-test
  results for the known-bug registry (`per_test`), and the compile-vs-test
  distinction (`compile_fail`). Absent → Tier 1 alone.
- **Fallback.** When the captured payload carries *no* exit code, Tier 2
  regexes are the sole signal — today's behavior, preserved.

Precedence: a present exit code is **authoritative for build/compile
success**. `test_summary` refines the *test* outcome (counts); a
`compile_fail` match forces build failure even on a zero exit (rare, but
keeps cargo's "linker failed" semantics). This is spelled out in §"Signal
precedence" below.

### 2. `.phronesis/toolchains.json` — declarative toolchain definitions

A JSON array of entries beside the existing
`.phronesis/{rules,bugs,journey,confidence}.json`. Only `matches` is
required; the three refinement regexes are optional.

```json
[
  {
    "id": "pytest",
    "matches": "pytest",
    "compile_fail": ["SyntaxError", "ImportError:"],
    "test_summary": "(\\d+) passed(?:, (\\d+) failed)?",
    "per_test": "^(PASSED|FAILED) (\\S+)"
  }
]
```

Field semantics:

| Field | Required | Meaning |
|---|---|---|
| `id` | yes | Stable name, for diagnostics / `phr-mcp toolchains`. |
| `matches` | yes | Substring (or regex) tested against the command string: "is this a build/test command I ground a signal from?" Recognition only — it does **not** decide pass/fail. |
| `compile_fail` | no | List of substrings/regexes; any match → `build_outcome = fail` regardless of exit code. Recovers the compile-vs-test split. |
| `test_summary` | no | Regex with capture group 1 = passed count, optional group 2 = failed count. **All** matches in the output are summed (multi-binary runs). Presence of ≥1 match → a `test_outcome`. |
| `per_test` | no | Regex, group 1 = status token (`PASSED`/`FAILED`/`ok`/…), group 2 = test name — or the reverse; a `pass_token` field names which token means pass (default `ok`/`PASSED`). Feeds `bugs::check`. |

Regexes are validated at load; a malformed entry is skipped with a stderr
warning (fail-open — a bad toolchain def must not break the hook).

### 3. Adapter layer: one generic engine, cargo as a built-in def

`OutcomeAdapter` (the trait) stays. The registry changes from "one
hand-written adapter" to **built-in defs ∪ project defs**, all driven
through a single generic `ConfigAdapter { def: ToolchainDef }`:

- `ToolchainDef` — the deserialized entry from §2 (also the shape of
  built-in defs).
- `ConfigAdapter::handles(cmd)` = `def.matches` tests true.
- `ConfigAdapter::parse(subject, cmd, output, command_exit)` applies the
  two-tier model: build outcome from exit code (overridden to fail by a
  `compile_fail` match), test outcome from summed `test_summary` captures,
  per-test from `per_test`.

**Cargo becomes a bundled built-in `ToolchainDef`** (a `const`/`LazyLock`
def compiled into the binary), not a hand-written adapter. It runs through
the exact same `ConfigAdapter` as user config. This dogfoods the format
and removes cargo's special status. The existing `cargo.rs` test suite is
the **fidelity bar**: the built-in cargo def must make every current
`CargoAdapter` test pass unchanged (multi-binary summing, warnings-are-not-
failures, compile-error-vs-test-failure, no-test-fact-on-compile-fail).

Registry assembly (per hook invocation, cheap):
1. Built-in defs (cargo; optionally pytest/tsc shipped as defaults —
   decided in the plan, not blocking here).
2. Project defs from `.phronesis/toolchains.json`, appended. A project def
   with the same `id` as a built-in **overrides** it (lets a project
   retune cargo parsing without a release).

`adapter::handles(cmd)` = any def matches. `adapter::extract(...)` = first
matching def's `ConfigAdapter::parse`.

The `parse` signature gains `command_exit: Option<i32>` (see §4). The
trait method changes accordingly; `CargoAdapter` is deleted once the
built-in def reaches fidelity.

### 4. Exit-code capture — a general event-recording invariant

**Every Bash/shell tool event that carries an exit code records it.** This
is a standalone invariant, useful beyond confidence, and the enabler for
Tier 1.

- **Extract.** `HookPayload` gains an accessor that pulls the command's
  exit code from the tool-response object. The exact location is
  CLI-specific and is **pinned by the payload-contract corpus** (this
  branch is stacked on `feat/payload-contract-corpus` for exactly this
  reason). Known candidates, tried in order: `tool_response.exit_code`,
  `.exitCode`, `.returncode`, `.code`, `.status`, then a trailing
  `exit code: N` line in the output text. Absent → `None`.
- **Record.** `JournalRecord` gains an optional `command_exit: Option<i32>`
  (serialized, `skip_serializing_if = None`), stamped on every Bash/shell
  record. The action-log hook event gains a parallel `command_exit` field.
  **Named `command_exit`, not `exit`** — the log already has an `exit`
  field for the *hook's own* exit code (0/1/2); the two must not collide.
- **Honesty.** When the CLI provides no exit code, `command_exit` is
  omitted and confidence falls back to Tier 2. The corpus asserts that a
  Claude Code / Gemini Bash payload's exit code *is* captured, so "absent"
  means the CLI genuinely didn't send one, not that we dropped it.

### 5. Journal compaction — subject-aware, bounded growth

`journey::journal::append` gains a size-cap check (mirroring the action
log's env-configurable `max_bytes`; new `PHRONESIS_MAX_JOURNAL_BYTES` /
`max_journal_bytes()`). When the journal exceeds the cap, it is
**compacted by atomic rewrite** (temp file + rename, like the existing
atomic writers), never blind-truncated.

The retained set:
1. **The most recent `K` records** (the tail journey windows and
   `SUFFIX_HARD_CAP` read from), **plus**
2. **for every `subject` appearing in the older, to-be-dropped prefix, its
   most recent `outcome:*`-bearing record** — so each work unit's latest
   build/test result survives for confidence banding.

Everything else is dropped. This is safe because the only two readers —
journey windowed aggregators and confidence per-subject history — both
still find what they need: recent activity in the tail, and the latest
grounded outcome per subject preserved explicitly. Compaction is
best-effort and fail-open: a compaction error logs and leaves the journal
untouched (append still succeeds).

This revives the deferred SPEC-journey-facts phase-2 retention, scoped down
to the minimal safe guard the exit-code write-volume increase warrants —
not the full repo-lifetime `r`-window checkpoint machinery.

## Signal precedence (worked cases)

| Command | exit | compile_fail hit? | test_summary | Result |
|---|---|---|---|---|
| `cargo build` | 0 | no | — | `build: pass` |
| `cargo build` | 101 | — | — | `build: fail` |
| `cargo build` | 0 | yes (linker) | — | `build: fail` (compile_fail overrides zero exit) |
| `pytest` | 1 | no | `10 passed, 2 failed` | `build: fail`? **No** — see note | 
| `pytest` | 0 | no | `12 passed` | `build: pass`, `test: 12/0` |

**Note on the pytest-exit-1 case:** a test *failure* exits non-zero but is
not a *compile* failure. Tier 1 maps a non-zero exit to build-fail only
when there is **no** `test_summary` match; when a test summary is present,
the non-zero exit is attributed to the test failures it reports (build =
pass, test = passed/failed from the summary). This mirrors cargo's existing
"a test failure is not a compile failure" rule and is the one place exit
code is *not* authoritative for build. Spelled out because it's the subtle
case.

## Surfaces

- **`phr-mcp toolchains`** — list active toolchain defs (built-in +
  project), like `phr-mcp journey`. `--json` for machine output. Read-only;
  helps a user confirm their `toolchains.json` is recognized.
- **`phr-mcp init`** — writes a commented `.phronesis/toolchains.json`
  example (pytest/tsc entries) when a relevant pack is selected; left alone
  on re-run.
- **`phr-mcp confidence`** — unchanged surface; now grounded for any
  configured toolchain.

## What this does not do

- **No process execution.** Parsing captured output only, as today.
- **No per-language Rust adapters.** cargo is the last hand-shaped
  toolchain and even it becomes a def. New toolchains are data.
- **No structured-output dependency.** We use exit code + text regexes, not
  `--message-format=json` / JUnit XML. A toolchain *may* emit structured
  output; we don't require it. (Possible future refinement, out of scope.)
- **No repo-lifetime journey checkpoint.** Compaction is the minimal safe
  bound, not the full phase-2 `r`-window machinery.

## Testing

- **Cargo fidelity:** the built-in cargo def passes the existing
  `cargo.rs` test suite verbatim (the acceptance bar for §3).
- **Generic engine:** unit tests for `ConfigAdapter` over a synthetic def
  (exit-code-only, test_summary summing, compile_fail override, per_test).
- **A real second toolchain:** a pytest def + captured-output fixtures
  proving neutrality (the SPEC-confidence-scoring "prove neutrality" item).
- **Exit-code capture:** payload-contract fixtures (this branch's parent)
  asserting `command_exit` is captured from real Claude Code / Gemini Bash
  payloads and recorded on the journal record.
- **Compaction:** a journal driven past the cap retains the recent tail and
  the latest `outcome:*` record per subject; a confidence read after
  compaction still bands correctly; compaction failure leaves the journal
  intact.

## Versioning

New config surface (`toolchains.json`), new CLI subcommand
(`toolchains`), new journal/log field (`command_exit`), new env var
(`PHRONESIS_MAX_JOURNAL_BYTES`) → **MINOR**, lockstep across the workspace.
Version number coordinates with the payload-contract branch it stacks on
(both target the same MINOR line; settle the exact number at release).

## Open questions (non-blocking; resolved in the plan)

- Which built-in defs ship beyond cargo (pytest/tsc bundled, or cargo-only
  built-in + examples in `toolchains.json`)?
- `per_test` token direction — a `pass_token` field vs. positional
  convention; pick during pytest-def authoring against real output.
- `K` (tail size) and the default `max_journal_bytes()` — set against the
  `SUFFIX_HARD_CAP` (10k) read cap so compaction never drops inside the
  read window.
