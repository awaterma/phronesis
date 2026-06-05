---
id: swift-legacy-constructor
date: 2026-06-04
status: accepted
enforces:
  - audit-swift-legacy-constructor
superseded_by: null
tags: [swift, idiomatic, legacy-api, audit]
---

# Audit legacy C-style geometry constructors

## Context

Pre-Swift-3 Apple frameworks shipped C-style "make" functions for
common geometry types: `CGRectMake`, `CGSizeMake`, `CGPointMake`,
`UIEdgeInsetsMake`, `NSMakeRect`, and so on. They survive in
documentation and in older codebases, but they're shadowed by
proper Swift initializers that take labeled parameters:

| Legacy | Modern |
|---|---|
| `CGRectMake(0, 0, 100, 50)` | `CGRect(x: 0, y: 0, width: 100, height: 50)` |
| `CGSizeMake(100, 50)` | `CGSize(width: 100, height: 50)` |
| `UIEdgeInsetsMake(8, 0, 8, 0)` | `UIEdgeInsets(top: 8, left: 0, bottom: 8, right: 0)` |

The labeled form is what every Apple sample, every WWDC video, and
every recent textbook uses. The unlabeled form invites order-of-
argument bugs (`CGRectMake(x, y, h, w)` vs `(x, y, w, h)`) that the
modern initializer prevents at the type-checker.

This rule mirrors [SwiftLint's `legacy_constructor`](https://realm.github.io/SwiftLint/legacy_constructor.html),
which is **default-enabled** — meaning the SwiftLint maintainers
judged it low-false-positive enough to ship on by default.

## Decision

Phase: `audit`. The legacy constructors are pure mechanical
substitutions, so warning on every edit would be noisy in
in-progress refactors. `phr-mcp audit` surfaces the population so
a one-shot cleanup pass can rewrite them.

The rule's `when` clause uses an `or` over the family of legacy
identifiers: `CGRectMake(`, `CGSizeMake(`, `CGPointMake(`,
`CGVectorMake(`, `UIEdgeInsetsMake(`, `NSMakeRect(`, `NSMakeSize(`,
`NSMakePoint(`, `NSMakeRange(`. Gated by `file_extension_is: swift`.

## Enforcement

`audit-swift-legacy-constructor` runs only under `phr-mcp audit`.
Each match surfaces with the file:line and the legacy identifier
that triggered it; the fix is mechanical.

## Consequences

- Legacy projects show a one-time debt count, paid down in a single
  refactor sweep. New code that imports old patterns gets caught.
- Server-side Swift (and other non-Apple Swift) projects show zero
  hits, so the rule is silent where the legacy APIs don't exist.
- The token list is closed by SDK — Apple no longer adds new C-style
  `*Make` constructors, so the rule won't drift out of date.
