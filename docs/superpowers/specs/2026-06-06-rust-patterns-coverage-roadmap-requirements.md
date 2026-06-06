# Requirements — Rust patterns coverage roadmap spec

**Status**: requirements brief (pre-spec). Captures what the eventual design spec must answer; the spec itself is a separate session's work.
**Date**: 2026-06-06
**Triggered by**: tail of the block-pattern-predicates feature (commits `9ff52d5..ab8f76b`). The audit-AST integration in Phase 3.5 made every existing AST predicate audit-eligible, which changes the value calculation for several patterns in the upstream guide that were previously "hook-only and noisy." We need a single document that walks the guide and decides which patterns are worth lifting into predicates.

## Purpose

Produce a prioritized backlog of predicate proposals derived from
`crates/phronesis-mcp/docs/RUST-PATTERNS-GUIDE.md`. The backlog lets
future sessions pick the next predicate to build without
re-deriving the analysis each time, and surfaces cross-cutting
infrastructure (liveness, regex predicates, per-fact line numbers)
that multiple predicates would share.

## Background context the spec author needs

- The block-pattern feature established the *audit-only heuristic
  predicate* pattern: surface candidate sites with a scope-aware
  walker, let the LLM-in-the-loop adjudicate. See
  `docs/superpowers/specs/2026-06-04-block-pattern-predicates-design.md`
  for the canonical shape.
- The audit runner now evaluates AST predicates end-to-end
  (`audit.rs`'s `is_ast_predicate` branch). Any predicate that
  fits the existing `SyntaxFacts` shape works in `phr-mcp audit`.
- `SyntaxFacts::PREDICATES` is the source of truth for what
  audit can evaluate. A drift test pins it to `all_facts()`.
- `RUST-PATTERNS-GUIDE.md` has 8 sections (Idioms, Design
  Patterns, Anti-Patterns, Error Handling, API Design, Concurrency,
  Memory Management, Code Organization) plus a "Block Pattern"
  appendix that we now have covered.
- Tree-sitter-rust grammar 0.23 quirks worth knowing: `block` is
  the kind name for value-position block expressions AND function
  bodies AND `if`/`else`/`match-arm`/`for`/`while`/`loop`/`unsafe`/
  `async`/`try`/`labeled` blocks; discrimination has to happen by
  parent kind.

## Scope

Primary source: **`crates/phronesis-mcp/docs/RUST-PATTERNS-GUIDE.md`**.

Secondary sources the spec MAY mine after the primary is done:
- [rust-unofficial/patterns](https://rust-unofficial.github.io/patterns/)
  (MPL-2.0; the upstream the guide is "based on")
- [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/)
  (already referenced in the guide's footer)
- [Effective Rust](https://www.lurklurk.org/effective-rust/)
  (also referenced)

**Out of scope**: Swift patterns (separate doc, separate spec —
the Swift pack work in this session already absorbed the
high-value Swift items).

## Output format

For each pattern in the guide, the spec must produce a row with:

| Field | Type | Notes |
|---|---|---|
| `pattern` | string | e.g. "Use `?` for Error Propagation" |
| `source` | string | e.g. "RUST-PATTERNS-GUIDE.md §Idioms.1" |
| `category` | enum | `covered` \| `ast-feasible` \| `needs-infra` \| `not-lintable` |
| `existing_rule` | string? | rule ID if category is `covered` |
| `predicate_sketch` | object? | for `ast-feasible`; shape below |
| `infra_dep` | string? | for `needs-infra`; what's missing |
| `reason` | string? | for `not-lintable`; brief why |
| `priority` | enum | `high` \| `medium` \| `low` |

`predicate_sketch` (when `category == ast-feasible`):
- `name` (singular, e.g. `function_let_mut_count_high`)
- `args` (positional list)
- `scoping_rules` (what halts/recurses in the walker)
- `threshold` (number, with justification)
- `phase` (`pre` \| `post` \| `audit`)
- `silence_property` (the design point that prevents
  punishing the very shape the rule surfaces, if applicable)

## Cross-cutting infrastructure section

After the per-pattern walk, the spec must also itemize shared
infrastructure that multiple `ast-feasible` or `needs-infra`
entries would benefit from. Known candidates already surfaced:

1. **Per-fact line number tracking** — currently AST hits emit at
   line 1; deferred from block-pattern Phase 3.5. Several
   future predicates would want function-opening-line spans.
2. **Variable liveness analysis (single-function scope)** —
   the deferred `function_mut_binding_frozen_after` predicate
   needed this; "frozen-after-init" / "build-then-read" /
   "unused-after-assignment" patterns share the same machinery.
3. **Regex / pattern-match predicates** — Swift Identifier
   pattern was deferred because phronesis only has substring
   predicates; same gap probably blocks several Rust patterns
   (regex on type signatures, name-shape conventions, etc.).
4. **Parent-kind discrimination helpers** — the block-pattern
   walker invented this; if 3+ future predicates use it, hoist
   to a shared helper in `syntax/rust.rs`.

The spec must surface ALL such infrastructure needs, ordered by
how many `ast-feasible` entries they unblock.

## Methodology the spec author should use

1. Walk `RUST-PATTERNS-GUIDE.md` end-to-end. Don't skip sections.
2. For each pattern, check against `SyntaxFacts::PREDICATES` and
   the on-disk rule pack in `init.rs::rust_rules()`. If covered,
   note which rule.
3. For uncovered patterns, ask:
   - Can a substring or single-AST-node predicate detect this?
     → `ast-feasible`. Sketch it.
   - Does it need flow-sensitive analysis (liveness, types,
     control-flow)? → `needs-infra`. Note which infra.
   - Is the anti-shape genuinely contextual (a judgment call
     that requires reading prose)? → `not-lintable`.
4. After the walk, group all `ast-feasible` entries by their
   infrastructure dependencies. Identify the cross-cutting
   investments.
5. Assign priorities. Suggested heuristic:
   - **high**: high-frequency anti-pattern, AST-feasible with
     existing infra, silence-on-adopter property is clear
   - **medium**: same value but needs new infra; or smaller
     value with easy implementation
   - **low**: nice-to-have, niche, or hard to silence on adopters
6. Produce an executive summary at the top: "build these N
   predicates first; build this infrastructure next; defer
   these to a future revision."

## Open questions the spec author must resolve

1. **Does the spec cover the appendix?** The guide has the
   inlined John Nunley "Block Pattern" post (lines ~1044-1180
   in the working doc; the phronesis-shipped copy was truncated
   before that). Block pattern is already covered; the
   underlying post may have *other* patterns worth lifting.
2. **Granularity for predicate priority** — three tiers or five?
   Three is simpler; five gives finer scheduling.
3. **Does each `ast-feasible` row become its own ADR
   eventually?** If yes, predicate-sketch fields need to be
   ADR-template-shaped now. If no, free-form is fine.
4. **How to handle Anti-Patterns category specifically** —
   anti-patterns are negative shapes the rule wants to discourage.
   Most are AST-feasible if we know the shape. Worth a focused
   pass after the broader walk.

## Acceptance criteria

The spec is "done" when:

- Every pattern/idiom/anti-pattern in `RUST-PATTERNS-GUIDE.md`
  has a row in the classification table.
- Every `ast-feasible` row has a populated `predicate_sketch`.
- Every `needs-infra` row names the specific infrastructure.
- The cross-cutting infrastructure section ranks shared
  investments by how many entries they unblock.
- The executive summary is small enough to fit in a single
  page-of-text and clear enough that a session three months
  from now can pull from it without re-reading the whole spec.

## Non-goals

- The spec is **not** an implementation plan. Implementation
  plans for individual predicates get their own
  `docs/superpowers/plans/` document each, following the
  block-pattern feature's shape.
- The spec is **not** a CLAUDE.md or NOTICES update; those are
  follow-up steps after individual predicates ship.
- The spec does **not** need to cover Swift, Python, TypeScript,
  or Rhai — Rust patterns only.

## References

- `docs/superpowers/specs/2026-06-04-block-pattern-predicates-design.md`
  — the canonical predicate-design template the spec should
  mirror in shape (Goals, Architecture, Walker semantics,
  Thresholds, Testing strategy, etc.).
- `docs/superpowers/plans/2026-06-04-block-pattern-predicates.md`
  — implementation plan shape for individual predicates.
- `crates/phronesis-mcp/src/syntax/facts.rs` — `SyntaxFacts`
  shape and `PREDICATES` const.
- `crates/phronesis-mcp/src/syntax/rust.rs` — existing
  extractors as patterns to mirror.
- `crates/phronesis-mcp/src/audit.rs::is_ast_predicate` —
  the audit-eligibility surface.
- `.phronesis/wiki/decisions/2026-06-04-rust-let-{mut,binding}-count-high.md`
  — ADR template for predicate decisions.
