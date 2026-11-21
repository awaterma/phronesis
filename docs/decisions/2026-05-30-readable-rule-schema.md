---
id: readable-rule-schema
date: 2026-05-30
status: accepted
enforces: []
superseded_by: null
tags: [rule-engine, schema, dx]
---

# Readable rule schema (when/then/predicate-as-key)

## Context

The original (v1) rule file shape used `conditions`/`actions` arrays
with `predicate`/`action_type` string keys and `args` arrays:

```json
{
  "conditions": [
    { "predicate": "new_content_contains", "args": [".unwrap()"] }
  ],
  "actions": [
    { "action_type": "constraint_violation",
      "message": "Avoid .unwrap() in src/" }
  ]
}
```

This works mechanically but is dense and noisy. Reviewers parse three
nested levels of key:value before reaching the actual predicate name
and its argument. Authors writing new rules by hand tend to make
mistakes around `args` shape (string vs array).

## Decision

Adopt v2: `when` is an array of single-key predicate objects whose
key *is* the predicate name and whose value is the argument; `then`
is a single-key object whose key is a verb (`block` / `warn` / `log`)
and whose value is the message.

```json
{
  "when": [{ "new_content_contains": ".unwrap()" }],
  "then": { "block": "Avoid .unwrap() in src/" }
}
```

Shipped in phronesis-mcp 0.8.0. The v2 SPEC also added the `or`
operator, expanded into separate OR-free rules in DNF at load time.

## Enforcement

Architectural decision — not enforceable as a code-shape rule. The
schema lives in `crates/phronesis-mcp/src/rules_file.rs`; `phr-mcp
migrate-rules` converts v1 to v2 in place.

## Consequences

- Authoring is materially terser. Real diffs in the seed packs
  shrank by roughly a third.
- v1 still parses on load (forward-compatible); only v2 is written.
- `or` clauses expand in DNF, multiplying rule count when used. Keep
  them tight.
- `not` is *not* supported yet; deferred to a later release.
