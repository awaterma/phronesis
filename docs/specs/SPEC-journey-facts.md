# SPEC: journey facts — repo-scale, session-scale predicates

**Status:** draft
**Authors:** Claude, Andrew Waterman
**Date:** 2026-06-18
**Target release:** phronesis-mcp 0.12.0 (MINOR — new fact family, new hook
              stage, new config file, new CLI/MCP surface). No breaking change
              to the `phr` library crate.
**Affects:** new `crates/phronesis-mcp/src/journey/` (mod, `journal.rs`,
              `tagger.rs`, `derive.rs`, `checkpoint.rs`),
              `crates/phronesis-mcp/src/{hook.rs, hook_facts.rs, init.rs,
              main.rs, server.rs, server_params.rs}`. No `phr` change — journey
              facts are ordinary `Fact`s asserted into the existing network.

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
  tests." (session-scale, count + co-occurrence)
- "You've run a destructive `psql`/migration command in the last five tool
  calls — slow down." (call-window)
- "This module has been refactored four separate times over the project's
  life with churn but no net line reduction." (repo-scale)
- "You changed the public API and haven't run the build since." (sequence /
  since-last)

This SPEC adds **journey facts**: a small, fixed, *domain-neutral* family of
predicates derived from the agent's accumulated activity and asserted into the
same network the point-in-time facts already feed. Rules match them with the
**same equality matcher and the same `__script__` Rhai thresholds** they
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
hard questions Meta's framing waved past:

| Hard question | Answer falls out of the stateless model |
|---|---|
| Where does state live? | In an append-only **journal on disk**, not in RAM. |
| When does a journey fact stop being true ("decay/retraction")? | **There is no retraction.** Every invocation recomputes from scratch over the current window. `changed_auth_3x` is asserted on the call where the window holds ≥3 auth edits and simply *isn't asserted* on a later call once the window slides past them. Decay is free. |
| Is firing deterministic? | Journey facts are a **pure function of (journal bytes, invocation timestamp, session id)**. Append-only ⇒ replaying the same journal yields identical facts. The only clock dependency is time-windowed facts, which are deterministic *given the invocation timestamp* — exactly the existing `clock_facts` contract. |
| How does it not blow up at repo scale? | Read a **bounded suffix** of the journal per call (last *K* records / last *T* seconds); roll **repo-lifetime** counters into a compacted **checkpoint** so cost is O(window), not O(project age). |

## Architecture: four pieces

```
                    POST-CHECK (action happened)
  tool call ───► point-in-time fact extraction (existing)
                          │
                          ▼
                 ┌─────────────────┐   one compact record per executed tool call
                 │  TAGGER         │── tags + atoms ──►  .phronesis/journey/events.jsonl   (1) journal
                 └─────────────────┘                              │
                          │ increment                             │
                          ▼                                       │
                 .phronesis/journey/state.json  (3) checkpoint    │
                 (repo-lifetime counters)                         │
                                                                  │
                    PRE-CHECK and POST-CHECK (every invocation)   │
                          ┌───────────────────────────────────────┘
                          ▼
                 ┌─────────────────┐   reads bounded suffix + checkpoint
                 │  DERIVE         │── journey_* Facts ──► fresh ReteNetwork  (2) derivation
                 └─────────────────┘                          │
                                                              fire (existing)
```

1. **Journal** (`journey/journal.rs`) — append-only, flock-serialized, one
   compact record per *executed* tool call. Same write discipline as
   `action_log.rs`; separate file and schema.
2. **Derivation** (`journey/derive.rs`) — runs every pre/post invocation,
   *before* `update_agenda()`. Reads a bounded suffix of the journal (+
   checkpoint for repo-window facts), aggregates, asserts `journey_*` facts.
3. **Checkpoint** (`journey/checkpoint.rs`) — incrementally-maintained
   repo-lifetime counters, so repo-window facts are O(1) and survive journal
   rotation.
4. **Tagger** (`journey/tagger.rs`) — turns a tool call's point-in-time facts
   into a compact set of **tags** + **atoms** written to the journal. Tags are
   the domain-neutrality seam (see below). The tagger *reuses the existing
   predicate machinery* — a tagger is a mini-rule whose action is "attach a
   tag," not "block."

### Why a dedicated journal, not `log.jsonl`

`log.jsonl` is human/`stats`-facing: it mixes `kind:"hook"` and `kind:"mcp"`,
rotates aggressively (50 MB, keeps a single `.1` predecessor), and records the
*decision* (tool, file, exit, consequences) but not the per-edit **atoms**
(`function_added` names, matched predicates) the aggregators need. Coupling
journey derivation to it would (a) make journey history hostage to stats-tuned
retention, and (b) silently drop repo-scale history at the first rotation. The
journal is a purpose-built, versioned, compact event store with its own
retention and a checkpoint companion. (Rejected: reusing `log.jsonl`. Rejected:
a long-lived sqlite — overkill, and a new dependency for what is an append +
tail-read.)

## The domain-neutrality seam: tags via reused predicates

The engine is explicitly domain-neutral; the journey layer must not hardcode
`sql`, `auth`, `payments`. It doesn't. A journey fact is *never* about "sql" in
the code — it is about a **tag**, and tags are defined by the project in
`.phronesis/journey.json`, **using the predicate vocabulary that already
exists**:

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
`or` expansion, same regex/AST predicates, same Rhai. `touched_sql_in_5_calls`
decomposes into "tagger attaches `sql`" (project-defined, reused engine) +
"aggregator counts `sql` over 5 calls" (fixed, domain-neutral).

`modules` group paths into a named entity so `refactored_module_x` can
aggregate across the files of a module. Entity identity in v1 is **path-based**
(normalized, repo-relative); renames are not tracked (open question).

Note: content matching today is **substring-only** (`new_content_contains`);
the only regex predicate is `bash_command_matches` (commands). A
case-insensitive content-regex predicate would let `sql` taggers match
`select`/`update` too — a small follow-up predicate, out of scope here.

## The journal record

One line per *executed* tool call. Written at **post-check** only — a call
blocked at pre-check (exit 2) never reaches post-check, so **only actions that
actually happened are journaled.** Compact, versioned, tags+atoms only (never
full content — privacy and size):

```json
{"v":1,"ts":1718700000,"sid":"s-2026-06-18-a1b2","seq":4137,"tool":"Edit","path":"src/auth/login.rs","ext":"rs","module":"auth","tags":["auth","sql"],"atoms":["fn_added:authenticate","import_added:sqlx"]}
```

| Field | Meaning |
|---|---|
| `v` | record schema version |
| `ts` | unix seconds (for time windows) |
| `sid` | session id (for `session` window) |
| `seq` | monotonic per-project counter (for call windows; survives rotation via checkpoint) |
| `tool` / `path` / `ext` | the call |
| `module` | resolved from `modules` globs, or absent |
| `tags` | tagger output |
| `atoms` | structural facts already computed at hook time (function/import deltas, matched predicate ids) — the raw material for richer aggregators |

**Session id** comes from the hook payload when the runtime supplies one;
otherwise the `session-context` SessionStart hook writes a fresh id to
`.phronesis/journey/session` and the journal reads it. Falls back to a
date-bucketed id if neither exists. (`context.rs::run_session_context` is the
natural place to stamp it.)

## Derivation: rule-driven aggregation

Aggregators are **not** configured separately. Mirroring how the hook already
scans loaded rules for the substring/command patterns it must look for
(`collect_content_patterns`, `collect_bash_command_patterns`), the derivation
pass **scans the loaded rules for `journey_*` conditions and computes exactly
those aggregates — nothing more.** Zero-config, and you never pay to derive a
journey fact no rule consumes.

The fixed, domain-neutral aggregator family:

| Predicate (asserted fact) | Args | Meaning |
|---|---|---|
| `journey_occurrence` | `[selector, window]` | **one fact per matching record** — the unit `facts_count` thresholds over |
| `journey_count` | `[selector, window, count]` | the count as a single bindable fact (reporting / equality) |
| `journey_seen` | `[selector, window]` | presence (≥1) — plain boolean fact |
| `journey_sequence` | `[selectorA, selectorB, window]` | an A record precedes a B record within `window` (ordered co-occurrence) |
| `journey_since_ge` | `[selector, k]` | emitted for each `k` ≤ distance-since-last (capped at max `k` any rule references) — `facts_count(...) >= 1` is the threshold test |
| `journey_distinct` | `[field, window, count]` | distinct values of `field` (e.g. `path`) in `window` |

Count-style aggregators (`*_occurrence`, `*_since_ge`) emit one fact per unit so
the **existing** `facts_count(...) >= N` DSL does the thresholding; the
single-value forms (`*_count`, `*_distinct`) are for binding/reporting and
equality matches. No arithmetic-in-conditions is required anywhere.

**Window encoding** (one token, parsed in `derive.rs`): `5c` = last 5 calls,
`30m`/`2h`/`7d` = wall-time, `s` = current session, `r` = repo lifetime.

**Thresholding rides the real DSL, not arithmetic.** The `__script__` layer is
*not* general Rhai — `script_evaluator.rs` supports only
`facts_contain('pred',[...])` and `facts_count('pred',[...]) >= N` (after `?n`
substitution, `n >= 3` becomes `3 >= 3`, which it rejects; see
SPEC-confidence-scoring §3 and Appendix). So counting works by **emitting one
`journey_occurrence(selector, window)` fact per matching record**, then
thresholding with `facts_count`. `journey_count`'s `?count` binding stays for
reporting / equality matches; numeric thresholds go through `facts_count`.

Worked example, the "edited auth 3× this session but never the tests" rule:

```json
{
  "id": "auth-churn-without-tests",
  "phase": "pre",
  "priority": 20,
  "when": [
    { "__script__": "facts_count('journey_occurrence', ['auth','s']) >= 3" },
    { "__script__": "facts_count('journey_occurrence', ['tests','s']) == 0" }
  ],
  "then": { "warn": "You've edited the auth module 3+ times this session without touching its tests. Add or update coverage before continuing." }
}
```

(The second clause is the supported way to express absence today —
`facts_count(... ) == 0` — until first-class `not` lands; see Open Questions.)

`touched destructive sql in last 5 calls`:

```json
{ "when": [ { "journey_seen": ["sql", "5c"] } ],
  "then": { "warn": "A SQL/migration edit happened in the last 5 tool calls — double-check it ran against the right database." } }
```

(`journey_seen` is the presence form — it asserts a plain boolean fact the
equality matcher handles directly, no count needed.)

`changed public API, haven't built since` — `journey_since` emits its distance
as facts the count DSL can threshold (one `journey_since_ge(selector, k)` fact
for each `k` up to the distance, capped at the largest `k` any rule references):

```json
{ "when": [ { "__script__": "facts_count('journey_since_ge', ['build','8']) >= 1" } ],
  "then": { "warn": "8+ tool calls since the last build/test. Run the build before reporting done." } }
```

### Cost: how the suffix stays bounded

For each invocation `derive.rs`:

1. From the scanned rules, computes `max_call_window` (largest `Nc`) and
   `max_time_window` (largest time token). Repo (`r`) and session (`s`) windows
   are handled via checkpoint / sid filter, not by reading more bytes.
2. Reads the journal **tail** — last `max_call_window` records, and/or records
   with `ts >= now - max_time_window`, whichever is larger — by reading from the
   end (like `action_log::read_recent` but tail-biased; v1 may read-whole-file
   with a hard line cap and optimize to true reverse-read later).
3. Buckets the suffix once, emits the scanned aggregates as facts.

So per-call work is O(window the rules actually ask for), independent of how
long the project has run. A project whose largest window is `20c`/`2h` reads at
most a few KB regardless of a multi-year journal.

### Repo-scale without replay: the checkpoint

`journey_count(["payments","r",...])` must not require replaying the whole
journal. At **append time** (post-check, where we already hold the new record),
`checkpoint.rs` increments a compact counters file:

```json
{"v":1,"through_seq":4137,"through_ts":1718700000,
 "counters":{"tag:sql":318,"tag:auth":204,"module:payments:edits":211,"module:payments:net_lines":-1400}}
```

Repo-window facts read the checkpoint directly (O(1)). The checkpoint is the
*only* thing that must survive journal rotation, so repo-lifetime facts remain
correct even after old journal segments are pruned. Checkpoint updates are
idempotent on `seq` (re-processing a record is a no-op), preserving determinism
under retries. **Phase 2** — v1 ships `c`/`m`/`h`/`d`/`s` windows (bounded
suffix only); `r` windows + checkpoint land in a follow-up commit so the first
release is small and the suffix path is proven before counters are added.

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

Journaling happens once, at the **tail of `run_post_check`**, after the decision
is logged (so it records a call that actually executed):

```rust
journey::journal::record(&project_root, &payload, &tool_name, &file_path, &atoms, &config).ok();
journey::checkpoint::apply(&project_root, &record).ok();   // phase 2
```

Critical ordering / correctness notes:

- **Pre-check sees only *prior* journey.** The current proposed call is not yet
  journaled; it is fully represented by the live point-in-time facts. This is a
  clean separation, not a gap: "have you done X before" (journey) vs. "are you
  doing X now" (diff). A pre-check rule can therefore *block* the current call
  based on the trajectory that led to it — the headline capability.
- **Blocked calls are never journaled** (they don't reach post-check), so the
  journey reflects what the agent actually did, not what it attempted.
- **Fail-open.** Every journey path is best-effort. A corrupt journal,
  malformed `journey.json`, or missing checkpoint degrades to "no journey
  facts" — it never turns into an exit-2. (Point-in-time blocking rules are
  unaffected.) This is the opposite of the rules-file policy, which fails
  closed, and is deliberate: journey facts are advisory enrichment.
- **Disable switch:** `PHRONESIS_NO_JOURNEY=1` mirrors
  `PHRONESIS_NO_ACTION_LOG` — skips both derivation and journaling.
- **Concurrency:** journal + checkpoint writes reuse the `action_log` flock
  discipline (exclusive advisory lock, auto-release on fd close).

## Determinism, tested as a contract

The property that makes this trustworthy gets a dedicated test:

> Given a fixed journal file and a fixed invocation timestamp + sid, two
> derivation runs assert byte-identical fact sets (same ids, predicates, args,
> ordering).

Count/session/repo-window facts are fully deterministic. Time-window facts are
deterministic *given the timestamp* (injected in tests, read from the clock in
prod — same contract as `clock_facts`). The checkpoint is deterministic via
`seq`-idempotent application.

## CLI & MCP surface

```
phr-mcp journey                 # what journey facts would assert right now (table); a "why did this fire" view
phr-mcp journey --json
phr-mcp journey --explain <rule-id>   # show the journey facts a rule depends on + current values
phr-mcp journey-compact         # prune journal to retention window, fold into checkpoint (maintenance)
```

MCP: `get_journey` (mirror of the table/json view) so the agent can ask "what
does my trajectory look like" in-conversation, and `audit_journey` is **not**
added (journey is inherently about live trajectory, not a tree sweep).

`init` changes:
- Write a starter `.phronesis/journey.json` (empty `taggers`/`modules`, with
  commented examples) only under a new `--packs journey` opt-in, so existing
  projects are untouched until they ask for it.
- Add `.phronesis/journey/` to the ignore set (journal + checkpoint + session
  are local state, like `log.jsonl`). `journey.json` itself is **tracked**
  (project knowledge, like `rules.json` and the wiki).

## Module layout

| File | Responsibility |
|---|---|
| `src/journey/mod.rs` (new) | Public surface: `Config` (taggers/modules), `record`, `derive::assert_facts`, errors. Re-exports. |
| `src/journey/journal.rs` (new) | Record schema (`JournalRecord`), append (flock, versioned), tail-read with bound. |
| `src/journey/tagger.rs` (new) | Load taggers as `tag`-action rules; fire against common facts; collect tags + resolve `module`. |
| `src/journey/derive.rs` (new) | Scan rules for `journey_*` + windows; bucket suffix; emit aggregator facts. |
| `src/journey/checkpoint.rs` (new, phase 2) | Repo-lifetime counters; `seq`-idempotent apply; O(1) repo-window reads. |
| `src/hook.rs` (modified) | Call `derive::assert_facts` (both phases); call `journal::record` + `checkpoint::apply` at post-check tail. |
| `src/hook_facts.rs` (modified) | Expose the `atoms` already computed so the journal can record them without recomputation. |
| `src/init.rs` (modified) | `--packs journey` starter config; gitignore `.phronesis/journey/`. |
| `src/context.rs` (modified) | Stamp `.phronesis/journey/session` at SessionStart. |
| `src/main.rs` / `server*.rs` (modified) | `journey` / `journey-compact` subcommands; `get_journey` MCP tool + params. |

No `phr` library change: `journey_*` are ordinary `Fact`s; taggers ride the
existing `ReteNetwork`/predicate path.

## Testing strategy

| Layer | Tests |
|---|---|
| Journal | append round-trips versioned record; flock serialization under concurrency (mirror `action_log_concurrency`); tail-read returns last N; malformed lines skipped; rotation predecessor read |
| Tagger | tag attached when `when` matches (regex / path / bash / AST / `or` DNF); no tag when no match; `module` resolution from globs; multiple tags per record |
| Derive — windows | `5c` honors call count exactly at the boundary; time window honors `ts` cutoff (injected clock); `s` filters by sid; aggregate only what rules reference |
| Derive — aggregators | `journey_count` value binding feeds `__script__` threshold end-to-end; `journey_since` in calls and secs; `journey_sequence` requires order; `journey_distinct` dedups |
| **Determinism** | fixed journal + fixed ts/sid ⇒ identical fact set across two runs (the contract test) |
| Checkpoint (phase 2) | `seq`-idempotent apply; repo count == full-replay count; survives simulated rotation |
| Hook integration | pre-check blocks on a journey trajectory; blocked call is NOT journaled; post-check journals exactly once; fail-open on corrupt journal/config; `PHRONESIS_NO_JOURNEY` disables both paths |
| init | `--packs journey` writes starter config + gitignore entry; idempotent; other packs untouched |

## Commit plan

1. **`feat(journey): journal record + append/tail-read`** — `journal.rs`,
   schema, flock append, bounded tail-read, unit + concurrency tests. No hook
   wiring.
2. **`feat(journey): taggers reuse the predicate engine`** — `tagger.rs`,
   `journey.json` config parse, module resolution, tests.
3. **`feat(journey): rule-driven derivation of journey_* facts`** —
   `derive.rs`, window parsing, the five aggregators, the determinism contract
   test. Still no hook wiring (driven by a test harness).
4. **`feat(journey): wire derivation + journaling into hooks; init pack`** —
   `hook.rs`/`hook_facts.rs`/`init.rs`/`context.rs`, fail-open, disable switch,
   integration tests, `--packs journey`.
5. **`feat: phr-mcp journey command + get_journey MCP tool; bump 0.12.0`** —
   CLI/MCP surface, CLAUDE.md docs, version bump.
6. **`feat(journey): repo-lifetime checkpoint (r windows)`** — phase 2:
   `checkpoint.rs`, `journey-compact`, idempotent apply, repo-window tests.

Commits 1–3 compile and test independently with no behavior change to existing
hooks. Commit 4 is the first user-visible behavior; 5 the release; 6 the
repo-scale follow-up.

## Rollout

1. Install 0.12.0; `phr-mcp init --packs journey` (or hand-write
   `.phronesis/journey.json`).
2. Define a handful of `taggers` for the project's risk surface (auth, sql,
   migrations, payments) and `modules` for the entities worth tracking.
3. Add journey rules to `rules.json` referencing `journey_*` predicates.
4. Use `phr-mcp journey` to see live values; `--explain <rule>` to debug why a
   journey rule did/didn't fire.
5. (Phase 2) Add `r`-window rules once checkpoint ships.

## Open questions

- **`not` support.** Several high-value journey rules ("changed X but *not* its
  tests") need negation, which the rule schema lacks (noted as planned in
  `CLAUDE.md`). Until then, absence is expressed as
  `facts_count('journey_occurrence', [selector, window]) == 0`. A first-class
  `not` is the cleanest fix and is
  arguably a prerequisite for the headline use cases — sequence with this
  SPEC's release.
- **Window encoding.** Packing `5c`/`30m`/`s`/`r` into a string arg is terse
  and rides the equality matcher, but it's stringly-typed. Alternative: a
  structured window object once the schema grows a richer condition grammar.
- **Entity identity across renames.** v1 is path-based; a file moved from
  `src/auth/` to `src/identity/` starts a fresh history. Git rename detection
  (or a `module` glob that spans both) could stitch them — defer.
- **Cross-session vs. cross-repo.** `s` (session) and `r` (repo) cover the two
  lifetimes we have a home for. "Across my last N sessions" would need session
  boundaries in the checkpoint; defer until asked for.
- **Journal retention vs. checkpoint.** Once checkpoints exist, the raw journal
  only needs to retain `max(window)` worth of suffix; `journey-compact` prunes
  the rest. Default retention TBD (proposal: 30d or 50k records, whichever
  smaller, configurable like `PHRONESIS_LOG_MAX_BYTES`).
- **Tagger cost.** Building a throwaway network per post-check to evaluate
  taggers adds one more RETE pass. Expected negligible (taggers are few, facts
  already computed), but worth a `perf_smoke` guard.
