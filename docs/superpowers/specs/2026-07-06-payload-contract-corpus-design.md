# Payload-contract corpus + hook liveness tests — design

**Status:** draft (awaiting review)
**Authors:** Andrew Waterman, Claude
**Date:** 2026-07-06
**Affects:** `crates/phronesis-mcp/src/{hook/mod.rs,payload_scrub.rs,main.rs}`,
            `crates/phronesis-mcp/tests/{payload_contract.rs,fixtures/**}`,
            `crates/phronesis-mcp/CLAUDE.md`

## Problem

The single most repeated bug class in this project is a mismatch between
what a host CLI actually sends to a hook and what the hook assumes. Three
shipped incidents, all invisible to the test suite:

1. **0.13.2** — Claude Code delivers Bash output under `tool_response`;
   the hook read only `tool_output`. Confidence scoring was wedged at
   "low" for every real cargo run.
2. **0.13.2** — `bash_command_matches` taggers silently never fired; the
   default `build` tagger no-op'd in live sessions.
3. **0.17.1** — `init` wired Gemini's turn-context hook under
   `BeforeModelRequest`, an event that does not exist. Per-turn context
   injection never ran in any Gemini session, ever.

All three share two properties: the failure mode is **silence** (the
system fails by *not acting*, which is indistinguishable from "all
clear"), and every existing test passed because tests synthesize their
own payloads — the tests and the code share the same wrong assumption.

Nothing in the repo pins what Claude Code and Gemini CLI *actually
send*, and nothing asserts that the plumbing produces *any* observable
effect for payloads that should trigger one.

## Design

Four pieces, smallest surface that closes the gap:

### 1. Payload capture (`PHRONESIS_CAPTURE_DIR`)

`hook::read_payload()` is the single choke point where pre-check and
post-check read stdin. When the env var `PHRONESIS_CAPTURE_DIR` is set,
the raw stdin string is appended as one JSONL record
(`{"ts": <epoch>, "phase": "pre"|"post", "raw": <payload>}`) to
`<dir>/payloads.jsonl` before parsing. Best-effort: capture failures
are swallowed and never alter hook behavior or exit codes (same
contract as `log_hook_event`). Off by default; zero cost when unset.

This is how the corpus stays alive: when a CLI changes its payload
shape, one live session with the var set harvests the new ground truth.

### 2. Anonymizing scrubber (`phr-mcp scrub-payload`)

Captured payloads contain data that reaches outside this project:
absolute `$HOME` paths, the OS username, session ids, transcript paths,
and paths into sibling repos. Fixtures are committed to a public repo,
so curation must scrub exactly that class — **project-internal content
is left byte-for-byte intact** (that's the point of a contract corpus).

`phr-mcp scrub-payload <file> [--write]` reads a captured JSONL file
(or a single fixture JSON), applies deterministic rewrites to every
string value, and prints the result (or rewrites in place with
`--write`):

- paths under the project root → prefix replaced with
  `/home/dev/project`
- any other path under `$HOME` → `/home/dev/external/p<N>` (N indexed
  per unique path, so distinct external paths stay distinct)
- the username (basename of `$HOME`) anywhere in a string → `dev`
- keys whose name (compared **case- and separator-insensitively**:
  `session_id`, `sessionId`, `SessionID` all match) identify a session
  → value replaced with `sess-00000000`; transcript-path keys likewise
  → `/home/dev/.claude/transcript.jsonl`. Matching the exact snake_case
  key only would leak a CLI that sends `sessionId` (adversarial review
  finding #3) — the match normalizes case and strips `_`/`-` before
  comparing against the id-key set.

Ephemeral values (timestamps, PIDs, temp-dir paths outside `$HOME`)
are deliberately **out of scrubber scope**: they are not privacy leaks,
and the contract runner never compares a captured `ts` against a fresh
one — it replays the `payload` object and asserts *effects*, not
byte-equality with the capture. A fixture stays stable across
environments because the runner's assertions (`exit`, `log_rule_fired`,
`journal_tag_*`) don't depend on ephemeral fields. If a future fixture
*does* assert on a value that varies per run, that is a fixture-authoring
bug caught in review, not a scrubber responsibility.

After rewriting, the scrubber verifies its own output and exits 1
naming any residual. **The verify step and the "leave content
verbatim" rule can contradict each other** — if the OS username is a
common word that legitimately appears inside a captured command's
*text* (`git commit -m "align the deck"` for a user named `al`), a
naive `contains(username)` check would fail a correctly-scrubbed
fixture, and no amount of re-running fixes it (breaking the
idempotence claim). The design resolves this by scoping verification,
not content:

- **Path-shaped residuals fail hard.** Any surviving `$HOME`-prefixed
  path, or the username *as a path component* (`/…/<user>/…`), exits 1
  — these are unambiguous leaks.
- **The bare username as a free-text token is a warning, not a
  failure.** It is reported on stderr for the human reviewer to
  adjudicate, and the run still exits 0. A 1–2 char username is never
  substring-replaced at all (it would shred ordinary words); path
  rules still cover it because paths embed the full `$HOME` prefix.

This keeps scrubbing idempotent (a second pass changes nothing and
still exits 0) while never silently shipping a path leak. The
mechanical scrub handles the unambiguous class; the human reviewer,
prompted by the warnings, catches semantic leaks the tool cannot know
about (a private name inside command text, a proprietary path that
isn't under `$HOME`).

### 3. Fixture corpus + data-driven contract runner

Fixtures live at
`crates/phronesis-mcp/tests/fixtures/payloads/<cli>/<name>.json`
(`cli` ∈ `claude-code`, `gemini`). Each fixture is self-describing:

```json
{
  "schema": 1,
  "source": { "cli": "claude-code", "event": "PostToolUse",
              "provenance": "authored", "captured": "2026-07-06" },
  "subcommand": "post-check",
  "packs": "llm,rust,confidence,journey",
  "payload": { "...": "verbatim what the CLI sends" },
  "expect": {
    "exit": 0,
    "stdout_json": true,
    "log_rule_fired": "warn-cargo-build-without-workspace",
    "journal_tag_new": ["build"],
    "journal_tag_from_output": ["outcome:test_pass"],
    "stderr_contains": []
  }
}
```

`tests/payload_contract.rs` walks the corpus. For each fixture it
creates a temp project, runs the real `phr-mcp init --packs <packs>`
(so contracts run against the *shipped* packs, not hand-rolled rules),
pipes `payload` verbatim into the real binary
(`CARGO_BIN_EXE_phr-mcp <subcommand>`, cwd = temp project), and asserts
every clause of `expect`.

**The assertion principle (learned from an adversarial review):** every
liveness clause must key on an effect *downstream of the exact code
path the fixture claims to exercise* — never on a universal artifact
the hook emits regardless. Two clauses from an earlier draft violated
this and are removed:

- A bare `log_event: "post_check"` check is **worthless as liveness**:
  `log_hook_event` writes a `pre_check`/`post_check` line on *every*
  completing invocation, so the entry exists even when the hook read
  nothing from the payload. Bug #1 (output under the wrong key) would
  have **passed** this check — the post-check still logged and still
  exited 0.
- A bare `journey_tags: ["build"]` check **fails to pin bug #1 too**:
  the `build` tag is derived from the *command text*
  (`tool_input.command`), not from the tool output. A payload with its
  output under a broken `tool_response` alias still tags `build` off
  the command and passes. That is a false-green in the very fixture
  meant to catch the alias bug.

The corrected clause vocabulary:

- `exit` — the exit code (guard, not liveness).
- `stdout_json` — stdout parses as JSON. This pins the `exit_ok()`
  contract Gemini depends on, but is explicitly a **guard, not
  liveness**: `exit_ok()` always prints `{}`, so passing proves only
  that the process exited cleanly, not that it processed the payload.
- `log_rule_fired` — the named rule id appears in the `consequences`
  array of the log entry. This is real liveness for block/warn
  fixtures: it proves *that specific rule* matched *this payload*, not
  merely that the process ran.
- `journal_tag_new` — the journal gained a record tagged thus **whose
  timestamp is from this invocation** (see the freshness guard below),
  not a record left by `init` scaffolding or a prior call.
- `journal_tag_from_output` — a tag whose derivation *requires reading
  the tool-output field* (an `outcome:*` tag produced by the confidence
  adapter parsing stdout). This is the clause that actually pins bug
  #1: it is present only if `tool_response`/`tool_output` was read and
  parsed. The regression fixture asserts this, not `build`.
- `stderr_contains` — each substring appears on stderr.

#### 3a. Path hermeticity (defeats silent-no-op false-green)

A captured payload's `file_path` is typically **absolute**
(`/home/dev/project/src/lib.rs` after scrubbing). Replayed as-is in a
temp project, a path-gated rule (`file_path_matches: "src"`) may not
fire, or a clean fixture may pass *for the wrong reason* — the rule
never evaluated (adversarial finding #4). Two defenses: (1) authored
fixtures use **project-relative** `file_path` values (`src/lib.rs`);
(2) the runner rewrites the scrubbed `/home/dev/project` prefix in
`file_path`-shaped fields to the actual temp-project root before
piping, so an absolute captured path resolves under the temp tree.
A fixture asserting `log_rule_fired` for a path-gated rule therefore
cannot pass unless the path actually resolved and the rule actually
matched.

#### 3b. Journal freshness guard (defeats stale-record false-green)

`journal_tag_new` / `journal_tag_from_output` must not be satisfiable
by a record the fixture's own hook did not write. The runner captures
`baseline = set of record identities in events.jsonl after init, before
the hook runs`, then asserts the required tag appears on a record
**absent from `baseline`**. Record identity is the `(sid, seq, ts)`
tuple the journal already stamps. This closes the case where `init`'s
journey scaffold or an accumulated record carries the tag independently
of the hook under test.

`provenance` is `"authored"` for fixtures hand-built from the CLIs'
documented schemas (the starter corpus) and `"captured"` for scrubbed
live captures, which supersede authored ones as they land. Authored
starter payloads carry the full real envelope (`session_id`,
`transcript_path`, `cwd`, `hook_event_name`, …), not the minimal
shape existing tests use — bug #1 lived precisely in the envelope
fields tests didn't bother to send.

Regression fixtures pin the two payload-shape incidents:

- `claude-code/post-bash-tool-response.json` — Bash post payload with
  `cargo test` output under **`tool_response`** (the real Claude Code
  key), packs `llm,rust,confidence,journey`, asserting
  `journal_tag_from_output` carries an `outcome:*` tag. This fails on
  the pre-fix code (alias absent → output unread → no outcome tag) and
  passes on current main — the true bug-#1 contract.
- `claude-code/post-bash-cargo-build.json` — a `cargo build --workspace`
  command payload asserting `log_rule_fired:
  "warn-cargo-build-without-workspace"`-style command-derived effect
  plus `journal_tag_new: ["build"]` with the freshness guard (bug #2:
  the tagger firing at all).

### 4. Hook-event-name registry (wiring contract)

Bug #3 was not a payload problem — it was `init` writing a hook under
an event name that doesn't exist. `tests/fixtures/hook_events.json`
records the valid event names per CLI:

```json
{
  "claude-code": ["PreToolUse", "PostToolUse", "SessionStart", "UserPromptSubmit"],
  "gemini": ["BeforeTool", "AfterTool", "SessionStart", "BeforeAgent"]
}
```

A contract test runs `init` in a temp project, parses
`.claude/settings.local.json` and `.gemini/settings.json`, and asserts
(a) every hook key `init` wrote appears in the registry for its CLI,
and (b) `BeforeModelRequest` appears nowhere (the 0.17.1 regression,
pinned by name). Changing the registry requires a deliberate edit in
the same PR that changes the wiring — that's the tripwire.

**Why a hand-maintained registry rather than deriving valid events
from `init`'s own code?** Deriving them would make the test tautological
— `init` would be checked against itself, and the 0.17.1 bug
(`init` confidently wiring a nonexistent event) would pass, because
the "valid set" would include whatever `init` wrote. The registry's
value is precisely that it is an *independent* statement of ground
truth, maintained by a human who has checked the CLIs' actual docs.
The assertion direction is one-way on purpose: `init`'s keys must be a
**subset** of the registry (you cannot wire an event not known-good),
but the registry may list known-good events `init` doesn't use yet.
The maintenance cost crush flags is real but small (four names per
CLI) and is the intended friction: a new host event is a deliberate,
reviewed addition, not an inference.

## What this does not do

- **No new rule surface.** No predicates, no packs, no schema changes.
- **No mock CLIs.** Fixtures are data; the binary under test is real.
- **No automated capture in CI.** CI replays the committed corpus;
  harvesting new captures is a manual, human-reviewed act.
- **No scrubbing of project-internal content** — file paths, rule ids,
  and command text that stay inside this repo are the contract being
  tested and are preserved verbatim.
- **No premature runner optimization.** Each fixture spawns `init` +
  one binary invocation in a temp project. At the starter corpus size
  (single digits) this is a second or two total. If the corpus grows
  past ~50 fixtures and the suite crosses a few seconds, the fix is
  `init`-once-per-`(packs, cli)` caching or running fixtures on a
  thread pool — noted, deliberately deferred until measured, per the
  project's evidence-gated perf discipline (`SPEC-next-round-perf`).

## Open questions surfaced in review

- **Schema-inference from captures.** Authored fixtures are educated
  guesses at the full envelope until a live capture supersedes them
  (Task 7). A future `scrub-payload --to-fixture` that scaffolds an
  `expect` block from a captured record — inferring `log_rule_fired`
  from the consequences the hook actually emitted — would shorten the
  capture→fixture loop.
  Out of scope for v1; the manual promotion path covers the need.
- **Envelope completeness.** The capture tee records whatever the CLI
  sends, so the corpus's envelope fidelity is only as good as the
  sessions harvested. A periodic diff of captured envelope keys against
  the authored fixtures' keys would flag envelope drift; not built here.

## Versioning

`scrub-payload` is a new subcommand and `PHRONESIS_CAPTURE_DIR` a new
env-var surface → **0.18.0** (MINOR), lockstep across the workspace.

## Acceptance

- All three historical bugs have a fixture or wiring test that fails
  on the pre-fix code shape and passes on current main.
- `cargo test --workspace` green; new tests run in CI unmodified.
- A captured payload from a live session on this repo, scrubbed with
  `phr-mcp scrub-payload`, contains no `$HOME`, username, session id,
  or extra-project path, and replays green through the contract runner.
