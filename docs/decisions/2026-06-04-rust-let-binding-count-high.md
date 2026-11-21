---
id: rust-let-binding-count-high
date: 2026-06-04
status: accepted
enforces:
  - audit-rust-let-binding-count-high
superseded_by: null
tags: [rust, refactor, block-pattern, audit]
---

# Audit functions with long outer-scope `let` ladders

## Context

John Nunley's "Rust's Block Pattern" (December 2025) makes the case
for scoping intermediate `let` bindings inside a block expression:

```rust
let config = {
    let raw = fs::read(cfg_file)?;
    let s = String::from_utf8(raw)?;
    let stripped = strip_comments(&s);
    serde_json::from_str(&stripped)?
};
```

Three benefits Nunley names: the block leads with intent (`let config
= ...`), intermediate variables don't pollute the outer namespace, and
the intermediates drop at block end so resources release earlier.

We can't detect the *opportunity* via substring matching — the
anti-shape is a function with too many top-level intermediate `let`
bindings, which requires AST traversal plus scope awareness. The
predicate `function_let_binding_count_high` does that: counts
*outer-scope* `let_declaration` nodes, halting at child `block_expression`
and `closure_expression` nodes so functions that already adopted the
pattern go silent.

## Decision

Phase: `audit`. Threshold: 8 outer-scope `let` declarations. Fires
only under `phr-mcp audit`, surfacing candidate sites for the LLM
(or human reviewer) to judge.

The walker is deliberately conservative about scope boundaries:
`if`/`match`/`for`/`while`/`loop` bodies recurse (they're
continuations of the outer flow), but `{ ... }` block expressions
and closures halt. This is what makes the rule worth shipping —
it does not punish the very pattern it surfaces.

## Enforcement

`audit-rust-let-binding-count-high` runs only under `phr-mcp audit`.
The AST predicate `function_let_binding_count_high(file, fn, count)`
is the trigger.

## Consequences

- Long ladders surface as audit-table entries the model can read and
  judge per-function. False positives (constructors of complex types,
  parsers with naturally long intermediate stages) just get dismissed
  in conversation.
- Block-pattern adopters stay silent — the rule does not generate
  pressure to un-do the very refactor it suggests.
- If real-world audits prove noisy at threshold 8, the threshold
  lives in `crates/phronesis-mcp/src/syntax/rust.rs` as a `const`
  and is a one-line bump.
