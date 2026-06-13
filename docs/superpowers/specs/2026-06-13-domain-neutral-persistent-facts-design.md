# Domain-neutrality cleanup — Design

## Problem

phronesis bills itself as a domain-neutral RETE engine, but a downstream
consumer's vocabulary has leaked into the engine and its tracked docs:

- `ReteNetwork::get_persistent_facts()` (network.rs) hardcodes a
  `PERSISTENT_PREDICATES` list of a consumer's game-state predicates.
- `engine_types.rs`'s core module doc frames the types as "card game logic"
  and uses domain-flavored doc examples.
- ~5 spec docs and one wiki decision name the consumer or use its vocabulary.

No phronesis code calls `get_persistent_facts` — it exists only for an
external consumer, which since 0.11 has a general replacement (the fact-query
API).

## Goal

The engine and its tracked docs know nothing of any specific consumer's
vocabulary. No newly-pushed file names a specific dependent.

**Explicitly out of scope:** retracting copies already on `origin/main`
(network.rs predicates, the published spec docs). That would need a
force-push of published history — deferred as a separate decision. This work
stops carrying the consumer *forward*; it does not rewrite published history.

## Scope & approach

### A. Engine API — two phases (deprecate 0.11, delete 0.12)

1. **Add** `ReteNetwork::facts_matching_predicates(&self, predicates: &[&str])
   -> Result<Vec<Fact>, ReteError>` — facts whose predicate is in the given
   set; owned, sorted by fact id. The generic replacement for
   `get_persistent_facts`, with the predicate set owned by the caller.
2. **Deprecate** `get_persistent_facts` with `#[deprecated(since = "0.11.0",
   note = "define your own predicate set and call
   facts_matching_predicates(&YOUR_SET); this hardcodes consumer-specific
   predicates and will be removed in 0.12")]`. Keep it working; neutralize its
   inline `// <consumer>:` comment.
3. **Delete** `get_persistent_facts` + `PERSISTENT_PREDICATES` in **0.12**,
   once the consumer has migrated off it.
4. Keep phronesis's `-D warnings` gate green (any in-engine test gets
   `#[allow(deprecated)]` or moves to the new method).

### B. Docs & comments — all in 0.11

5. `engine_types.rs`: module doc "card game logic" → neutral ("rule-based
   logic" / "the RETE network"); neutralize domain-flavored doc examples.
6. Scrub the consumer's name from the spec docs that mention it (rule-schema
   v2 PLAN + SPEC, wiki-drift PLAN + SPEC, rhai-evaluator design) → neutral
   phrasing ("a downstream consumer", "the embedding host").
7. Remove the consumer-specific `card-game-vocabulary` wiki decision from
   tracking (`git rm`).
8. CHANGELOG: a "Deprecated" note under 0.11.0 for `get_persistent_facts`.

## Out of scope

- `restore_persistent_facts` / `_sync` — already generic (bulk-assert any
  `Vec<Fact>`); no leak.
- The `Consequence` / `Actor` / narration vocabulary and the diverse
  example-host lists ("a game engine, a conversational module, an MCP
  server") — legitimate domain-neutral framing of the engine's own purpose.
- Rewriting already-published history on `origin`.

## Testing

- Unit test for `facts_matching_predicates`: set membership, empty set,
  multiple predicates, a predicate with no facts, deterministic ordering.
- Deprecation compiles clean under `-D warnings`.
- Existing suite (734 tests) stays green.
- After cleanup, `git grep -i <consumer-name>` over tracked files returns
  only the deferred, already-public items (the network.rs predicate list,
  alive until the 0.12 deletion) — the new/cleaned surface is name-free.

## Sequencing

Phase A's 0.11 steps (1, 2) and all of B are non-breaking — a deprecation is
a warning, not an error — so they ship in 0.11 without disrupting the
consumer's in-flight migration. The 0.12 deletion (step 3) is gated on the
consumer having moved onto `facts_matching_predicates`.
