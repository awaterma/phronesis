# Implementation decisions: Rust ownership evidence

**Status:** Binding for Phase One implementation
**Governs:** [`SPEC-rust-ownership-evidence.md`](./SPEC-rust-ownership-evidence.md)

The spec leaves several points underspecified in ways that would let two
implementers build incompatible extractors. This document closes each one.
Where a decision amends the spec rather than merely refining it, it is marked
**AMENDS**.

Every decision here is binding: an implementer must not substitute their own
interpretation. Where a case is not covered, the spec's §7.11 rule applies —
emit no edge.

## Grammar baseline

`tree-sitter-rust` 0.24 (workspace pin, `crates/phronesis-mcp/Cargo.toml`).
The following grammar facts are load-bearing and already relied on by
`src/syntax/rust/{counts,hazards}.rs`:

- There is **no** `method_call_expression` node. A method call `recv.m(args)`
  is a `call_expression` whose `function` field is a `field_expression`, whose
  `value` field is the receiver and whose `field` field is a `field_identifier`
  holding the method name.
- `expr.await` is an `await_expression` with field `value` and a literal
  `await` keyword child.
- `let g = ...;` is a `let_declaration` with fields `pattern` and `value`.
- Blocks are `block`. Function bodies are `function_item` with field `body`.
- Bodyless trait methods are `function_signature_item`, a distinct kind, and
  are already skipped by `walk::walk_function_items`.
- UFCS `Clone::clone(&x)` is a `call_expression` whose `function` is a
  `scoped_identifier`.
- **Turbofish wraps the callee.** `x.collect::<Vec<_>>()` parses as
  `call_expression { function: generic_function { function: field_expression } }`.
  Existing code (`counts.rs::count_clone_calls`, `extract.rs::watched_calls`)
  tests `func.kind() == "field_expression"` directly and therefore **silently
  misses every turbofish call**. Since `collect` is nearly always written with a
  turbofish, the ownership extractor **must unwrap `generic_function` first**.
  A helper `fn callee_field_expression(call: Node) -> Option<Node>` that
  unwraps `generic_function` then matches `field_expression` is mandatory, and
  every site kind must go through it.
- **`await_expression` has no named fields.** The awaited expression is an
  unnamed child. Never call `child_by_field_name` on it.
- Compound assignment is `compound_assignment_expr` — abbreviated, not
  `..._expression`.
- `&mut x` and `&x` are both `reference_expression` with only a `value` field;
  mutability is an anonymous `mut` token child.

## D1. Site ID anchor byte — the operation-name token

**Problem.** §5.2 defines a site ID as `<function-id>#ownership:<kind>:<start-byte>`
but does not say which node's start byte. Anchoring on the enclosing expression
is **provably broken**: in the chain `xs.filter(p).cloned()`, the outer
`call_expression` for `cloned` and the inner one for `filter` both start at the
byte of `xs`. Two sites in one chain would collide on a single ID.

**Decision.** The anchor is the start byte of the token that *names the
operation*:

| Site kind | Anchor node |
|---|---|
| clone / filter / mutation-method / lock, method form | the `field_identifier` (method name) inside the callee's `field_expression` |
| clone, UFCS form (`Clone::clone(..)`) | the `scoped_identifier` callee node |
| await | the literal `await` keyword child of `await_expression` |
| mutation by assignment | the operator token (`=`, `+=`, …) of the `assignment_expression` / `compound_assignment_expr` |

`ownership_site_span(site, file, start, end)` is **separate and unaffected**: it
records the full span of the anchoring *expression* (the whole
`call_expression`, `await_expression`, or assignment), because that is what a
human needs to see. Only the ID uses the operation-token offset.

Offsets are `Node::start_byte()` / `Node::end_byte()` — already UTF-8 byte
offsets. Never convert to character offsets (§7.4).

## D2. `filter_before_clone` — receiver-chain reachability

**Problem.** §6.2 says the clone "directly wraps a filter in the same
expression chain". Undefined for `.filter(p).map(f).cloned()` and for
non-method forms.

**Decision.** From the clone site's `call_expression`, walk the **receiver
chain**: take the callee via `callee_field_expression` (which unwraps
`generic_function`) → `value`, then repeatedly descend through
receiver-position links, unwrapping `call_expression` (via
`callee_field_expression`→`value`), `field_expression` (via `value`),
`try_expression` (via `value`), `await_expression` (via its **unnamed first
child** — it has no named fields), `reference_expression` (via `value`), and
`parenthesized_expression` (via its inner child). Emit `filter_before_clone` if **any**
`call_expression` in that chain has method name `filter` or `filter_map`.

Consequences, all intended:

- `.filter(p).map(f).cloned()` — **emits**. Intervening adapters do not break
  the chain; they are still one expression chain.
- `let ys = xs.filter(p); let zs = ys.cloned();` — **no edge**. Different
  expression chains; the receiver chain of `cloned` terminates at the
  identifier `ys`. This is the §6.2 requirement that mere line ordering is
  insufficient, and the adversarial "unrelated filter and clone on adjacent
  lines" fixture.
- `Iterator::filter(xs, p)` UFCS — **no edge**. Out of scope for Phase One;
  recorded as a known incompleteness, not as absence of a filter.

The closed filter-method set is exactly `{filter, filter_map}`. `take_while` /
`skip_while` are **not** included — adding them is a graph-format change.

## D3. `mutation_site` method list vs. §7.10

**Problem.** §6.1 permits "a known mutable-borrow method"; §7.10 forbids
inferring receiver types from spelling. These read as contradictory.

**Decision.** They are not in conflict, and the resolution is binding:
matching a **method name** against a closed list is an observation of the name
that was written, not an inference about the receiver's type. The emitted
`mutation_site(site, operation, place)` records `operation` as the literal
observed name and asserts nothing about what the receiver is. Nothing in the
extractor may map a name to a type.

Closed method list (Phase One, exact match on the `field_identifier`):

```text
get_mut  iter_mut  values_mut  as_mut  borrow_mut  entry
push  push_str  insert  remove  clear  extend  retain  truncate  drain
```

Plus the assignment forms: `assignment_expression` and
`compound_assignment_expr` whose `left` field is a `field_expression` or
`index_expression` (i.e. assignment *through a projection*, per §6.1). A plain
`x = 1` to a bare local is **not** a mutation site.

`lock` / `read` / `write` are excluded here — they are `sync_lock_site`.

## D4. Root place resolution

**Problem.** §6.2's `read_before_mutation` requires "the same syntactically
identified root place", undefined for `self.party.members[i].pos`.

**Decision.** The root place is found by descending to the **base** of the
place expression: repeatedly take `field_expression`→`value`,
`index_expression`→first child, `unary_expression`→operand when the operator is
`*`, `reference_expression`→`value`, `try_expression`→`value`, and
`parenthesized_expression`→inner. Stop at the first node that is an
`identifier` or `self`; its source text is the root place.

If the descent terminates at anything else — a `call_expression`, a macro, a
literal — there is **no root place and no edge** (§7.11). So
`self.party.members[i].pos` has root place `self`, and
`lookup(id).field = v` has none.

Two sites relate only when their root-place strings are byte-equal. This is
deliberately coarse: it over-groups everything hanging off `self`. That is a
named false-positive class (see D8), not a defect to fix with aliasing
analysis, which the spec forbids.

## D5. `collect` and the "ownership-producing" qualifier

**Problem.** §6.1 says clone sites include "ownership-producing `collect`".
Whether a `collect` produces ownership depends on the target type, which the
AST provider cannot know. Either reading violates something.

**Decision.** Emit `clone_site(site, "collect", operand)` for **every**
`collect` call. The "ownership-producing" qualifier is a `type_resolved`-level
claim, not an `ast`-level one, and the evidence level already carries that:
the site gets `ownership_evidence(site, ast, tree_sitter_rust)` and nothing
more until a compiler provider confirms it.

This is required by the acceptance corpus — `check_secrets_on_arrival` demands
clone/collect sites — and it is honest: the graph says "a collect was observed
here", never "an allocation happened here". The CLI rendering must not describe
a bare `collect` site as producing ownership.

## D6. `lock_scope_ends_before_await` and explicit `drop`

**Problem.** §6.2 defines the relation lexically ("narrowest lexical block
containing the bound guard ends before the await"), but §12 demands an
adversarial fixture for `drop(guard)` before await, which is semantic. As
written the fixture cannot pass.

**Decision.** Emit `lock_scope_ends_before_await(function, lock_site, await_site)`
when **any** of the following holds:

1. **Lexical.** The narrowest enclosing `block` of the guard's
   `let_declaration` has `end_byte` < the await anchor's `start_byte`.
2. **Explicit drop.** A `call_expression` whose callee is the bare identifier
   `drop`, with a single argument that is a bare `identifier` byte-equal to the
   guard binding, appears at a start byte between the guard's `let_declaration`
   end and the await anchor's start, within the same enclosing block.

Case 2 assumes `drop` resolves to the prelude function. A locally shadowed
`drop` would make this wrong; that is a named false-positive class (D8), and it
is acceptable because the relation is query-only.

Note what is *not* affected: `lock_scope_may_cross_await` is never emitted by
this code path under any circumstances (§6.2, §13.1). The AST extractor must
have no code path that can produce it, and a unit test must assert that.

3. **Unbound temporary.** *(Added 2026-08-15, precision round.)* The lock site
   has no guard binding. Walk up from the lock site's anchor to the nearest
   ancestor node whose kind is `expression_statement` or `let_declaration`;
   that node's `end_byte` is the guard's release point, because Rust extends a
   temporary's life to the end of the enclosing statement. Emit when that end
   byte is less than the await anchor's `start_byte`.

   The **statement** is the boundary, never the innermost expression. In
   `match m.lock() { .. }` written as a statement, the temporary scrutinee
   lives past the whole `match` expression — so an await inside a match arm is
   still covered by the guard, and taking the scrutinee's own end byte would
   emit a false "dropped early" claim, exactly the over-claim §6.2 forbids.
   The same holds for an `if let` scrutinee.

   When no ancestor `expression_statement` or `let_declaration` exists (a tail
   expression with no semicolon), no boundary can be established and §7.11
   requires emitting nothing. Do not guess.

An **unbound** temporary guard (`m.lock().field`) still produces a
`sync_lock_site` with `guard` recorded as the empty string — no name is
invented — but under case 3 it is no longer barred from a scope conclusion.

**Superseded 2026-08-15.** This decision originally read: "An unbound temporary
guard produces a `sync_lock_site` … and **no** scope relation at all (§7.9)."
A precision field test over an external corpus reviewed nine functions that a
naive "lock + await with no safe edge ⇒ hazard" reading would flag and found
**zero** real hazards; the unbound temporary was the one class where the
extractor was silent about a drop point it could actually establish. The
absence of the binding was treated as absence of a scope, which it is not.

## D7. Operand text normalization and the digest marker

**Problem.** §7.7 caps operand text at 240 bytes, says to normalize internal
whitespace and store "a stable digest marker", specifying none of it.

**Decision.**

1. Take the node's UTF-8 source text.
2. Replace every maximal run of ASCII whitespace (space, tab, CR, LF) with a
   single space; trim leading and trailing space.
3. If the result is ≤ 240 bytes, that is the operand value.
4. Otherwise the operand value is `sha256:` followed by the first 16 lowercase
   hex characters of the SHA-256 of the **normalized** string from step 2.

Never truncate into a partial expression — a truncated expression reads as a
real but wrong expression, which is worse than an opaque marker. Step 3's
boundary is on bytes, not characters; a multi-byte character straddling the
cap pushes the value into the digest branch, which is correct and
deterministic.

## D8. Named false-positive classes

§11 requires named false-positive classes before any relation could become
audit-eligible. Phase One must record these in user-facing docs:

- `read_before_mutation` over-groups every place rooted at `self` (D4).
- `lock_scope_ends_before_await` is wrong under a shadowed `drop` (D6).
- **The absence of `lock_scope_ends_before_await` is not evidence of a hazard.**
  This is the single most important user-facing lesson from the precision
  field test, where a reviewer found zero real hazards among nine functions a
  naive reading would have flagged. Two shapes produce no scope conclusion
  while being perfectly safe: an await that lexically *precedes* the lock, and
  an await at a loop head with the guard block-scoped later in the loop body
  (the extractor has no back-edge reasoning). Emitting
  `lock_scope_ends_before_await` for a lock acquired after the await would make
  the relation's own name a false statement about the code, so the extractor
  emits nothing — correct but incomplete.
- An unbound temporary guard's drop point is its enclosing statement (D6
  case 3), not the innermost expression. Taking the expression would be a
  false-positive source in exactly the `match`/`if let` scrutinee shapes where
  Rust extends the temporary past it; the implementation and its fixture both
  pin the statement boundary.
- `filter_before_clone` misses UFCS iterator calls (D2) — an absence, not a
  claim of cleanliness.
- `clone_site` with operation `collect` does not establish that ownership was
  produced (D5).
- Every lexical-ordering relation (`clone_before_await`, `read_before_mutation`)
  is ordering evidence only, and says nothing about reachability: an early
  `return` between the two sites is invisible to it.
- Declarative-macro-generated calls are observed only where the macro
  invocation's arguments happen to parse as expressions; expansion is not
  analyzed.

## D9. **AMENDS** — `stale` is an allowed analysis status

§6.1's status enum is `available | partial | unavailable | failed`, but §9
requires incremental edits to "mark compiler evidence stale", and Addendum A.4
requires that "partial, **stale**, failed, and unavailable analysis is visible
in CLI and MCP output". There is no way to represent it.

**Decision.** The allowed status set is amended to:

```text
available | partial | unavailable | failed | stale
```

An incremental single-file update emits
`ownership_analysis_status(<file>, type_inference, stale, incremental_edit)`
and the same for `mir_lowering`, replacing any prior compiler status for that
file, while AST edges for the file are re-extracted normally.

## D10. Scope boundary for Phase One

Binding scope decisions for this implementation round:

- **External-corpus field testing (§12, §14.10) is NOT performed.** The sibling
  checkout is owned by a concurrent session and must not be read, built, or
  written by this work. Acceptance uses minimized repository-local fixtures
  derived from the five documented case shapes, plus dogfooding on Phronesis
  itself. The unrun field test is reported as an open item against §15, not
  quietly marked done.
- **The rust-analyzer provider ships interface-plus-availability only.** §8.2
  permits exactly this when no stable structured interface is used. It emits
  `ownership_analysis_status` and nothing else. It never emits `resolved_type`,
  `ownership_transfer`, `borrow_live_across`, or any MIR relation in this round.
- **No rule, no audit participation, no catalogue entry, no pack change**
  (§11, §14.12). A test must assert that `init --packs rust` installs no
  ownership rule.
- **Addendum A.2's generic evidence vocabulary is NOT built.** It is
  explicitly deferred by the addendum itself.

## D11. Relationship to the in-flight `syntax/rust/hazards.rs`

`hazards.rs` (uncommitted on `main` at the time of writing) contains
`extract_sync_lock_guards_across_await`, which detects a similar shape via
substring matching on `let` initializer text and reports guards crossing
`.await`.

These do **not** merge and must not be refactored into each other in this
round:

- `hazards.rs` feeds `SyntaxFacts` — per-file, hook-time, heuristic, and it
  deliberately makes a crossing *claim* to drive a warning.
- The ownership extractor feeds the **graph**, is repository-wide, and is
  forbidden from making that crossing claim from AST (§6.2).

The ownership extractor must be new code in the graph layer. Do not modify
`hazards.rs`, and do not import its helpers.

Note also that `hazards.rs` matches on **substring text**
(`value_text.contains(".lock()")`, a file-level `source.contains("std::sync")`).
The ownership extractor is forbidden from doing that by §7.5 and must match on
node structure, so the "comments and strings containing `.clone()`" adversarial
fixture passes for ownership even though `hazards.rs` would fail it.

---

# Decisions forced by the codebase

The following were discovered by reading the implementation. Each contradicts
a reasonable reading of the spec, so they are binding.

## D12. The §6.2 relations belong to the base tier, not the derived tier

**Problem.** The spec groups `filter_before_clone`, `clone_before_await`,
`read_before_mutation`, and `lock_scope_ends_before_await` under "derived
structural relations". The codebase also has a literal `Edge::derived` with
`d: true`, so the obvious reading is that these should use it. They should not,
and the reason is a contract rather than a hazard.

**What `d` actually means here.** It is a *storage tier*, not a semantic
label. `store::compact` documents it directly — derived edges are dropped
"because they are regenerated by the derivation pass over the resulting base
set" — and `derive::derive_all(base: &[Edge]) -> Vec<Edge>` is a pure function
of the edge set, with no syntax tree and no I/O. So `d: true` means precisely
**"regenerable by `derive_all` from base edges alone."**

All four ownership relations need the syntax tree: a shared expression chain,
a root place, a narrowest enclosing block. None can be recomputed from the edge
set, so none qualifies for that tier.

**Decision.** Every ownership relation is emitted as
`Edge::base(relation, args, <repo-relative source file>)` with `d: false` and
`src` set to the file path. They are primary observations of one source file,
keyed to that file — which is what a base edge is.

Three things follow from being in the right tier:

1. `src` is the only compaction key, so replacing a file removes its stale
   ownership sites for free — exactly what §13.1 and §15 require.
2. Provenance stays `graph:<file>` rather than the `graph:structural` that
   `Edge::to_fact` stamps on `src: ""` edges, which Addendum A.4 requires.
3. Nothing is added to `derive.rs`. An implementer who "helpfully" adds an
   ownership case to `derive_all` has put a tree-dependent computation into a
   pure edge-set pass, and it cannot work.

An ownership edge with an empty `src` is unreachable by compaction and becomes
permanently stale. A test must assert every emitted ownership edge has a
non-empty `src`.

Note for anyone adding a relation later: `compact` deliberately drops any
`d: true` edge supplied as *fresh*, and does so without an error — see the test
`compaction_refuses_to_persist_derived_edges_supplied_as_fresh`. That guard is
intentional, but it is quiet, so a relation put in the wrong tier fails by
disappearing after the next incremental save rather than by complaining.

## D13. Extraction happens inside `extract_rust_at_module`, using the live `Scope`

**Problem.** §5.1 and §7.3 tell the extractor to "consume the package/module
index used by the Rust graph extractor". **No such reusable index exists.**
`UnitMap` indexes packages only; module paths are recomputed per file by
`module_path()`, and the function-name suffix comes from a live mutable `Scope`
maintained during the walk. Worse, `#[path]` inclusions mean `self_module` is
**not** always `module_path(file_path, unit)`.

**Decision.** Ownership extraction is a hook **inside** the existing
`extract_rust_at_module` walk, at the point where a function body is visited
and its canonical ID is already known. It obtains that ID from `Scope::qualify`
— the same call that produces `defines_fn` — and never reconstructs an ID.

This is not negotiable: reconstructing IDs independently is the "second
function identity scheme" §5.1 forbids, and it will diverge on generic impls
(`impl Foo<Bar>` yields the literal segment `Foo<Bar>`; generics are not
normalized) and on `#[path]`-included modules.

Do not add a third parse of the file. Every file is already parsed twice per
rebuild (once in `sync::rust_path_inclusions` scanning for `#[path]`, once in
`extract_rust_at_module`); §7.1 requires reusing the tree already in hand.

## D14. Test functions emit no ownership sites

`visit_function` takes an early return for test functions **before**
`defines_fn` is emitted, so a `#[test]` function has a `defines_test` identity
but no `defines_fn`. §15 requires every ownership site to resolve to a real
graph function.

**Decision.** Ownership sites are not emitted inside `#[test]` or
`#[cfg(test)]` bodies, reusing the existing `is_test_attribute` /
`cfg_asserts_test` gate. This also keeps fixture-heavy test modules from
dominating edge volume.

## D15. Config parsing — a new section-aware scanner, no new dependency

**Problem.** §9 specifies a `[ownership.rust]` TOML table with a bool, a
string, two string arrays, and an integer. **There is no TOML parser.** The
`toml` crate is not a dependency (`serde_norway` is YAML). `.phronesis/graph.toml`
is read by a 30-line hand-rolled line scanner in `data_contracts.rs` that
understands only `[[generated_artifacts]]` headers and quoted scalars.

That scanner is also **actively unsafe** for this change: it is section-unaware,
so any `key = value` line after a `[[generated_artifacts]]` block is absorbed
into that block regardless of what header intervenes. Adding `[ownership.rust]`
to a file that already has generated-artifact bindings would silently corrupt
them.

**Decision.** Write a new section-aware scanner in a new module
(`src/graph/ownership/config.rs`) supporting exactly: section headers, `bool`,
quoted string, integer, and array-of-quoted-strings. Do **not** add the `toml`
crate — a new dependency needs the user's approval and this parser is small.

In the same change, make `load_bindings` section-aware so that an unrecognized
header terminates the current binding instead of absorbing subsequent keys. Add
a regression test with `[ownership.rust]` following `[[generated_artifacts]]`.

Missing file, missing section, or `enabled = false` all mean **disabled**, and
disabled must emit zero ownership edges (§13.2).

## D16. `**` globbing is not available in the graph layer

`graph::query::glob_matches` supports only single-level `*`/`?` and would
mishandle `src/**/*.rs`. A `**`-capable matcher exists only as a private
`glob_match` in `src/journey/tagger.rs`.

**Decision.** Promote that matcher to a shared location (or reimplement it in
the ownership config module) and use it for `include`/`exclude`. Include and
exclude **filter the output of `sync::tracked_files`**; they never perform an
independent directory walk. An independent walk would index files the freshness
check can never match, producing permanent un-self-healing drift.

## D17. Do not let enabling ownership turn every save into a full rebuild

**Problem.** Today, if `.phronesis/graph.toml` merely **exists**, the
`data_contract_input` clause in `sync.rs` makes every save of a `.rs`/`.json`/
`.yaml` file trigger a full `rebuild(root)`. Since enabling ownership means
creating that file, turning the feature on would silently convert every Rust
save into a whole-repo rebuild — blowing the hook latency budget and
contradicting §9's incremental requirement.

**Decision.** Narrow that clause so a full rebuild is triggered by an edit
**to** `.phronesis/graph.toml`, not by the mere existence of the file during an
edit to some other input. Config changes still force a full rebuild (§9), which
is what the existing freshness hashing of `graph.toml` already provides. Add a
test pinning that a `.rs` save with `graph.toml` present takes the incremental
path.

## D18. Registration checklist

Mechanical, and each item silently fails if skipped:

- Add every ownership relation name to `hydrate::GRAPH_RELATIONS`. A relation
  absent from that list is persisted and queryable but **never hydrates into
  RETE** — which would break Goal 7. Adding names costs nothing while no rule
  mentions them, because `needed_relations` short-circuits.
- Add every ownership relation name to `QUERY_ONLY_RELATIONS`, which is what
  actually keeps them out of audit output (§11). Note this suppresses audit
  only; it does not prevent a project-authored rule from firing, and Addendum A
  anticipates such rules, so that is correct behavior, not a gap.
- Bump `GRAPH_FORMAT` (currently 17 → 18) and **fix its doc comment**, which
  still narrates format 5. Expect `graph status` text and any test pinning
  format 17 to need updating.
- Arguments are all `String`; byte offsets are decimal strings. There is no
  numeric argument type anywhere in the graph or engine.
- `Edge::fact_id` joins arguments with `U+001F`. Normalized operand text must
  never contain that byte.
- There is no arity validation anywhere — a wrong-arity edge produces no error,
  just a pattern that never matches. §13.1's arity tests must be written from
  scratch and are the only thing standing between a typo and silent nonsense.

## D19. `AST_EMITTABLE` — structural enforcement, not discipline

§13.1 requires that "AST extraction never emits MIR-only relations". Nothing in
the code prevents it, and the derivation code sits directly beside the AST code,
so a copy-paste is all it takes to promote AST evidence to a compiler claim —
the single worst failure this feature can have.

**Decision.** Define `pub const AST_EMITTABLE: &[&str]` listing exactly the
relations the AST provider may emit. Then:

1. The ownership emit helper asserts the relation is in `AST_EMITTABLE` before
   emitting.
2. A test runs the extractor over **every** fixture in the corpus and asserts
   the union of emitted relation names is a subset of `AST_EMITTABLE`.

`lock_scope_may_cross_await`, `ownership_transfer`, `borrow_live_across`,
`ownership_conflict_diagnostic`, and `resolved_type` are excluded from
`AST_EMITTABLE`. That test is what actually satisfies §13.1; a comment is not.

## D20. `sync_lock_site` excludes `.lock().await` structurally

`hazards.rs` distinguishes std from tokio locks by `source.contains("std::sync")`
and matches `.lock()` by substring — both forbidden here (§7.5 bans regex and
substring matching; §7.10 bans inferring from spelling).

**Decision.** Emit `sync_lock_site` only when the lock acquisition is **not** the
direct operand of an `await_expression`. An awaited `.lock().await` is an async
lock and produces no `sync_lock_site`. This is a structural test on the tree, not
a judgement about the receiver's type, so it satisfies both §7.5 and §7.10.

Add fixtures asserting that `.lock().await` produces no `sync_lock_site`, and
that a `.lock()` appearing only inside a comment or string produces nothing.

## D22. `macro_rules!` definition bodies emit nothing

Raised by the fixture author: does the extractor walk a `macro_rules!` body as
if it were a function body?

**Decision.** No. A `macro_rules!` definition is a `macro_definition` node, not
a `function_item`, so the existing walk does not descend into it as a function.
More fundamentally, a site inside a macro *definition* has no enclosing
function and therefore no canonical function ID, so §7.11 requires emitting
nothing.

At a macro *invocation*, sites are observed only where the invocation's
arguments happen to parse as ordinary expressions. Expansion is never analyzed.
This is already listed as a named incompleteness in D8.

## D21. Function-ID guard test

A site ID that embeds a function ID diverging from `defines_fn` is unresolvable,
violating §15, and the divergence sources are all silent: generic impls
(`impl Foo<Bar>` keeps the literal segment), trait default methods (no trait
segment), `#[path]` module overrides, and non-`src/` targets.

**Decision.** A mandatory guard test asserts **set equality** between the
function IDs appearing in `ownership_site_in_function` and the IDs emitted as
`defines_fn`, over a fixture exercising all four divergence sources. This is the
only cheap way to catch D13 being violated.
