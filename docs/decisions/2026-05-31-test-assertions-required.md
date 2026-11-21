---
id: test-assertions-required
date: 2026-05-31
status: accepted
enforces:
  - warn-empty-test
superseded_by: null
tags: [rust, testing, quality]
---

# Tests must have assertions or error propagation

## Context

LLMs frequently generate placeholder tests that compile and "pass"
but assert nothing:

```rust
#[test]
fn test_foo() {
    let _ = foo();
}
```

These tests inflate the test count without catching regressions.
Worse, they provide false confidence — "all 47 tests pass" sounds
good until you realize 12 of them don't check anything.

A test that propagates errors via `?` (returning `Result`) is
acceptable because it will fail if the called code returns `Err`.

## Decision

Warn on `#[test]` functions that contain no `assert` macro and no
`?` operator. The fix is to add an assertion that checks the
function's actual behavior, or to propagate errors with `?` and a
`Result` return type.

## Enforcement

One pre-tool-use hook rule (`warn-empty-test`) matching the pattern
via predicate. Warning severity (exit 1).

## Consequences

- Placeholder tests get caught at write time. The model must add a
  real assertion before proceeding.
- Occasionally fires on tests that verify behavior through side
  effects (e.g., "this doesn't panic") — the model can ignore the
  warning for those cases.
