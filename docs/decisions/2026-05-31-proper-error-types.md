---
id: proper-error-types
date: 2026-05-31
status: accepted
enforces:
  - enforce-no-result-string-error
superseded_by: null
tags: [rust, error-handling, thiserror]
---

# Use typed errors, not Result<_, String>

## Context

`Result<T, String>` is the quickest way to propagate errors in Rust
but it throws away structure. Callers can't match on error variants,
can't distinguish recoverable from fatal, and get no compile-time
help when a new failure mode is added. The error message is a
free-form string that varies by call site — testing against it is
brittle.

`thiserror` makes defining a proper error enum nearly zero-cost:
a `#[derive(Error)]` with a few `#[error("...")]` variants is two
lines per case and gives callers pattern-matchable variants, Display
for humans, and `?`-based propagation.

## Decision

Block `Result<_, String>` returns in `src/` at the pre-tool-use
hook. The replacement is a `thiserror`-derived error enum scoped to
the module or crate.

## Enforcement

One pre-tool-use hook rule (`enforce-no-result-string-error`)
matching the pattern via AST predicate
(`function_returns_result_string`). Blocking severity (exit 2).

## Consequences

- Every module that returns errors needs a `FooError` enum. This is
  slightly more boilerplate but pays back immediately in match
  ergonomics and test precision.
- Existing `Result<_, String>` returns flagged by `phr-mcp audit`
  are cleanup candidates.
- `anyhow::Error` is acceptable for binaries and top-level CLI code
  where callers don't need to match on variants.
