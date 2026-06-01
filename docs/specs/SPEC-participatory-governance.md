# SPEC: participatory governance — LLM as rule-evolution participant

**Status:** draft
**Authors:** Claude, Andrew Waterman
**Date:** 2026-05-31
**Target release:** phronesis-mcp 0.10.0 (MINOR — new behavioral directives, enhanced MCP tools)
**Depends on:** SPEC-wiki-drift (0.9.0)

## Premise

Wiki-drift (0.9.0) gave phronesis a structured decision layer:
ADR-style pages that link to rules via `enforces:` frontmatter, with
a drift detector that surfaces gaps. But the LLM's role is passive —
it writes code, gets blocked by rules, and occasionally writes a
decision page when asked. It doesn't participate in rule evolution.

In Andrew's 2009 RulesFest work on participatory ecological modeling,
participants in a game governed by rules could propose rule changes
as part of play. The same structure applies here: the LLM is a
participant in a game (software development) governed by rules
(phronesis). It should be able to propose rule changes through a
formal channel (decision pages), not by circumventing enforcement.

This SPEC adds three participatory workflows, implemented primarily
as durable directives (behavioral guidance re-injected every turn)
with small code enhancements to support them.

## Goals

1. **Remember → decide → enforce pipeline.** When the user says
   "remember X," the model scaffolds a decision page, proposes a
   rule if enforceable, wires the `enforces:` link, and asks for
   approval. One interaction closes the loop from observation to
   durable enforcement.

2. **Friction-driven rule proposals.** When the model hits the same
   rule 3+ times in a session, it pauses to assess: is the rule
   too broad, or is the model's habit the problem? If the rule
   scope needs refinement, the model proposes a decision page with
   the change. The human approves or rejects.

3. **Cross-session knowledge transfer.** When the model discovers
   something significant (a bug pattern, a design insight, a
   rollout lesson), it offers to write a decision page. Future
   sessions read it via durable directives. Session-local
   discoveries become durable project knowledge.

## Out of scope

- **LLM-assisted Jaccard replacement.** Using the LLM to score
  decision↔rule similarity instead of token overlap. Valuable but
  changes the drift detector's contract (currently: no LLM call).
  Separate SPEC.
- **Consequence-driven rule mining.** Analyzing the action log for
  patterns (which rules fire most, which correlate with bugs) and
  inductively proposing new rules. The SPEC-wiki-drift explicitly
  deferred this; it remains deferred.
- **Orphan-rule detection.** Rules with no decision linking back to
  them. Natural follow-up to wiki-drift but separate scope.
- **Session-end review hook.** A hook that fires at session close
  and prompts "anything learned this session worth recording?"
  Attractive but depends on host-level support (SessionEnd hook)
  that may not exist in all environments.

## Implementation

### Layer 1: Durable directives (behavioral, no code)

The primary implementation is guidance in `.phronesis/durable.md`
that instructs the model on the three workflows. This is already
deployed (see the "Participatory governance" section added to the
default template in `init.rs`). The guidance fires every turn and
takes immediate effect — no binary upgrade needed.

Key design choices in the directives:
- **"Ask the human before committing"** — the model proposes, the
  human approves. No autonomous rule changes.
- **"3+ blocks by the same rule"** — the friction threshold is a
  heuristic. Too low (1) and every false positive triggers a
  proposal. Too high (5+) and real friction goes unnoticed.
- **"Not every insight warrants a formal decision"** — the model
  asks before writing knowledge-transfer pages. Avoids decision
  bloat.

### Layer 2: Code enhancements (future commits)

Small tool improvements that make the behavioral workflows smoother:

#### 2a. `propose_rule_for_decision` MCP tool

**Problem:** The current `suggest_rule()` emits a TODO placeholder
for the `when` clause. The model has to manually figure out which
predicate to use. When the model is following the "remember → decide
→ enforce" workflow, it needs to pick the right predicate from the
available vocabulary.

**Solution:** A new MCP tool that takes a decision ID and returns a
structured suggestion:

```rust
#[tool(description = "Given a decision ID, propose a v2 rule ...")]
async fn propose_rule_for_decision(
    &self,
    #[arg(description = "decision ID from frontmatter")] decision_id: String,
) -> Result<CallToolResult, McpError>;
```

Implementation: read the decision page, extract the Decision section
text, match it against the predicate vocabulary (a hardcoded table
of predicate names + descriptions + example arguments), and return
a structured JSON rule with the best-fit predicate and argument
pre-filled. Still heuristic (no LLM call in the tool itself), but
much better than a TODO placeholder because it uses the predicate
vocabulary as a lookup table.

The model can then review the suggestion, adjust it, and write it
to `rules.json` via existing tools.

**Files:** `src/wiki_drift.rs` (enhance `suggest_rule`),
`src/server.rs` (register tool), `src/server_params.rs` (params).

#### 2b. `get_session_friction` MCP tool

**Problem:** The "friction-driven proposals" workflow tells the
model to check `get_action_log` with `only_nonzero_exit: true`, but
the model has to manually scan the results and count fires per rule.

**Solution:** A convenience tool that reads the action log, groups
by rule ID, filters to the current session (since last SessionStart
event), and returns a summary:

```json
{
  "session_start": "2026-05-31T10:00:00Z",
  "friction": [
    {"rule_id": "enforce-no-unwrap-in-src", "fires": 5, "last_fire": "..."},
    {"rule_id": "warn-clone-heavy", "fires": 2, "last_fire": "..."}
  ],
  "threshold_exceeded": ["enforce-no-unwrap-in-src"]
}

```

The threshold (default 3) is configurable. Items in
`threshold_exceeded` are candidates for friction-driven proposals.

**Files:** `src/action_log.rs` (add aggregation fn),
`src/server.rs` (register tool), `src/server_params.rs` (params).

#### 2c. Recent-decisions in session-context

**Problem:** The "cross-session knowledge transfer" workflow depends
on future sessions reading decision pages. Currently, the model only
sees decisions if it runs `wiki-drift` or reads the files manually.

**Solution:** Extend the `session-context` hook (SessionStart) to
include a one-line summary of recent decisions (last 30 days).
Something like:

```
## Recent decisions (last 30d)
- 2026-05-31 no-llm-deflection (covered)
- 2026-05-31 verify-before-done (covered)
- 2026-05-30 tag-pre-feature-state (uncovered — procedural)
```

This is cheap (one directory scan + frontmatter parse) and surfaces
the decision corpus to every new session without the model having to
remember to check.

**Files:** `src/context.rs` (add decision summary),
`src/wiki.rs` (reuse `walk_decisions`).

## Testing strategy

| Layer | Tests |
|-------|-------|
| Durable directives | Manual: start a new session, say "remember X", observe whether the model follows the pipeline |
| `propose_rule_for_decision` | Unit: known decision → expected predicate; unknown predicates → fallback; missing decision → error |
| `get_session_friction` | Unit: action log with repeated fires → correct grouping; empty log → empty result; threshold filtering |
| Recent-decisions context | Unit: decisions within/outside 30d window; empty wiki dir; integration with session-context output |

## Commit plan

1. **`feat: participatory governance directives in durable.md`** —
   durable.md template update + project's own durable.md. Already done.

2. **`feat: propose_rule_for_decision MCP tool`** — predicate
   vocabulary table, enhanced suggestion, MCP tool registration.

3. **`feat: get_session_friction MCP tool`** — action log
   aggregation, threshold filtering, MCP tool registration.

4. **`feat: recent-decisions in session-context`** — decision
   summary in SessionStart hook output.

Commits 2–4 are independent and can land in any order.

## Open questions

- **Friction threshold.** 3 fires per session is a guess. Should it
  be configurable per-rule (a `friction_threshold` field on the rule
  itself)?
- **Predicate vocabulary table.** Where does the mapping from
  "intent description" to "predicate name + args" live? Hardcoded
  in Rust is simplest but means adding a predicate requires a code
  change. A JSON vocabulary file would be more flexible.
- **Decision bloat.** If the model writes too many knowledge-transfer
  decisions, the wiki becomes noisy. Should there be a
  `wiki-lint` command that flags low-value pages? (Deferred from
  SPEC-wiki-drift as well.)
- **Autonomous vs. supervised.** The directives say "ask the human
  before committing." Should there be an opt-in mode where the
  model can autonomously write decisions and propose rules, with
  the human reviewing asynchronously?
