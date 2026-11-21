---
id: swift-mutable-singleton
date: 2026-06-04
status: accepted
enforces:
  - audit-swift-mutable-singleton
superseded_by: null
tags: [swift, singleton, concurrency, audit]
---

# Audit `static var shared` mutable singletons

## Context

The Singleton pattern, demonstrated in
[eleev/swift-design-patterns](https://github.com/eleev/swift-design-patterns)
under *Common Design Patterns/Creational/Singleton*, is canonically
written as `static let shared = MyService()`. The instance is built
once, lazily, and is thread-safe because Swift guarantees
once-and-only-once initialization for `let` static properties.

`static var shared` is strictly worse:

- The reference can be reassigned at runtime, turning the singleton
  into a globally-mutable variable. Concurrent writes race with
  concurrent reads, and the compiler doesn't surface that under
  pre-Swift-6 concurrency checking.
- Under strict concurrency (Swift 6 / `-strict-concurrency=complete`),
  a `static var` requires `nonisolated(unsafe)` to compile across
  actor boundaries — an explicit "I am asserting this is safe and the
  compiler can't verify it" annotation. `static let` of a `Sendable`
  type needs no such escape hatch.

Both `static let` and `static var` get once-and-only-once initialization,
so the difference isn't about lazy init — it's purely about whether
the reference is reassignable.

If you want to swap the instance under test, the right fix is
constructor injection (the DependencyInjection pattern from the same
catalog), not a mutable static.

## Decision

Phase: `audit`. The hook stays silent because flipping `let` ↔ `var`
in mid-refactor is a frequent intermediate state, and `phr-mcp audit`
surfaces the count on demand.

## Enforcement

`audit-swift-mutable-singleton` runs only under `phr-mcp audit`.
Token match on `static var shared` plus `file_extension_is: swift`.

## Consequences

- Existing mutable singletons appear in the audit table as debt to
  pay down — convert to `static let shared` and route runtime
  substitution through a Dependency Injection seam.
- False positives (a `static var shared` that's intentionally mutable
  for an exotic reason) are silenced per-rule with a doc comment
  explaining why.
