---
id: no-unwrap-in-src
date: 2026-05-30
status: accepted
enforces:
  - enforce-no-unwrap-in-src
superseded_by: null
tags: [rust, error-handling, lint]
---

# Don't ship `.unwrap()` in src/

## Context

`.unwrap()` and `.expect()` cause unwanted panics in production paths.
Rust idiom encourages explicit error propagation via `?` or pattern
matching on `Result` / `Option`. Library code that panics is hostile
to callers — they have no way to recover or even observe the failure
before the process aborts.

## Decision

`.unwrap()` is banned in `crates/*/src/**`. The replacement is one of:

- `?` operator when the surrounding function returns `Result`
- `.expect("clear message")` only when the call is truly infallible
  by construction (and the message documents *why* it can't fail)
- Explicit pattern match when the `None`/`Err` branch has work to do

Test code (`#[cfg(test)]` modules and `tests/` directories) is exempt
— panics in tests fail loudly with no production blast radius.

## Enforcement

This is enforced as a pre-tool-use hook rule (the file fails to be
written when it would introduce a new `.unwrap()` in `src/`).

## Consequences

- Library APIs return `Result<T, ConcreteError>` typed via `thiserror`
  rather than panicking on bad inputs.
- The hook is loud; expect to see it fire on hot-edit loops. Use
  `expect("…")` with a real message when the call is genuinely
  infallible — that signals intent and silences the lint.
