---
id: swift-fatal-error
date: 2026-06-04
status: accepted
enforces:
  - audit-swift-fatal-error
superseded_by: null
tags: [swift, error-handling, audit]
---

# Audit `fatalError(` in Swift sources

## Context

`fatalError(_:)` unconditionally aborts the process. It's the Swift
analog of Rust's `panic!()` — and we already block that one. But the
seed pack can't block `fatalError` outright, because UIKit and AppKit
projects are full of it as the body of `required init?(coder:)` and
similar framework-imposed initializers.

When `fatalError` is *not* boilerplate, it usually fits one of two
shapes:

- **Recoverable condition mis-modeled as a crash.** The honest fix is
  a `throws` function so the caller can decide. (`do`/`catch`,
  `try?`, `Result` — Swift's error model is already in scope.)
- **Sanity check.** `precondition(_:)` traps in both debug and
  optimized builds (`-O`); `assertionFailure(_:)` and `assert(_:)`
  trap in debug only (`-Onone`). Either reads better at the call
  site than `fatalError` — the name encodes the intent.

There's no pattern in [eleev/swift-design-patterns](https://github.com/eleev/swift-design-patterns)
that prescribes this directly — it's a Swift-idiom call, not a
catalog pattern.

## Decision

Phase: `audit`. Hook-time warnings on `fatalError(` would be too
loud for UIKit/AppKit projects (every `init?(coder:)` boilerplate
would fire). `phr-mcp audit` surfaces the population on demand so a
deliberate sweep can audit each occurrence: keep the framework-required
ones, rewrite the rest.

## Enforcement

`audit-swift-fatal-error` runs only under `phr-mcp audit`. Token match
on `fatalError(` plus `file_extension_is: swift`.

## Consequences

- Framework boilerplate `init?(coder:)` calls stay; the audit table
  just reports the count. Authors can per-rule suppress with a
  comment if they want a clean board.
- Genuinely recoverable conditions get rewritten as `throws`.
- Debug traps get rewritten as `precondition` / `assertionFailure`.
