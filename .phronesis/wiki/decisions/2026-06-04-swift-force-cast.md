---
id: swift-force-cast
date: 2026-06-04
status: accepted
enforces:
  - warn-swift-force-cast
superseded_by: null
tags: [swift, force-bang, lint]
---

# Warn on `as!` force-casts

## Context

Swift offers three "force-bang" escape hatches that turn a typed
absence into a runtime crash: `!` (force-unwrap), `try!` (force-try),
and `as!` (force-cast). The Swift seed pack already warns on the first
two; this rule completes the trio. Without it the pack signals that
two of the three crash-on-failure operators are worth flagging while
silently accepting the third, which is misleading.

The safer pair `as?` returns an `Optional<T>` that the caller can bind
with `if let` / `guard let`. That's the same shape as the
`ValueBinding` pattern in
[eleev/swift-design-patterns](https://github.com/eleev/swift-design-patterns)
*Swift Design Patterns/ValueBinding* — applicable here, but not the
primary motivation. The primary motivation is consistency with the
other force-bang rules.

## Decision

`as!` is a warning, not a block — there are legitimate cases (test
fixtures asserting a known concrete type, bridging Objective-C
collections where the runtime type is genuinely guaranteed by the
framework contract). The Swift seed pack flags occurrences so the
author confirms the cast can't fail or rewrites it as `as?` + binding.

The warning is gated on `file_extension_is: swift` to avoid colliding
with `as!` substrings in other languages' comments or string literals.

## Enforcement

`warn-swift-force-cast` fires in the pre-tool-use hook. Token-level
match on `as!`; no AST predicate needed because Swift's grammar reserves
the spelling.

## Consequences

- Authors who genuinely need a force-cast either suppress per-rule or
  rewrite to `if let value = thing as? T`.
- Pairs naturally with the existing `warn-swift-force-unwrap` and
  `warn-swift-try-bang` rules — together they cover the three
  "force-bang" escape hatches Swift offers.
