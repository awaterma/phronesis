# Rust Ownership Evidence

Phronesis can record where Rust source performs clones, filters, awaits,
mutations, and synchronous lock acquisitions, and a few bounded relationships
between those sites. It answers **what did we observe, and how strong is that
observation** — not whether this code is wrong.

The findings are evidence with stated limits, not proof.

## Why this exists

Phronesis already emits `function_clone_count(function, count)`. A count
cannot distinguish a cheap identifier clone, an `Arc` refcount bump, a deep
collection clone, or a deliberate snapshot taken before mutation. Tree-sitter
can identify the syntax and approximate lexical order, but it cannot generally
prove resolved types, control-flow reachability, borrow liveness, move paths,
or runtime cost.

This feature records individual sites with their source spans, derives a small
set of ordering relations where the syntax tree is sufficient, and optionally
enriches observations with type or MIR availability from a compiler-aware
provider.

## What it is

The ownership extractor runs as an opt-in enrichment of the structural code
graph. It walks Rust function bodies and records:

- **Site relations** — clone, filter, await, mutation, and synchronous lock
  acquisition sites, each anchored to a source span.
- **Structural relations** — a fixed set of derivations the AST can support,
  such as "a clone in the same expression chain that directly wraps a filter"
  or "a synchronous lock guard whose lexical scope ends before an await."
- **Evidence metadata** — each site carries an evidence level and a provider
  name, so queries can distinguish what came from source syntax versus type
  resolution or MIR.

All relations are queryable through the existing `query_code_graph` tool and
the dedicated `phr-mcp graph ownership` command.

## Turning it on

Ownership enrichment is **opt-in and off by default**. Add the following
section to `.phronesis/graph.toml`:

```toml
[ownership.rust]
enabled = true
provider = "ast"
include = ["src/**/*.rs"]
exclude = ["target/**", "vendor/**"]
max_sites_per_file = 2000
```

| Key | Default | Description |
|---|---|---|
| `enabled` | `false` | Must be `true` to produce any edges. A missing section or a missing file means disabled. |
| `provider` | `"ast"` | `"ast"` uses tree-sitter only. `"rust-analyzer"` additionally asks the compiler-aware provider to report on type inference and MIR lowering; it never replaces the AST provider. In Phase One that provider is **availability-reporting only** — it records both capabilities as `unavailable`, with reason `tool_missing` when no rust-analyzer is installed and `no_structured_interface` when one is, because this round exposes no stable structured interface to ask it through. It runs during an explicit `graph rebuild` and on no other path. |
| `include` | *all tracked files* | Repository-relative glob patterns for files to process. Only files already tracked by the graph are considered. |
| `exclude` | *none* | Repository-relative glob patterns to skip. Filters after `include` is applied. |
| `max_sites_per_file` | `2000` | Emit a `partial` status and keep what was found when the limit is reached. Do not panic. |

Changing any of these values triggers a full graph rebuild. Enabling ownership
for the first time also requires a rebuild.

## Querying it

```
phr-mcp graph ownership <function-id-or-glob>
```

The command groups evidence by function and site, and renders source locations,
observed operations, structural relationships, evidence level and provider,
type/MIR availability, and explicit limits.

### Example output

Real output from a project with `provider = "rust-analyzer"`, abridged to one
function:

```text
Function: rust:ownership-demo::hot_path

Observed:
  clone `cloned` (operand `xs.iter().filter(|x| !x.is_empty())`) at src/lib.rs:7 (bytes 157..201)
    evidence: ast (tree_sitter_rust)
  clone `collect` (operand `xs.iter().filter(|x| !x.is_empty()).cloned()`) at src/lib.rs:7 (bytes 157..221)
    evidence: ast (tree_sitter_rust)
    note: a `collect` was observed; that does not establish that ownership was produced
  filter (operand `xs.iter()`) at src/lib.rs:7 (bytes 157..192)
    evidence: ast (tree_sitter_rust)

Relationships:
  filter_before_clone
    filter site: filter (operand `xs.iter()`) at src/lib.rs:7 (bytes 157..192)
    clone site: clone `cloned` (operand `xs.iter().filter(|x| !x.is_empty())`) at src/lib.rs:7 (bytes 157..201)
    evidence: ast (tree_sitter_rust)
    limit: a shared expression chain is structural evidence; it is not runtime cost evidence, and it does not observe UFCS iterator calls

Evidence:
  AST: available (complete) for src/lib.rs
  type inference: unavailable (no_structured_interface)
  MIR: unavailable (no_structured_interface)

Limit:
  Ownership relations are observations with stated limits, not verdicts about correctness or cost.
  A `collect` site records that a collect was observed; whether it produced ownership is a type-level claim this evidence does not make.
  `filter_before_clone` does not observe UFCS iterator calls, so its absence is an incompleteness, not a claim of cleanliness.
  At least one capability is partial, stale, failed, or unavailable; AST observations are not upgraded by its absence.
```

`complete` is the reason paired with a successful `ast_extraction` status; the
other reason it can carry is `site_cap`, when `max_sites_per_file` was reached.

### Rebuild diagnostics

A rebuild that runs the compiler-aware provider also reports the analysis it did
**not** perform. Build scripts and procedural macros stay disabled, because
enabling either means running project code:

```text
$ phr-mcp graph rebuild
Rebuilt graph: 73 base edges, 18 derived, 0 items skipped, 0 rules migrated.
  limitation: rust_analyzer:build_scripts_disabled
  limitation: rust_analyzer:proc_macros_disabled
```

The same strings appear as `diagnostics` in `graph rebuild --json` and in the
`rebuild_code_graph` MCP result. A run with `provider = "ast"` reports none,
because no provider ran.

**What each part means:**

- **Observed** — sites the extractor found, with their source locations and any
  directly derivable structural relationship (here, the lock guard's lexical
  scope ends before the function's await point).
- **Evidence** — which analysis capabilities were available for this function,
  and for capabilities nobody reported on, an explicit note that no provider
  reported them. The AST is always extracted when ownership is enabled. Type
  inference and MIR are reported on only when `provider = "rust-analyzer"` is
  configured, and in Phase One both come back `unavailable` — that is the
  provider's documented scope limit, not a failure.
- **Limit** — a statement of what the observed evidence does **not** prove.

An empty query result means "no indexed ownership evidence matched." It never
means "this code has no ownership concern."

### Using `query_code_graph`

All ownership relations are also available through the generic graph query tool
with identical exact-match and glob semantics. For example, querying for
`filter_before_clone` returns the same evidence as the dedicated ownership
command.

## Evidence levels

Every observation carries an evidence level that says how far it has traveled
from the source:

| Level | Meaning |
|---|---|
| `ast` | The operation was observed in the tree-sitter syntax tree. This is the baseline level that Phase One always produces. |
| `type_resolved` | A compiler-aware provider resolved the relevant operand or result type. |
| `mir` | A relationship was confirmed by MIR dataflow analysis. |
| `diagnostic` | A compiler diagnostic with mapped spans supports the claim. |
| `runtime` | Measured cost or allocation evidence is available. |

Phase One emits only `ast`. Future providers may add `type_resolved` and
higher.

When an analysis capability is **unavailable**, it is reported explicitly:

```text
MIR: unavailable (async_lowering)
```

This is an ordinary outcome, not a bug. The bounded experiment documented in
the research note found that rust-analyzer MIR lowering succeeded for synchronous
bodies but failed for all four async bodies. **Absence of analysis is a
capability result, not a clean bill of health.**

### `stale` after an incremental edit

Compiler results are full-rebuild-only. Saving one Rust file re-extracts that
file's AST evidence in place, but no compiler-aware provider runs at hook time,
so the previous rebuild's type and MIR conclusions would otherwise describe
bytes they never saw. The save therefore replaces them with an explicit stale
marker keyed on the edited file:

```text
Evidence:
  AST: available (complete) for src/lib.rs
  type inference: stale (incremental_edit) for src/lib.rs
  MIR: stale (incremental_edit) for src/lib.rs
```

Running `phr-mcp graph rebuild` regenerates them. `stale` means "this was true
of an earlier version of the file"; it never means the file is clean.

## What it does not do

This is not a linter, a borrow checker, or a clone-cost calculator.

- It ships **no rule** referencing ownership relations in Phase One.
- `phr-mcp audit` creates no findings from ownership evidence.
- It never calls a clone expensive without cost evidence.
- It never calls a snapshot unnecessary from syntax alone.
- It never warns merely because an async function contains a lock operation.
- It does not replace rustc diagnostics.
- It does not index every local variable, expression, or control-flow edge.

An empty query result means "no indexed ownership evidence matched," never
"this code has no ownership problem."

## Known false positives and blind spots

The following classes of false positives are named explicitly. The extractor
is designed to emit evidence with honest limits rather than pretending to
have stronger analysis.

- **`read_before_mutation` over-groups `self`.** The root place resolution
  descends to the base identifier of a place expression. Every field access
  like `self.party.members[i].pos` has root place `self`, so the relation
  fires for any read-mutation pair where both reference something rooted at
  `self`, even if they touch unrelated fields. Unknown aliasing produces no
  edge — over-grouping, not under-grouping.

- **`lock_scope_ends_before_await` is wrong under a shadowed `drop`.** The
  extractor matches a bare `drop(guard)` call where `guard` is a bare
  identifier. A locally shadowed `drop` function would make this claim
  incorrect. This is acceptable because the relation is query-only and the
  false-positive class is documented.

- **The absence of `lock_scope_ends_before_await` is not evidence of a
  hazard.** This is the most important limit on this page. A lock site and an
  await site in the same function, with no scope relation between them, means
  the extractor could not establish a boundary — not that the guard is held
  across the suspension. Two entirely safe shapes produce no conclusion: an
  await that lexically *precedes* the lock, and an await at a loop head with
  the guard block-scoped later in the loop body (there is no back-edge
  reasoning). A field test over an external corpus reviewed nine functions
  that a naive "lock + await with no safe edge means hazard" reading would
  have flagged, and found **zero** real hazards. Read the relation as
  positive evidence of safety where it appears, and read nothing at all into
  where it does not.

- **An unbound temporary guard is treated as dropped at the end of its
  enclosing statement.** `self.cache.lock().insert(k, v);` binds no guard, but
  Rust releases the temporary at the end of that statement, so the extractor
  emits `lock_scope_ends_before_await` when the statement ends before the
  await. The boundary is the enclosing *statement* — the nearest ancestor
  `expression_statement` or `let_declaration` — never the innermost
  expression. That distinction matters for a scrutinee: in
  `match m.lock() { .. }` the temporary lives past the whole `match`, so an
  await inside a match arm correctly yields **no** relation. Where no
  enclosing statement exists (a tail expression with no semicolon), the
  extractor emits nothing rather than guessing at a boundary.

- **`filter_before_clone` misses UFCS iterator calls.** The extractor
  matches method-call forms only. `Iterator::filter(xs, p)` in UFCS style
  is recorded as a known incompleteness, not as evidence of cleanliness.
  `take_while` and `skip_while` are similarly excluded.

- **`collect` sites do not establish ownership production.** Every `collect`
  call is recorded as a clone site at the AST level, but the "ownership-
  producing" qualifier is a `type_resolved`-level claim. An AST-only
  observation of `collect` says "a collect was observed here," not "an
  allocation happened here."

- **Lexical ordering says nothing about reachability.** Every relation based
  on lexical order (`clone_before_await`, `read_before_mutation`) can be
  invalidated by an early `return`, a `match` arm, a closure boundary, or a
  loop exit between the two sites. The extractor does not analyze control flow.

- **Declarative macro-generated calls are partially observed.** At a macro
  invocation site, the extractor observes operations only where the invocation's
  arguments happen to parse as ordinary expressions. The macro body itself is
  never expanded or analyzed.

## Current status

This is **query-only incubation.** The following decisions bound what shipped:

- No packaged rule references ownership relations.
- `phr-mcp audit` does not create ownership findings.
- `init --packs rust` installs no ownership rule.
- The catalogue contains no ownership enforcement entry.
- The rust-analyzer provider ships interface-plus-availability only. It emits
  `ownership_analysis_status` but never emits `resolved_type`,
  `ownership_transfer`, `borrow_live_across`, or any MIR relation in Phase One.
- Field testing against the original five-case external corpus has not been run.

Relationships will not become audit-eligible until they have a measured
precision report over Phronesis and a second real-world corpus, named
false-positive classes (this document), and a warning message that states their
evidence limit. MIR-unavailable or partial evidence cannot satisfy a rule condition
that claims MIR confirmation.

## Credit

The design follows Christian Schott's master's thesis, *Visualizing Ownership
and Borrowing in Rust Programs* (Julius-Maximilians-Universität Würzburg,
2024), and its tool [BORIS](https://christianschott.github.io/boris-viewer/):
ownership is place-sensitive dataflow rather than something you can count,
strong claims need MIR-level analysis, and an explanation must carry its own
provenance. The implementation here is independent — a tree-sitter extractor,
where BORIS works through rust-analyzer.
