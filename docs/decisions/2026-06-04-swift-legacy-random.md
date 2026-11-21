---
id: swift-legacy-random
date: 2026-06-04
status: accepted
enforces:
  - audit-swift-legacy-random
superseded_by: null
tags: [swift, idiomatic, legacy-api, audit]
---

# Audit legacy `arc4random` / `drand48` calls

## Context

Pre-Swift-4.2 random number generation on Apple platforms used
Darwin's C library: `arc4random()`, `arc4random_uniform()`,
`drand48()`. Two problems with these in modern Swift:

- **Not portable.** They live in Darwin and aren't available on
  Linux or Windows builds of Swift. Server-side Swift or any
  cross-platform package that uses them either fails to compile
  off Apple or has to wrap them in `#if canImport(Darwin)`.
- **Modulo bias.** The classic `arc4random() % N` pattern is
  non-uniform unless `N` is a power of two (or you use the more
  awkward `arc4random_uniform(N)`). The Swift standard library hides
  this behind correct-by-construction APIs.

Swift 4.2 (2018) introduced uniform, cross-platform random APIs that
the book teaches and that Apple's sample code uses everywhere:

| Legacy | Modern |
|---|---|
| `Int(arc4random_uniform(10))` | `Int.random(in: 0..<10)` |
| `Double(drand48())` | `Double.random(in: 0..<1)` |
| `array[Int(arc4random_uniform(...))]` | `array.randomElement()` |

This rule mirrors [SwiftLint's `legacy_random`](https://realm.github.io/SwiftLint/legacy_random.html),
which is **default-enabled**.

## Decision

Phase: `audit`. Like `legacy_constructor`, this is a mechanical
substitution best handled in a one-shot sweep rather than at every
hook invocation.

The `when` clause uses an `or` over the tokens `arc4random(`,
`arc4random_uniform(`, and `drand48(`. Gated by `file_extension_is:
swift`.

## Enforcement

`audit-swift-legacy-random` runs only under `phr-mcp audit`. Each
match surfaces with file:line; the rewrite is local and mechanical.

## Consequences

- Cross-platform Swift packages drop a `#if canImport(Darwin)`
  block and a portability gotcha at the same time.
- Modulo-bias bugs that the legacy idiom invited disappear by
  construction.
- Users who genuinely want the C library RNG (cryptographic
  contexts, reproducibility against an existing seed) can per-rule
  suppress with a comment explaining why.
