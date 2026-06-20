# SPEC: extract_rules — sane defaults, message cleanup, condition fan-out

**Status:** draft
**Authors:** Claude, Andrew Waterman
**Date:** 2026-06-21
**Target release:** phronesis-mcp 0.13.x or 0.14.x (PATCH or MINOR depending
              on whether we change the rule-action default — which is
              surface behavior visible to existing users)
**Affects:** `crates/phronesis-mcp/src/server.rs::extract_rules` (or peer
              module), `crates/phronesis-mcp/CLAUDE.md` (documentation of
              the tool's contract), and the `set_section_context` MCP tool.

## How we got here

A live user invocation of `extract_rules` against
`crates/phronesis-mcp/docs/RUST-PATTERNS-GUIDE.md` added 27 rules to a
project's `.phronesis/rules.json` overnight. The rules all looked like:

```json
{
  "id": "rust-patterns-guide-idioms-1",
  "phase": "pre",
  "priority": 5,
  "when": [
    { "markdown_rule": ["crates/phronesis-mcp/docs/RUST-PATTERNS-GUIDE.md", "Idioms"] }
  ],
  "then": { "block": "[pattern] Use the `?` operator instead of manual error handling." }
}
```

Four problems jump out, all in the **extractor**, not in any individual
rule.

## Problem 1: `block` is the wrong default

Every extracted rule has action `block`. With the rules loaded and the
section context set to any H2 (e.g., `Anti-Patterns`), every pre-check
fires every rule in that section at once. Concretely:

- Pre-check sees `markdown_rule(file, "Anti-Patterns")` asserted.
- All 6 anti-pattern rules' `when` clauses match the same fact.
- All 6 fire as separate `constraint_violation` consequences.
- Hook exits 2; every tool call is blocked until the section context is
  retracted.

That's hostile, not enforcement. The patterns guide is study material —
discursive prose explaining good idioms. A rule that fires `block` on
every tool call while the model is *reading* an idioms section is a
denial-of-service against the model.

**Fix:** default extracted rule action to `warn` (or `log` for purely
reference patterns — see Problem 4). Block is a sharper tool reserved for
known-bad code shapes like `.unwrap()` in src/, not for pattern
reminders.

## Problem 2: extraction metadata leaks into user-facing messages

Every extracted rule's `then.<verb>` message has a bracketed tag prefix:

| Prefix | Origin |
|---|---|
| `[pattern]` | Idioms / Design Patterns / Error Handling / etc. |
| `[anti_pattern]` | Anti-Patterns heading subtype |
| `[context]` | Design pattern context (e.g., "Complex object construction with optional parameters.") |
| `[problem]` | Anti-pattern's "what goes wrong" subtype |

These are extraction-time discriminators. They were useful while building
the rule corpus; they have no place in a sentence the model reads at hook
time. The model sees `[anti_pattern] Avoid: Clone to Satisfy Borrow
Checker` and has no way to know `[anti_pattern]` isn't part of the
guidance.

**Fix:** strip the tag prefix when serializing the rule message. Keep the
tag as a separate field on the rule's metadata if downstream needs it
(audit categorization, stats grouping), but never as the leading bytes
of the user-visible string.

## Problem 3: identical conditions across a section produce simultaneous fan-out

All rules within an H2 section share the *same* condition:

```json
{ "when": [{ "markdown_rule": ["<file>", "<section>"] }] }
```

When the matching fact is asserted, every rule with that condition fires
once. For a 6-rule section, the agent receives 6 separate
consequences for what is, in user intent, "remind me of the patterns in
this section." The result feels like spam, not like guidance.

There are two clean fixes, in order of effort:

**3a (cheaper).** Have the extractor produce one rule per section with a
multi-line message that enumerates the patterns. Trades discoverability
in `phr-mcp stats` (no per-pattern hit counts) for sane firing semantics
on read. Recommended for a fast follow-up.

**3b (better).** Have the extractor add a per-pattern marker condition
in addition to the section condition. E.g., the rule for "Use `?` for
Error Propagation" gets:

```json
{ "when": [
    { "markdown_rule": ["<file>", "<section>"] },
    { "markdown_rule_pattern": "?-operator-for-error-propagation" }
]}
```

Then the agent (or another mechanism) asserts the *specific* pattern
they want a reminder about, not just the section. This is the right
shape but requires the agent to be more deliberate about declaring
intent. Likely a follow-up to 3a.

## Problem 4: many extracted rules duplicate stronger structural rules

The Rust pack already enforces several of the extracted patterns
*structurally* — with AST/regex predicates that fire on the actual code
shape, not on a section-context fact. For example:

| Extracted rule | Structural enforcement |
|---|---|
| `anti-patterns-8/9` (clone to satisfy borrow) | `warn-clone-heavy` (counts `.clone()` calls per fn) |
| `anti-patterns-10/11` (Deref polymorphism) | `warn-deref-for-non-pointer-type` |
| `anti-patterns-12/13` (overuse `unwrap()`) | `enforce-no-unwrap-in-src` (blocks the literal call site) |
| `error-handling-14` (use `thiserror`) | `enforce-no-result-string-error` |
| `api-design-17` (`&str`/`&[T]` over `&String`/`&Vec`) | `warn-rust-public-fn-takes-{string,vec,box}-ref` |

The structural rules fire when the smell *actually appears in code*. The
extracted rules fire when the model *declares it's reading the
section*. The structural rules do real enforcement; the extracted rules
add noise without protection.

**Fix:** the extractor should know which patterns already have
structural rules and demote the extracted version to `log` (recorded
but not surfaced to the model) — or skip extraction entirely. Two
implementation paths:

**4a.** A static table in the extractor: "if pattern matches one of
these keywords (`unwrap`, `clone`, `Deref`, `&String`, `&Vec`,
`thiserror`), demote to log." Brittle but cheap.

**4b.** Cross-reference at load time: `phr-mcp init` (or a new
`check-rules` command) walks the rules file looking for pairs where an
extracted rule and a structural rule cover the same pattern, and emits
a warning. Lets the human curate; doesn't preempt extraction. Better
shape; needs a heuristic.

Recommend 4a for v1 with an explicit list documented in the patterns
guide itself, and 4b as a follow-up audit tool.

## Problem 5 (orthogonal but related): the dormancy problem

`set_section_context` is a manual MCP tool call. The model has to
*remember* to call it before working in a section. In practice, neither
Claude nor Gemini routinely does. So the extracted rules sit dormant —
the fact they condition on is never asserted, and they never fire.

This isn't an extractor bug; it's a UX problem with the
section-context mechanism. Three sketches for future thought:

- **Auto-context from CWD heuristics.** If the agent is editing files
  under `crates/foo/src/error.rs`, infer "Error Handling" as the active
  section and assert the marker. Requires a mapping from filenames to
  guide sections.
- **Auto-context from recent CLAUDE.md / README reads.** When the model
  has Read the patterns guide in the last N tool calls, assert the
  section context for the H2 the model was viewing. Possible only if
  the hook can see Read tool calls (it doesn't today — Read isn't in
  the pre/post-check filter list).
- **Drop the section context entirely** and let the rules fire all the
  time, with action defaulting to `log` (so they accumulate in stats
  for audit but don't surface inline). Treats the extracted corpus as
  reference material rather than active enforcement. Honest framing of
  what the corpus actually is.

The third option pairs naturally with Problem 4's fix: structural rules
do enforcement; extracted rules become a passive reference layer.

## Salvage path for the 27 rules already on disk

Until the extractor is fixed, projects that have already invoked it
need a recipe:

1. **Strip the prefix** from every extracted rule's message.
2. **Demote to `warn` or `log`:**
   - `log` for rules that duplicate a structural rule (use Problem 4's
     pattern list).
   - `warn` for the rest.
3. **Optionally collapse** identical-condition rules into one
   multi-line warn (Problem 3a) — defer until the prefix and verb
   changes are in.

A `phr-mcp migrate-extracted-rules` command (or a flag on
`migrate-rules`) could automate this against an existing
`rules.json`. Scope-creep for this SPEC; flag as future work.

## Out of scope

- **Replacing the patterns guide with prose-only documentation.** The
  guide is useful as a learning resource; the extraction was the
  problem, not the source.
- **Removing `set_section_context` from the MCP surface.** Even if the
  dormancy problem (Problem 5) goes unfixed, the tool remains useful
  for the agent that explicitly says "I'm studying section X."
- **A new aggregator for "X since last Y" journey facts.** Surfaced
  during the same playtest but unrelated to extract_rules. Separate
  spec.

## Rollout plan

1. **PATCH** (next 0.13.x):
   - Strip the bracketed prefix in extractor output (Problem 2).
   - Default action to `warn` (Problem 1).
   - Add `phr-mcp migrate-extracted-rules` (the salvage path above) for
     existing projects.
2. **MINOR** (0.14.x or later):
   - Per-pattern marker conditions (Problem 3b).
   - Static skip-list for structurally-enforced patterns (Problem 4a).
   - Optional: cross-reference audit (Problem 4b).
3. **Future** (TBD):
   - Auto-context heuristics for section markers (Problem 5).

## Open questions

- **Should the prefix survive in metadata?** Audit tooling might want
  to group by pattern type. If yes, add a `pattern_class` field on the
  rule record; if no, drop entirely. Lean: drop.
- **Should `extract_rules` be opt-in per pack?** The patterns guide is
  Rust-specific. A future TypeScript or Python guide would want its own
  extractor and its own pack flag. Lean: yes — `--packs patterns-rust`,
  `--packs patterns-typescript`, etc.
- **What about the priority field?** All 27 came out at priority 5 —
  same as the LLM nudges. They could outrank or interleave with
  deflection rules. Priority 1 or 2 seems more honest for advisory
  reminders.

## References

- `crates/phronesis-mcp/src/server.rs::extract_rules` — the extractor
  entry point.
- `crates/phronesis-mcp/src/server.rs::set_section_context` — the
  section-marker assertion path.
- `crates/phronesis-mcp/docs/RUST-PATTERNS-GUIDE.md` — the source
  corpus the extractor reads.
- The 27 rules on disk at `.phronesis/rules.json` (gitignored) under
  ids `rust-patterns-guide-*-N`. Locally rewritten to non-block
  actions per this SPEC's salvage path.
