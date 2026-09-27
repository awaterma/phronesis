# SPEC: Recovering dynamic-dispatch reach in the resolver, without guessing

**Status:** Draft for review — not approved for implementation
**Author:** w-S5-sonnet (swarm worker, agent-drafted)
**Created:** 2026-09-27
**Scope:** `crates/phronesis-mcp/src/graph/resolve.rs`, `graph/derive.rs` — no engine changes
**Reuses:** `SPEC-triple-store-rete.md` §4.4 (D4, `tested_by`/`test_reaches`/`bin_reaches`), `SPEC-coverage-evidence.md` (dynamic coverage as the fallback for what statics cannot see)

## Summary

389 functions execute during the whole-suite `cargo-llvm-cov` run but have no
static path from any test in the code graph (`tested_by` / `test_reaches` /
`bin_reaches` all empty). 266 are methods. Roughly half sit under
`crates/phronesis-mcp/src/graph/`, the extractor's own source. This spec
classifies *why* the resolver misses them, proposes resolution strategies
that stay inside D4 ("never guess a call edge"), and lays out a phased plan
with acceptance criteria tied to dynamic coverage as a check against
over-resolution.

## Problem

D4 is deliberately conservative: an edge the resolver cannot ground in
syntactic evidence visible to the caller is dropped and counted, never
guessed. That policy is correct — a wrong edge silently asserts coverage
that does not exist (the `String::is_empty` / `UnitMap::is_empty` example in
§4.4) — but it means every one of Rust's non-syntactic dispatch mechanisms
(trait objects, generics, macro-generated code, function-pointer tables,
proc-macro-expanded call sites) produces false negatives: functions that
demonstrably run, that the graph reports as unreached, feeding
`no_direct_test` and confidence scoring with noise.

The 389-function list is real signal that the *count* of misses is large
enough to matter (roughly 1% of ~35–40k graph functions, concentrated in a
few architecturally-recurring shapes) and that a handful of targeted,
evidence-only strategies could recover a meaningful fraction without
touching D4's no-guessing invariant.

## Evidence: sample classification

Stratified sample of 70 of the 389 (random seed over the full list; sample
distribution is an estimate, roughly ±11 percentage points at 95% confidence
for a proportion near 50%, not a guarantee for each class).
Classes were confirmed by grepping call sites and impl blocks per function,
not just by name pattern — see the worked traces below the table for the
non-obvious ones.

| Class | Sample count | Sample % | Extrapolated (of 389) |
|---|---|---|---|
| Trait method invoked through a language/std macro (`Display`/`Debug` `fmt`, `Ord`/`PartialOrd`/`Hash`) | 5 | 7% | ~27 |
| serde `Deserialize`/`Serialize`/`Visitor` trait methods (`deserialize`, `visit_seq`, `visit_map`, …) | 6 | 9% | ~35 |
| `rmcp` `#[tool_router]`/`#[tool]` macro-dispatched methods on `EpistemeMcp` | 4 | 6% | ~23 |
| Method call on a receiver bound by an untyped `for`/closure pattern (loop variable, iterator adapter param) | 9 | 13% | ~50 |
| `self.method()` chain whose root entry point is reached only through CLI-subprocess invocation (`Command::new("phr-mcp")` in an integration test) crossing an unresolved hop before reaching the leaf | 14 | 20% | ~78 |
| Free-function or fn-item dispatch table (extension → extractor registry, `match` returning a fn item) | 3 | 4% | ~17 |
| `dyn Trait` / generic `T: Trait` dispatch (parser/sensor abstractions across language packs) | 4 | 6% | ~23 |
| Test-only helper reached solely from `#[cfg(test)] mod tests` in *another* crate's integration fixture, via a path the single-file extractor cannot see | 3 | 4% | ~17 |
| Unclassified / needs deeper trace (plausibly a mix of the above, compounded — e.g. a Sensor method nested two dispatch hops deep) | 22 | 31% | ~171 |

The "unclassified" bucket is large by construction: most `graph/` subtree
misses are several hops downstream of a single unresolved dispatch point
(the top-level language dispatch, or the generated `#[tool_router]` boundary), so one
root cause produces a chain of dependent misses. Classifying the *chain
root* is more tractable than classifying each leaf; §Strategies below is
organized around chain roots for this reason.

### Worked traces (representative, not exhaustive)

- **`fmt` trait impls** (`ReteError::fmt`, `RulesLoadError::fmt`,
  `StoreCorruption::fmt`): `impl std::fmt::Display for X { fn fmt(...) }`.
  No literal `.fmt(` call site exists anywhere in source — invocation is
  inserted by the `{}`/`{:?}` formatting macros at compile time. The
  extractor parses source text; it cannot see macro-inserted trait-method
  calls.
- **serde Visitor** (`StrictJsonVisitor::visit_seq`,
  `context::capsule::StrictVisitor::visit_map`/`visit_seq`): implements
  `serde::de::Visitor`; called by `serde_json`'s generic deserializer
  through `T: Visitor` — a dependency-crate generic bound, invisible to a
  single-file, project-only extractor.
- **`rmcp` macro dispatch** (`EpistemeMcp::rebuild_code_graph`,
  `list_emitted_capsules`, `code_graph_status`): confirmed —
  `crates/phronesis-mcp/src/server.rs:292` carries `#[tool_router]` above
  the `impl EpistemeMcp` block; `#[tool(...)]` per method. The proc macro
  expands to a generated routing table at compile time; the source text has
  no call expression naming these methods from the dispatcher.
- **Untyped receiver via loop binding** (`audit::Level::as_str` reached as
  `r.level.as_str()` in `crates/phronesis-mcp/src/audit.rs:92`, inside a
  `for r in reports` (or `.iter().map(...)`) whose element type is a struct
  field, not a `let` annotation or a typed function parameter). D4 already
  documents this exact gap (§4.4, "a method call on a receiver whose type
  the extractor cannot read... stays unresolved").
- **CLI-subprocess boundary**: several `phronesis-mcp/src/graph/*` and
  `hook_facts.rs` functions (e.g. `graph::lua::file_type`, reached from
  `graph::lua::extract_lua`, reached from `hook_facts::assert_language_pack_facts`)
  have a normal, resolvable, bare-function call chain in isolation.
  Integration tests that exercise them do so largely by invoking the
  compiled `phr-mcp` binary as a subprocess (`assert_cmd`/`Command::new`),
  which the graph's own `cargo_bin`/`bin_reaches` machinery is built to
  handle (§4.4) — but only when the test's call to
  `env!("CARGO_BIN_EXE_<name>")` is direct or one hop through a same-file
  free-function helper. Where the harness wraps that call behind a shared
  test-utility module (a different file, or a method rather than a free
  function), `bin_reaches` never gets built for that test, and the whole
  hook-pipeline closure downstream of `main` is invisible even though
  `main`'s own resolved closure is otherwise intact. This is not a *new*
  gap in the resolver's per-call policy; it's a gap in how narrowly
  `extract.rs::record_binary_runs`'s helper-following rule is scoped (§4.4: "a bare
  call to a free function the caller's own module defines... never through
  a glob import or a method").

## Classes and strategies

Each strategy below stays inside D4: every edge it adds must be justified
by evidence already in the source (an impl block, a `use`, a signature, a
macro attribute) — never a name match alone.

### 1. Trait-object (`dyn Trait`) dispatch — recoverable, narrow

**Strategy:** when a call site is `receiver.method()` where the resolver
already knows the *trait* (from a typed `dyn Trait` parameter, `Box<dyn
Trait>` field, or `impl Trait` return) but not the concrete type, and every
`impl Trait for X` in the crate is visible to the extractor (no
`#[cfg]`-gated or external impls), emit one edge to **each** implementor's
method, tagged with a new edge kind `calls_dyn` distinct from `calls`. This
mirrors the existing `impl_of` machinery already used for typed-hint
resolution (§4.4) — it is enumeration over *visible* evidence, not a guess
about which implementor is live at runtime.

- **Precision risk:** medium. A `calls_dyn` edge is disjunctively true (one
  of N implementors), not a certain fact like `calls`. It must not be
  conflated with `calls` in reachability without a policy decision (§Plan).
- **Cost:** moderate — needs a project-wide impl-set index already largely
  present via `impl_of`.
- **Sample estimate (from the 70/389 sample):** ~23 functions, extrapolated from the 70/389 sample; the class also
  includes generic dispatch, which remains excluded.

### 2. Generic `T: Bound` dispatch — do NOT resolve statically

Monomorphization means the same call site can reach an unbounded number of
concrete impls depending on the instantiation, and — unlike `dyn Trait` —
the crate's own `impl` set is not even the right search space (a generic
fn can be instantiated from another crate or a test-only type). Enumerating
implementors here crosses from "evidence" into "plausible guess."
**Recommendation: leave to coverage.** `SPEC-coverage-evidence.md`'s
per-test dynamic hits already answer "did this monomorphization actually
run," which is the only sound source of truth for generic dispatch.

### 3. `fmt`/`Ord`/`Hash`/other std-macro-invoked trait methods — recoverable, cheap, high confidence

**Strategy:** when a function is the sole method in an `impl <StdTrait> for
X` block for the specifically invoked trait methods (`Display`, `Debug`,
`Ord`, `PartialOrd`, `Hash`, `PartialEq`, or `Eq`), do not infer reach from
type participation alone. Emit an edge
only at a source-visible operation site whose syntactic shape invokes that
trait operation and whose receiver/operand type resolution succeeds. The
closed recognized shapes are: `format!`/`println!`/`write!` format strings
containing `{}` (Display) or `{:?}` (Debug), `==`/`!=` (PartialEq/Eq),
`<`/`<=`/`>`/`>=` and `.cmp()` (Ord/PartialOrd), `.sort()`/`.sort_by()`
and `.sort_by_key()` (Ord or an explicitly visible comparator), and a
resolved `HashMap<X, _>` key insertion/lookup position (Hash and Eq).
Formatting braces and comparison operators must be parsed from the actual
macro/token or expression shape; ambiguous formatting arguments, operators,
or sort closures are not evidence. No Clone edge is proposed.

- **Precision risk:** low only when receiver/operand types and the operation
  shape both resolve. Mere construction or participation of `X` is not a
  call site and never emits an edge.
- **Cost:** low. A few new call-site shapes (`format!`, `{}`/`{:?}` in
  `println!`/`write!`, `.sort()`, `.sort_by_key()`) recognized alongside
  the existing method-call grammar.
- **Sample estimate:** ~27 (fmt) + ~2 (cmp/hash family) ≈ 29 functions, extrapolated from the
70/389 sample.
  This is the single best cost/precision ratio in the set.

### 4. serde `Deserialize`/`Serialize`/`Visitor` — source-visible hand-written impls only

Full serde-generic resolution (trait method invoked through
`serde_json::from_str::<T>()`) is dependency-crate generic dispatch, not
project code. The only eligible sub-case is a source-visible, hand-written
call chain: the extractor sees `serde_json::from_str::<ConcreteType>(...)`
(or an equivalent explicit call) and the concrete type's hand-written
`Deserialize::deserialize` implementation visibly calls a hand-written
`Visitor::visit_*` method, with each call resolving by ordinary scope and
type evidence. A derive attribute, named concrete type, generated Visitor
name, or compiler-generated implementation is not evidence for a calls
edge. Derived cases and dependency generic dispatch remain coverage-only.

- **Precision risk:** low for the fully source-visible chain; any missing
  source call hop means no synthetic edge is emitted.
- **Cost:** moderate — follow only explicit hand-written impl and Visitor
  calls; no association to generated impls is attempted.
- **Sample estimate:** small; at most 1–2 of the ~35 extrapolated from the
  70/389 sample. Derived visitors stay in class 7.

### 5. `rmcp` tool-macro dispatch — coverage only absent a verified expansion contract

**Recommendation: do not emit calls edges for macro-generated routing.**
`#[tool_router]` and `#[tool(...)]` attributes identify declarations, but
the source has no syntactic call expression from the generated router to
those methods. The repository dependency specifies `rmcp` version `0.17` (Cargo.lock
pins the resolved release), but a dependency version alone does not
establish a verified expansion contract. Reconsider only if
a version-specific, auditable expansion contract is documented and tested
to enumerate exactly these dispatch targets and call behavior, or if a
separate explicitly weak relation is designed outside certain `calls` and
`test_reaches`.

- **Precision risk:** unresolved under D4; attributes alone do not ground
  caller-to-callee edges.
- **Cost:** high if a closed macro contract is later established; attribute
  recognition by itself is insufficient.
- **Sample estimate:** 0 under this proposal; ~23 functions were attributed
  to this class in the 70/389 sample, but remain for coverage absent a
  separately justified contract/relation.

### 6. Function pointers / dispatch tables — conditional, disjunctive recovery

**Strategy:** when a `match` arm's body is exactly a bare function name (an
fn item, not a call) assigned to a variable, returned, or stored in a
`const`/`static` array/map keyed by a literal (extension string, enum
discriminant), and every array/map entry is a *named, in-crate* free
function or associated function, emit `calls_dyn` from the reached lookup site to each entry. These
entries are disjunctive alternatives, not certain calls, and may not feed
`calls`, `test_reaches`, or coverage selection as certain reachability.
Emit only when the lookup site itself is syntactically present and its
table/key relationship is resolved; runtime-dependent keys retain all
alternatives as disjunctions. Never emit edges from an unreachable table
definition.

- **Precision risk:** low as a disjunctive relation — table entries are
  syntactically named, not inferred. Attach alternatives to the reached
  lookup call (`table[key]()`), never to the table definition.
- **Cost:** moderate — needs a syntax shape for "value position is a bare
  path to a function," which the grammar mostly already has via
  `@method:Type:name` typed-hint resolution; extending it to non-call
  positions is new surface.
- **Sample estimate:** ~17, extrapolated from the 70/389 sample.

### 7. Macro-generated call sites the parser cannot see at all — do NOT resolve statically

`#[derive(Deserialize)]`-generated `Visitor` implementations, `#[derive(Debug)]`
bodies, and any other case where the *only* representation of the call is
compiler-internal (no source text names it, not even inside the macro
invocation) cannot be resolved by a source-text extractor without
reimplementing a subset of macro expansion (out of scope; tree-sitter has
no macro-expansion pass, and running the real expander would mean
depending on rustc internals or `cargo expand`, a much larger and slower
dependency than this extractor currently carries).
**Recommendation:** leave entirely to coverage evidence. This is exactly
the case `SPEC-coverage-evidence.md` exists for — dynamic hits are the only
sound signal here, and no static edge should ever claim to cover it.

### 8. CLI-subprocess boundary gaps — a scoping fix to `record_binary_runs`, not a new dispatch class

**Strategy:** this is not dynamic dispatch — it is that §4.4's existing
helper-following rule for `CARGO_BIN_EXE_<name>` tests is scoped to "a bare
call to a free function the caller's own module defines... never through a
glob import or a method." Several integration-test suites in this repo
route subprocess invocation through a shared test-utility *method* (e.g. a
helper on a test fixture struct) rather than a same-module free function,
so the existing mechanism silently produces zero `tested_by(<bin>::main,
T)` edges for those tests. **Recommendation:** extend the helper-following
rule one more hop for the single, narrow shape "a call to `self.method()`
or a same-crate free function that itself makes exactly one
`env!("CARGO_BIN_EXE_...")`-anchored call," keeping the "no glob import,
no ambiguous receiver" guardrails from §4.4 intact. This is evidence-based
(the call chain is still syntactically visible), just currently
under-scoped.

- **Precision risk:** low — same evidence bar as the existing rule, just
  one hop deeper, with the same non-glob/non-ambiguous restriction.
- **Cost:** low — a bounded extension to existing logic in
  `graph/extract.rs::record_binary_runs` (with helper propagation in
`graph/binary_runs.rs`).

Only unconditional, unambiguous helper propagation is eligible: every path
through a followed helper must reach exactly one identical `env!` binary
reference. A helper with conditional invocation, multiple branches that
invoke different binaries, or a path that can return without invoking the
binary is not followed for a `tested_by` edge. This conservatively treats
any optional branch as insufficient proof of an unconditional binary run. Methods remain excluded
unless their receiver resolves under the existing D4 rules. This avoids
treating syntactic presence somewhere in a helper body as proof that the
test runs that binary.
- **Sample estimate:** ~78, extrapolated from the 70/389 sample. This is the largest sample
  estimate; downstream dependent recovery remains unmeasured.

## Summary table: strategy vs. class vs. recommendation

| # | Class | Static resolution? | Recovered (est.) | Precision risk |
|---|---|---|---|---|
| 1 | `dyn Trait` dispatch, closed impl set | Conditional — disjunctive `calls_dyn` only | ~23 | medium |
| 2 | Generic `T: Bound` dispatch | No | 0 | n/a |
| 3 | `fmt`/`Ord`/`Hash` std-trait macros | Yes | ~29 | low |
| 4 | serde Deserialize/Visitor (named-concrete-type only) | Partial | ~1–2 | low |
| 5 | `rmcp` `#[tool_router]` macro dispatch | No — coverage only | 0 (sample estimate; ~23 classified) | unresolved |
| 6 | fn-pointer / dispatch tables | Yes | ~17 | low |
| 7 | Derive-generated call sites (no source text at all) | No | 0 | n/a |
| 8 | CLI-subprocess boundary (scoping fix) | Yes | ~78 (+ dependents) | low |

Sample-based estimated recovery without guessing: roughly **120–150 of the
389** (31–39%), extrapolated from the 70/389 sample and highly uncertain;
this range is an approximate sum, not a confidence interval.
This excludes class 5 (~23) because macro attributes alone do not ground a
call edge, class 1 (~23) pending a decision for disjunctive edges, and class
4 except its source-visible hand-written subset. It is not a measured total.
The remainder is a mix of genuinely unresolvable generic/derive dispatch (classes 2 and 7,
correctly left to coverage) and the "unclassified" 31% that likely
decomposes into combinations of the above once traced hop-by-hop — the
phased plan below front-loads class 8 partly *because* fixing chain roots
should shrink that bucket automatically before it needs its own strategy.

## Plan

**Phase 0 — measurement harness.** Before any resolver change, snapshot
`unresolved` and `no_direct_test` counts, and cross them against the
existing coverage store (`SPEC-coverage-evidence.md`) so every subsequent
phase can be checked against dynamic evidence, not just count deltas.

**Phase 1 — class 8 (CLI-subprocess helper scoping).** This targets the
largest sample-estimated class (about 78 extrapolated from 70/389), subject
to the unconditional-path acceptance criteria. Reclassify the sample after
implementation to measure how much of the unclassified bucket changes.

**Phase 2 — class 3 (std-trait operations).** Implement only the enumerated
source call shapes with resolved receiver/operand types. Class 5 remains
coverage-only unless a version-specific audited macro contract is established.

**Phase 3 — class 6 (fn-pointer/dispatch tables).** Needs new grammar
surface (bare-path-in-value-position); ship after 1–2 have proven the
measurement harness catches regressions.

**Phase 4 — class 1 (`dyn Trait`, closed impl set).** Before implementation,
decide whether `calls_dyn` remains separate and whether any explicit opt-in
consumer may use it. By default, it does not count as `calls`, `test_reaches`,
`bin_reaches`, or certain coverage. Reject the phase unless its tests prove
that separation. Exclude any trait with a `cfg`/feature-gated implementor
or any implementor whose presence is conditional; do not emit `calls_dyn`
for such a trait.

**Phase 5 — reclassify the full original 389-function set.** Attribute each
miss first to its earliest unresolved chain root, then count functions by
root cause (classes 1–8 plus generic, derive-generated, and unknown).
Report both root counts and downstream dependent counts to avoid double
counting. For every remaining miss record whether it is generic dispatch,
derive/macro-generated with no source call, or unknown after tracing. Keep
unknown as a visible bucket; do not redistribute it by inference.

## Acceptance criteria

1. **Evidence provenance audit for every phase.** Independently of coverage,
   audit every added edge and record the exact source site(s), syntax shape,
   resolved caller/callee identities, receiver/operand type evidence, and
   recognizer that produced it. Reject edges supported only by matching
   names, attributes, type participation, or dynamic co-occurrence. Each
   phase adds fixtures for positive and negative provenance cases, including
   same-named unrelated definitions.
2. **Coverage is a secondary cross-check, not proof of grounding.** Where
   coverage exists, compare proposed reach against per-test hits and flag
   disagreements for review. A hit never validates source provenance; lack
   of a hit is not alone proof that an edge is wrong.
3. **`unresolved` count moves down**, not just `no_direct_test`. A phase that
   reduces `no_direct_test` by reclassifying calls as `tested_by` without
   reducing the underlying `unresolved`/`ambiguous` counters from §4.4 has
   not actually resolved anything.
4. **Incremental update equals full rebuild.** Each new recognizer is tested
   against the two-tier extraction model (§4.5): a single-file edit plus
   incremental derive produces byte-identical results to a from-scratch
   rebuild.
5. **Disjunctive relations stay distinct.** `calls_dyn` and class 6 table
   alternatives are never counted as certain `calls` or included in
   `test_reaches`/`bin_reaches` unless a consumer explicitly opts into that
   weaker meaning. Acceptance tests assert default exclusion and separately
   verify any opt-in behavior.
6. **Class 6 lookup scope.** Tests prove no table-entry edges arise from an
   unreachable table, and runtime-dependent keys produce only disjunctive
   alternatives at the reached lookup site.
7. **Class 8 path behavior.** Tests cover a helper that conditionally runs
   the binary, a helper with multiple branches (same and differing binary
   references), and a helper reached only on some paths. Emit a certain
   `tested_by` edge only if every eligible path invokes exactly one same
   binary; otherwise retain the unresolved/missed case.
8. **Per-phase recognizer audit and no name-only matching.** Acceptance
   tests for each phase verify that every newly recognized edge has the
   recorded syntax and resolution evidence and that same-name-only matches
   are rejected, in addition to coverage cross-checks.

## Non-goals

- Full macro expansion (`cargo expand`-equivalent) — out of scope; class 7
  stays with coverage evidence by design.
- Resolving generic `T: Bound` dispatch statically (class 2) — the search
  space is not crate-local and the "evidence" would not meet D4's bar.
- Percentage-based reachability targets. This spec reduces false
  negatives in a closed-world graph; it does not set a target reach
  percentage, consistent with `SPEC-coverage-evidence.md`'s non-goal of
  coverage percentages.
- Changing D4's no-guess policy itself. Every strategy above is scoped to
  stay inside it; none proposes probabilistic or heuristic edges.

## Review response

- **Blocker — derived serde code:** Class 4 now permits only hand-written,
  source-visible impl and Visitor call chains; derive-generated cases stay
  with coverage.
- **Blocker — tool router:** Class 5 emits no `calls` edges from attributes;
  reconsideration requires a version-specific audited contract or a separate
  weak relation.
- **Blocker — coverage as grounding:** Acceptance criteria require an
  independent provenance audit for every edge; coverage is only a secondary
  cross-check.

## Resolved design points

1. Resolved for this proposal: `calls_dyn` is separate and excluded from
   certain `calls`, `test_reaches`, and `bin_reaches` by default. Any future
   opt-in consumer requires documented semantics and tests before phase 4.
2. Class 8 remains limited to same-file free-function helpers and only to
   unconditional, unambiguous paths as specified above; conditional or
   partial-path helpers are negative cases in acceptance tests.
3. Resolved for this proposal: class 6 alternatives attach only to a
   reached lookup site, as disjunctive `calls_dyn` edges, never at definition.
4. Phase 5 answers the unclassified question by tracing each miss to its
   earliest chain root and reporting separate root-cause and dependent counts,
   with remaining generic, derive/macro-generated, and unknown misses named.
