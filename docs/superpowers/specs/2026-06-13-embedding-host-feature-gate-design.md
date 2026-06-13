# Embedding-host feature gate — Design

## Problem

phronesis's public API should be justified by in-repo consumption — the
bundled MCP (`phronesis-mcp`). An audit of `ReteNetwork`'s public methods
found ~9 the MCP never calls; they exist only for an external embedding host
(bulk save/restore, batch-retraction ids, instrumentation, single-step
agenda, a predicate-query alias). That is asymmetric surface: the engine
shaped around a dependent rather than its own job.

(`get_persistent_facts` is the worst case and is handled separately — it's
deprecated in 0.11 and deleted in 0.12 per the domain-neutrality design.)

## Goal

The **default** public surface of the `phronesis` crate equals what the
bundled MCP consumes. Methods that only an embedding host needs live behind
an opt-in `embedding-host` cargo feature. The MCP builds against the default
(so the compiler enforces the symmetry); embedding hosts enable the feature.

## Approach — 0.12, after the external consumer's 0.11 migration lands

### Feature

- Add an `embedding-host` feature to `crates/phronesis/Cargo.toml`, **off by
  default**.

### Methods to gate (`#[cfg(feature = "embedding-host")]`)

Re-run the audit at execution time and gate exactly what the MCP doesn't
consume. Current candidates:

- `restore_persistent_facts`, `restore_persistent_facts_sync`
- `fact_ids_matching`, `fact_count`, `facts_matching_predicate`
- `execute_next_agenda_item`
- `get_rules_count`, `get_wmes_by_condition`
- `get_performance_stats`, `log_performance_stats`, `reset_cycle_values`

Any method that turns out to be consumed by **neither** the MCP nor the
embedding host is dead surface — delete it outright (as with
`get_persistent_facts`) rather than gate it.

### Tests

- Tests exercising gated methods are themselves `#[cfg(feature =
  "embedding-host")]`, so the default test run covers the default surface and
  the feature run covers the rest. Watch `fact_query.rs` (`fact_ids_matching`,
  `fact_count`).

### CI

- Build + test the workspace **twice**: default features, and `--features
  embedding-host`, so gated code can't rot.

### Docs

- Each gated method gets a doc note: "Requires the `embedding-host` feature;
  not consumed by the bundled MCP." A lib-level doc paragraph explains the
  feature and who it's for.

### Consumer opt-in

- The external consumer enables `phr = { …, features = ["embedding-host"] }`.
  Coordinate with its migration: it must (a) be off the deprecated/removed
  APIs and (b) add the feature in the same change.

## Verification

- `cargo build -p phronesis` (default) compiles; `phronesis-mcp` builds
  against default `phr` with no gated method referenced — symmetry is enforced
  by the compiler, not by convention.
- `cargo build -p phronesis --features embedding-host` and its tests pass.

## Sequencing & risk

0.12, gated on the external consumer (a) migrating off the deprecated APIs and
(b) enabling the feature in the same change. It is **breaking** for that
consumer, so coordinate. The main implementation risk is feature-gating the
test suite cleanly: the default run must not reference gated items, and CI
must explicitly exercise the feature so it doesn't bit-rot.
