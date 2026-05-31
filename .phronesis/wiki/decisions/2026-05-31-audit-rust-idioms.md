---
id: audit-rust-idioms
date: 2026-05-31
status: accepted
enforces:
  - audit-manual-err-return
  - audit-newtype-id-string
  - audit-newtype-id-u64
  - audit-if-let-opportunity-none-empty
  - audit-if-let-opportunity-err-empty
  - audit-rc-refcell-in-src
  - audit-string-concat-with-plus
  - audit-allow-dead-code-in-src
  - audit-env-set-var-in-src
superseded_by: null
tags: [rust, idioms, audit, debt]
---

# Audit-only Rust idiom rules

## Context

Some code patterns are worth flagging on periodic sweeps but too
noisy to fire on every edit. These are debt indicators — not bugs,
but shapes that correlate with maintainability problems. Surfacing
them at hook time would create warning fatigue; surfacing them via
`phr-mcp audit` lets the team address them in focused cleanup
sprints.

## Decision

Nine patterns are enforced as audit-only rules (`phase: "audit"`,
surfaced by `phr-mcp audit` but silent at hook time):

| Pattern | Signal |
|---|---|
| `=> return Err(...)` in match arms | Use `?` operator instead |
| `*_id: String` fields | Newtype opportunity (`FooId(String)`) |
| `*_id: u64` fields | Newtype opportunity (`FooId(u64)`) |
| `None => {}` match arms | Use `if let Some(x) = ...` |
| `Err(_) => {}` match arms | Silent error swallowing; handle or log |
| `Rc<RefCell<T>>` in src/ | Fighting the borrow checker; rethink ownership |
| `" + &` string concatenation | Use `format!()` for readability |
| `#[allow(dead_code)]` in src/ | Delete the dead code or document why it lives |
| `env::set_var(` in src/ | Unsound under concurrent reads (unsafe in edition 2024) |

## Enforcement

Nine rules with `phase: "audit"` and `audit: true`. They do not
fire at hook time. `phr-mcp audit` surfaces them with file and line
numbers; `phr-mcp trend` tracks counts across snapshots.

## Consequences

- Periodic `phr-mcp audit` runs produce a debt inventory without
  disrupting the edit flow.
- Each pattern has a clear fix path documented in the rule message.
- `env::set_var` is the most safety-critical — it's unsound under
  concurrent reads and formally `unsafe` in Rust edition 2024.
  Flagging it in audits catches it before the edition migration.
- The `Err(_) => {}` rule catches silent error swallowing, which is
  the match-arm equivalent of an empty `catch {}` in other languages.
