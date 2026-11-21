---
id: borrow-ergonomic-apis
date: 2026-05-31
status: accepted
enforces:
  - warn-rust-public-fn-takes-string-ref
  - warn-rust-public-fn-takes-vec-ref
  - warn-public-fn-takes-box-ref
superseded_by: null
tags: [rust, api-design, borrowing]
---

# Prefer borrowed slices in public API signatures

## Context

Public functions that take `&String`, `&Vec<T>`, or `&Box<T>` force
callers to own the heap-allocated container before they can borrow
it. The dereferenced forms (`&str`, `&[T]`, `&T`) accept both owned
and borrowed data — strictly more ergonomic and idiomatic.

This is a well-known Rust API guideline (rust-unofficial/patterns,
Clippy `ptr_arg` lint). The project enforces it as a warning rather
than a block because private helper functions sometimes take owned
references for internal convenience, and the cost of a false
positive is low.

## Decision

Warn on public functions whose parameters use `&String`, `&Vec<T>`,
or `&Box<T>`. The fix is to change the parameter type:

- `&String` → `&str`
- `&Vec<T>` → `&[T]`
- `&Box<T>` → `&T`

Private functions are not covered — the warning matches on `pub fn`.

## Enforcement

Three pre-tool-use hook rules, each matching the parameter pattern
via `new_content_contains` scoped to `pub fn`. Warning severity
(exit 1).

## Consequences

- Public API surfaces accept broader input types, reducing `.to_string()`
  and `.to_vec()` ceremony at call sites.
- Occasionally triggers on `pub(crate)` functions where the owned
  reference is intentional — low-cost false positive, the model
  rephrases or ignores the warning.
