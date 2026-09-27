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

Stratified sample of 70 of the 389 (random seed over the full list, so
distribution should track the full set at the ±10pp level given `n=70`).
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
(the top-level language dispatch, or the `#[tool_router]` boundary), so one
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
  `record_binary_runs`'s helper-following rule is scoped (§4.4: "a bare
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
- **Expected recovery (from sample):** ~23 functions crateside, entirely
  within the "dyn Trait / generic T: Trait" class.

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
X` block for a small, closed set of traits the extractor already
recognizes by name (`Display`, `Debug`, `Ord`, `PartialOrd`, `Hash`,
`PartialEq`, `Eq`, `Default`, `From`/`Into` where the source/target type is
visible), do not try to find call sites — instead emit a direct
`tested_by`-eligible synthetic reachability fact conditioned on the
*type* `X` being constructed and formatted/compared/hashed/cloned anywhere
reachable from a test. Concretely: `emit_std_trait_edge(caller_context, X,
trait_method)` fires when `{}`/`{:?}` is used on an expression whose
static type resolves to `X` (the same typed-receiver machinery §4.4
already uses for method-call resolution), or when `X` participates in a
`.sort()`/`.cmp()`/`HashMap<X, _>` key position under a resolvable type.

- **Precision risk:** low — this only fires where the extractor can already
  name the receiver's type by the same rule it uses for ordinary method
  resolution; it merely maps a formatting/comparison *macro or std-lib
  call* to a trait-impl entry point rather than requiring a literal
  `.fmt(`/`.cmp(` call.
- **Cost:** low. A few new call-site shapes (`format!`, `{}`/`{:?}` in
  `println!`/`write!`, `.sort()`, `.sort_by_key()`) recognized alongside
  the existing method-call grammar.
- **Expected recovery:** ~27 (fmt) + ~2 (cmp/hash family) ≈ 29 functions.
  This is the single best cost/precision ratio in the set.

### 4. serde `Deserialize`/`Serialize`/`Visitor` — do NOT resolve statically, but *can* be special-cased narrowly

Full serde-generic resolution (trait method invoked through
`serde_json::from_str::<T>()`) is the same class as #2 — the dependency
crate's own generic machinery, not project code. However, a narrower,
sound sub-case exists: where the extractor already sees
`serde_json::from_str::<ConcreteType>(...)` or `#[derive(Deserialize)]` on
a named struct/enum whose `Deserialize` impl (derived or hand-written) is
in-crate, it can emit an edge from the call site directly to that type's
`deserialize`/`Visitor::visit_*` methods — the concrete type is named at
the call site, exactly the same evidence bar as a typed-hint method call.
**Recommendation:** implement only the named-concrete-type sub-case;
leave `Deserialize<'de> for T` behind a fully generic `T` unresolved.

- **Precision risk:** low for the named sub-case; the unresolved-generic
  case stays unresolved (correctly).
- **Cost:** moderate — needs to associate a derive/impl with its generated
  or hand-written `Visitor`, which for `#[derive(Deserialize)]` is
  compiler-generated and has no source text to anchor to at all (only the
  hand-written case in `rules_file.rs::SourceRule` is anchorable).
- **Expected recovery:** small — most of this repo's `Deserialize` traffic
  goes through `#[derive(Deserialize)]`, which is compiler-generated code
  the extractor cannot see by definition (tree-sitter parses source, not
  macro output). Estimate 1–2 of the ~35 extrapolated. The rest — mostly
  derive-generated visitors — belong to class 7 below.

### 5. `rmcp` tool-macro dispatch — recoverable, crate-local, high confidence

**Strategy:** recognize `#[tool_router]` on an `impl` block and `#[tool(...)]`
on its methods as a closed, in-crate dispatch table: the macro's dispatch
target set *is* exactly the annotated methods, with no external
implementors to enumerate (unlike class 1). Emit a synthetic entry-point
fact — `graph_definition` for the tool router, `calls` from that
definition to each `#[tool]` method — and connect it to test reachability
via the *actual* call sites already in test code (`server.serve(...)` /
direct method calls in `server.rs`'s own `#[cfg(test)] mod tests`, which
the sample shows exists).

- **Precision risk:** very low. The macro's expansion is closed and
  syntactically declared (the attribute, not a guess); every `#[tool]`
  method genuinely is a dispatch target.
- **Cost:** low — one recognizer for two attribute names, scoped to this
  crate's actual macro usage (no general proc-macro-expansion engine
  needed).
- **Expected recovery:** ~23, concentrated in `server.rs` and
  `server_handlers/`.

### 6. Function pointers / dispatch tables — recoverable, narrow

**Strategy:** when a `match` arm's body is exactly a bare function name (an
fn item, not a call) assigned to a variable, returned, or stored in a
`const`/`static` array/map keyed by a literal (extension string, enum
discriminant), and every array/map entry is a *named, in-crate* free
function or associated function, emit `calls` from the table's usage site
to each entry.

- **Precision risk:** low — the table's entries are syntactically named,
  not inferred. Risk is scope creep if "the table's usage site" isn't
  itself provably reached; the resolver should attach the edge to the
  *lookup call* (`table[key]()`), and let ordinary reachability rules
  decide whether the lookup call itself is reached (no special-casing of
  which key is picked at runtime — every entry gets the edge, same
  disjunctive-truth caveat as class 1).
- **Cost:** moderate — needs a syntax shape for "value position is a bare
  path to a function," which the grammar mostly already has via
  `@method:Type:name` typed-hint resolution; extending it to non-call
  positions is new surface.
- **Expected recovery:** ~17.

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
  `graph/derive.rs`'s `record_binary_runs`.
- **Expected recovery:** ~78 — the largest single number in the sample,
  and it fixes a *root cause* that several downstream chain misses hang
  off of, so the true recovery from fixing this alone is likely higher
  than its own extrapolated count once dependent chain members are
  included.

## Summary table: strategy vs. class vs. recommendation

| # | Class | Static resolution? | Recovered (est.) | Precision risk |
|---|---|---|---|---|
| 1 | `dyn Trait` dispatch, closed impl set | Yes — new `calls_dyn` edge kind | ~23 | medium |
| 2 | Generic `T: Bound` dispatch | No | 0 | n/a |
| 3 | `fmt`/`Ord`/`Hash` std-trait macros | Yes | ~29 | low |
| 4 | serde Deserialize/Visitor (named-concrete-type only) | Partial | ~1–2 | low |
| 5 | `rmcp` `#[tool_router]` macro dispatch | Yes | ~23 | very low |
| 6 | fn-pointer / dispatch tables | Yes | ~17 | low |
| 7 | Derive-generated call sites (no source text at all) | No | 0 | n/a |
| 8 | CLI-subprocess boundary (scoping fix) | Yes | ~78 (+ dependents) | low |

Total confidently recoverable without guessing: roughly **170–190 of the
389** (44–49%), concentrated in classes 3, 5, 6, and 8. The remainder is a
mix of genuinely unresolvable generic/derive dispatch (classes 2 and 7,
correctly left to coverage) and the "unclassified" 31% that likely
decomposes into combinations of the above once traced hop-by-hop — the
phased plan below front-loads class 8 partly *because* fixing chain roots
should shrink that bucket automatically before it needs its own strategy.

## Plan

**Phase 0 — measurement harness.** Before any resolver change, snapshot
`unresolved` and `no_direct_test` counts, and cross them against the
existing coverage store (`SPEC-coverage-evidence.md`) so every subsequent
phase can be checked against dynamic evidence, not just count deltas.

**Phase 1 — class 8 (CLI-subprocess helper scoping).** Smallest, most
self-contained change (one function in `derive.rs`), largest single
expected recovery, and a bug-fix framing (existing rule under-scoped) more
than a new feature. Ship first; re-run the sample classification afterward
to see how much of the "unclassified" 31% collapses.

**Phase 2 — class 3 and class 5 (std-trait macros, `rmcp` macro).** Both
are closed, in-crate, low-risk, and independent of each other — can ship
together. New edge kinds are not needed (these connect to ordinary
`calls`/`tested_by`), which keeps the change footprint inside
`derive.rs`'s existing call-recognition grammar.

**Phase 3 — class 6 (fn-pointer/dispatch tables).** Needs new grammar
surface (bare-path-in-value-position); ship after 1–2 have proven the
measurement harness catches regressions.

**Phase 4 — class 1 (`dyn Trait`, closed impl set).** Introduces the
`calls_dyn` edge kind and the disjunctive-truth question of whether it
should count toward `test_reaches` at all, or only toward a weaker
"reachable-if-this-impl-is-live" fact. This needs its own design decision
(see Open Questions) and should not block phases 1–3.

**Phase 5 — re-run the 389-function sample classification** (or the full
set, now cheaper since phases 1–4 should have shrunk it) to validate the
extrapolations above and decide whether classes 2/4/7's "leave to
coverage" boundary needs revisiting once `SPEC-coverage-evidence.md` is
further along.

## Acceptance criteria

1. **No increase in wrong edges.** Every edge added by phases 1–4 is
   checked against the coverage store where coverage data exists: a new
   static edge `calls(A, B)` (or `tested_by`/`test_reaches`) that coverage
   *never* confirms for any test claiming to reach it is flagged as
   suspect and reported (not silently kept) — mirroring how
   `region_without_dynamic_evidence` already reports the inverse gap. This
   check is the primary defense against a resolver bug reintroducing D4's
   original sin (a coincidental-name match masquerading as evidence).
2. **`unresolved` count moves down**, not just `no_direct_test`. A phase
   that reduces `no_direct_test` by reclassifying calls as `tested_by`
   without reducing the underlying `unresolved`/`ambiguous` counters from
   §4.4 has not actually resolved anything — it would indicate a bug in
   how the new edge kind is counted.
3. **Incremental update equals full rebuild.** Each new recognizer must be
   exercised by the existing two-tier extraction model (§4.5: parse the
   edited file only, derive over the whole graph). A test asserting that a
   single-file edit plus incremental derive produces byte-identical
   results to a from-scratch rebuild is required per phase, following the
   pattern already used for the base extractor.
4. **`calls_dyn` (if phase 4 ships) never silently merges into `calls`.**
   Any rule or query that currently treats `test_reaches` as certain
   coverage must make an explicit choice about whether to join through
   `calls_dyn`, documented at the point of use.

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

## Open questions

1. Should `calls_dyn` (class 1) count toward `test_reaches` at all, or
   should it live as a separate, explicitly-weaker relation that rules opt
   into? The disjunctive-truth property (one of N implementors, not a
   certainty) is a different epistemic status than every other edge in
   the current schema (§2.1's table — all current relations are asserted
   as facts, not "possibly true" facts). This may need a `RULE_PHASES`-style
   explicit-opt-in decision rather than silent inclusion.
2. Is the `record_binary_runs` one-hop extension (class 8) itself
   conservative enough, or does it reopen the same "helper indirection"
   risk §4.4 originally fenced off by restricting to same-module free
   functions? The risk is a test helper that conditionally invokes the
   binary only in some branches, which a syntactic one-hop rule cannot
   distinguish from an unconditional call.
3. For class 6 (dispatch tables), should the edge be attached at the
   table's *definition* site or every *lookup* call site? Attaching at
   definition risks the same "unconditional inclusion" problem as class 1;
   attaching at each lookup is more edges but keeps each one closer to
   actual evidence of use.
4. How much of the 31% "unclassified" bucket is actually class 8's
   dependents versus genuinely new shapes? Phase 1's re-classification
   (Phase 5) is the cheapest way to answer this empirically rather than by
   further manual tracing now.
