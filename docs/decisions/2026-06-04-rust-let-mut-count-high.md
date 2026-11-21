---
id: rust-let-mut-count-high
date: 2026-06-04
status: accepted
enforces:
  - audit-rust-let-mut-count-high
superseded_by: null
tags: [rust, refactor, block-pattern, mutability, audit]
---

# Audit functions with multiple outer-scope `let mut` declarations

## Context

A second benefit of John Nunley's "Rust's Block Pattern"
(December 2025) is *erasure of mutability*: a `let mut` inside a
block expression returns an immutable binding to the outer scope.

```rust
let data = {
    let mut data = vec![];
    data.push(1);
    data.extend_from_slice(&[4, 5, 6, 7]);
    data
};
// `data` is now immutable for the rest of the function.
```

Functions with three or more outer-scope `let mut` declarations are
candidates for this refactor — the mutability is often local to a
short build phase that could be scoped away.

This rule mirrors the design of
[rust-let-binding-count-high](2026-06-04-rust-let-binding-count-high.md),
sharing the same `count_outer_scope_let_declarations` walker but with
a `has_mut_keyword` filter so only `let mut` declarations count.

## Decision

Phase: `audit`. Threshold: 3 outer-scope `let mut` declarations.
Matches the precedent set by `function_clone_counts_high` (also 3).
Two `let mut`s are common; three start to suggest the block pattern
applies.

## Enforcement

`audit-rust-let-mut-count-high` runs only under `phr-mcp audit`.
The AST predicate `function_let_mut_count_high(file, fn, count)`
is the trigger.

## Consequences

- Functions with build-then-freeze patterns surface for review.
- Block-pattern adopters who already scoped their mutability stay
  silent.
- The rule is intentionally conservative: a `let mut` deep inside an
  `if` arm still counts (the mutability is still visible to the
  outer flow), but a `let mut` inside a `{ ... }` block expression
  is exempt.
