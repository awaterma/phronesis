---
id: no-panic-in-production
date: 2026-05-31
status: accepted
enforces:
  - enforce-no-todo-in-src
  - enforce-no-panic-in-src
  - enforce-no-unimplemented-in-src
superseded_by: null
tags: [rust, error-handling, panics]
---

# No panicking macros in production code

## Context

`todo!()`, `panic!()`, and `unimplemented!()` abort the process
unconditionally. In library code, a panic is hostile — callers have
no way to recover or even observe the failure before the process
exits. Each of these macros has the same effect (process abort) but
signals different intent:

- `todo!()` — "I haven't written this yet" (incomplete code shipped)
- `unimplemented!()` — "I chose not to handle this" (intentional gap)
- `panic!()` — "something is catastrophically wrong" (usually wrong
  in library code; the caller should decide what's catastrophic)

All three are inappropriate in `crates/*/src/**` paths that run in
production or as library code consumed by other crates.

## Decision

Block all three macros in `src/` at the pre-tool-use hook:

- `todo!()` — finish the implementation or split it into a tracked
  task before shipping.
- `panic!()` — return a `Result` and let the caller decide.
- `unimplemented!()` — implement the path or remove it.

Test code (`#[cfg(test)]` modules and `tests/` directories) is
exempt — panics in tests fail loudly with no production blast radius.

This decision complements [[no-unwrap-in-src]], which covers
`.unwrap()` specifically.

## Enforcement

Three pre-tool-use hook rules, each matching the macro name via
`new_content_contains` and scoped to `src/` via `file_path_matches`.
Blocking severity (exit 2).

## Consequences

- Forces explicit error propagation via `?` or `Result` returns
  throughout library code.
- Incomplete code paths must be resolved before the file is written,
  which prevents "I'll finish this later" stubs from shipping.
- Test code remains free to panic — `.unwrap()`, `todo!()`, etc.
  are all fine in test modules.
