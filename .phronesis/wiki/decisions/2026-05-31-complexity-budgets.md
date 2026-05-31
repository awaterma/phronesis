---
id: complexity-budgets
date: 2026-05-31
status: accepted
enforces:
  - warn-rust-function-param-count-high
  - audit-file-loc-high
superseded_by: null
tags: [rust, complexity, maintainability]
---

# Function and file complexity budgets

## Context

Large functions with many parameters and large files with many
responsibilities are the two most reliable predictors of
maintenance burden. Both correlate with God-object debt: a file or
function that does too much, changes for too many reasons, and
resists decomposition because everything depends on everything.

LLMs are especially prone to growing functions incrementally — each
edit adds a parameter or a branch rather than splitting the concern
into a new unit.

## Decision

Set two complexity budgets:

1. **Function parameters: 5 max** — functions with more than 5
   parameters get a warning. The fix is a builder/options struct or
   splitting the function into focused units.

2. **File lines: 800 max (src/ only, excluding test blocks)** —
   files exceeding 800 lines of non-test code get an audit-level
   warning. The fix is splitting into focused submodules. A top-of-
   file `//! phronesis-allow: audit-file-loc-high <reason>` exempts
   intentional god-files.

## Enforcement

- `warn-rust-function-param-count-high`: pre-tool-use hook, warning
  severity. Uses `function_param_count_high` predicate.
- `audit-file-loc-high`: audit-only (not fired at hook time),
  surfaced by `phr-mcp audit`. Scoped to `src/`.

## Consequences

- Long parameter lists get caught early, before the function
  accumulates callers that all pass 7 arguments.
- File-size warnings are audit-only to avoid disrupting active
  editing. The author sees them on periodic sweeps.
- The 800-line threshold is a guideline, not a hard gate — the
  allow-comment escape hatch exists for files that are large by
  design (e.g. a parser with many arms).
