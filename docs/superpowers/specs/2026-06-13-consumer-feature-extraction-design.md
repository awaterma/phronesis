# Consumer-feature extraction pattern — Design

## Principle

phronesis is a domain-neutral engine. Over time, consumer-specific *features*
(policy + logic) have leaked into it — most visibly a hardcoded predicate
list (a save-game policy). The rule: phronesis provides generic **mechanism**
(primitives); consumers own **policy** (which predicates matter, what they
mean). A domain-neutral engine carries zero policy.

This doc captures the migration *pattern* for pulling such features out, and
the plan to apply it comprehensively.

## The extraction recipe (proven on `get_persistent_facts`)

For each consumer feature found in phronesis:

1. **Identify the generic primitive** underneath it.
2. **Ensure phronesis exposes that primitive** (add it if missing).
3. **Move the policy/logic into the consumer**; it composes the feature from
   the primitive.
4. **Deprecate** the feature method (one minor), **delete** it the next.

Worked example, already in flight:

| | |
|---|---|
| Feature (policy in engine) | `get_persistent_facts` — a hardcoded persistent-predicate list |
| Primitive (added 0.11) | `facts_matching_predicates(&set)` |
| Consumer now owns | its own predicate list, calls the primitive |
| Lifecycle | deprecated 0.11 → deleted 0.12 |

## Two distinct cases — don't conflate them

- **Consumer FEATURES (policy/logic) in phronesis** → *extract* via the recipe
  above. These are the real target.
- **Generic PRIMITIVES only the consumer happens to call** (bulk save/restore,
  batch-retraction ids, instrumentation, single-step agenda) → *not* features;
  they're already mechanism. These get the `embedding-host` feature gate (see
  the separate design), not extraction.
- A method that is **neither a feature nor consumed by any in-repo code** is
  dead surface → delete it outright.

## Joint audit — the comprehensive pass (0.12, after the consumer's 0.11 migration)

Requires reading **both** repos, so it's deferred until the consumer's
in-flight migration lands and the two can be inspected together without
colliding with that work.

1. **phronesis → consumer-shaped behavior:** scan the engine and MCP for any
   policy/vocabulary/logic belonging to a specific consumer (hardcoded
   predicate names, domain assumptions, save-game shapes). Each → the recipe.
2. **consumer → phronesis gaps:** scan the consumer for *workarounds* — logic
   done in Rust because a phronesis primitive was missing or too weak. Each
   workaround signals a generic primitive to add, so the consumer composes
   instead of working around.

### Known candidate (case 2)

The consumer pre-filters facts in Rust because the guard DSL (`facts_contain`
/ `facts_count`) isn't expressive enough (see the rhai-evaluator design). The
forward move: add a generic **richer-guard primitive** (expressive conditions
over fact args — numeric comparisons, boolean combinators) that any consumer
composes its guards from. Guard *logic* stays generic in phronesis; guard
*policy* lives in the consumer.

## Out of scope (for now)

- Doing the extraction. The only clearly-identified feature
  (`get_persistent_facts`) is already on its deprecate → delete path; the rest
  needs the joint audit, which needs both repos accessible.

## Sequencing

0.12, gated on the consumer's 0.11 migration landing. The joint audit and any
resulting primitive additions / feature removals are coordinated — each
removal is breaking for the consumer, so it pairs with a consumer change.
