# Embedding-host follow-ups

> **Status: needs evidence (proposed).** Deferred from the first governed
> session change on 2026-11-22; these are requirements, not shipped APIs.
> Inspection of [the network](../../crates/phronesis/src/network.rs),
> [agenda](../../crates/phronesis/src/agenda.rs), and
> [batch tests](../../crates/phronesis/tests/fact_batch.rs) found batching
> without rollback, and no `FactDelta`, fault-plan, or observer API.
> `failed_operation_does_not_rollback_prior_batch_work` explicitly records
> the current non-transactional boundary. No release is claimed.

The collaborator selected selective agenda execution and ordered batching
for implementation now. The following three embedding improvements remain
separate follow-up work; their signatures are proposals.

## 3. Atomic fact delta

Provide `FactDelta { retract: Vec<String>, assert: Vec<Fact> }` and
`apply_delta(...) -> Result<DeltaReport, ReteError>` to apply related
mutations together with one agenda scan. If a step is refused, restore
working memory, alpha/beta state, agenda (including ordering), and refraction
exactly to the pre-call state. The error must identify the refused fact.
Existing `FactBatch` is not a transaction and cannot satisfy this contract.

A smaller first increment could be `replace_fact(id, fact)`. Replacing
identical content must be a no-op, either by default or through an explicit
option: no retraction, refraction reset, or activation re-latch.

Acceptance evidence must compare complete before/after state for failures
at each mutation boundary, joins and pending activations, and demonstrate
that an identical replacement does not re-fire an already consumed match.
Concurrency and observer delivery at commit/rollback need design decisions
before implementation.

## 4. Fault injection for embedding tests

Offer an off-by-default `test-util` feature with a `FaultPlan` that can
refuse assertions for selected predicates and optionally refuse retractions,
plus a way to clear it. Failure must occur at the engine operation being
tested, rather than after a wrapper has already changed engine state.

Tests should establish deterministic refusal, recovery after clearing, state
consistency, and absence of this capability from ordinary production builds.
Do not expose private mutexes merely to let consumers poison them. This
proposal does not itself promise atomic rollback for existing operations.

## 5. Fact mutation observer

An optional `Arc<dyn FactObserver>` could receive `asserted(&Fact)` and
`retracted(&str)` notifications for every successful working-memory mutation,
including store-only restore inserts. If engine-owned rule actions acquire
mutation behavior in the future, they must use the same observation path;
current execution returns action descriptions for the embedding host.

Before implementation, specify ordering, callback reentrancy, panics/errors,
locking, and interaction with atomic deltas. No callbacks should claim a
mutation committed when it was refused or rolled back. Tests must enumerate
all public mutation paths, not only the normal assert/retract wrapper.

Back to [Specs](README.md).
