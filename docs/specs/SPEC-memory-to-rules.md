# SPEC: Memory → Rules workflow

**Status:** draft
**Authors:** Claude, Andrew Waterman
**Date:** 2026-05-26
**Affects:** `crates/phronesis-mcp/src/{claude_md_drift.rs, init.rs, server.rs}`,
              `.phronesis/durable.md`, new optional `.phronesis/memory-drift.md`

## Premise

Claude Code's auto-memory system and phronesis solve adjacent
problems. Both write guidance to disk, both inject from outside the
context window. They differ in trigger, form, and scope:

|         | Auto-memory                              | Phronesis rules                  | Phronesis durable.md      |
|---------|------------------------------------------|----------------------------------|---------------------------|
| Trigger | SessionStart (read once)                 | Every tool call (fires repeated) | Every turn (re-injected)  |
| Form    | Freeform prose + frontmatter             | Predicate + action               | Freeform prose            |
| Scope   | Per-user, per-project                    | Per-project (shared)             | Per-project (shared)      |
| Cost    | One read at session start                | One hook per tool call           | One prepend per turn      |

A memory at rest in `~/.claude/projects/.../memory/` fires *once* per
session. The same fact, encoded as a phronesis rule, fires at every
relevant tool call — i.e. at the moment of action, which is when the
guidance actually has to apply. That is the durability win phronesis
was built for. The auto-memory system catches a fact once at hour
zero; phronesis catches it again in token nine hundred thousand.

So: when an auto-memory entry is **actionable**, it should be ported
to a phronesis rule. When it is **project-shareable ambient prose**,
it should be ported to `durable.md`. When it is **personal**, it
stays in `MEMORY.md`.

## The three buckets

### Bucket 1 — Actionable memories → phronesis rule

**Shape:** a memory that names a specific tool call, command, or
code shape that should trigger guidance.

**Example.** Memory: *"Andrew prefers to revert the failing branch
before debugging — debugging a divergent branch wastes hours."*

```json
{
  "id": "remind-revert-before-bisect",
  "phase": "pre",
  "priority": 5,
  "conditions": [
    { "predicate": "bash_command_contains", "args": ["git bisect"] }
  ],
  "actions": [{
    "action_type": "constraint_warning",
    "params": [
      "Andrew's noted preference: revert the failing branch before bisecting. Debugging a divergent branch has wasted hours in the past."
    ]
  }]
}
```

Trigger criteria for porting:

- The memory references a specific *operation*: a tool, a command, a
  file path, a code pattern, a commit step.
- The memory is project-shareable (not personal preference about voice).
- The cost of ignoring the memory is concrete enough to justify a
  warning at the moment of action.

### Bucket 2 — Project-ambient prose → `.phronesis/durable.md`

**Shape:** a fact every model interaction in this project should
treat as ground truth, but that does not correspond to a specific
tool call.

**Example.** Memory: *"This codebase uses card-game framing
(hand / card / member). The earlier RPG vocabulary (party / player /
DM) was deliberately removed in 2026-05; reintroducing it would
undo that work."*

This belongs in `durable.md`. It is project-shareable, prose, and
applies to *every* model action — not just one specific tool call.
Putting it in a rule would be awkward (every Edit fires the
warning); putting it in durable.md re-injects it at every turn so
the model writes consistent code from the start.

### Bucket 3 — Personal preferences → `MEMORY.md` (unchanged)

**Shape:** facts about the user — role, communication style,
domain expertise, history of feedback the user has given the
assistant.

**Examples that should stay personal:**

- "User is a senior Rust engineer with experience in production
  rule systems."
- "User prefers terse responses with no trailing summaries."
- "User wants suggestions framed as recommendations, not decisions."

These are not project-shareable (a teammate would not benefit, and
might find them misleading). They have no tool-call trigger. They
belong where they already are: `~/.claude/projects/.../memory/`.

## Tooling — `phr-mcp memory-drift`

`claude-md-drift` already exists. It walks `CLAUDE.md`, extracts
imperative bullets, matches each against the current rule pack by
token overlap, and reports bullets without confident matches. The
same mechanism applied to auto-memory would close a real loop.

**Proposed command:**

```
phr-mcp memory-drift                          # heuristic match against rules + durable.md
phr-mcp memory-drift --memory-dir <path>      # default: ~/.claude/projects/<encoded-cwd>/memory
phr-mcp memory-drift --suggest                # emit candidate JSON for each unmatched actionable
phr-mcp memory-drift --json                   # machine-readable
```

**Output shape:**

For each memory file in the memory directory:

1. Classify by frontmatter `metadata.type` (`feedback`, `project`,
   `user`, `reference`).
2. For each entry, attempt one of:
   - **Match to rule** — token overlap with the message in an
     existing rule's `constraint_warning` / `constraint_violation`.
   - **Match to durable.md** — token overlap with a paragraph in
     durable.md.
   - **No match** — surface as drift.
3. Suggest a destination bucket using the type field:
   - `feedback` + names a tool call → "candidate rule"
   - `project` + ambient → "candidate durable.md addition"
   - `user` → "stays personal" (no drift)
   - `reference` → "stays personal" (URL-style pointer, not a directive)

**Output mode (table):**

```
Memory                                          Bucket                Suggestion
------                                          ------                ----------
feedback_revert_before_debug                    actionable            → rule candidate (emit with --suggest)
project_card_vocabulary                         ambient               → durable.md candidate
user_role                                       personal              (no action)
reference_signalbloom_meta_memory               personal              (no action)
```

The `--suggest` flag emits draft JSON rules / draft durable.md
paragraphs to stdout, which the operator pastes into the project's
`rules.json` or `durable.md` after review. Phronesis never
auto-writes from this command — drift detection is heuristic, and
the human chooses what crosses the boundary.

## What this does *not* solve

- **Meta memory.** Porting memories to rules does not give the
  model calibrated self-knowledge. It just relocates the storage
  from "fades in context" to "fires at action time." See the
  `signalbloom.ai` note that prompted this design: meta memory in
  the strict sense (knowing what you know with confidence) remains
  unsolved at the model layer. The workflow here is the same
  externalize-and-verify crutch that phronesis applies elsewhere.

- **Auto-porting.** A naive auto-port (`memory → rule` without
  human review) would generate spammy rules and project the
  preferences of one user onto all collaborators. The
  `--suggest` flag emits drafts; the operator chooses.

- **Cross-project memory.** Some user-level memories ("I'm a
  senior engineer") apply across every project. Those stay
  personal; this spec only addresses the per-project subset.

## Implementation sketch

1. New module: `crates/phronesis-mcp/src/memory_drift.rs`. Mirrors
   `claude_md_drift.rs` in shape. Walks the memory directory (path
   computed from cwd the same way the Claude Code harness does),
   reads each `.md` file, parses frontmatter, extracts the body.

2. New CLI subcommand `memory-drift` in `src/main.rs`, plus the
   matching MCP tool `get_memory_drift` for server-mode callers.

3. Classification rules (per-entry, heuristic):
   - `metadata.type == "feedback"` + body mentions a tool /
     command / file path → **actionable candidate**.
   - `metadata.type == "project"` + body is descriptive (no
     imperative) → **ambient candidate**.
   - `metadata.type == "user"` or `"reference"` → **personal**.

4. Drift detection: token-overlap match against `rules.json`
   messages and `durable.md` paragraphs. Threshold tuned by
   inspecting false-positive rate on a real memory directory.

5. `--suggest` emission: a minimal rule template (with a
   `// TODO: pick a predicate` marker on the conditions) and a
   minimal durable.md block. Operator fills in the predicate and
   reviews wording before saving.

## Open questions

- **Predicate guessing.** The hardest part of porting actionable
  memories is matching the memory's described situation to an
  existing predicate. We could ship a small predicate-suggester
  (token-similarity over the predicate registry) but it will be
  wrong often enough that the operator must always review.

- **Drift the other direction.** Should phronesis also detect rules
  that *should* be memories? A rule that never fires for a given
  user across N sessions might be a personal preference that doesn't
  belong in the shared rules.json. Probably out of scope for v1.

- **Frequency of running.** `claude-md-drift` is on-demand. The
  same applies here. We do not want this firing automatically; it
  is an editorial pass.
