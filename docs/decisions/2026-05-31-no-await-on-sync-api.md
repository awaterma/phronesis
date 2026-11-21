---
id: no-await-on-sync-api
date: 2026-05-31
status: accepted
enforces:
  - block-await-on-sync-execute-all-agenda-items
  - block-await-on-sync-fire-all-consequences
superseded_by: null
tags: [rust, async, phronesis-api]
---

# No .await on synchronous phronesis API methods

## Context

The phronesis engine methods `execute_all_agenda_items()` and
`fire_all_consequences()` were originally async. A refactor (the
039 commit) made them synchronous. LLMs that were trained on or
have cached context from the async-era API continue to generate
`.await` calls on these methods, which produces a compile error
(`Result` does not implement `IntoFuture`).

The error message is confusing — it talks about `IntoFuture` rather
than "this method isn't async anymore" — so the model often
flounders for multiple iterations before finding the fix.

## Decision

Block `.execute_all_agenda_items().await` and
`.fire_all_consequences().await` at the pre-tool-use hook. The fix
is to drop the `.await` — the methods return `Result` directly.

## Enforcement

Two pre-tool-use hook rules, each matching the specific
`.method().await` substring via `new_content_contains`. Blocking
severity (exit 2).

## Consequences

- Catches the stale-API pattern before it reaches the compiler,
  saving one or more failed-build iterations.
- These rules are phronesis-specific and only useful in this
  workspace. They serve as a template for similar "API changed,
  block the old shape" rules in other projects.
