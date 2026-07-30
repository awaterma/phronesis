---
id: import-cycle-detection
date: 2026-07-29
status: accepted
enforces: [warn-import-cycle]
superseded_by: null
tags: [code-graph, structural-rules, architecture]
---

# Warn when the edited file's module participates in an import cycle

## Context

Import cycles are a layering smell the LLM cannot see from a single-file
diff, and the RETE engine cannot compute them either: transitive closure
is not expressible in flat `when` clauses, and consequences do not
re-assert facts, so there is no forward chaining (SPEC-triple-store-rete
§4.5). Something outside the rule language must find the cycles.

## Decision

Precompute `in_cycle(module, cycle_id)` with Tarjan's SCC over the
graph's `imports` edges on **every** save (derive tier — set ops over
edges already on disk, no reparse), and ship a warn-only rule that joins
`edited_file` → `declares_module` → `in_cycle`. Scoping to the edited
file matters: unscoped, the rule would repeat every cycle in the repo on
every tool call.

Correctness of the `imports` edges is load-bearing: `use super::` is
~40% of this repository's intra-crate imports, and an earlier revision
that only resolved `crate::`-anchored paths was blind to any cycle formed
through a relative import — a recall failure invisible by construction
(spec §4.7). Anchors now resolve against the enclosing module scope.

Measured on this repository: 2 cycles found, 2/2 genuine on hand-audit
(spec §10). This is the only structural rule that can fire on a Python
project, since `calls_api` is Rust-only.

## Enforcement

- `warn-import-cycle` in `.phronesis/rules.json` (structural pack,
  `phr-mcp init --packs structural`). Also auditable tree-wide via
  `phr-mcp audit` (`audit: true`).
- Warn-only until a second-corpus false-positive measurement (spec §8
  task 7); stale/outdated graphs demote enforcement with a stderr notice.

## Consequences

- Cycle facts are only as fresh as the graph: edits that bypass the hook
  (checkout, rebase, shell edits) mark the graph stale until
  `phr-mcp graph rebuild`.
- Cycles are reported at module granularity with an opaque `cycle_id`;
  the graph records *that* a module is in a cycle, not a line number, so
  audit findings use the line-1 placeholder plus bound entities.
