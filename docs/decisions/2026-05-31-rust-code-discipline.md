---
id: rust-code-discipline
date: 2026-05-31
status: accepted
enforces:
  - warn-dbg-in-src
  - warn-clone-heavy
  - warn-expect-with-empty-message
  - warn-deref-for-non-pointer-type
  - block-deny-warnings-attribute
superseded_by: null
tags: [rust, hygiene, anti-patterns]
---

# Rust code discipline rules

## Context

Several Rust anti-patterns recur in LLM-generated code. Each is
minor in isolation but compounds into noisy, fragile, or surprising
code when unchecked:

- **`dbg!()`** left in production code after a debugging session.
- **Excessive `.clone()`** hiding borrow-checker fights rather than
  resolving them.
- **`.expect("")`** with an empty message — worse than `.unwrap()`
  because the empty string suggests the author considered the panic
  case but had nothing to say about it.
- **`impl Deref for NonPointer`** — Deref polymorphism on
  non-pointer types is the Rust equivalent of C++ implicit
  conversions: surprising method resolution and hidden coercions.
- **`#![deny(warnings)]`** — breaks builds on toolchain upgrades
  because each rustc release may add new warnings. `warn` is the
  right default; CI can use `RUSTFLAGS=-Dwarnings` when it wants a
  hard gate.

## Decision

Enforce the following at the pre-tool-use hook:

| Pattern | Severity | Action |
|---|---|---|
| `dbg!()` in src/ | warn | Remove or replace with `tracing::debug!()` |
| 3+ `.clone()` in one function | warn | Review whether references or borrows suffice |
| `.expect("")` in src/ | warn | Add a real message or use `.unwrap()` |
| `impl Deref for` | warn | Use explicit conversion methods instead |
| `#![deny(warnings)]` | block | Remove; use `RUSTFLAGS=-Dwarnings` in CI |

## Enforcement

Five hook rules. Four are `warn` (advisory), one (`deny(warnings)`)
is `block` because it causes hard build failures that are expensive
to diagnose on toolchain upgrade day.

## Consequences

- `dbg!()` and `.expect("")` are caught before commit, keeping
  production code clean.
- Clone-heavy functions get a signal to reconsider ownership — not
  a hard block, because sometimes cloning is the right call.
- `deny(warnings)` removal is a one-time migration per crate.
