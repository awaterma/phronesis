# SPEC: call-graph reachability, suppression rules, and worktree state

**Status:** proposed
**Date:** 2026-08-08
**Related:** `SPEC-triple-store-rete.md` (structural pack), `SPEC-non-code-span-masking.md` (ordering dependency, §7), `SPEC-confidence-scoring.md` (§5)

## 0. Provenance and method

A downstream project that embeds this engine submitted seven proposals: three
predicates, three rules, and one confidence-gate input. This document is the
phronesis-side response. It is not a transcription — every premise was tested
against the real binary and the real graph before being accepted, and three of
the seven do not survive that test.

The consumer's proposals were well-formed: explicit dependency chains,
acceptance tests, stated non-goals. The corrections below are about what is
already shipped and what the mechanisms can actually do, not about the quality
of the request.

## 1. Triage

| Proposal | Verdict | Why |
|---|---|---|
| `is_async` predicate | **Already exists** | `function_is_async` ships today (`syntax/facts.rs:131`), for Rust and Swift, at per-function granularity — finer than the file-level version requested |
| `has_attribute` predicate | **Not needed yet** | The rule that motivates it needs only a substring match (§3) |
| `warn-await-unwrap-expect` rule | **Premise false; narrower gap is real** | `.await.unwrap()` is already blocked (§2). `.await.expect("msg")` is not |
| `enforce-no-allow-clippy` rule | **Accept — shippable now** | Real gap, no new predicate required (§3) |
| `caller_of` predicate | **Accept, restated** | Belongs in the graph as `calls_fn`, not as a Rhai provider (§4) |
| `untested_reverse` predicate | **Accept, restated** | A derived fact, not a Rhai provider (§4.3) |
| `worktree_has_changes` | **Accept with a measured budget** | Real, but see §5 and §6 |

## 2. `.await.unwrap()` is already blocked

Two of the seven proposals rest on this claim: *"None are currently flagged by
the unwrap rule because the `.await.` token sits between the function call and
`.unwrap()`."*

Probed through the real `phr-mcp pre-check` against the live rule pack, each a
`Write` of a `.rs` file under `src/`:

| Probe | Result |
|---|---|
| `c.fetch().await.unwrap()` | **BLOCKED** (exit 2) — `enforce-no-unwrap-in-src` |
| `c.fetch().await.expect("boom")` | not blocked (exit 0) |
| `#[allow(clippy::too_many_lines)]` | not blocked (exit 0) |

`new_content_contains(".unwrap()")` is a plain substring test and
`.await.unwrap()` contains `.unwrap()`. The rule fires. Nothing about `.await`
breaks it, and `is_async` would add a condition that is *always* true when the
substring matches — `.await` is only legal inside an `async fn` or `async`
block, so a file containing `.await.unwrap()` is necessarily async.

**If those call sites are not being flagged in practice, the cause is
elsewhere** and must be diagnosed before any predicate is built for it. Two
known candidates, both documented in `SPEC-non-code-span-masking.md`:
`strip_test_blocks` removing the match when it sits in a test module, and the
`Edit`-fragment gap where a partial payload carries no `#[cfg(test)]` marker
for the stripper to key on.

### 2.1 The residual gap

`.expect("non-empty message")` is uncaught anywhere, in async code or not:
`warn-expect-with-empty-message` deliberately matches only `.expect("")`.
Whether a message-carrying `.expect` should be flagged is a **policy question
about `.expect` generally**, not about async, and this spec does not decide it.
Recorded so the async framing does not smuggle in a broader policy change.

## 3. `enforce-no-allow-clippy` — accept, no predicate needed

Nothing currently catches `#[allow(clippy::…)]`. The nearest rule,
`audit-allow-dead-code-in-src`, matches only `#[allow(dead_code)]`.

The proposal presents `has_attribute` as foundational, but its own rule
definition needs none of it:

```json
{ "id": "enforce-no-allow-clippy",
  "phase": "pre",
  "code_only": true,
  "when": [
    { "new_content_contains": "#[allow(clippy::" },
    { "file_path_matches": "src" }
  ],
  "then": { "warn": "…" } }
```

A generic `has_attribute(?file, ?func, ?attr)` predicate would be a larger
surface than the rule requires, and phronesis has no second consumer for it
yet. **Defer the predicate; ship the rule.** Revisit if a rule appears that
genuinely needs attribute *position* (inside a body vs. on an item) or the
enclosing function's name in the message.

`warn` rather than `block`, initially. A clippy suppression is sometimes the
correct call — a false positive in clippy itself, or a lint that is wrong for
one call site — and a `block` on a judgment call trains operators to work
around the rule. Promotion to `block` should follow measured precision, the
same gate `SPEC-triple-store-rete.md` applies to the structural rules.

## 4. Call-graph reachability

This is the substantive item. The consumer's stated problem is real:
`untested(?fn)` fires on functions that tests exercise *transitively* through
dispatchers and command pipelines but never call directly, so the signal is
noisy on exactly the handler-shaped functions where coverage matters.

### 4.1 The graph already has call edges

`graph/extract.rs:332` emits `tested_by(callee, test_fn)` — a call edge,
restricted to calls made inside a `#[test]` function, and only direct ones.
`untested` is then "every `defines_fn` with no `tested_by` naming it"
(`graph/derive.rs:37`).

So this is not a new mechanism. It is generalising an existing one: emit
`calls_fn(caller, callee)` for every call, not only those originating in test
functions.

### 4.2 Emit `calls_fn` in the sensor

```
{"p":"calls_fn","a":["caller_fn","callee_fn"],"src":"file"}
```

Caller and callee are qualified function paths, matching `defines_fn`'s
existing convention. Note the resolution limit already documented at
`graph/derive.rs:27-35`: the extractor cannot resolve a callee to its defining
module without whole-crate name resolution, so `tested_by` carries bare callee
names while `defines_fn` carries qualified ones, and matching bridges them on
the final path segment. `calls_fn` inherits that limitation. It
over-approximates — two functions sharing a short name are conflated — and
that direction is deliberate for the same reason `untested` chose it: a missed
warning is recoverable, a false accusation blocks legitimate work.

`tested_by` becomes derivable from `calls_fn` plus a test-function marker.
Deriving it is not required by this spec and should not be bundled into it:
7,271 of the current 10,385 edges are `tested_by`, so changing how it is
produced is a large blast radius for no user-visible gain.

### 4.3 `untested_reverse` is a derived fact, not a Rhai provider

The consumer proposed computing this in a `.phronesis/predicates/*.rhai`
provider. **That cannot work**, for three independent reasons.

**The provider sandbox has no access to facts.** There are two distinct Rhai
surfaces:

| Surface | Receives | Used by |
|---|---|---|
| Fact provider — `evaluate(script, event)` | `event`, `emit_fact()` | `.phronesis/predicates/*.rhai` |
| Rule-condition script — `ScriptEval::evaluate(script, facts, bindings)` | `facts`, `bindings` | the `__script__` predicate |

`crates/phronesis-rhai/src/lib.rs:197` registers `emit_fact` and nothing else
on the provider engine; the `facts` scope variable is pushed at `:284`, in the
*other* evaluator. A provider calling `facts("defines_fn", […])` is a compile
error. And in the surface that does have `facts`, it is a flat array in scope,
not a callable taking a predicate and an argument pattern.

**The wrong layer, by the codebase's own stated rule.** `graph/derive.rs`
opens: *"Derived facts: whole-graph computations the engine cannot express.
The engine has no negation-as-failure at the pattern level and no forward
chaining, so `untested` (closed-world negation) and `in_cycle` (transitive
closure) are computed here instead."* `untested_reverse` is closed-world
negation over a transitive closure — both halves, and the module that exists
for exactly that already implements the closure machinery for `in_cycle`.

**The wrong cost profile.** A derived fact is computed once per graph sync and
is a pure function of the edge set. The provider version would walk
callers × calls × callees on every hook invocation, per evaluation.

So:

```rust
/// `untested_reverse(F)` for every F in `defines_fn` that no test function
/// reaches through any chain of `calls_fn` edges.
pub fn untested_reverse(base: &[Edge]) -> Vec<Edge>
```

Computed in `graph/derive.rs` alongside `untested` and `in_cycle`, seeded from
the set of test functions and closed forward over `calls_fn`.

`untested` is **kept**, not replaced. The two answer different questions —
"does a test call this directly" and "can a test reach this at all" — and a
rule author should choose. The existing `warn-untested-risky-call` should
migrate to `untested_reverse`, since transitive reachability is what its
message actually claims.

### 4.4 Cycles and depth

`calls_fn` contains cycles (recursion, mutual recursion). The closure must
mark-and-sweep with a visited set, as `in_cycle` already does. No depth cap:
a cap would make reachability depend on traversal order, and a function
"untested at depth 8 but tested at depth 9" is not a distinction anyone can
act on.

## 5. `worktree_has_changes`

Accept the design as proposed, with one addition and one deferral.

The design is sound where it matters: the Rust host computes the Boolean, Rhai
only translates it into project vocabulary, and a Boolean rather than paths or
contents minimises disclosure. Staged, unstaged, and untracked count; ignored
files do not. Outside a worktree the value is `false`. A failure to inspect an
apparent worktree must produce a diagnostic and must not silently report
clean — that last point is the one most likely to be dropped in
implementation, and it is the difference between an advisory and a lie.

**The policy question the consumer raised should be answered conservatively.**
Adding `worktree_has_changes` to an existing confidence rule silently changes
it from "gate every matching operation" to "gate only when the tree is dirty",
and a clean worktree is no evidence that an incoming merge or pull compiled.
Use it for a new advisory rule only. Leave the existing unconditional
merge/pull/commit gating alone.

**One observation from operating the gate.** Across a long implementation
session on this repository, the confidence gate blocked commits five times.
In every case the worktree was dirty and the *signals* were stale — build and
test evidence grounded against an earlier commit. `worktree_has_changes` would
not have changed any of those outcomes. What would is signal freshness
relative to the current change set, which is a different predicate and is not
specified here. Recorded so this feature is not expected to fix that.

## 6. Cost budget — the gate on §4 and §5

Both accepted features add per-invocation work to a hook that runs on **every
tool call**. Neither ships without measurement.

Current graph, measured on this repository (10,385 edges, 1.6 MB):

| Edges | Predicate |
|---|---|
| 7,271 | `tested_by` |
| 1,455 | `defines_fn` |
| 887 | `untested` (derived) |
| 245 | `imports` |
| 205 | `calls_api` |
| 159 | `declares_module` |
| 159 | `file_type` |
| 4 | `in_cycle` (derived) |

Test functions alone produce 7,271 call edges at ~4.5 calls each. Extending to
1,455 production functions at a comparable rate implies **roughly +6,500 to
+7,500 edges — close to doubling the file.**

**`hydrate` does not save the I/O.** `graph::hydrate::hydrate` filters to the
relations the loaded rules need and returns early when none are needed, but
`graph::store::load` reads and parses the entire `graph.jsonl` first. So the
filtering avoids matching work, not reading or parsing. Any project with a
single structural rule enabled pays the larger file on every hook.

Required before either lands:

1. Measured hook latency, before and after, on this repository and on one
   external corpus — the same two-corpus gate `SPEC-triple-store-rete.md`
   already applies.
2. A stated budget. If added latency is material, the mitigations in
   preference order are: emit `calls_fn` only for production functions
   (`file_type` already distinguishes them); store it in a separate file
   loaded only when a rule needs that relation; or precompute
   `untested_reverse` at sync time and ship only the derived edges, dropping
   `calls_fn` from the file entirely.
3. For `worktree_has_changes`: a measured per-invocation cost for the git
   inspection, not an assertion that it is bounded.

Option 3 in that list is worth noting — if no rule ever matches on `calls_fn`
directly, the graph never needs to carry it, and the whole size concern
disappears. That should be the first thing tried.

## 7. Ordering dependency

`enforce-no-allow-clippy` and any `.expect`-based rule **must carry
`code_only: true`**, which does not exist yet — it is specified in
`SPEC-non-code-span-masking.md`. Without it, `#[allow(clippy::` or
`.await.expect(` inside a doc comment or a string literal is a false positive,
at `block` or `warn` level, in a codebase whose own guidance quotes these
tokens constantly.

So: **masking lands first, then the suppression rules.** Shipping them in the
other order means shipping a known false-positive class and then removing it,
which spends operator trust for no reason.

`calls_fn` and `worktree_has_changes` have no such dependency and can proceed
in parallel.

## 8. Recommended sequence

1. **`SPEC-non-code-span-masking.md`** — unblocks the rules below and fixes a
   measured 5/5 false-positive rate on existing `block`-level rules.
2. **`enforce-no-allow-clippy`** — one rule, one pack entry, no new mechanism.
   The cheapest real win here.
3. **`calls_fn` + `untested_reverse`** — the substantive work. Start with §6
   option 3 (derive-only, do not ship `calls_fn` in the graph file) and fall
   back to shipping the edges only if a rule needs to match them directly.
4. **`worktree_has_changes`** — independent; sequence by appetite, not
   dependency.

Deferred with reasons recorded, not silently dropped: `has_attribute` (§3),
`is_async` (§1 — already shipped), `warn-await-unwrap-expect` as framed (§2),
`tested_by` re-derivation (§4.2), and `.expect`-with-message policy (§2.1).

## 9. Acceptance criteria

**`calls_fn` / `untested_reverse`**

1. A function called only through one intermediary from a test is `untested`
   but **not** `untested_reverse`. This is the whole point of the feature.
2. A function no test reaches at any depth is both.
3. Mutual recursion between two unreached functions terminates and marks both
   `untested_reverse`.
4. A recursive function reached from a test is not `untested_reverse`.
5. `untested` output is byte-identical to today's for the existing corpus —
   this adds a relation, it does not change an existing one.
6. Derivation ignores pre-existing derived edges, so a stale `untested_reverse`
   cannot feed back and pin itself in place (the invariant at
   `graph/derive.rs:20-25`).
7. Graph size and hook latency reported before and after, per §6.

**`enforce-no-allow-clippy`**

8. `#[allow(clippy::too_many_lines)]` in `src/` warns.
9. The same text in a doc comment or string literal does **not** warn, once
   `code_only` exists. Until then this criterion cannot be met and the rule
   should not ship.
10. `#[allow(dead_code)]` still routes to the existing audit rule — the two do
    not double-report.

**`worktree_has_changes`**

11. The consumer's twelve acceptance cases, adopted as written: clean → false;
    modified, staged, untracked, deleted, renamed → true; ignored-only →
    false; `git stash` clears when only tracked changes existed and does not
    when an untracked file remains; `git stash -u` clears; a synthetic
    provider test sets the Boolean directly; existing sandbox tests still
    prove providers cannot touch the filesystem or spawn processes.
12. An inspection failure inside an apparent worktree produces a diagnostic
    and does not report `false`.
