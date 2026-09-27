# SPEC: Unified test evidence for functions

**Status:** Draft for review — not approved for implementation  
**Scope:** `crates/phronesis-mcp` host only — no engine changes  
**Derives from:** `SPEC-triple-store-rete.md` §1.2, §4.4, §5.5; `SPEC-coverage-evidence.md` §3–§8  
**Decision:** D3 — coverage observations are valid only at the current revision; stale hits never count

## Summary

Give each function one queryable view of its test evidence without conflating static reachability with execution. The revision-free graph persists `test_evidence_static(fn, test, kind)` for direct test attribution and statically resolved reachability. A bounded query-time `test_evidence_binary(fn, test)` view represents tests reaching a Cargo binary whose code reaches the function. Revision-bound coverage remains in its own store; current observations join at query or hook time as kind `observed` and include their revision.

The same host-side derivation will power `phr-mcp graph evidence <fn>`, an MCP query tool, and the hook-time closed-world fact `no_test_evidence(fn)`. Existing `no_direct_test(fn)` keeps its meaning and remains available for compatibility.

## Problem

`no_direct_test(fn)` answers a narrow question: does a direct `tested_by(fn, test)` edge exist? It does not count transitive static reachability or a test that runs a Cargo binary reaching `fn`. The whole-suite measurements supplied with this proposal report 1,651 functions flagged by this relation; of those, 1,494 executed during tests, 1,115 had static `test_reaches` evidence, and 379 executed without any static reach edge. About 101 did not execute, of which about 60 were substantive functions.

These are run-specific measurements, not guarantees about another checkout or revision. The numbers and their limitations are recorded in §8.

## Goals

1. Provide one static relation that states which test is evidence for which function and why.
2. Preserve graph derivation as a pure function of graph edges. No test-evidence edge depends on a Git revision, coverage store, command execution, or external state.
3. Make observed execution evidence available alongside static evidence while enforcing D3: stale coverage never counts.
4. Expose complete evidence for one function through CLI and MCP interfaces.
5. Let rules distinguish absence of all known evidence from absence of a direct test.
6. Reuse existing graph hydration, coverage store validation, and RETE facts; do not change the engine.

## 1. Evidence semantics

The word *evidence* describes the relation the resolver observed. It is not proof that a test asserts the relevant behavior, nor that every runtime path was exercised.

| Kind | Source | Meaning | Revision-bound? |
|---|---|---|---|
| `direct` | `tested_by(fn, test)` | The extractor attributed a direct test edge to this function. | No |
| `static_reach` | `test_reaches(test, fn)` | Resolved call-graph closure reaches the function from a directly attributed test. | No |
| `binary_reach` | Virtual `test_evidence_binary(fn, test)` view over `test_reaches` and `bin_reaches` | The test statically reaches a Cargo binary's `main`, whose stored closure reaches the function. | No |
| `observed` | Current, verified coverage store at query/hook time | The coverage collector recorded that this test executed a region belonging to the function. | Yes; include revision |

`direct` and `static_reach` are persisted derived rows; `binary_reach` is computed only by the query-time virtual view. A test may have more than one kind for a function. `observed` is added by consumers at read time and is never persisted to `graph.jsonl`.

## 2. Static graph relation

### 2.1 Relation and derivation

Add the derived, serialized relation `test_evidence_static` with positional arguments:

```text
test_evidence_static(fn, test, kind)
```

`fn` is the canonical graph function identity, `test` is the canonical test identity, and `kind` is exactly `direct` or `static_reach`.

Derive the rows from the existing relations as follows:

```text
tested_by(fn, test)                         -> test_evidence_static(fn, test, direct)
test_reaches(test, fn)                      -> test_evidence_static(fn, test, static_reach)
```

The first two arguments follow the same function-first convention as `tested_by`; `test_reaches` is test-first. The third argument is a stable string enum. `static_reach` includes the resolved closure, whether or not the test has a `direct` row for that same function.

The derivation belongs in `crates/phronesis-mcp/src/graph/derive.rs`. The current `derive_all` passes the base edge slice independently to each derivation and appends results; ordering a base-only `test_evidence_static(base)` call after `test_reachability(base)` would not expose that function's results. Implement this as a true two-stage derivation: compute `test_reachability(base)` once, retain its results, append them to the output, and pass only its `test_reaches` edges (filtering out `bin_reaches`) alongside `base` to `test_evidence_static`, or recompute only the `test_reaches` relation there. Do not let `bin_reaches` feed `static_reach`; binary evidence remains query-time only. `direct` rows derive from base `tested_by`; `static_reach` rows derive from the derived `test_reaches` relation. Both stages are pure and deterministic functions of the base edge set, with no revision or coverage input. `no_direct_test` remains a single-stage derivation directly over base `tested_by` edges, unchanged.

Re-deriving from a changed base edge set must retract obsolete results through the existing source-file compaction and rebuild paths. Both `crates/phronesis-mcp/src/graph/sync/rebuild.rs::rebuild` and `on_save` recompute derived edges wholesale on every save: each keeps only base edges (`!edge.d`) and calls `derive_all(&base)` over the full accumulated base set (`rebuild.rs` line 115), rather than patching old derived edges incrementally. `test_evidence_static` must follow this same wholesale-recompute discipline: because no revision or coverage data enters this relation, and because its stages are recomputed from the full base set and its corresponding `test_reaches` output, an incremental graph update produces the same edge set as a full rebuild over the same base graph.

### 2.2 Avoiding binary fan-out

Do **not** materialize a `test_evidence_static(fn, test, binary_reach)` edge for every pair produced by the join. For a test suite invoking a large CLI, those per-test copies of `bin_reaches` closure can add roughly 147 MB to the graph, defeating the current design that stores each binary's closure once.

Retain `test_reaches(test, main)` and `bin_reaches(main, fn)` as the bounded representation of binary evidence. Expand the join only when a query asks for a function or test's evidence. For a requested function `F`, index or scan `bin_reaches(main, F)` to find relevant mains, then match `test_reaches(test, main)` and synthesize `test_evidence_binary(F, test)` rows (rendered as kind `binary_reach`). CLI/MCP query and hook host derivation share the same query helper so their join semantics agree. A bounded in-memory result for one function is acceptable; do not cache or serialize the repository-wide fan-out.

This query-time join preserves the current graph's revision independence and storage bound: the cost is proportional to the requested function's matching mains and tests, rather than all test/function pairs in a binary closure.

### 2.3 Graph persistence and hydration

`test_evidence_static` is serialized using the existing graph edge schema (`p`, `a`, `src`) and format/version discipline. `src` must be computed from base edges, never read off an intermediate derived edge: `Edge::derived` (`graph/model.rs::Edge::derived`) always sets `src: String::new()` for a derived edge, so `test_reaches(test, fn)` itself carries no provenance to copy forward. Using it directly as `static_reach`'s support, as an earlier draft did, means every `static_reach` row has empty support and is omitted under the "no support has non-empty provenance" rule, which breaks incremental retraction (goal 2, criterion 5). The policy instead attributes each row's `src` to the *base* edges that produced it:

- For `direct`: unchanged — `src` is the `tested_by(fn, test)` base edge's `src`.
- For `static_reach`: `src` is the lexicographically smallest non-empty `src` among (a) every base `tested_by(fn0, test)` edge whose resolved closure — as computed by `test_reachability`, stopping at Cargo binary mains (`derive.rs::test_reachability`) — reaches `fn`, (b) every base `calls` edge consulted by that closure, including calls adjacency used to determine the path and stop behavior, and (c) every base `cargo_bin` edge that identifies a binary main used as a traversal stop. `test_reachability` reads `cargo_bin` as well as `tested_by` and `calls`, and changing those edges can change which `test_reaches` rows it produces. This support set is the base input needed to justify the relevant `test_reaches(test, fn)` output; it is not merely the tested_by/calls subset.

Duplicate supports across files use the minimum path across all supports; this is deterministic under edge order (both `calls` traversal and the base-edge scan are order-independent — `test_reachability` accumulates into `BTreeMap`/`BTreeSet`) and lets synchronization recompute rows when contributing source files change. Because `rebuild` and `on_save` both recompute `test_evidence_static` from the full base edge set on every save (§2.1), a `src`-contributing base edge's removal — a deleted `tested_by` edge, or the last `calls` edge on every path into `fn` — causes the row itself to disappear on the next wholesale recompute, which is what "incremental retraction" means for a relation with no dedicated retraction step. Omit a row only if no support (of either kind, for either relation) has non-empty provenance.

Register only `test_evidence_static` in `crates/phronesis-mcp/src/graph/hydrate.rs` `GRAPH_RELATIONS`, so rules demand-gate direct/static rows. No `test_evidence_binary` predicate is registered or hydrated into RETE: binary evidence is query-only and rules cannot trigger all-pairs fan-out. `no_test_evidence` is separately computed on demand by the host and is not persisted; see §5.

## 3. Revision-bound observations

Coverage stays in `.phronesis/coverage.jsonl` with `.phronesis/coverage.index`; do not copy observed rows into the graph or change graph revision semantics. At `graph evidence` query time and hook time, join only a loaded, integrity-verified store whose revision equals the current HEAD revision. Every joined observation has kind `observed` and carries the coverage revision that justified it.

Coverage's existing region identities (`fn:<file>::<item-path>`, plus branch-site identities) are the join basis. Map `fn:` regions only by exact repo-relative file equality plus exact equality between the region item's final `::` segment and the final `::` segment of a `defines_fn` identity, following `coverage/select.rs::static_region_matches`. Accept only one `defines_fn` candidate in that file; zero or multiple candidates yield no observation (never guess). Map `branch:` regions by the same file and enclosing item path before branch anchor/ordinal, again requiring a unique identity. Coarse `file:` and legacy unqualified IDs do not map to an individual function. Preserve exact region IDs in observations.

Per settled decision D3, a store at a different revision contributes no `observed` row. It also cannot suppress `no_test_evidence`. CLI and MCP results should report coverage availability/staleness separately so absence of an observed row is not mistaken for proof that the test never ran. Missing, busy, or corrupt stores contribute no observation; busy/corrupt diagnostics follow `SPEC-coverage-evidence.md`. Tests must assert stale coverage emits no `observed` and never suppresses `no_test_evidence`.

The query result's observation shape is:

```json
{"test":"rust:phronesis_mcp#test:hook_integration::test_name",
 "kind":"observed",
 "revision":"<40-hex HEAD sha>",
 "regions":["fn:crates/phronesis-mcp/src/hook.rs::check"]}
```

Only current verified records appear here. A stale row must never be emitted with `kind: observed`.

## 4. Query interfaces

### 4.1 CLI

Add:

```text
phr-mcp graph evidence <fn> [--path <project-root>] [--json]
```

`<fn>` is the exact canonical function identity. The default path is `.`. The command checks graph freshness using the existing graph status rules. If the graph is stale/outdated, it must say static evidence is unavailable or stale and must not present any static rows from that graph; stale/outdated output therefore suppresses static rows entirely rather than labeling them current. It then adds current coverage observations, if available.

Human-readable output is one header for the function and one row per `(test, kind)` pair, sorted by test then kind. Observed rows show the revision and region IDs. A concise status line reports graph and coverage availability (`current <sha>`, `stale <sha>`, `outdated <sha>`, `missing`, `busy`, `corrupt: <reason>`, or `unavailable`), aligned with JSON statuses. Stale/outdated/unavailable graph exits 2 and supplies no static rows; coverage problems are successful partial results (exit 0). Unknown canonical function exits 4, identifies it as unknown, and has no rows. Empty known-function results explicitly say no evidence rows were found under available sources.

### 4.2 JSON contract

`--json` emits exactly one JSON object with this versioned shape:

```json
{
  "schema_version": 1,
  "function": "rust:phronesis::network::fire_all_consequences",
  "graph": {"status": "current"},
  "coverage": {"status": "current", "revision": "<40-hex>"},
  "evidence": [
    {"test": "rust:phronesis#test:network::fires", "kind": "direct"},
    {"test": "rust:phronesis#test:network::fires", "kind": "static_reach"},
    {"test": "rust:phronesis-mcp#test:cli::runs", "kind": "binary_reach"},
    {"test": "rust:phronesis#test:network::fires", "kind": "observed",
     "revision": "<40-hex>", "regions": ["fn:crates/phronesis/src/network.rs::fire_all_consequences"]}
  ]
}
```

`graph.status` is `current`, `stale`, `outdated`, or `unavailable`. `coverage.status` is `current`, `stale`, `missing`, `busy`, `corrupt`, or `unavailable`; `revision` is present only for a verified loaded store. Include `function_status: "known"|"unknown"`; unknown has empty evidence and CLI exit 4. Stale/outdated/unavailable graph has no static rows and CLI exit 2; coverage problems are partial results with exit 0. JSON/MCP return status objects in every case. Each evidence object has required `test` and `kind`. `revision` and `regions` are required for `observed` and forbidden for static kinds. The list is sorted by `test`, then kind order `direct`, `static_reach`, `binary_reach`, `observed`, then region list. Empty evidence is `[]`, never `null`.

### 4.3 MCP tool

Add a read-only MCP tool named `get_test_evidence` in `crates/phronesis-mcp/src/server.rs`, with query/formatting logic factored into a host module shared with the CLI. Input is `{ "function": "<canonical function identity>" }`. Return the same schema-version-1 object as CLI JSON. It must report stale/unavailable source states instead of silently mixing them with current evidence. It does not mutate graph or coverage stores.

## 5. Hook-time absence fact

Add host-derived `no_test_evidence(fn)` for each production function for which neither static nor current observed evidence exists. Keep `no_direct_test(fn)` unchanged: it remains defined only by the absence of a `tested_by(fn, test)` edge.

`no_test_evidence(fn)` is true exactly when:

1. there is no `test_evidence_static(fn, test, direct|static_reach)` in the fresh graph and the bounded binary query finds no `test_evidence_binary(fn, test)`; and
2. there is no matching `observed` evidence from a verified coverage store at the current HEAD revision.

The fact is closed-world host derivation because RETE has no pattern-level negation. Compute it at hook time only when a rule names it; never persist it. The production function universe exactly reuses the `no_direct_test` filter: `defines_fn(file, fn)` with `file_type(file, production)`, falling back to all defined functions only when the graph has no production classifications. Binary evaluation queries `bin_reaches(main, fn)` then `test_reaches(test, main)` per demanded production function, retaining one function's result at a time; cost is bounded by demanded functions and matching mains/tests, with indexes where available. A stale, busy, corrupt, absent, or unavailable coverage store supplies no current observation and therefore does not suppress the fact. A stale or unavailable graph must not be treated as a fresh empty graph: do not assert `no_test_evidence` from unavailable static evidence; report/demote through existing graph freshness behavior. This prevents tool failure from becoming a false claim of no tests.

Add `no_test_evidence` to the closed predicate vocabulary with argument order `[fn]`, but exclude it from `GRAPH_RELATIONS`: it is a host-computed on-demand fact, never a serialized graph edge.

## 6. Code placement

| Concern | Proposed location |
|---|---|
| Derived direct/static edges and graph-level derivation | `crates/phronesis-mcp/src/graph/derive.rs` |
| Binary join helper and bounded function-scoped query | `crates/phronesis-mcp/src/graph/` query module shared by CLI/MCP and host hook derivation, never RETE hydration |
| Graph relation demand-gating | `crates/phronesis-mcp/src/graph/hydrate.rs` |
| Revision-checked observed joins and hook-time `no_test_evidence` host derivation | `crates/phronesis-mcp/src/coverage/` (new evidence resolver or an extension of coverage hydration) |
| CLI command and render dispatch | `crates/phronesis-mcp/src/main.rs` plus shared query/render module |
| Read-only MCP tool | `crates/phronesis-mcp/src/server.rs`, delegating to shared query code |
| Tests | `crates/phronesis-mcp/src/graph/derive.rs`, graph query/hydrate tests, coverage tests, CLI integration, MCP server tests |

This table locates proposed work; those changes are not part of this specification-only task.

## 7. Acceptance criteria

1. For a base `tested_by(fn, test)`, derivation returns `test_evidence_static(fn, test, direct)`.
2. For `test_reaches(test, fn)`, derivation returns `test_evidence_static(fn, test, static_reach)` with the stated argument order.
3. Given `test_reaches(test, main)` and `bin_reaches(main, fn)`, a function-scoped evidence query returns `binary_reach`; it does not serialize one derived edge per test/function pair.
4. Graph update and full rebuild over identical base edges produce identical persisted `test_evidence_static` rows, independent of edge order. Virtual binary results are excluded from persisted-edge equality and deterministic for an identical graph.
5. Replacing a test source or call edge retracts affected static rows on incremental synchronization; §2.1 provenance makes multi-file support order independent. Virtual binary results follow current inputs directly.
6. `phr-mcp graph evidence <fn>` lists every available test/kind pair; `--json` conforms to §4.2, is deterministic, and reports graph/coverage states.
7. The MCP `get_test_evidence` tool returns the same schema and evidence set as CLI JSON for the same root and function.
8. A current matching coverage record appears as `observed` with exact revision and region using §3 mapping. Revision mismatch, busy/corrupt/missing store emits no observed row and stale coverage never suppresses `no_test_evidence`.
9. A function with only static or current observed evidence has no `no_test_evidence` fact; a production function with neither has exactly one.
10. Stale or unavailable graph state does not produce a false `no_test_evidence` fact from an empty read.
11. `no_direct_test` outputs are byte-for-byte unchanged for the same base graph before and after this feature.
12. No binary fan-out rows are persisted or hydrated to rules; storage growth is bounded by direct/static graph evidence plus existing per-binary `bin_reaches` closures.

## 8. Measurements and evidence limits

The motivating measurements supplied for this spec are: 1,651 production functions flagged by the current `no_direct_test`; 1,494 of those observed during a whole-suite llvm-cov run; 1,115 with static `test_reaches` reachability; 379 observed at runtime without any static reach edge; and approximately 101 not observed, of which approximately 60 were described as substantive.

No run artifact, revision SHA, exact command line, classification rule for “substantive,” or underlying per-function export was included with the task materials or located in the requested source files. Therefore these figures are attributed reports, not independently reproduced results. They motivate the design and are not acceptance thresholds. Dynamic execution on one run/revision does not establish behavioral assertion quality or future reachability.

## 9. Non-goals

1. Do not change or remove `no_direct_test`.
2. Do not add declared, hinted, annotated, or user-authored test edges in this phase. Such edges would introduce a separate trust/provenance policy and are deferred.
3. Do not copy coverage observations into the revision-free graph or change the coverage store format in this spec.
4. Do not materialize the `test_reaches ⨝ bin_reaches` all-pairs fan-out.
5. Do not claim that static reach is runtime execution or that runtime execution is an assertion of correctness.
6. Do not introduce engine negation, graph history, a coverage dashboard, coverage percentages, test execution, test generation, or CI policy.

## 10. Decisions recorded for implementation

1. Function-region mapping is fixed by §3: exact file and item-path mapping, unique `defines_fn` candidate only; branch regions use the enclosing item path, and coarse/legacy IDs do not map to individual functions.
2. Use MCP tool name `get_test_evidence`; check for collision before implementation and resolve it before coding if occupied.
3. Unknown canonical function has `function_status: "unknown"` and empty evidence. CLI uses exit 4; JSON/MCP return the versioned status object. Stale graph uses exit 2 and no static rows; stale coverage is a successful partial result.

## Source grounding

- `SPEC-triple-store-rete.md` §1.2 defines `tested_by`, `test_reaches`, `bin_reaches`, and `cargo_bin`; §4.4 explains direct attribution, resolved reachability, binary-main factoring, and limitations; §5.5 describes demand-gated graph hydration.
- `graph/derive.rs::test_reachability` stops closure traversal at Cargo binary mains and stores each `bin_reaches(main, fn)` closure once. `no_direct_test` uses only direct `tested_by` edges.
- `coverage/select.rs` already unions changed-region observations with static reach and labels its coverage half by freshness. `coverage/hydrate.rs` validates revision and suppresses hit facts when stale or busy.
- `graph/hydrate.rs::GRAPH_RELATIONS` is the demand-gated list of graph predicates currently available to rules.
- The CLI command and MCP tool described above are proposed additions; the current graph CLI exposes rebuild/query/status/ownership, and the coverage CLI exposes collect/import/select.

## Review response

- Relation/fan-out contradiction: §§2.1–2.3 separate persisted `test_evidence_static` from virtual `test_evidence_binary`; only direct/static is registered or rule-visible.
- Revision-bound `no_test_evidence`: §5 defines an on-demand host fact excluded from persistence/`GRAPH_RELATIONS` and states the exact `no_direct_test` production filter.
- Deterministic `src`: §2.1 specifies minimum non-empty supporting source path and support edges.
- Function-region mapping: §3 specifies exact function and branch mapping plus ambiguity, coarse-region, and legacy-ID behavior.
- Stale graph and unknown function: §§4.1–4.3 define exit codes, static-row omission, unavailable status, and unknown status.
- Rule demand-gating: §§2.3 and 5 make binary evidence query-only and restrict RETE facts to static rows plus the host absence fact.
- Incremental/full equality: criteria 4–5 scope persisted equality to static rows and define deterministic virtual binary behavior.
- Stale coverage safeguards: §3 and criteria 8–10 require no stale observation and no suppression of `no_test_evidence`.
- `static_reach` provenance: §2.3 includes base `cargo_bin` edges alongside `tested_by` and `calls`, because binary-main stops affect `test_reachability`'s `test_reaches` output.
- Two-stage derivation: §2.1 requires `derive_all` to retain and pass `test_reachability` output into the static derivation; ordering base-only calls is insufficient.
- Reachability result filtering: §2.1 explicitly filters `test_reachability` output to `test_reaches`, excluding `bin_reaches` from `static_reach` input.
- Stale graph rows: §4.1 explicitly suppresses all static rows when graph status is stale or outdated, so stale data cannot be presented as current.
