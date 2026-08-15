# SPEC: Evidence-backed Rust ownership graph

**Status:** Draft for implementation
**Audience:** Fable and Phronesis maintainers
**Target release:** Unscheduled; query-only incubation before any rule ships
**Research basis:** a private research note held outside this repository (it records downstream-consumer internals and is deliberately not published here).

## Summary

Phronesis will add an opt-in Rust ownership-evidence enrichment to the
structural graph. It will represent clone, filter, mutation, await, and
synchronous-lock sites as bounded graph relations; derive a small set of
ordering and scope relations; and optionally enrich those observations with
type or MIR availability reported by a compiler-aware provider.

The enrichment produces evidence, not verdicts. Phase One adds no packaged
rule and no predicates named `expensive_clone`, `unnecessary_clone`,
`legitimate_snapshot`, or `bad_lock_usage`. Its product is a queryable,
source-linked explanation that distinguishes:

- an operation observed in the AST;
- a type resolved by compiler-aware analysis;
- a relationship confirmed by MIR;
- a compiler capability that was unavailable; and
- runtime cost evidence, if a later data source supplies it.

The initial acceptance sample is the five-case downstream-consumer corpus documented in
the research note. Success means separating the known filter-before-clone
performance incident from legitimate snapshots and phase boundaries without
guessing. Finding many clones is not a success criterion.

## 1. Problem

Phronesis currently emits aggregate facts such as
`function_clone_count(function, count)`. Counts cannot distinguish a cheap
identifier clone, an `Arc` reference-count increment, a deep collection clone,
or an intentional snapshot taken before mutation or suspension.

Tree-sitter can identify syntax and lexical order, but it cannot generally
prove resolved types, control-flow reachability, borrow liveness, move paths,
or runtime cost. The BORIS thesis required MIR dataflow for those stronger
claims and documents limitations around async lowering, macros, closures,
interior mutability, and source mapping. The bounded downstream-consumer prototype
reproduced one important limitation: rust-analyzer inferred every expression
in all five selected functions, but MIR lowering succeeded only for the one
synchronous body and failed for all four async bodies.

Phronesis needs a representation that preserves useful AST evidence without
silently promoting it to compiler proof.

## 2. Goals

1. Represent ownership-relevant source sites with stable, queryable graph IDs.
2. Preserve the distinction between AST, type-resolved, MIR, diagnostic, and
   runtime evidence.
3. Record unavailable analysis explicitly rather than interpreting absence as
   a clean result.
4. Compose ownership evidence with existing function, file, call, test,
   coverage, ADR, and rule relations.
5. Explain the five bounded downstream-consumer cases accurately.
6. Keep normal hook latency and graph size bounded.
7. Hydrate ownership relations into RETE through the existing `Edge::to_fact`
   path without changing RETE matching semantics.
8. Incubate relations as query-only until precision is measured on Phronesis
   and the downstream consumer.

## 3. Non-goals

- Implementing a borrow checker in tree-sitter.
- Reconstructing full MIR dataflow in Phronesis.
- Calling a clone expensive without measured cost evidence.
- Calling a snapshot necessary or unnecessary from syntax alone.
- Warning merely because an async function contains a lock operation.
- Replacing rustc diagnostics.
- Indexing every local, expression, or control-flow edge in a repository.
- Adding or distributing an ownership rule in Phase One.
- Running Cargo, build scripts, or procedural macros implicitly during a hook.

## 4. Architectural decision

### 4.1 Canonical storage

Ownership observations are structural graph edges. They become ordinary RETE
facts when the graph is hydrated:

```text
Rust source
  -> bounded ownership extractor
  -> graph Edge { p, a, src, d }
  -> Edge::to_fact()
  -> RETE working memory
```

They are not added only to `SyntaxFacts`. `SyntaxFacts` serves per-file hook
analysis, while ownership evidence must remain available for repository-wide
queries and joins. A later hook optimization may reuse the same extractor and
assert a changed file's observations directly, but the persisted graph is the
canonical representation.

### 4.2 Conflict with the existing graph budget

`SPEC-triple-store-rete.md` deliberately excludes local variables and
intra-function control flow from the core graph. Ownership sites are
intra-function detail and can multiply edge volume. Therefore ownership
extraction is an **opt-in enrichment**, not an unconditional expansion of the
Rust language pack.

Phase One must measure edge count, graph size, rebuild time, and hydrate time
before changing that default.

### 4.3 Evidence is queryable data

`Fact.source` and `Edge.src` identify the subsystem and source file that
produced a fact. They do not say whether an ownership claim came from lexical
AST order, type resolution, MIR, or runtime measurement. Evidence strength is
therefore represented by explicit relations, not overloaded into provenance
display text.

No change to the core `Fact` type is required.

## 5. Identity model

### 5.1 Functions

Every relation uses the existing canonical Rust function ID:

```text
rust:<crate>::<module-path>::<function-or-Type::method>
```

The ownership extractor must consume the package/module index used by the Rust
graph extractor. It must not create a second function identity scheme.

### 5.2 Sites

A site ID is local to a graph generation and deterministic for identical
source bytes:

```text
<function-id>#ownership:<kind>:<start-byte>
```

Examples:

```text
rust:example-app::game_core::initialize_runtime::execute_rete_with_provenance#ownership:filter:28901
rust:example-app::game_core::initialize_runtime::execute_rete_with_provenance#ownership:clone:28983
```

Byte offsets are UTF-8 byte offsets, matching tree-sitter and rust-analyzer.
Site IDs may change after edits before the site. Provenance-keyed compaction
removes obsolete edges from that source file, so cross-generation identity is
not promised.

## 6. Closed relation set

Adding or changing a relation is a graph-format change.

### 6.1 Base AST relations

| Relation | Arguments | Meaning |
|---|---|---|
| `ownership_site` | `[site]` | Declares an ownership observation site. |
| `ownership_site_in_function` | `[site, function]` | Associates a site with one canonical function. |
| `ownership_site_span` | `[site, file, start_byte, end_byte]` | Exact source span. |
| `clone_site` | `[site, operation, operand]` | An observed `.clone()`, `Clone::clone`, `.cloned()`, `to_owned`, `to_string`, or ownership-producing `collect`; `operation` preserves the distinction. |
| `filter_site` | `[site, operand]` | An iterator/filter operation and its bounded source text. |
| `await_site` | `[site]` | An await expression. |
| `mutation_site` | `[site, operation, place]` | A bounded mutation operation such as `get_mut`, assignment through a projection, or a known mutable-borrow method. |
| `sync_lock_site` | `[site, operation, guard]` | A synchronous `lock`, `read`, or `write` acquisition with its lexical binding when known. |
| `ownership_evidence` | `[subject, level, provider]` | Evidence available for a site, relationship, or function. |
| `ownership_analysis_status` | `[subject, capability, status, reason]` | Explicit provider result for a function or file, including unavailable or bounded analysis. |
| `resolved_type` | `[site, type]` | Compiler-aware provider resolved the relevant operand/result type. |

Allowed `level` values are:

```text
ast
type_resolved
mir
diagnostic
runtime
```

Phase One providers are `tree_sitter_rust` and, when explicitly enabled,
`rust_analyzer`. Future provider names are additions to configuration, not new
evidence levels.

Allowed Phase One analysis capabilities and statuses are:

```text
capability: ast_extraction | type_inference | mir_lowering
status: available | partial | unavailable | failed
```

`reason` is a stable machine value such as `async_lowering`, `tool_missing`,
`project_load_failed`, or `provider_error`, never free-form stderr.

### 6.2 Derived structural relations

| Relation | Arguments | Derivation |
|---|---|---|
| `filter_before_clone` | `[function, filter_site, clone_site]` | A clone-producing iterator operation directly wraps a filter in the same expression chain. Mere line ordering is insufficient. |
| `clone_before_await` | `[function, clone_site, await_site]` | Clone site lexically precedes an await in the same function. This is ordering evidence, not proof that the cloned value crosses suspension. |
| `read_before_mutation` | `[function, read_site, mutation_site]` | A bounded read/snapshot site lexically precedes a mutation of the same syntactically identified root place. Unknown aliasing produces no edge. |
| `lock_scope_ends_before_await` | `[function, lock_site, await_site]` | The narrowest lexical block containing the bound guard ends before the await. |
| `lock_scope_may_cross_await` | `[function, lock_site, await_site]` | Emitted only from compiler/MIR evidence, or from an explicit rustc diagnostic. AST containment alone must not emit this relation. |
| `ownership_transfer` | `[function, from_place, to_place, kind, site]` | Compiler-backed transfer classified as `move`, `copy`, `shared_borrow`, `mutable_borrow`, `assign`, or `drop`. Not emitted by the AST provider. |
| `borrow_live_across` | `[function, borrow_site, boundary_site, boundary_kind]` | Compiler-backed liveness across `mutation`, `call`, `await`, or `loop_back_edge`. Not emitted by the AST provider. |
| `ownership_conflict_diagnostic` | `[function, primary_site, related_site, code]` | Imported rustc diagnostic with mapped primary and related spans. |

`clone_before_filter` is intentionally absent. The downstream-consumer incident has
`filter` before `cloned`; a useful performance interpretation requires the
expression-chain relationship plus runtime or historical cost evidence, not a
name that reverses the observed order.

### 6.3 Future runtime relation

```text
clone_cost_evidence(site, bytes_or_allocations, run_id)
```

This relation is reserved but not implemented in Phase One. Coverage proves
execution, not allocation size, so coverage alone must not emit it.

## 7. AST extractor

The Rust implementation belongs beside the existing Rust graph extractor and
uses the already-parsed tree-sitter tree. The research Python script is a
fixture oracle, not production code.

Requirements:

1. Parse each eligible file once and reuse its syntax tree.
2. Walk implemented functions and methods, excluding bodyless trait items.
3. Use canonical function IDs from the existing module/package index.
4. Extract UTF-8 byte spans without converting byte offsets to character
   offsets.
5. Strip comments and strings structurally by visiting syntax nodes, never by
   regular expression.
6. Preserve operation kinds; `.clone()`, `.cloned()`, `collect`, `to_owned`,
   and `to_string` are not interchangeable costs.
7. Cap operand/place source text at 240 bytes. Normalize internal whitespace.
   If text exceeds the cap, store a stable digest marker rather than truncating
   into an ambiguous expression.
8. Derive `filter_before_clone` only from a shared expression chain.
9. Derive lexical scope using the narrowest enclosing block for a bound lock
   guard. An unbound temporary lock produces a site but no scope conclusion.
10. Never infer receiver types from identifier spelling or imported type names.
11. Emit no edge when a unique function, place root, guard binding, or span
    cannot be established.

## 8. Compiler-aware provider

### 8.1 Boundary

Compiler enrichment is isolated behind a provider interface:

```rust
trait OwnershipEvidenceProvider {
    fn analyze(
        &self,
        project_root: &Path,
        functions: &[OwnershipFunction],
    ) -> Result<OwnershipEvidenceReport>;
}
```

The AST extractor remains usable when no provider is installed.

### 8.2 Rust-analyzer provider

The first provider may invoke rust-analyzer only during an explicit graph
rebuild with compiler enrichment enabled. It must not run implicitly in
`pre-check`, `post-check`, graph hydration, or incremental single-file hook
updates.

It may emit:

- `resolved_type` when a type maps uniquely to an AST site;
- `ownership_evidence(..., type_resolved, rust_analyzer)`;
- `ownership_analysis_status` for type inference and MIR lowering; and
- MIR relations only when the provider returns usable structured MIR evidence.

Parsing human-formatted `analysis-stats` timing output is not a production
interface. Phase One may use it in a research test harness, but implementation
must use a stable structured interface or explicitly limit the provider to
availability reporting.

Build scripts and procedural macros are disabled by default. When disabled,
the provider records that limitation in rebuild diagnostics. It must not claim
macro-complete analysis.

### 8.3 Rustc provider

A future rustc provider may import JSON diagnostics and MIR-derived evidence
from an explicitly authorized build or supplied artifact. Phronesis must not
run Cargo against a sibling or active checkout merely to obtain ownership
facts. Imported evidence records the toolchain version, command/run identity,
and source revision in the rebuild report.

## 9. Configuration and boundedness

Add this optional section to `.phronesis/graph.toml`:

```toml
[ownership.rust]
enabled = true
provider = "ast"              # "ast" or "rust-analyzer"
include = ["src/**/*.rs"]
exclude = ["target/**", "vendor/**"]
max_sites_per_file = 2000
```

Requirements:

- Missing configuration means ownership enrichment is disabled.
- `provider = "rust-analyzer"` includes AST extraction and requests compiler
  enrichment; it does not replace the AST provider.
- Paths are repository-relative and must pass existing graph security and
  ignore handling.
- Files already skipped by the graph remain skipped and do not create drift.
- Reaching `max_sites_per_file` emits
  `ownership_analysis_status(file, ast_extraction, partial, site_cap)` and
  retains the bounded observations already produced.
- Dynamic configuration changes trigger a full ownership-enriched rebuild.
- Compiler-provider results are full-rebuild-only in Phase One. Incremental
  hook updates replace AST edges for the edited file and mark compiler evidence
  stale rather than mixing generations.

The graph format must be bumped because older graphs lack the new closed
relation set and invalidation semantics.

## 10. Query surface

All relations are immediately available through `query_code_graph` and the CLI
graph query command with identical exact/glob semantics.

Phase One should also add a read-only convenience command and MCP tool handler:

```text
phr-mcp graph ownership <function-id-or-glob>
```

The response groups evidence by function and site and must include:

- source location;
- observed operation;
- structural relationships;
- evidence level and provider;
- type/MIR availability;
- stale/partial status; and
- explicit limits.

Example rendering:

```text
Function: rust:example-app::llm::scheduler::Scheduler::acquire

Observed:
  sync lock acquired at src/llm/scheduler.rs:66
  lexical lock scope ends before await at line 107

Evidence:
  AST: available
  type inference: available
  MIR: unavailable (async_lowering)

Limit:
  lexical scope is not general control-flow or borrow-liveness proof
```

An empty query means “no indexed ownership evidence matched,” never “the code
has no ownership concern.”

## 11. RETE integration and rule eligibility

`Edge::to_fact()` already converts each graph relation into a RETE fact and
preserves its source-file provenance. No core engine change is required.

Phase One relations are query-only:

- no packaged rule references them;
- `phr-mcp audit` does not create ownership findings from them;
- `init --packs rust` does not install an ownership rule; and
- the catalogue contains no ownership enforcement entry.

Before any relation becomes audit-eligible, it must have a measured precision
report over at least Phronesis and the downstream consumer, named false-positive classes, and a
warning message that states its evidence limit. MIR-unavailable or partial
evidence cannot satisfy a rule condition that claims MIR confirmation.

## 12. Downstream-consumer acceptance corpus

Tests use minimized repository-local fixtures derived from these shapes; field
testing uses a temporary copy or a concurrently safe read-only view. Phronesis
must not write configuration, graph output, caches, or build artifacts into the
active downstream-consumer checkout.

| Case | Required output | Forbidden conclusion |
|---|---|---|
| `execute_rete_with_provenance` filter-before-clone incident | `filter_before_clone`; clone/filter sites and spans | `expensive_clone` without runtime or historical cost evidence |
| `check_records_on_arrival` snapshot | clone/collect sites and `clone_before_await` | `unnecessary_clone` or `borrow_live_across` from AST |
| `reposition_group_member` read/write separation | `read_before_mutation`; successful MIR availability when the provider supports the fixture | mutation conflict without diagnostic/MIR evidence |
| `handle_unchecked_action` current-location clone | clone site; `resolved_type` only when actually resolved; `clone_before_await` | deep/expensive clone inferred from the field name |
| `Scheduler::acquire` lock service | two lock sites and `lock_scope_ends_before_await` | `lock_scope_may_cross_await` from co-occurrence |

Additional adversarial fixtures must cover:

- clone-before-filter versus filter-before-clone;
- unrelated filter and clone statements on adjacent lines;
- small scalar/identifier and aggregate clones with identical syntax shape;
- explicit `drop(guard)` before await;
- a guard genuinely live across await;
- unbound temporary guards;
- early return, match, loop, closure, and nested async-block boundaries;
- declarative macro-generated calls;
- procedural macro analysis disabled;
- type resolution unavailable;
- MIR unavailable; and
- comments and strings containing `.clone()`, `.lock()`, or `.await`.

## 13. Tests and gates

### 13.1 Unit tests

- Stable site IDs use UTF-8 byte offsets.
- Function IDs exactly match existing `graph_function` IDs.
- Each base relation has the documented arity and argument order.
- Comments, strings, and bodyless trait signatures emit no sites.
- Chained filter-before-clone is detected; mere lexical order is not.
- Lock scope selects the narrowest binding block.
- AST extraction never emits MIR-only relations.
- Provider failure emits status and preserves AST observations.
- File replacement removes stale ownership sites.
- Site caps report partial analysis without panicking.

### 13.2 Integration tests

- Graph rebuild with ownership disabled emits no ownership relations.
- Enabling AST ownership emits and persists expected edges.
- Graph hydration exposes those edges as facts with graph provenance.
- Compiler evidence becomes stale after an incremental source edit.
- Configuration changes force the required rebuild.
- CLI and MCP ownership queries return identical grouped evidence.
- Exact and embedded-glob function queries behave like other graph queries.
- Old graph formats rebuild rather than mixing evidence generations.

### 13.3 Quality gates

Run:

```text
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Also run the project MSRV workflow and measure, with ownership disabled and
enabled:

- base and derived edge counts;
- serialized graph bytes;
- full rebuild time;
- incremental AST update time;
- hydration time; and
- peak memory if readily available.

Ownership disabled must remain within normal measurement noise of the current
graph path. Enabled measurements are reported, not assumed acceptable.

## 14. Implementation sequence for Fable

1. Add relation constants/schema tests and bump the graph format.
2. Add `.phronesis/graph.toml` ownership parsing, validation, and invalidation.
3. Port the bounded Python AST probe into the Rust graph extractor using the
   existing parsed tree and canonical function index.
4. Emit site, span, operation, evidence, and analysis-status base edges.
5. Add the four bounded AST derivations, keeping MIR-only relations impossible
   from that code path.
6. Add persistence, compaction, stale-generation, and site-cap tests.
7. Add the grouped CLI/MCP ownership explanation query.
8. Introduce the compiler-provider interface.
9. Implement rust-analyzer type/status enrichment only through a stable
   interface; otherwise ship the interface with the provider experimental and
   availability-only.
10. Run the five-case downstream-consumer field test through a safe read-only view and
    dogfood the same query on Phronesis.
11. Publish measurements and false-positive analysis.
12. Stop. Do not add a rule as part of this implementation.

## 15. Release criteria

The query-only feature is ready when:

- all five downstream-consumer cases produce the required evidence and none of the
  forbidden conclusions;
- the scheduler negative control does not produce a crossing-await relation;
- all compiler-unavailable cases are explicit rather than silently absent;
- every ownership site resolves to a real graph function and tracked file;
- incremental edits cannot retain stale site edges;
- graph size and latency measurements are documented;
- stable and MSRV CI pass; and
- the user-facing documentation says that findings are evidence with stated
  limits, not proof.

## 16. Deferred decisions

- Whether rustc MIR should be imported from an external artifact or collected
  by a dedicated Cargo wrapper.
- Whether ownership evidence eventually belongs in a sidecar graph when scale
  exceeds the core graph budget.
- Whether runtime allocation traces can map reliably to source site IDs.
- What measured precision permits any relation to become audit-eligible.
- Whether a future ADR should govern ownership rules after the query surface
  has demonstrated value.

## 17. Rejected alternatives

### Lower the clone-count threshold

Rejected. Count has no cost, type, frequency, or lifetime meaning.

### Add `rust_expensive_clone` as an AST predicate

Rejected. Tree-sitter cannot establish allocation size or runtime frequency.

### Warn on every lock in an async function

Rejected. The downstream-consumer scheduler is a concrete negative control: both
synchronous guards end before its await.

### Treat missing MIR as no ownership problem

Rejected. The bounded experiment produced missing MIR for all four async
functions. Absence is a capability result, not negative evidence.

### Run rust-analyzer or Cargo on every hook

Rejected. It would add latency, mutate build state in some configurations, and
can conflict with an actively edited sibling checkout. Compiler enrichment is
explicit and rebuild-only.

### Put the observations only in `SyntaxFacts`

Rejected. That would make them ephemeral and prevent repository-wide graph
queries and composition with tests, coverage, runtime traces, and decisions.

### Reconstruct general borrow liveness from lexical scopes

Rejected. Lexical scope is useful bounded evidence but diverges from
control-flow liveness around early exits, loops, closures, desugaring, and
explicit drops.

## Addendum A: Ownership evidence as a provenance extension

This feature is also a concrete extension of Phronesis's provenance offering.
The existing provenance work and this specification answer different links in
one explanation chain:

```text
source observation
  -> graph claim
  -> RETE matched fact
  -> rule consequence
  -> governing ADR
```

The links have distinct meanings:

- `Fact.source` answers **which subsystem asserted this fact**.
- Evidence relations answer **what observation supports this claim, at what
  strength, and what analysis was unavailable**.
- `Provenance::RuleFiring` answers **which matched facts caused a consequence**.
- `decision_enforces` and `rule_governed_by` answer **why the rule exists**.

None substitutes for another. In particular, `source: graph:src/foo.rs` does
not establish that a relationship was MIR-confirmed, and
`ownership_evidence(site, ast, tree_sitter_rust)` does not establish that the
observation is correct on every runtime path.

### A.1 Phase One requirement

The ownership implementation must preserve enough lineage for its explanation
query to traverse from every derived ownership relationship to its supporting
sites and from those sites to evidence level, provider, span, and analysis
status.

For example:

```text
filter_before_clone(function, filter_site, clone_site)
ownership_evidence(filter_site, ast, tree_sitter_rust)
ownership_evidence(clone_site, ast, tree_sitter_rust)
ownership_site_span(filter_site, file, start, end)
ownership_site_span(clone_site, file, start, end)
ownership_analysis_status(function, mir_lowering, unavailable, async_lowering)
```

This supports a bounded explanation:

> A filter and clone occur in the same expression chain. The relationship is
> supported by AST evidence. Type inference was available, but MIR lowering
> was unavailable for this async body. This is not runtime cost evidence.

The query must render the unavailable MIR fact alongside the positive AST
facts. Omitting unavailable capabilities would make weak evidence appear
stronger than it is.

The implementation already in progress does not need to stop for a general
provenance refactor. The relations in Sections 6 and 10 satisfy this Phase One
requirement when their traversal is preserved and tested.

### A.2 Candidate shared evidence vocabulary

Ownership should be the first demanding consumer of a future language-neutral
evidence-lineage layer, not a permanent ownership-only provenance system. A
follow-on specification should evaluate these generic relations:

| Relation | Arguments | Meaning |
|---|---|---|
| `evidence_supports` | `[evidence, claim]` | Evidence directly supports a reified claim. |
| `evidence_kind` | `[evidence, kind]` | `ast`, `type`, `ir`, `diagnostic`, `runtime`, or another closed kind. |
| `evidence_provider` | `[evidence, provider, version]` | Tool or subsystem that produced the evidence. |
| `evidence_span` | `[evidence, file, start_byte, end_byte]` | Bounded source location. |
| `evidence_run` | `[evidence, run, revision]` | Optional execution/rebuild identity and source revision. |
| `analysis_status` | `[subject, capability, status, reason]` | Available, partial, unavailable, or failed capability. |
| `derived_from` | `[claim, premise]` | A derived claim depends directly on another claim or observation. |

This vocabulary is intentionally deferred. Reifying every graph edge as a
claim can substantially increase graph volume, complicate compaction, and
duplicate the existing stable `Edge::fact_id`. The follow-on design must first
decide whether an edge's fact ID can serve as its claim identity, whether only
evidence-bearing edges are reified, and how derivation provenance survives
incremental rebuilds.

### A.3 Uses beyond ownership

If the shared layer proves worthwhile, the same chain can describe:

- `tested_by` supported by static call resolution, a coverage trace, or both;
- `calls_rhai_fn` supported by a parsed script call site;
- `consumes_data` supported by a literal artifact path and deserialization
  call;
- CUE import resolution supported by package-index candidates;
- drift findings supported by the compared document spans; and
- a rule consequence connected backward through its facts and evidence and
  forward to its governing ADR.

Each consumer must retain its own semantic limits. Coverage demonstrates that
a function executed during a test run; it does not by itself prove which test
assertion validated the function. A literal artifact path demonstrates bounded
dataflow evidence; it does not prove the consumer used every key. Generic
lineage makes those limits visible but does not erase them.

### A.4 Provenance acceptance criteria

In addition to Section 15, Phase One is acceptable only when:

- every rendered ownership relationship names its supporting sites;
- every supporting site has a source span and evidence provider;
- partial, stale, failed, and unavailable analysis is visible in CLI and MCP
  output;
- rule-firing provenance retains the graph fact sources if a project later
  writes a custom rule over these query-only relations;
- the explanation never upgrades AST evidence to type, MIR, diagnostic, or
  runtime evidence; and
- an empty evidence path is rendered as “unattributed” or “no indexed evidence
  found,” never as proof that the relationship is false.

This addendum does not authorize a packaged ownership rule. It establishes the
lineage contract that any future rule would need to inherit.
