# SPEC: journey_filtered_since_ge — distance counted over a subset

**Status:** draft
**Authors:** Claude, Andrew Waterman
**Date:** 2026-06-21
**Target release:** phronesis-mcp 0.14.0 (MINOR — new aggregator predicate
              in the journey-facts family; no breaking changes to the
              existing four)
**Affects:** `crates/phronesis-mcp/src/journey/derive.rs` (rule scan +
              one new emitter), tests in
              `crates/phronesis-mcp/tests/journey_derive.rs`,
              `crates/phronesis-mcp/docs/CLAUDE.md`, and the
              SPEC-journey-facts §"Five aggregators" reference.

## How we got here

The 0.13.x `journey_since_ge(selector, k)` aggregator emits a k-ladder of
facts up to the **distance-since-last** record matching `selector` — and
"distance" means raw record count, every tool call. A rule like
`build-staleness` that fires on "8+ tool calls since the last build/test"
ends up flagging long Bash sessions that did no writes at all, because
every `echo`, `ls`, and `grep` counts toward the distance the same as a
file edit.

The local salvage was to gate the rule's `when` on
`change_type: edit|write|multiedit|replace|write_file` so it doesn't
*fire* on observational tool calls. But the threshold itself is still 8
raw tool calls — the model could do 8 Reads + 1 Edit and trip the rule
even though no code changed since the last build. The rule's intent is
"you haven't built since you started writing again," which the existing
aggregator can't express.

The pattern generalizes. Users will reach for similar shapes any time the
threshold is about a *kind* of activity, not all activity:

- "≥ N writes since last build" — the immediate case
- "≥ N SQL-tagged calls since last migration command" — a payments team
- "≥ N test failures since last green run" — once outcomes are unified
  enough to express this
- "≥ N auth-module edits since the last auth-test edit" — drift
  detection within a module

All have the shape: *count records matching one selector, since the
most recent record matching a different selector.*

## What we are not changing

- **`journey_since_ge(selector, k)` stays as-is.** Existing rules keep
  working; the count-all-records semantics is the right shape for
  "anything happened" rules.
- **The other four aggregators stay as-is.** `journey_occurrence`,
  `journey_count`, `journey_seen`, `journey_distinct` are unchanged.
- **No changes to the tagger config schema.** The new aggregator
  consumes the same `[selector, ...]` tags the existing four do.
- **Window semantics unchanged.** The same `c`/`m`/`h`/`d`/`s` window
  family applies; `r` (repo lifetime) remains phase 2.
- **Engine `__script__` DSL unchanged.** No new comparison operators or
  arithmetic; `facts_count(...) >= 1` is still the threshold test.

## What we are adding

One new aggregator predicate:

| Predicate (asserted fact) | Args | Meaning |
|---|---|---|
| `journey_filtered_since_ge` | `[target_selector, counted_selector, k]` | emitted for each `k` ≤ (count of `counted_selector` records that appear *after* the most recent `target_selector` record), capped at max `k` any rule references for that `(target, counted)` pair |

`facts_count('journey_filtered_since_ge', ['build','write','8']) >= 1`
fires when 8 or more write-tagged records sit between the most recent
build-tagged record and the present.

### Worked example

The current build-staleness rule, with the salvage filter, looks like:

```json
{
  "id": "build-staleness",
  "phase": "pre",
  "priority": 5,
  "when": [
    { "__script__": "facts_count('journey_since_ge', ['build','8']) >= 1" },
    { "or": [
        { "change_type": "edit" },
        { "change_type": "write" },
        { "change_type": "multiedit" },
        { "change_type": "replace" },
        { "change_type": "write_file" }
    ]}
  ],
  "then": { "warn": "8+ tool calls since the last build/test. Run the build before reporting done." }
}
```

The new aggregator collapses the `or` filter into the threshold itself:

```json
{
  "id": "build-staleness",
  "phase": "pre",
  "priority": 5,
  "when": [
    { "__script__": "facts_count('journey_filtered_since_ge', ['build','write','8']) >= 1" }
  ],
  "then": { "warn": "8+ writes since the last build/test. Run the build before reporting done." }
}
```

The rule's message text now matches reality. And the project needs a
`write` tagger:

```json
{ "tag": "write", "when": [ { "or": [
    { "change_type": "edit" },
    { "change_type": "write" },
    { "change_type": "multiedit" },
    { "change_type": "replace" },
    { "change_type": "write_file" }
] } ] }
```

(One tagger pays for every "writes since X" rule the project adds.)

## Implementation

Three small additions in `journey/derive.rs`:

### 1. Rule scan picks up the new predicate

The existing scan walks `__script__` clauses and pure-fact clauses
looking for `journey_*` predicates and the selectors + windows they
reference. Extend it to also collect `(target_selector,
counted_selector, max_k)` triples for `journey_filtered_since_ge`.

The selector-validation pass already errors on undefined tags/modules
in `journey.json`; it just needs to validate *both* selectors of the
new aggregator instead of one.

### 2. New emitter walks records once per (target, counted) pair

```rust
async fn emit_filtered_since_ge(
    network: &ReteNetwork,
    records: &[JournalRecord],
    scan: &RuleScan,
) {
    for ((target, counted), max_k) in &scan.filtered_since_max_k {
        // Find the index of the most recent target-matching record.
        let Some(target_idx) = records.iter().rposition(|r| matches_selector(r, target)) else {
            continue;
        };
        // Count counted-matching records *after* that index.
        let count = records[target_idx + 1..]
            .iter()
            .filter(|r| matches_selector(r, counted))
            .count() as u32;
        let upper = (*max_k).min(count);
        for k in 1..=upper {
            let id = format!("journey_filtered_since_ge:{}:{}:{}", target, counted, k);
            let _ = network.assert_fact(Fact {
                id,
                predicate: "journey_filtered_since_ge".to_string(),
                args: vec![target.clone(), counted.clone(), k.to_string()],
                timestamp: 0,
            }).await;
        }
    }
}
```

The shape mirrors `emit_since_ge` exactly; only the inner counting
loop changes from "count all records after target" to "count records
matching `counted` after target."

### 3. Determinism test extended

The existing
`crates/phronesis-mcp/tests/journey_derive.rs::determinism_contract`
test runs the derive pass twice over a fixed journal and asserts
byte-identical fact sets. Add a fixture rule referencing
`journey_filtered_since_ge` and confirm the property holds for the new
aggregator too. (Sorting in the emit loop is by `BTreeMap` iteration
order on the `(target, counted)` pair, which is deterministic.)

### Edge cases

- **No target record in the window suffix** → emit nothing (matches
  existing `journey_since_ge` behavior).
- **No counted records between target and end** → `count = 0`, emit
  nothing.
- **`target_selector == counted_selector`** → `count` is "number of
  self-matching records strictly after the most recent self-matching
  record" — which is always 0. The aggregator emits nothing, which is
  the right behavior (a fact can't be "after the last instance of
  itself" by definition).
- **Both selectors undefined in `journey.json`** → load-time rejection
  via the existing selector-validation guard.

## Cost

Per invocation, derive does one extra `rposition` + one extra `filter +
count` pass over the suffix per `(target, counted)` pair the loaded
rules reference. With the current SUFFIX_HARD_CAP of 10k and the
expected handful of `journey_filtered_since_ge` rules (one per
"writes since X" pattern a project cares about), this is sub-millisecond.

The `perf_smoke` test budget on the tagger isn't affected — taggers
run once at journal-append time, the new aggregator runs at derive
time over already-loaded records.

## Out of scope

- **A "windowed-since-ge" variant** that counts only records matching
  both selectors within a window (e.g., "writes since last build, in
  the last hour"). Composable as a separate aggregator if a use-case
  surfaces; not needed for the immediate friction.
- **An OR shape on either selector.** A rule that wants "writes since
  last build OR check OR test" can ship one tagger that emits
  `build` on any of `(cargo build|check|test)` (which the default
  tagger already does). Don't push OR semantics into the aggregator
  args.
- **Negation in the target selector** ("writes since last
  *non-write*"). Doesn't have a clear use case; skip unless asked for.
- **Phase 2 `r` window with filtered counting.** When the checkpoint
  ships, `journey_filtered_since_ge` over `r` is a natural extension
  but its semantics (count writes since the last build in repo
  history) want a separate think. Defer.

## Open questions

- **Naming.** `journey_filtered_since_ge` is clear but verbose.
  Alternatives considered:
  - `journey_subset_since_ge` — subset of records, since target
  - `journey_typed_since_ge` — typed by counted selector
  - `journey_among_since_ge` — among records of one tag, since another
  - `journey_X_since_Y` — too generic to grep for
  Lean: keep `filtered`; document the shape in CLAUDE.md so readers
  understand the predicate at a glance.
- **Migration story.** Projects with existing `journey_since_ge` rules
  don't need to change anything. Projects that want the new semantics
  add a write-tagger and a new rule referencing the new aggregator.
  No automated migration; document in CHANGELOG.
- **Ordering of args.** `[target, counted, k]` reads as "since target,
  count among counted, threshold k." `[counted, target, k]` would read
  "count among counted, since target." Slight preference for the
  former (matches "since" verb order in English).

## Rollout plan

1. Implement scan + emit in `derive.rs` (~40 lines + matching tests).
2. Extend selector validation to handle the two-selector form.
3. Add inline unit tests for the new aggregator's edge cases.
4. Extend the determinism contract test.
5. Update `SPEC-journey-facts.md` "Five aggregators" reference to a
   "Six aggregators" reference, with the new entry alongside.
6. CHANGELOG: note the new predicate, give the writes-since-build
   migration story.
7. MINOR bump (`phr-mcp` 0.13.x → 0.14.0; `phr` itself stays at 0.13.3
   since the engine is unchanged).

## Why this isn't a 0.13.x

Pre-1.0 semver: MINOR is "new feature surface." A new predicate in the
journey-fact family is a new surface piece. PATCH would understate the
change.

## References

- `crates/phronesis-mcp/src/journey/derive.rs::emit_since_ge` — the
  one-selector aggregator the new one mirrors.
- `crates/phronesis-mcp/src/journey/derive.rs::matches_selector` —
  the shared selector-matching helper.
- `docs/specs/SPEC-journey-facts.md` §"The fixed v1 aggregator
  family" — table that will become "v1-plus-one."
- `.phronesis/rules.json::build-staleness` — the rule the new
  aggregator replaces a workaround in (locally; gitignored).
