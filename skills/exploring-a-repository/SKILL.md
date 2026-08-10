---
name: exploring-a-repository
description: Use when starting work in a repository that has Phronesis installed (a
  `.phronesis/` directory) — before adding a feature, fixing a bug, or refactoring —
  or when the codebase is unfamiliar, large, or you are returning to it after a break.
---

# Exploring a Repository with Phronesis

## Overview

A Phronesis project carries evidence about itself: the rules that will block or
warn you, a structural graph of the code, a log of which rules actually fire, and
a grounded confidence signal. Query that evidence before you open source files.

**Core principle: constraints and structure first, files second.** Reading source
tells you what the code *is*. Phronesis tells you what the project *requires*,
what depends on what, and what will stop your commit.

## When to Use

- Before implementing a feature, fixing a bug, or refactoring in a repo with `.phronesis/`
- The codebase is unfamiliar, or you have been away from it
- You are about to guess at conventions, test coverage, or blast radius

**Not for:** repos without `.phronesis/` (there is nothing to query — orient
normally); or mid-task, once you already have the briefing.

## Interface

Prefer the MCP tools when the `phronesis` server is connected. Otherwise use the
`phr-mcp` CLI — the passes below give both. Every command is read-only except
`rebuild_code_graph`.

| Pass | MCP tool | CLI |
|---|---|---|
| 1. Constraints | `list_rules`, `get_drift` | read `.phronesis/rules.json`; `phr-mcp drift` |
| 2. Ground truth | `get_code_graph_status`, `rebuild_code_graph` | `phr-mcp graph status`, `phr-mcp graph rebuild` |
| 3. Structure | `query_code_graph` | `phr-mcp graph query [RELATION] [ARG...]` |
| 4. History and debt | `get_stats`, `get_journey`, `audit_codebase` | `phr-mcp stats`, `phr-mcp journey`, `phr-mcp audit --path <dir>` |
| 5. Confidence | `get_confidence` | `phr-mcp confidence` |

## The Five Passes

Run them in order. Each pass changes what you ask in the next one.

### 1. Constraints before code

`list_rules` — these are the durable requirements. They fire from hooks outside
the context window, so they are still enforcing at token 900k, long after a
`CLAUDE.md` line has faded. Note every rule at `block` level that touches the
files you expect to change; a plan that violates one is a plan you will unwind.

Also read, if present:

- `.phronesis/durable.md` and `.phronesis/kernel.md` — project directives that are
  guidance rather than enforcement. These also record which subsystems are
  incomplete, which can save you a pass.
- The nearest `CLAUDE.md` / `AGENTS.md`, **including a per-crate or per-package
  one** next to the code you are changing. Expect this to be the single best
  architecture summary available — better than any graph query — because it
  records intent the graph cannot represent: which modules are legacy-compat,
  which are the new path, and why both exist. Read it here in pass 1, not later.

Optionally `get_drift` (`source: "claude_md"` or `"wiki"`) to see written guidance
that no rule enforces. Those are conventions you are expected to follow that
nothing will catch you breaking.

`get_drift(claude_md)` discovers root and package-level `CLAUDE.md` and
`AGENTS.md` within three directory levels, and names the originating file on
every finding. **On a phronesis older than 0.26.1 it reads only a root
`CLAUDE.md`** — so in a repo whose guidance lives in `AGENTS.md` or a per-crate
file, it reports `not present (no file)`, meaning "the tool looked in one place,"
not "this project has no written guidance." Either way, it does not replace the
reading above.

### 2. Ground truth: is the graph current?

`get_code_graph_status`. If it reports missing, stale, or an outdated format, run
`rebuild_code_graph` before querying, and say in your briefing that you did.

**A stale graph is worse than no graph** — it answers confidently about code that
has moved. `git checkout`, `git mv`, and rebases all bypass the sensor that keeps
it fresh, so a graph is frequently stale at the start of a session.

### 3. Structure

Start with `query_code_graph` and **no relation**: it returns the relation
vocabulary and edge counts — the cheapest read of the repo's shape and which
languages were actually extracted.

**Find the entity name first.** The graph is keyed by entity, not by the
subsystem name you were given. Locate a file with `ls`/`find`, then run
`declares_module` with that file path to get its exact entity name. Guessing the
name and querying with a wrong one returns an empty result indistinguishable
from a real answer.

Then query what your task needs:

- `file_type` — the production / test / example / build split
- `imports` with `["*", "<entity>"]` — what depends on a module (dependents, not dependencies)
- `in_cycle` — module tangles you should not make worse

**Entity naming gotchas.** All three of these fail silently — they return an
empty result identical to a real "nothing here" answer:

- **Modules are fully qualified**, as `<lang>:<package>[#<target>]::<module path>`
  — `rust:phronesis::wme`, `typescript:myapp::src::billing`. A bare module path
  matches nothing.
- **The package keeps its hyphens.** `rust:phronesis-mcp::syntax`, not
  `phronesis_mcp`, even though Rust itself would underscore it in a `use` path.
- **Functions are not qualified the same way in every relation.** `defines_fn`
  and `untested` carry the fully qualified name
  (`rust:phronesis-mcp::syntax::facts::SyntaxFacts::all_facts`), but `tested_by`
  carries the **bare** name (`all_facts`) — the extractor cannot resolve a
  callee to its defining module, and the `untested` derivation bridges the two
  by final segment. So query `tested_by` with the bare function name. Passing
  the qualified name returns nothing and reads as "no tests."
- **`*` is a whole-position wildcard, not a glob.** `["*", "rust:pkg::mod"]` is
  right; `"rust:pkg::mod::*"` matches nothing. There is no prefix search.

A subsystem is usually *several* entities, one per file — there is no
directory-level entity — so expect a handful of `declares_module` calls rather
than one.

**Treat an empty `imports` result as unknown, not as zero.** Edges are
module-level, so a dependency on a submodule may appear on its parent instead.
Spend **at most one** extra query (the parent module) confirming an empty
result, then grep for the module path and move on. Do not keep re-querying the
graph to prove a negative.

### 4. History and debt

- `get_stats` — which rules actually fire in this repo. This is the lived
  convention set, as distinct from the aspirational one in the docs.
- `get_journey` — the recent trajectory of tool calls, and what temporal rules
  are currently matching. Especially useful when picking up someone else's work.
  An empty `[]` means no `journey_*` fact currently matches a loaded rule — it is
  not evidence that nothing has happened. A project with no `journey_*` rules
  always returns empty however busy the journal is. Cross-check `get_stats`
  before reading anything into it.
- `audit_codebase` scoped with `path` to the area you are touching — pre-existing
  violations there. Do not audit the whole tree during orientation; you are
  sizing your own blast radius, not opening a cleanup project.

  Graph-derived rules (import cycles, untested risky calls) are still
  *evaluated* against the whole graph — the test covering your function may
  live anywhere — but only findings inside `path` are reported. If you are
  running a phronesis older than 0.26.1, a scoped audit also returns
  out-of-scope files; check the paths before calling anything "debt in my area."

### 5. What "done" will require

`get_confidence` reports the band (high/medium/low) and the grounded compile /
tests / known-bug signals for the open work unit. Commits are typically gated on
this band, so read it now rather than discovering the gate at commit time.

## Task-Specific Queries

| Task | Ask the graph |
|---|---|
| Adding a feature | `imports` `["*", "<target module>"]` for blast radius; `untested` and `defines_fn` on the files you will touch — but an `untested` hit next to an obvious `tests/<name>_*` file means transitive coverage, not a gap |
| Fixing a bug | `tested_by` `["<bare fn name>", "*"]` — the tests to run and extend; `get_journey` for what just happened |
| Refactoring | `in_cycle` for tangles; `calls_api` for risky-API sites; `audit_codebase --rule <id>` for one debt class |
| Reviewing someone else's change | `get_stats`, then `tested_by` on each changed function |

## The Briefing

End with six lines, in chat. Do not write a file.

1. **Constraints that will bite** — the blocking rules on this path
2. **Shape** — languages, production/test split, the modules involved
3. **Blast radius** — what imports what you are about to change
4. **Coverage** — the tests that cover it, or that none do
5. **Existing debt** — audit findings already in that area
6. **Done means** — the confidence signals you will have to turn green

Then start the actual work. This skill orients; it does not decide.

## Limits of the Evidence

State these limits when you report, rather than presenting findings as proof.

- `tested_by` is a **direct-call heuristic**. Code covered only transitively still
  reports as `untested`. Absence of a test edge is a prompt to look, not a verdict.
- `get_drift` scores by **token-overlap Jaccard with no semantic matching**. It
  produces a triage list, not ground truth; a paraphrased rule reads as drift.
- The graph is **derived, gitignored state**. It reflects the last sensor run, not
  necessarily HEAD. See pass 2.
- `audit_codebase` only reports rules that are **opted in**, and whole classes
  of rule may not participate at all — AST-predicate rules are excluded from
  the audit engine, for instance. So a clean audit of the very subsystem those
  rules target is guaranteed clean regardless of its real debt. Check which
  rules could even have fired before reading a zero as good news.
- An **empty result is missing evidence, not negative evidence** — for `imports`,
  `tested_by`, and `get_journey` alike. Say "the graph shows no edge" rather than
  "nothing depends on it."
- Your host may inject its own `CLAUDE.md` / `AGENTS.md` files (including
  per-crate ones) through a channel these passes do not control. That guidance is
  often the best architecture summary available. Read it when it appears; it
  complements the passes rather than competing with them.

## Common Mistakes

| Mistake | Consequence |
|---|---|
| Reading source files first, then querying | You have already formed a plan the rules will reject |
| Querying a stale graph | Confident answers about code that has moved |
| Guessing an entity name instead of getting it from `declares_module` | Empty results that read as "nothing depends on this" |
| Reading an empty `imports` as "no dependents" without checking the parent module or grepping | You change a module you believe is isolated and break five call sites |
| `audit_codebase` on the whole tree | A debt inventory you did not ask for, and a large response |
| Treating `untested` as "no coverage" | You delete or rewrite code that transitive tests do cover |
| Skipping `get_confidence` until commit | The commit gate is a surprise instead of a plan input |

## Note for Non-Claude Hosts

`SKILL.md` frontmatter is a Claude Code convention. On Codex, Gemini CLI, or any
other host, read this file as a runbook and use the `phr-mcp` CLI column — the
five passes are the same.
