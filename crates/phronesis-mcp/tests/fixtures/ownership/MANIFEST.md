# Ownership Evidence Fixture Corpus — MANIFEST

Every entry below is executed by `tests/ownership_corpus.rs`. Entries marked
**Corrected** were written before the corpus was ever run and stated an
expectation the decisions document does not support; each says what was wrong
and why the code, not the test, is right.

## filter_before_clone.rs

**Shape:** A function containing an iterator chain where `.filter(..)` appears in the receiver chain of a `.cloned()` call, plus a second function where `.filter(p).map(f).cloned()` has an intervening adapter.

**Expect:** `filter_before_clone` linking the `filter` site and the `cloned` site in `fn chained` (intervening `.map`), `fn chained_with_intervening` (intervening `.map`), and `fn chained_direct` (no intervening adapter). Per D2, intervening adapters (`map`) do not break the chain; the direct case verifies the same relation without an adapter.

**Must NOT produce:** No relation between the three functions (they are separate chains).

**Corrected 2026-08-14 (second corpus run):** this entry previously also said
"no `filter_before_clone` from the `collect` call". That is unsatisfiable and
wrong. D5 makes *every* `collect` a `clone_site` unconditionally, and each
function's trailing `.collect()` has the same `filter` in its receiver chain,
so it correctly relates too. Actual output is six relations — per function, the
filter related to both the `.cloned()` and the `.collect()` clone site.

## clone_before_await.rs

**Shape:** An `async fn` that performs a `.clone()` and later has a `.await` in the same body.

**Expect:** `clone_before_await` linking the clone site and the await site in `fn clone_then_await`.

**Must NOT produce:** No `clone_before_filter` (that relation is intentionally absent from the spec).

## read_before_mutation.rs

**Shape:** A function that reads/snapshots something rooted at `self`, then mutates through the same root place (reads `self.party.members`, then calls `self.party.members.get_mut(0)`).

**Expect:** `read_before_mutation` linking the clone (read) site and the `get_mut` (mutation) site in `fn snapshot_and_mutate`. Per D4, the root place of `self.party.members[0].pos` and `self.party.members.get_mut(0)` is `self`.

**Must NOT produce:** No relation if the root places differ (not applicable here since they share `self`).

## clone_then_await_field.rs

**Shape:** An `async fn` that clones a struct field and later awaits.

**Expect:** `clone_before_await` linking the clone site and the await site in `fn clone_field_then_await`.

**Must NOT produce:** Nothing unexpected beyond the single ordering relation.

## lock_scope_ends_before_await.rs

**Shape:** An `async fn` with two synchronous lock acquisitions bound to named guards, where the enclosing block of each guard ends *before* a later `.await`.

**Expect:** `lock_scope_ends_before_await` linking `_g1`'s lock site and the await in `fn lock_scope_ends_before_await`, and similarly for `_g2`. The narrowest enclosing `block` of each `let` declaration ends before the await.

**Must NOT produce:** No `lock_scope_may_cross_await` from AST alone (per §6.2).

## clone_before_filter.rs

**Shape:** The reverse order — `.cloned()` appears *before* `.filter(..)` in the chain. There must be no filter in the receiver chain of the clone.

**Expect:** A clone site and a filter site, and **no** `filter_before_clone` naming the `.cloned()` site. The cloned chain wraps `data` directly; filter is a separate statement.

**Must NOT produce:** No `filter_before_clone` whose clone end is the `.cloned()` site — the filter is in a different statement chain.

**Corrected 2026-08-14 when the corpus was first executed:** this entry
previously said "no `filter_before_clone` relation" at all, which the fixture
cannot satisfy. Its filter statement is spelled `data.iter().filter(..)
.collect()`, and D5 makes *every* `collect` a clone site unconditionally, so
that statement is a genuine filter-in-the-receiver-chain of a clone and
correctly produces the relation. The adversarial claim is about the `.cloned()`
in the other statement, and that is what `tests/ownership_corpus.rs`
asserts. Same correction applies to `unrelated_filter_and_clone.rs` below.

## unrelated_filter_and_clone.rs

**Shape:** A `.filter(..)` statement and a `.clone()` statement on adjacent lines but in separate `let` bindings, so they are in different expression chains.

**Expect:** A filter site and a clone site, and **no** `filter_before_clone` naming the `xs.clone()` site. Per D2, the receiver chain of that clone terminates at the identifier `xs`, not reaching the filter.

**Must NOT produce:** No `filter_before_clone` whose clone end is the `xs.clone()` site — the filter and that clone are in different expression chains despite being on adjacent lines. (The file's own `filter(..).collect()` statement does produce the relation, per the D5 note above.)

## scalar_and_aggregate_clone.rs

**Shape:** Two clones with syntactically identical AST shape — one of a small scalar (`i32`) and one of a large collection (`Vec<u8>`). Syntax cannot tell them apart.

**Expect:** Two `clone_site` observations with `operation` = `clone` but different operands (one `small`, one `big`). The extractor must record both without conflating them.

**Must NOT produce:** No evidence about allocation size from syntax alone. No relation derived from operand *size* (only from syntax).

## explicit_drop_before_await.rs

**Shape:** A lock guard explicitly `drop(guard)`-ed before a later `.await`, in the same block.

**Expect:** `lock_scope_ends_before_await` linking the lock site and the await via D6 case 2 (explicit drop path). The `call_expression` for `drop(g)` appears between the guard's `let` and the await, within the same enclosing block.

**Must NOT produce:** No scope crossing claim from AST containment alone.

## guard_live_across_await.rs

**Shape:** A guard genuinely still in scope across an `.await` (the function body is a single block).

**Expect:** A lock site and an await site, but **no** `lock_scope_ends_before_await` scope relation (the guard's block encompasses the await). The extractor emits no scope relation — it simply does not emit the "ends before" link.

**Must NOT produce:** No `lock_scope_ends_before_await` — the guard is live across the await. No `lock_scope_may_cross_await` from AST.

## unbound_temporary_guard.rs

**Shape:** Four locks acquired without binding them to a name: one used inline in a non-async function, one in a statement that closes before a later `.await`, one acquired *after* the await, and one as a `match` scrutinee whose await sits inside the match.

**Expect:** Four `sync_lock_site` observations, all with `guard` recorded as the empty string — no name is invented. Exactly **one** `lock_scope_ends_before_await`, in `fn unbound_temporary_released_before_await`. Per D6 case 3, an unbound temporary has no binding but does have a drop point: Rust releases it at the end of the enclosing statement (the nearest ancestor `expression_statement` or `let_declaration`), so the relation follows when that statement's `end_byte` precedes the await anchor's `start_byte`.

**Must NOT produce:** No guard name for any of the four. No scope relation from `fn unbound_temporary_acquired_after_await` — a lock acquired after the await ends nothing before it. **No scope relation from `fn unbound_temporary_scrutinee_lives_across_await`** — a temporary `match` scrutinee is live across the whole match expression, so the await inside the match is still covered by the guard. This is the boundary control: an implementation that took the innermost expression rather than the enclosing statement would emit a false "dropped early" claim here. No `lock_scope_may_cross_await` from AST.

**Amended 2026-08-15 (precision round):** this entry previously said the file
produced "**no** scope relation at all". That was the behaviour before D6 case 3
existed — an unbound temporary was skipped outright, so a lock that demonstrably
ends before the await was indistinguishable from one that crosses it. The
fixture was widened from one function to four to cover the new conclusion and,
just as importantly, its two negatives.

## control_flow_boundaries.rs

**Shape:** Clone and await sites separated by various control-flow boundaries: early return, match, loop, closure, and nested async block. One function per shape.

**Expect:** `clone_before_await` in each function regardless of control-flow shape (per D8, these are ordering evidence only). The extractor sees lexical order, not reachability.

**Must NOT produce:** No conclusion about reachability — an early `return` between the two sites is invisible to the ordering relation.

**Corrected 2026-08-14 (second corpus run):** this entry said "five ordering
relations expected (one per function)". Actual output is **six**.
`control_flow_nested_async` contains two awaits — the one inside the `async`
block and the one applied to the block itself — and the single clone lexically
precedes both, so it relates to each. `clone_before_await` pairs every earlier
clone with every later await; it is not one edge per function.

## macro_generated_calls.rs

**Shape:** A declarative macro (`macro_rules!`) whose expansion contains `.clone()`, plus an invocation of it.

**Expect:** **Zero** ownership sites, and the enclosing function still walked (it appears as a `defines_fn`). The only `.clone()` in the file is inside the `macro_rules!` body, which D22 skips, and the invocation's single argument (`data`) is a bare identifier that contains no operation. This is an absence of evidence, which is allowed; it is not a claim that the function is clone-free.

**Must NOT produce:** No expansion-level analysis. No site attributed to the invocation.

**Corrected 2026-08-14 (second corpus run):** the previous entry was marked
UNCERTAIN and speculated that "the `macro_rules!` body itself is a function
body, so the `.clone()` inside may be observed". It is not — `collect_sites`
skips `macro_definition` children outright (D22), and a site there would have no
canonical function id anyway (§7.11). Resolved to zero sites.

## comments_and_strings.rs

**Shape:** The important negative control. Contains `.clone()`, `.lock()`, and `.await` inside: a line comment, a block comment, a doc comment, a normal string literal, and a raw string literal (`r#"..."#`).

**Expect:** **Zero** ownership sites. Every occurrence is in a comment or string literal. Per §7.5, the extractor must strip comments and strings structurally by visiting syntax nodes.

**Must NOT produce:** No sites at all — every occurrence is in a comment or string.

## turbofish_collect.rs

**Shape:** `collect` calls written with a turbofish (`.collect::<Vec<_>>()`), with a typed binding, and with a filter-before-clone chain ending in turbofish collect.

**Expect:** Three `clone_site` observations for `collect` (per D5, every collect is a clone_site with operation `collect`). The turbofish calls parse as `generic_function { function: field_expression }` wrapping the callee — per D15's grammar note, the extractor must unwrap `generic_function` first. The third function exercises `filter_before_clone` through the turbofish path: `.filter(p).cloned().collect::<Vec<_>>()` should emit `filter_before_clone` because the clone wraps the filter in the same chain, and the collect is further downstream.

**Must NOT produce:** No missed turbofish collect sites (a naive implementation missing `generic_function` would miss them).

## ufcs_clone.rs

**Shape:** `Clone::clone(&x)` and `<T as Clone>::clone(&x)` UFCS forms, plus `Iterator::filter(xs, p)` UFCS form.

**Expect:** **One** `clone_site`, for the plain `Clone::clone(&data)` form, with `operation` = `Clone::clone` and `operand` = `&data`. Per D2, `Iterator::filter(xs, p)` UFCS produces no filter site and therefore **no** `filter_before_clone` edge (out of scope for Phase One; recorded as a known incompleteness).

**Must NOT produce:** No `filter_before_clone` from the UFCS filter. No filter site from the UFCS call (known incompleteness).

**Corrected 2026-08-14 (second corpus run) — open incompleteness:** this entry
expected **two** clone sites, including one for `<Vec<i32> as Clone>::clone(&data)`.
The extractor produces only one. D1's grammar row covers the plain spelling,
whose `scoped_identifier` callee has `path` = the bare identifier `Clone`; the
qualified form's `path` is a `bracketed_type` (`<Vec<i32> as Clone>`), which
fails the `path.rsplit("::").next() == Some("Clone")` test in
`classify_call`, so §7.11's "emit no edge" applies.

Nothing false is emitted, so this is an incompleteness rather than a defect, and
it is deliberately **not** fixed in this round: widening what the extractor
observes changes graph output and belongs in a decision, alongside D2's parallel
ruling that UFCS iterator calls are out of scope for Phase One. The behaviour is
pinned by `the_ufcs_clone_call_is_a_site_while_the_qualified_and_filter_forms_are_not`,
which will fail the day the qualified form is handled — forcing this entry and
the decisions document to be updated with it.

## bodyless_trait_items.rs

**Shape:** A `trait` with bodyless method signatures (no bodies) alongside one default-bodied method that contains a `.clone()`.

**Expect:** **Two** clone-family sites, both in `fn default_method`: the `.to_string()` and the `.clone()`. Bodyless trait methods (`fn bodyless_method`) must emit nothing — they are `function_signature_item` nodes (per D1 grammar baseline). The `Impl::bodyless_method` is a regular function, but it has no clone call.

**Must NOT produce:** No sites in bodyless trait signatures. No sites in `Impl::bodyless_method` (it has no clone call).

**Corrected 2026-08-14 (second corpus run):** this entry said "one clone site".
`.to_string()` is in §6.1's clone-family list exactly as `.clone()` is, and
§7.6/D5 require the two to stay distinct rather than be collapsed, so the
default body produces two sites with operations `to_string` and `clone`.

## clone_operation_kinds.rs

**Shape:** One function exercising each distinct clone operation kind separately: `.clone()`, `.cloned()`, `.to_owned()`, `.to_string()`, and `.collect()`.

**Expect:** **Seven** `clone_site` observations across the five functions, covering all five distinct operations with `operation` preserving the distinction: `clone`, `cloned`, `to_owned`, `to_string`, `collect`. Per §7.6 and D5, these are not interchangeable.

**Must NOT produce:** No conflation of operation kinds.

**Corrected 2026-08-14 (second corpus run):** this entry said "five, one per
function… each function has exactly one clone operation". Two of the functions
(`cloned_method` and `collect_method`) are both written `data.iter().cloned()
.collect()`, which is two clone-family calls in one chain, so the file produces
seven sites. The five *operations* are all present, which is what the fixture
is for.

## mutation_kinds.rs

**Shape:** One example of each mutation form: `get_mut(..)`, `iter_mut()`, assignment through a field projection (`self.x.y = v`), assignment through an index projection (`v[i] = w`), compound assignment through a projection (`self.n += 1`), and a plain assignment to a bare local.

**Expect:** **Four** mutation sites: `get_mut` in `fn mutation_get_mut`, `iter_mut` in `fn mutation_iter_mut`, field projection assignment (`=`, place `h.x`) in `fn mutation_field_assignment`, index projection assignment (`=`, place `v[0]`) in `fn mutation_index_assignment`. Per D3, assignment to a bare local is **not** a mutation site, whether plain or compound.

**Must NOT produce:** No mutation site in `fn mutation_plain_assignment_is_not_mutation` or in `fn mutation_compound_assignment` — both assign to a bare local.

**Corrected 2026-08-14 (second corpus run):** this entry expected five sites,
describing the compound case as `self.n += 1`. The fixture actually writes
`n += 1` against a bare local, which D3 excludes exactly like the plain
assignment beside it, so the file has two negative controls and four positives.
The extractor is right; the expectation was not.

Known corpus gap, deliberately left rather than papered over: because of the
above, **no fixture here exercises compound assignment through a projection**
(`s.items[0] += 3`), which D3 does treat as a mutation. That case is pinned by
the extractor's own unit test
`assignment_is_a_mutation_site_only_through_a_projection`. Changing the fixture
body to `h.x += 1` would close the corpus gap and make this entry's original
wording true; it was not done here because editing a fixture changes what is
under test, which is a separate decision from correcting a manifest.

## long_operand.rs

**Shape:** A clone whose operand expression, after collapsing whitespace, exceeds 240 bytes (exercising D7's digest branch), plus one whose operand is comfortably under the cap.

**Expect:** Four clone sites, **exactly one** of which carries a digest. In `fn long_operand_dot_clone`, the `.clone()` call's operand (`data.get_first_...get_twelfth_...`) is 571 source bytes and still far over the cap after whitespace collapse, so it is stored as `sha256:` plus sixteen lowercase hex characters. In `fn short_operand_clone`, the operand `small` is under the cap and stored as its own source text.

**Must NOT produce:** No truncated operand text. The over-cap operand uses the digest marker, never a partial expression — a truncated expression reads as a real but wrong expression, which is worse than an opaque marker (D7).

**Corrected 2026-08-14 (second corpus run):** this entry claimed
`fn long_operand_clone`'s `.cloned()` receiver also exceeded the cap. It does
not: `data .iter() .filter(|x| x.is_positive()) .map(|x| x * 2) .filter(|x| x.is_negative().not())`
normalises to 92 bytes, and the trailing `.collect()`'s operand to 102 — both
well under 240, both stored verbatim. That function exercises the *under*-cap
path; only `long_operand_dot_clone` reaches the digest branch.

## async_lock_not_sync.rs

**Shape:** An async fn using `.lock().await`, `.read().await`, and `.write().await` (async locks) alongside the same methods without `.await` (synchronous locks), exercising D20's structural distinction.

**Expect:** `sync_lock_site` for `fn sync_lock`, `fn sync_read`, and `fn sync_write` only. The async variants (`fn async_lock`, `fn async_read`, `fn async_write`) must produce NO `sync_lock_site` because their lock acquisition is the direct operand of an `await_expression`.

**Must NOT produce:** No `sync_lock_site` from any async variant (`async_lock`, `async_read`, `async_write`). The extractor must distinguish structurally, not by text matching `std::sync`.

## function_id_divergence.rs

**Shape:** One `.clone()` inside each of six function contexts: a generic impl method (`impl GenericFoo<i32>`), a plain impl method (`impl Foo`), a trait impl method (`impl SomeTrait for Foo`), a default-bodied trait method (`TraitWithDefault::default_method`), a deeply nested function (`mod a { mod b { fn deeply_nested } }`), and a top-level free function.

**Expect:** Six distinct `ownership_site_in_function` edges whose function IDs match the IDs the graph emits for `defines_fn`. Each function must have a distinct, resolvable function ID.

**Must NOT produce:** Any function ID in `ownership_site_in_function` that does not also appear as a `defines_fn` edge. The guard test asserts set equality between the two.

## macro_definition_body.rs

**Shape:** A `macro_rules!` definition whose body contains `.clone()` and `.lock()`, plus a normal function that both invokes the macro and contains its own real `.clone()`. Exercises D22: macro definition bodies emit nothing because they have no enclosing function.

**Expect:** One `clone_site` from `fn macro_invocation_with_own_clone` (the `.clone()` in the normal function body). The macro definition body's `.clone()` and `.lock()` must produce zero sites because they have no canonical function ID.

**Must NOT produce:** Any sites from the macro definition body itself. The macro invocation is not expanded or analyzed for sites.
