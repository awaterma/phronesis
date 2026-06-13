# Domain-neutral working memory: remove `get_persistent_facts` — Design

## Problem

`ReteNetwork::get_persistent_facts()` (in `crates/phronesis/src/network.rs`)
returns the facts whose predicate appears in a hardcoded
`PERSISTENT_PREDICATES` list. That list mixes
generic demo predicates with **consumer-specific** ones (a downstream host's
game-state vocabulary). This couples the "domain-neutral" engine to one
consumer's predicate names.

No code inside phronesis calls the method — it exists solely for an external
consumer, which since 0.11 has a general way to do the same thing (the public
fact-query API: `facts_snapshot`, `facts_matching_predicate`, …).

## Goal

Get consumer-specific predicates out of the engine without breaking the
in-flight consumer migration. The engine should know nothing about any
consumer's predicate vocabulary.

## Approach — two phases: deprecate in 0.11, delete in 0.12

### Phase 1 — 0.11 (this release)

1. **Add** `ReteNetwork::facts_matching_predicates(&self, predicates: &[&str])
   -> Result<Vec<Fact>, ReteError>` — facts whose predicate is in the given
   set; owned clones, sorted by fact id (consistent with the other 0.11
   query methods). Generic: the natural replacement for "give me the facts I
   treat as persistent," with the predicate set owned by the caller.
2. **Deprecate** `get_persistent_facts` with
   `#[deprecated(since = "0.11.0", note = "define your own persistent
   predicate set and call facts_matching_predicates(&YOUR_SET); this method
   hardcodes consumer-specific predicates and will be removed in 0.12")]`.
   It keeps working unchanged.
3. Keep phronesis's `-D warnings` gate green: any in-engine test exercising
   the method gets `#[allow(deprecated)]` (or moves to the new method).
4. **CHANGELOG**: a "Deprecated" note under 0.11.0. The consumer-side
   replacement snippet lives in the private (gitignored) migration map — not
   in any published file.

### Phase 2 — 0.12 (after the consumer migrates off it)

5. **Delete** `get_persistent_facts` and the `PERSISTENT_PREDICATES` const.
   Bump to 0.12.0. CHANGELOG "Removed."

## Out of scope

- `restore_persistent_facts` / `restore_persistent_facts_sync` — already
  generic (bulk-assert any `Vec<Fact>`), no leak; left as-is.
- Any renaming.

## Testing

- Unit test for `facts_matching_predicates`: set membership, empty set →
  empty, multiple predicates, a predicate with no facts, deterministic
  ordering.
- Deprecation compiles clean under `-D warnings` (no internal callers warn).
- Existing suite (734 tests) stays green.

## Risk / sequencing

The external consumer is mid-0.11 migration. Phase 1 is **non-breaking** — a
deprecation is a warning, not an error — so it ships in 0.11 without
disrupting that work. Phase 2 is gated on the consumer having moved onto
`facts_matching_predicates`.
