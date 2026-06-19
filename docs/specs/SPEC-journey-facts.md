# SPEC: journey facts — repo-scale, session-scale predicates

**Status:** draft (revised 2026-06-19)
**Authors:** Claude, Andrew Waterman
**Date:** 2026-06-18; revised 2026-06-19
**Target release:** phronesis-mcp 0.13.0 (MINOR — new fact family, new hook
              stage, new config file, new CLI/MCP surface; subsumes the
              standalone per-subject outcomes ledger shipped in 0.12.0). No
              breaking change to the `phr` library crate.
**Affects:** new `crates/phronesis-mcp/src/journey/` (mod, `journal.rs`,
              `tagger.rs`, `derive.rs`),
              `crates/phronesis-mcp/src/{hook.rs, hook_facts.rs, init.rs,
              main.rs, server.rs, server_params.rs, context.rs, outcomes/*}`.
              `phr` unchanged.

## Premise

phronesis predicates today are **point-in-time**: regex over the current diff
(`diff_extract.rs`), tree-sitter over the current file (`syntax/`), and the
wall clock (`clock_facts.rs`). Each `pre-check`/`post-check` is a fresh process
that builds an empty network (`let network = ReteNetwork::new();`), asserts
facts about *this one tool call*, fires, and exits. Nothing the agent did three
calls ago, or in yesterday's session, is visible to a rule firing now.

That ceiling is fine for "don't write `.unwrap()` in this file" but useless for
the patterns that actually cause trouble in long agent runs, which are
*temporal and cross-file*:

- "You've edited the auth module three times this session but never touched its
  tests." (session-scale, count + absence)
- "You've run a destructive `psql`/migration command in the last five tool
  calls — slow down." (call-window presence)
- "You changed the public API and haven't run the build since." (since-last)

This SPEC adds **journey facts**: a small, fixed, *project-defined* family of
predicates derived from the agent's accumulated activity and asserted into the
same network the point-in-time facts already feed. Rules match them with the
**same equality matcher and the same `__script__` Rhai-shaped DSL** they
already use. No new engine machinery.

## The one hard constraint that shapes everything

**The hook is, and stays, a stateless per-invocation process.** This is not an
accident to be fixed — it is *the* property that makes phronesis fire
identically at token 900k as at token 800, immune to context compaction. Any
design that introduces a long-lived in-memory working memory accumulating facts
across tool calls reintroduces exactly the state-and-drift problem the project
exists to kill, and is the "blowing up memory" failure mode.

So journey facts are **never accumulated in the network**. They are *recomputed
from a durable event log every invocation*, over a bounded window, and asserted
into the otherwise-fresh network. This single decision answers four of the
hard questions:

| Hard question | Answer falls out of the stateless model |
|---|---|
| Where does state live? | In an append-only **journal on disk**, not in RAM. |
| When does a journey fact stop being true ("decay/retraction")? | **There is no retraction.** Every invocation recomputes from scratch over the current window. `changed_auth_3x` is asserted on the call where the window holds ≥3 auth edits and simply *isn't asserted* on a later call once the window slides past them. Decay is free. |
| Is firing deterministic? | Journey facts are a **pure function of (journal bytes, invocation timestamp, session id)**. Append-only ⇒ replaying the same journal yields identical facts. The only clock dependency is time-windowed facts, deterministic *given the invocation timestamp* — the existing `clock_facts` contract. |
| How does it not blow up at repo scale? | Read a **bounded suffix** of the journal per call (last *K* records / last *T* seconds). Repo-lifetime counters land in phase 2 — see §"Phase 2". |

## Architecture: three pieces (four in phase 2)

```
                    POST-CHECK (action happened)
  tool call ───► point-in-time fact extraction (existing)
                          │
                          ▼
                 ┌─────────────────┐   tags + optional subject
                 │  TAGGER         │── per record ──►  .phronesis/journey/events.jsonl  (1) journal
                 └─────────────────┘
                                                              │
                    PRE-CHECK and POST-CHECK (every invocation)│
                          ┌───────────────────────────────────┘
                          ▼
                 ┌─────────────────┐   reads bounded suffix
                 │  DERIVE         │── journey_* Facts ──► fresh ReteNetwork  (2) derivation
                 └─────────────────┘                          │
                                                              fire (existing)
```

1. **Journal** (`journey/journal.rs`) — append-only, flock-serialized, one
   compact record per *executed* tool call. Same write discipline as
   `action_log.rs`; separate file and schema. Carries an optional `subject`
   for per-work-unit reads (see §"Subject and the outcomes fold-in").
2. **Derivation** (`journey/derive.rs`) — runs every pre/post invocation,
   *before* `update_agenda()`. Reads a bounded suffix of the journal,
   aggregates, asserts `journey_*` facts. Validates rule selectors against
   `journey.json` at load time (see §"Selector validation").
3. **Tagger** (`journey/tagger.rs`) — turns a tool call's point-in-time facts
   into a compact set of **tags** + a resolved `module` written to the journal
   record. Tags are the project-defined seam (see below). The tagger *reuses
   the existing predicate machinery* — a tagger is a mini-rule whose action is
   "attach a tag," not "block."
4. **Checkpoint** (phase 2) — incrementally-maintained repo-lifetime
   counters. Deferred; see §"Phase 2".

### Why a dedicated journal, not `log.jsonl`

`log.jsonl` is human/`stats`-facing: it mixes `kind:"hook"` and `kind:"mcp"`,
rotates aggressively (50 MB, keeps a single `.1` predecessor), and records the
*decision* (tool, file, exit, consequences) but not the per-call **tags** the
aggregators need. Coupling journey derivation to it would (a) make journey
history hostage to stats-tuned retention, and (b) silently drop call history
at the first rotation. The journal is a purpose-built, versioned, compact
event store with its own retention. (Rejected: reusing `log.jsonl`. Rejected:
a long-lived sqlite — overkill, and a new dependency for what is an append +
tail-read.)

## The project-defined seam: tags via reused predicates

The engine is project-neutral; the journey layer must not hardcode `sql`,
`auth`, `payments`. It doesn't — but be honest about what that buys: every
team will name their own risk surface; the engine just doesn't pick it. The
hardcoding moves from code to project config. That is the actual win, and
"project-defined" is its honest name.

A journey fact is *never* about "sql" in the code — it is about a **tag**, and
tags are defined by the project in `.phronesis/journey.json`, **using the
predicate vocabulary that already exists**:

```json
{
  "version": 1,
  "taggers": [
    { "tag": "sql",     "when": [ { "or": [
                                     { "new_content_contains": "INSERT INTO" },
                                     { "new_content_contains": "DELETE FROM" },
                                     { "file_path_matches": "migrations/" } ] } ] },
    { "tag": "auth",    "when": [ { "file_path_matches": "src/auth/" } ] },
    { "tag": "tests",   "when": [ { "file_path_matches": "tests/" } ] },
    { "tag": "build",   "when": [ { "bash_command_matches": "cargo (build|check|test)" } ] }
  ],
  "modules": [
    { "name": "payments", "paths": [ "src/payments/**", "crates/pay/**" ] },
    { "name": "auth",     "paths": [ "src/auth/**" ] }
  ]
}
```

A `tagger` is structurally a rule whose `when` is evaluated against the *same*
point-in-time facts a normal rule sees, and whose effect is "write this tag to
the journal record" instead of "block/warn." This means **zero new matching
code**: `journey/tagger.rs` builds a throwaway `ReteNetwork`, loads the taggers
as rules whose `then` is the sentinel action `tag`, asserts the same common
facts the hook already asserts, fires, and collects the fired tags. Same DNF
`or` expansion, same regex/AST predicates, same `__script__`.

`modules` group paths into a named entity so a tagger can stamp a `module`
field on each record (used by aggregators that key on modules instead of
tags). Entity identity in v1 is **path-based**; renames are an explicit
non-goal (see §Non-goals).

Note: content matching today is **substring-only** (`new_content_contains`);
the only regex predicate is `bash_command_matches` (commands). A
case-insensitive content-regex predicate would let `sql` taggers match
`select`/`update` too — a small follow-up predicate, out of scope here.

## The journal record

One line per *executed* tool call. Written at **post-check** only — a call
blocked at pre-check (exit 2) never reaches post-check, so **only actions that
actually happened are journaled.** Compact, versioned, tags + optional subject
only (never full content — privacy and size):

```json
{"v":1,"ts":1718700000,"sid":"s-2026-06-18-a1b2","seq":4137,"tool":"Edit","path":"src/auth/login.rs","ext":"rs","module":"auth","tags":["auth"],"subject":"auth-fix-3"}
```

| Field | Meaning |
|---|---|
| `v` | record schema version |
| `ts` | unix seconds (for time windows) |
| `sid` | session id (for `session` window) |
| `seq` | monotonic per-project counter (for call windows; survives rotation via checkpoint when phase 2 lands) |
| `tool` / `path` / `ext` | the call |
| `module` | resolved from `modules` globs, or absent |
| `tags` | tagger output |
| `subject` | optional work-unit id (the outcomes-fold-in seam, see below) |

**No `atoms` field in v1.** The original draft reserved space on every record
for structural deltas (function/import names, matched predicate ids), but no
v1 aggregator consumes them. Reserving schema bytes for a future feature with
no design pressure ages badly — the field gets serialized in tooling and ends
up impossible to change. The lesson: don't reserve fields, version the schema.
Atoms come back when a concrete atom-keyed aggregator wants them, with a
schema bump.

**Session id** comes from the hook payload when the runtime supplies one;
otherwise the `session-context` SessionStart hook writes a fresh id to
`.phronesis/journey/session` and the journal reads it. Falls back to a
date-bucketed id if neither exists. (`context.rs::run_session_context` is the
natural place to stamp it.)

### Subject and the outcomes fold-in

phronesis-mcp 0.12.0 shipped a per-subject outcomes ledger at
`.phronesis/outcomes/<subject>.jsonl` (`outcomes/ledger.rs`). Its module
comment names this SPEC as where it folds in. v1 delivers that fold-in:

- The journal record carries an optional `subject` field.
- The outcomes adapter (`outcomes/cargo.rs` today; future `pytest` etc.) sets
  `subject` on the post-check record for its tool calls and emits
  outcome-shaped tags (`outcome:compile_ok`, `outcome:test_pass`,
  `outcome:bug_caught`).
- Reading "outcomes for subject S" becomes a tail-read of the journal
  filtered by `subject == S` (`journal::read_recent_subject`). Cheap, because
  outcome traffic is sparse compared to edit traffic.
- `outcomes/ledger.rs` deletes. `outcomes/derive.rs` reads via
  `journey::journal::read_recent_subject` instead of the per-subject file.

The net is **one storage layer, one append/tail-read path, one set of
flock-serialized invariants.** The outcomes module keeps its adapters and its
gate-rule emission; only the storage primitives collapse. The fold-in lands
atomically in commit 4 — there is no transitional state where two storage
layers coexist.

## Derivation: rule-driven aggregation

Aggregators are **not** configured separately. Mirroring how the hook already
scans loaded rules for the substring/command patterns it must look for
(`collect_content_patterns`, `collect_bash_command_patterns`), the derivation
pass **scans the loaded rules for `journey_*` conditions and computes exactly
those aggregates — nothing more.** Zero-config, and you never pay to derive a
journey fact no rule consumes.

The fixed, project-neutral v1 aggregator family:

| Predicate (asserted fact) | Args | Meaning |
|---|---|---|
| `journey_occurrence` | `[selector, window]` | **one fact per matching record** — the unit `facts_count` thresholds over; also the unit `== 0` tests for absence |
| `journey_count` | `[selector, window, count]` | the count as a single bindable fact (reporting / equality) |
| `journey_seen` | `[selector, window]` | presence (≥1) — plain boolean fact |
| `journey_since_ge` | `[selector, k]` | emitted for each `k` ≤ distance-since-last (capped at max `k` any rule references) — `facts_count(...) >= 1` is the threshold test |
| `journey_distinct` | `[field, window, count]` | distinct values of `field` (e.g. `path`) in `window` |

Count-style aggregators (`*_occurrence`, `*_since_ge`) emit one fact per unit
so the **existing** `facts_count(...) >= N` and `facts_count(...) == 0` DSL
does the thresholding and absence test; the single-value forms (`*_count`,
`*_distinct`) are for binding/reporting and equality matches. No
arithmetic-in-conditions is required anywhere.

**Window encoding** (one token, parsed in `derive.rs`): `5c` = last 5 calls,
`30m`/`2h`/`7d` = wall-time, `s` = current session. `r` (repo lifetime) is
**phase 2**.

**Thresholding rides the real DSL.** `script_evaluator.rs` supports
`facts_contain('pred',[...])` and `facts_count('pred',[...]) <op> N` with
`<op>` ∈ `>= == > <`. So both "≥ N occurrences" and "exactly zero occurrences"
are first-class — no arithmetic, no extension. `journey_count`'s `?count`
binding stays for reporting / equality matches.

### Selector validation (the silent-typo guard)

`== 0` makes "absence" expressible — and introduces a silent failure mode: a
rule referencing `['tests','s']` when the project's `journey.json` actually
defines the tag as `test` (singular) will find zero records, evaluate the
absence to true, and fire constantly. The wrong default for an
absence-as-zero-count regime.

`derive.rs` therefore validates at rule-load time:

> Walk loaded rules; collect every tag/module selector referenced in
> `journey_*` conditions; verify each appears in `journey.json`'s `taggers` or
> `modules`. **Rules referencing undefined selectors are rejected at load
> time** with the same severity as a malformed predicate.

Cheap, contained to `derive.rs`, and removes the most likely real-world
footgun. Added to the testing matrix as a contract test.

### Headline v1 rules

What `facts_count(...) >= N` and `facts_count(...) == 0` actually nail today.
These four are the demo for v1 — each fully expressible in the supported DSL,
each lands a recognizable pattern in long agent runs.

**1. Auth churn (count over a session)**

```json
{
  "id": "auth-churn-session",
  "phase": "pre",
  "priority": 20,
  "when": [
    { "__script__": "facts_count('journey_occurrence', ['auth','s']) >= 3" }
  ],
  "then": { "warn": "You've edited the auth module 3+ times this session. Take a moment — does this need test coverage or a review before the next change?" }
}
```

**2. Auth churn *without* tests (count + absence)**

```json
{
  "id": "auth-churn-without-tests",
  "phase": "pre",
  "priority": 25,
  "when": [
    { "journey_seen": ["auth","s"] },
    { "__script__": "facts_count('journey_occurrence', ['auth','s']) >= 3" },
    { "__script__": "facts_count('journey_occurrence', ['tests','s']) == 0" }
  ],
  "then": { "warn": "You've edited the auth module 3+ times this session without touching its tests. Add or update coverage before continuing." }
}
```

> **Why the `journey_seen` anchor:** the current engine's `add_rule` filters
> `__script__` conditions out of alpha-network state creation
> (`crates/phronesis/src/network.rs:280-293`). A rule whose `when` is entirely
> `__script__` clauses has no terminal state and never reaches the agenda.
> The `journey_seen` clause provides a non-script leaf that anchors the
> rule; it is logically a no-op here because count ≥ 3 implies presence.
> A first-class fix — letting pure-script rules fire — is a follow-up item.

The composite shows journey rules are **composable through ordinary `when`
conjunction** — no special "but not" syntax. Selector validation makes the
`== 0` clause safe: a typo on `'tests'` is a load-time rejection, not a
silently always-fires rule.

**3. Destructive SQL in the last 5 calls (presence)**

```json
{
  "id": "sql-recent-call-window",
  "phase": "pre",
  "when": [ { "journey_seen": ["sql", "5c"] } ],
  "then": { "warn": "A SQL/migration edit happened in the last 5 tool calls — double-check it ran against the right database." }
}
```

`journey_seen` asserts a plain boolean fact the equality matcher handles
directly; no count needed.

**4. No build in a long while (distance-since-last)**

```json
{
  "id": "build-staleness",
  "phase": "pre",
  "when": [ { "__script__": "facts_count('journey_since_ge', ['build','8']) >= 1" } ],
  "then": { "warn": "8+ tool calls since the last build/test. Run the build before reporting done." }
}
```

`journey_since_ge` emits one fact for each `k` up to the distance since the
last `build`-tagged record, capped at the largest `k` any rule references.
`facts_count(...) >= 1` is the threshold test.

### Cost: how the suffix stays bounded

For each invocation `derive.rs`:

1. From the scanned rules, computes `max_call_window` (largest `Nc`) and
   `max_time_window` (largest time token). If any rule references `s`, the
   read bound additionally satisfies a **session floor** — read backward
   until either the configured suffix cap is hit *or* a record with a
   different `sid` appears (the boundary of the current session). The session
   floor ensures session-only rules still see the whole session even when no
   call/time window is referenced.
2. Reads the journal **tail** — the largest of `max_call_window`,
   `max_time_window`-worth of records, and (if applicable) the session floor
   — by reading from the end (like `action_log::read_recent` but tail-biased;
   v1 may read-whole-file with a hard line cap and optimize to true
   reverse-read later).
3. Buckets the suffix once, emits the scanned aggregates as facts. Records
   outside any referenced window are filtered out before bucketing.

Per-call work is O(window the rules actually ask for), independent of how long
the project has run. A hard suffix cap (default 10k records) bounds the
pathological case where session-floor scan and a misconfigured retention
combine.

## Phase 2 (deferred): repo-lifetime windows + checkpoint

`r` windows (`journey_count(['payments','r',...])` etc.) and the
incrementally-maintained counter file (`journey/checkpoint.rs`) ship in a
follow-up minor release. Reasons:

- The v1 contract — "bounded suffix tail-read" — is well understood and worth
  shipping alone before adding a second storage shape.
- `r` windows want rotation-survival semantics (the checkpoint *is* the only
  thing that has to survive when old journal segments are pruned), which is
  more design surface than the rest of v1 combined.
- No outcomes-or-confidence rule depends on `r` windows; v1 use cases all fit
  in `c`/`m`/`h`/`d`/`s`.

When checkpoint ships, the journal record schema is unchanged; `derive.rs`
gains a checkpoint-read path for `r` selectors; `journal::record` gains a
`checkpoint::apply` step at the tail of post-check; and a `journey-compact`
CLI command lands to prune the journal to retention and fold pruned records
into the checkpoint.

## Where it plugs into the hook

`hook.rs`, both `run_pre_check` and `run_post_check`, after the existing
`assert_*_facts` block and **before** `network.update_agenda()`:

```rust
// Journey facts: recomputed from the durable journal, bounded by what the
// loaded rules actually reference. Failure is non-fatal — a missing/corrupt
// journal must never block an edit; we degrade to "no journey facts."
if let Err(e) = journey::derive::assert_facts(&network, &project_root, &rules, now).await {
    eprintln!("phronesis: journey derivation skipped: {e}");
}
```

Journaling happens once, at the **tail of `run_post_check`**, after the
decision is logged (so it records a call that actually executed):

```rust
journey::journal::record(&project_root, &payload, &tool_name, &file_path, &subject, &tags, &config).ok();
```

Critical ordering / correctness notes:

- **Pre-check sees only *prior* journey.** The current proposed call is not
  yet journaled; it is fully represented by the live point-in-time facts.
  Clean separation: "have you done X before" (journey) vs. "are you doing X
  now" (diff). A pre-check rule can therefore *block* the current call based
  on the trajectory that led to it — the headline capability.
- **Blocked calls are never journaled** (they don't reach post-check), so the
  journey reflects what the agent actually did, not what it attempted.
- **Fail-open.** Every journey path is best-effort. A corrupt journal,
  malformed `journey.json`, or missing config degrades to "no journey facts"
  — never exit-2. (Point-in-time blocking rules are unaffected.) This is the
  opposite of the rules-file policy (fails closed), and deliberate: journey
  facts are advisory enrichment.
- **Disable switch:** `PHRONESIS_NO_JOURNEY=1` mirrors
  `PHRONESIS_NO_ACTION_LOG` — skips both derivation and journaling.
- **Concurrency:** journal writes reuse the `action_log` flock discipline
  (exclusive advisory lock, auto-release on fd close).

## Determinism, tested as a contract

The property that makes this trustworthy gets a dedicated test:

> Given a fixed journal file and a fixed invocation timestamp + sid, two
> derivation runs assert byte-identical fact sets (same ids, predicates, args,
> ordering).

Count/session-window facts are fully deterministic. Time-window facts are
deterministic *given the timestamp* (injected in tests, read from the clock in
prod — same contract as `clock_facts`).

## Tagger performance budget

Building a throwaway `ReteNetwork` per post-check to evaluate taggers adds one
more RETE pass. v1 names a concrete bound rather than asserting "expected
negligible":

> **≤ 2 ms p95 to evaluate 20 taggers against 100 common facts on a 2024-class
> laptop.**

Enforced by a `perf_smoke` test that fails CI on regression. The bound is
generous (production tagger counts are expected single-digit-to-low-teens);
the point is to pin the cost as a measured value, not a hope.

## CLI & MCP surface

```
phr-mcp journey                 # what journey facts would assert right now (table); a "why did this fire" view
phr-mcp journey --json
phr-mcp journey --explain <rule-id>   # show the journey facts a rule depends on + current values
```

MCP: `get_journey` (mirror of the table/json view) so the agent can ask "what
does my trajectory look like" in-conversation. `audit_journey` is **not**
added (journey is inherently about live trajectory, not a tree sweep).
`journey-compact` is deferred to phase 2 (only meaningful once the checkpoint
exists).

`init` changes:
- Write a starter `.phronesis/journey.json` (empty `taggers`/`modules`, with
  commented examples) only under a new `--packs journey` opt-in, so existing
  projects are untouched until they ask for it.
- Add `.phronesis/journey/` to the ignore set (journal + session are local
  state, like `log.jsonl`). `journey.json` itself is **tracked** (project
  knowledge, like `rules.json` and the wiki).

## Module layout

| File | Responsibility |
|---|---|
| `src/journey/mod.rs` (new) | Public surface: `Config` (taggers/modules), `record`, `derive::assert_facts`, errors. Re-exports. |
| `src/journey/journal.rs` (new) | Record schema (`JournalRecord` with `subject`), append (flock, versioned), tail-read, `read_recent_subject`. |
| `src/journey/tagger.rs` (new) | Load taggers as `tag`-action rules; fire against common facts; collect tags + resolve `module`. |
| `src/journey/derive.rs` (new) | Scan rules for `journey_*` + windows; validate selectors against `journey.json`; bucket suffix; emit aggregator facts. |
| `src/hook.rs` (modified) | Call `derive::assert_facts` (both phases); call `journal::record` at post-check tail with subject + tags. |
| `src/hook_facts.rs` (modified) | Expose the common facts the tagger needs. |
| `src/outcomes/ledger.rs` (**deleted**) | Storage folds into the journey journal. |
| `src/outcomes/derive.rs` (modified) | Read per-subject outcome history via `journey::journal::read_recent_subject` instead of the standalone ledger. |
| `src/outcomes/cargo.rs` (modified) | Emit `outcome:*` tags and set `subject` on the post-check record. |
| `src/init.rs` (modified) | `--packs journey` starter config; gitignore `.phronesis/journey/`. |
| `src/context.rs` (modified) | Stamp `.phronesis/journey/session` at SessionStart. |
| `src/main.rs` / `server*.rs` (modified) | `journey` subcommand; `get_journey` MCP tool + params. |

No `phr` library change: `journey_*` are ordinary `Fact`s; taggers ride the
existing `ReteNetwork`/predicate path.

## Testing strategy

| Layer | Tests |
|---|---|
| Journal | append round-trips versioned record incl. `subject`; flock serialization under concurrency (mirror `action_log_concurrency`); tail-read returns last N; `read_recent_subject` filters correctly; malformed lines skipped |
| Tagger | tag attached when `when` matches (regex / path / bash / AST / `or` DNF); no tag when no match; `module` resolution from globs; multiple tags per record |
| Tagger perf | **`perf_smoke`: 20 taggers × 100 facts ≤ 2 ms p95** (CI gate) |
| Selector validation | rule referencing undefined tag/module rejected at load; defined selectors accepted; error message names the missing selector |
| Derive — windows | `5c` honors call count exactly at the boundary; time window honors `ts` cutoff (injected clock); `s` filters by sid; aggregate only what rules reference |
| Derive — aggregators | `journey_occurrence` count feeds `facts_count >= N` and `facts_count == 0` end-to-end; `journey_since_ge` in calls and secs; `journey_distinct` dedups; `journey_seen` is a plain boolean |
| **Determinism** | fixed journal + fixed ts/sid ⇒ identical fact set across two runs (contract test) |
| Outcomes fold-in | `outcomes::derive` reads via `journey::journal::read_recent_subject`; the three confidence signals reproduce the pre-0.13 outcomes-ledger behavior on a fixture journal |
| Hook integration | pre-check blocks on a journey trajectory; blocked call is NOT journaled; post-check journals exactly once; fail-open on corrupt journal/config; `PHRONESIS_NO_JOURNEY` disables both paths |
| init | `--packs journey` writes starter config + gitignore entry; idempotent; other packs untouched |

## Commit plan

1. **`feat(journey): journal with subject — append/tail-read/per-subject read`**
   — `journal.rs`, schema (incl. `subject`), flock append, tail-read,
   `read_recent_subject`, unit + concurrency tests. No hook wiring.
2. **`feat(journey): taggers reuse the predicate engine`** — `tagger.rs`,
   `journey.json` config parse, module resolution, `perf_smoke` budget test,
   tests.
3. **`feat(journey): rule-driven derivation + selector validation`** —
   `derive.rs`, window parsing, the five aggregators, selector validation at
   load, the determinism contract test. Still no hook wiring (driven by a
   test harness).
4. **`feat(journey): wire hook + fold outcomes ledger into the journey
   journal`** — `hook.rs`/`hook_facts.rs`/`context.rs` wiring;
   `outcomes/ledger.rs` deleted; `outcomes/derive.rs` + `outcomes/cargo.rs`
   switched to the journal; `init --packs journey`; fail-open; disable switch;
   integration tests.
5. **`feat: phr-mcp journey command + get_journey MCP tool; bump 0.13.0`** —
   CLI/MCP surface, CLAUDE.md docs, version bump.

Commits 1–3 compile and test independently with no behavior change to existing
hooks. Commit 4 is the first user-visible behavior *and* the ledger fold-in
(atomic — there is no transitional state where two storage layers coexist).
Commit 5 is the release. Phase 2 (`r` windows + checkpoint + `journey-compact`)
lands in a separate follow-up minor release.

## Rollout

1. Install 0.13.0; `phr-mcp init --packs journey` (or hand-write
   `.phronesis/journey.json`).
2. Define a handful of `taggers` for the project's risk surface (auth, sql,
   migrations, payments) and `modules` for the entities worth tracking.
3. Add journey rules to `rules.json` referencing `journey_*` predicates. Start
   with the headline four (auth churn, auth-churn-without-tests, recent SQL,
   build staleness) and add from there.
4. Use `phr-mcp journey` to see live values; `--explain <rule>` to debug why a
   journey rule did/didn't fire.
5. (Phase 2) Add `r`-window rules once the checkpoint ships.

## Non-goals (v1)

These are not gaps to fix in v1 — they are explicit boundary lines so v1 stays
small and the spec doesn't pretend to deliver them.

- **Rename tracking.** Entity identity is path-based. A file moved from
  `src/auth/` to `src/identity/` starts a fresh history. The supported
  workaround is a `modules` glob that spans both paths. Git rename detection
  (`--follow`-style) is not in v1 and not on the roadmap until a concrete use
  case asks for it.
- **First-class `not` (as a schema-level negation).** Absence is *capability*
  v1 already has: `facts_count(..., ['tests','s']) == 0` is supported by
  `script_evaluator.rs` and the auth-churn-without-tests headline rule
  exercises it. What v1 does *not* ship is the syntactic sugar — a `not_seen`
  aggregator, or a top-level `not` condition wrapper — that would make absence
  rules read more naturally. Sugar is deferred; capability is here.
- **`journey_sequence`.** Ordered co-occurrence ("A precedes B within window")
  is an obvious aggregator but its binding semantics (which pair? assert once
  or per-pair? what's bindable downstream?) need their own design pass and no
  headline v1 rule needs it. Deferred to its own follow-up.
- **Atoms.** The original draft reserved an `atoms` field on every record for
  structural deltas. v1 cuts it — no aggregator consumes atoms today, so it
  would ship as dead bytes on every record. Atoms come back with a concrete
  atom-keyed aggregator, behind a schema version bump.

## Open questions

- **Negation ergonomics.** `facts_count(... ) == 0` works but reads
  awkwardly, especially inside a `when` that mixes positive and negative
  clauses. A future round can add either (a) a first-class `not_seen(selector,
  window)` aggregator that asserts a boolean when the selector did *not*
  occur, or (b) a `not` wrapper at the rule-condition level. Both are
  ergonomics, not capability — explicit so we don't conflate them in a future
  spec.
- **Window encoding.** Packing `5c`/`30m`/`s` into a string arg is terse and
  rides the equality matcher, but it's stringly-typed: typos in the *window*
  pass silently (`5C` vs `5c`). v1 mitigation: `derive.rs` validates every
  window token it sees in a loaded rule at startup (the same selector-
  validation pass), and refuses to load rules with malformed windows.
  Structured window objects wait on a richer condition grammar.
- **Subject-aware tail-read cost.** Per-subject reads filter a tail scan.
  Cheap today (outcomes traffic is sparse); the first time a project has
  thousands of outcome records per subject, an index of `subject → byte
  offsets` may be wanted. Revisit when that traffic actually appears.
- **Journal retention.** Default proposal: 30 days or 50k records, whichever
  smaller, configurable like `PHRONESIS_LOG_MAX_BYTES`. Concrete number lands
  with the phase-2 `journey-compact` command; v1 ships with no automatic
  pruning (manual `rm` if a project really wants to start over).
- **Tagger cost across many taggers.** v1's `perf_smoke` pins 20 × 100. A
  project that ends up with 50+ taggers may want an early-exit fast path or a
  shared-network reuse across calls in the same session. Revisit when the
  budget is actually exceeded.
