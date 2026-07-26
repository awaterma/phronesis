# SPEC: Structural Graph Facts for the RETE Engine

**Status:** Phase One implemented on `feat/structural-graph-facts`; measurements in §2.3 and §10
**Target release:** 0.23.0
**Revision:** 3 — engine cross-check review applied, then implemented and measured (see §11)

## Summary

This specification defines requirements and architecture for representing **code structure** as facts in the Phronesis working memory, so that structural violations (untested risky calls, layering breaches, dependency cycles) can be caught at the hook boundary with **zero token cost** and **zero LLM latency**.

The durable graph lives on disk as line-delimited JSON (`.phronesis/graph.jsonl`), rebuilt incrementally by a language-scoped **extractor** (the sensor) on `PostToolUse`, and hydrated into the RETE network on `PreToolUse`.

The centerpiece of this work is the **extractor**, not the RETE plumbing. Relational matching over `Fact { predicate, args }` with `?var` binding and beta joins already exists in `crates/phronesis`; no engine changes are required. What does not exist is a trustworthy source of structural facts. Sections 4 and 5 carry the detail budget accordingly.

---

## Technical Goals & Rationale

1. **Zero-Token Relational Reasoning:** Avoid loading dependency trees, AST maps, or history logs into the LLM context. Relational logic is offloaded entirely to compiled Rust.
2. **Runtime Governance at the Hook Boundary:** Structural rules are evaluated before tool execution. Violations produce a deterministic structural explanation.
3. **Bounded On-Disk Footprint:** Graph size is proportional to codebase structure, not to edit history.
4. **No Engine Changes:** Structural facts use the existing `Fact` shape and the existing predicate index. The `phr` crate is untouched, which fully derisks the embedded-engine consumer.

Note: `graph.jsonl` is **derived state and is gitignored**. It is rebuildable from source at any time and carries no auditability guarantee. Git-auditability remains a property of `rules.json`, which is version-controlled.

---

## 1. Data Representation: Relation-as-Predicate Facts

An earlier draft of this spec funneled every edge into a single `Fact { predicate: "triple", args: [s, p, o] }`. **That design is rejected.** `WmeManager` indexes WMEs by predicate (`predicate_index`, `crates/phronesis/src/wme.rs:35`), and alpha-node filtering keys off that index (`wme.rs:113`). With ~15,000 facts all named `triple`, the index degenerates to one bucket: every rule condition alpha-matches all 15k WMEs. Beta joins are unhashed nested loops over left and right memories (`crates/phronesis/src/beta_network.rs:100-133`), so a four-condition rule becomes roughly O(n²)–O(n³) over that bucket. The "microseconds" claim would not survive implementation.

### 1.1 The Fact Shape

The triple's **relation becomes the fact predicate**; subject and object become args:

```rust
Fact {
    predicate: "defines_fn".to_string(),
    args: vec![file, func],
}
```

This works in the engine **today, unmodified**. It reuses the predicate index for free, keeps each alpha memory small and per-relation, and eliminates the proposed `TripleWme` type entirely.

### 1.2 Closed Relation Set (Phase One, Rust only)

| Relation | args | Meaning |
|---|---|---|
| `file_type` | `[file, kind]` | `production` \| `test` \| `example` \| `build` |
| `defines_fn` | `[file, func]` | Fully-qualified function path defined in file |
| `calls_api` | `[func, callee]` | Call edge to a watched API (closed watchlist, §4.3) |
| `imports` | `[file, module]` | Module-level dependency edge |
| `tested_by` | `[func, test_func]` | Test coverage edge (§4.4) |
| `in_cycle` | `[module, cycle_id]` | Module participates in an import cycle (§4.5) |
| `provenance` | `[entity, file]` | Source file that produced facts about `entity` (§3) |

The set is deliberately closed. Adding a relation is a spec change, not an extractor implementation detail, because each relation is a promise the enforcement layer makes to the user.

---

## 2. On-Disk Representation: `graph.jsonl`

Stored at `.phronesis/graph.jsonl`, gitignored. Each line is one standalone JSON object, one edge, with a mandatory `src` provenance field.

### 2.1 File Schema

```json
{"p": "file_type",  "a": ["crates/phronesis/src/network.rs", "production"],        "src": "crates/phronesis/src/network.rs"}
{"p": "defines_fn", "a": ["crates/phronesis/src/network.rs", "fire_all_consequences"], "src": "crates/phronesis/src/network.rs"}
{"p": "calls_api",  "a": ["fire_all_consequences", "std::unwrap"],                  "src": "crates/phronesis/src/network.rs"}
{"p": "tested_by",  "a": ["fire_all_consequences", "tests::test_fire_all"],         "src": "crates/phronesis/tests/network_test.rs"}
```

`p` and `a` map directly onto `Fact { predicate, args }`. `src` is metadata for compaction and is not asserted into working memory.

### 2.2 Memory Footprint Budget

* **Core scope:** cross-cutting architectural relations only — module imports, function definitions, watched API calls, test mappings. Never local variables or intra-function control flow.
* **Scale estimate:**
  * 1,000 files, 10,000 functions.
  * ~15 edges per file ≈ 15,000 lines.
  * ~90 bytes/line ≈ 1.4 MB on disk.
  * Parse + hydrate: **measured**, not asserted — see §2.3.

### 2.3 Measured latency (`benches/graph_sync.rs`, release, Apple M-series)

| Graph size | Per-save (PostToolUse) | Hydrate (PreToolUse) |
|---|---|---|
| 1,000 edges | 5.4 ms | 0.51 ms |
| 5,000 edges | 6.7 ms | 1.46 ms |
| **15,000 edges** (spec scale) | **10.1 ms** | **3.8 ms** |
| 30,000 edges | 17.0 ms | 7.5 ms |

Both scale linearly in total edges, as expected: the parse tier sees one file regardless of graph size, so the slope belongs entirely to the derive and I/O tiers.

**Projects with no structural rules pay 6.7 ns** — hydration is gated on whether any loaded rule names a graph relation (§5.5), so this feature cannot regress hook latency for existing users.

Full rebuild of this repository (138 files, 7,161 base + 699 derived edges) takes 160 ms warm. That is the exceptional resync path, not the per-save path.

---

## 3. Provenance and Compaction

### 3.1 Why subject-keyed compaction fails

The earlier draft discarded lines where `subject == edited file`. But most interesting edges have a **function**, not a file, as their subject — `fire_all_consequences → calls_api → std::unwrap`. Deleting or renaming that function leaves its edges in the graph forever, and stale edges in an enforcement layer produce false blocks.

### 3.2 Provenance-keyed compaction

Every line carries `src`, the file whose extraction produced it. On save of `F`:

1. **Extract:** the sensor parses `F` and derives its current edge set.
2. **Discard:** all existing lines where `src == F` are dropped, regardless of subject.
3. **Merge + atomic write:** remaining lines plus new edges are written to `graph.jsonl.tmp` and `rename(2)`d over the original.

This bounds the file by codebase size, not edit count.

### 3.3 Naming: this is not LSM

Read-filter-rewrite of the entire file on every save is not log-structured merge — there are no levels and no deferred merge. Phase One does exactly this (correct and simple at ~1.4 MB) and calls it **rewrite compaction**. If write cost becomes a problem, Phase Two may move to append-then-periodic-compact, at which point the LSM framing becomes accurate.

### 3.4 Resync: edits outside the hook path

`git checkout`, `git mv`, branch switches, rebases, and plain shell edits never reach `PostToolUse`. The graph silently drifts, and drift in an enforcement layer means false blocks. Phase One mitigations:

* **Content hash per source file** stored alongside the graph (`.phronesis/graph.index`). At hydrate time, cheaply stat/hash tracked files; if any hash mismatches, mark the graph **stale**.
* **Stale graph downgrades enforcement to warn**, never block, and emits a one-line notice with the resync command.
* **`phr-mcp graph rebuild`** performs a full scan. Recommended as a `post-checkout` / `post-merge` git hook; documented, not auto-installed.

---

## 4. The Sensor (Extractor)

This is the hard part of the spec and the main Phase-One risk. A structural enforcement layer is only as trustworthy as its facts; a single false block destroys confidence in the whole system.

### 4.1 Language scope

**Phase One is Rust only.** The extractor uses `tree-sitter-rust`. Regex extraction is explicitly rejected — it cannot distinguish a call inside a string literal, a comment, or a `cfg`-disabled block, and each of those is a false-block generator. Other languages are out of scope until the Rust extractor's false-positive rate is measured.

### 4.2 Entity naming

Functions are identified by module-qualified path (`crate::network::fire_all_consequences`) derived from file path plus enclosing `mod` blocks. Trait impl methods are qualified with the impl type (`crate::network::Network::fire`). Ambiguity (macro-generated items, `#[path]` attributes) causes the item to be **skipped and counted**, not guessed.

### 4.3 Watched-API list

`calls_api` is emitted only for a closed watchlist configured in `.phronesis/graph.toml` (default: `unwrap`, `expect`, `panic!`, `todo!`, `unimplemented!`). Resolution is syntactic — a method call named `unwrap` on any receiver counts. This over-approximates (a user type with an `unwrap` method matches); the watchlist being explicit and small keeps that tractable.

### 4.4 Test mapping — and why it drives the architecture

The earlier draft's `has_test = "false"` smuggles in **closed-world negation**. The engine has no negation-as-failure at the pattern level (only `!` inside script guards), so *something* must assert the falsehood. Deriving "this function has no test" requires whole-repo knowledge, which means the extractor cannot be a single-file parser.

Design:

* The extractor emits only **positive** `tested_by` edges.
* A test function is any `fn` under `#[test]`/`#[tokio::test]`, or any `fn` in `tests/` or a `#[cfg(test)] mod`.
* A `tested_by(F, T)` edge is emitted when `T`'s body syntactically calls `F`. This is a **direct-call heuristic**: it misses functions covered transitively and misses table-driven or macro-generated tests.
* The extractor parses one file at a time and so **cannot resolve a callee to its defining module** — that would need whole-crate name resolution. `tested_by` therefore carries the bare callee name while `defines_fn` carries a module-qualified one, and the derivation matches them on the **final path segment**.
* Short-name matching **over-approximates coverage**: two functions sharing a name are both treated as covered when either is tested. This is the deliberate choice of error direction. A missed warning is recoverable; a false "untested" verdict blocks legitimate work, and false blocks are what destroy trust in an enforcement layer. The heuristic errs toward silence, not toward accusation.
* Because coverage is whole-repo, saving a test file can add edges pointing at functions in other files, and saving a *source* file changes which functions are uncovered. Derived facts must therefore be recomputed on **every** save — see §4.5.

### 4.5 Two-tier extraction: parse one file, derive over the whole graph

Derived facts (`untested`, `in_cycle`) need whole-repo knowledge on every save. That does **not** mean reparsing the repo on every save. The two costs are separable, and conflating them is what makes "full scan on each save" sound unaffordable:

| Tier | Scope | Input | Cost at 1k files / 15k edges |
|---|---|---|---|
| **Parse** | edited file only | source text → tree-sitter | one file, sub-millisecond |
| **Derive** | whole graph | the 15k edges already on disk | set ops + Tarjan, no I/O of source |

On every `PostToolUse` save:

1. **Parse** the edited file only, producing its base edges (`defines_fn`, `calls_api`, `imports`, `tested_by`).
2. **Compact** by provenance (§3.2), yielding the complete current base-edge set.
3. **Derive** over that whole set, unconditionally:
   * `untested(F)` = every `F` in `defines_fn` with no `tested_by(F, _)` — a set difference over ~10k functions.
   * `in_cycle(M, C)` = Tarjan's SCC over the `imports` edges — linear in edges.
4. **Write** base edges plus derived edges atomically.

Derived edges are marked in the file (`"d": true`) so compaction discards and regenerates them wholesale rather than trying to attribute them to a source file — they are a function of the entire graph, not of any one file.

The result is that `untested` and `in_cycle` are correct after **every** save, not only after a full rebuild, so rules keyed on them are not gated on a rare rebuild. Full reparse of every file is still needed when the staleness check fails (§3.4), i.e. after `git checkout` and friends — but that is the exceptional path, not the per-save path.

Both derivations are pure functions of the edge set, so they are also the cheapest part of the pipeline to test.

* **Why cycles are derived, not matched:** transitive closure is not expressible in the engine — consequences do not re-assert facts, so there is no forward chaining, and fixed-length `when` clauses only catch cycles of one hard-coded length. Precomputing `in_cycle` keeps rules flat and single-condition.

### 4.6 Failure behavior

Parse failure on a file drops that file's edges and marks the graph stale for that file. The extractor never emits partial edge sets for a file it could not fully parse.

---

## 5. Structural Rule Matching (`rules.json`)

Rules are ordinary Phronesis rules over the relations in §1.2 — no new syntax.

### 5.1 Untested risky call

```json
{
  "rules": [
    {
      "id": "warn-untested-production-unwrap",
      "phase": "pre",
      "priority": 100,
      "when": [
        { "predicate": "file_type",  "args": ["?file", "production"] },
        { "predicate": "defines_fn", "args": ["?file", "?func"] },
        { "predicate": "calls_api",  "args": ["?func", "std::unwrap"] },
        { "predicate": "untested",   "args": ["?func"] }
      ],
      "then": {
        "action_type": "warn",
        "params": [
          "`?file` defines `?func`, which calls `std::unwrap` and has no direct test."
        ]
      }
    }
  ]
}
```

### 5.2 Import cycle

```json
{
  "id": "warn-import-cycle",
  "phase": "pre",
  "priority": 90,
  "when": [
    { "predicate": "in_cycle", "args": ["?module", "?cycle"] }
  ],
  "then": {
    "action_type": "warn",
    "params": ["Module `?module` participates in import cycle `?cycle`."]
  }
}
```

### 5.3 Warn before block

All Phase-One structural rules ship as `warn`. Promotion to `constraint_violation` requires a measured false-positive rate from real use (§8, task 6). Blocking on a heuristic that has never been measured is how an enforcement layer loses the user's trust permanently.

### 5.4 Freshness is a fact, not harness logic

Hydration asserts `graph_fresh(true|false)` alongside the structural relations. A rule that wants to *block* must include `{"predicate": "graph_fresh", "args": ["true"]}` in its `when`; a rule content to warn on possibly-drifted data can omit it.

This keeps the stale-downgrade decision in `rules.json` rather than hard-coded in the hook, which is the same participatory model the rest of phronesis uses — and it means the policy is reviewable in git.

### 5.5 Demand-gated hydration

The graph is loaded only if some rule in `rules.json` names one of the relations in §1.2. A project with no structural rules performs no file I/O and asserts no facts (measured at 6.7 ns, §2.3). This is what makes the feature safe to ship enabled.

### 5.6 Evaluation pipeline

```
               [ .phronesis/graph.jsonl ]
                           │
                           ▼ (hydrated on PreToolUse; staleness checked first)
         ┌─────────────────────────────────┐
         │         RETE Alpha Nodes        │ one memory per relation
         └─────────────────┬───────────────┘   (file_type, defines_fn, …)
                           │
                           ▼
         ┌─────────────────────────────────┐
         │         RETE Beta Nodes         │ join on ?file, ?func
         └─────────────────┬───────────────┘
                           │
                           ▼
                   [Warn / Violation]
```

---

## 6. Hook Lifecycles

```
        ┌──────────────┐
        │  LLM Action  │ (Edit src/lib.rs)
        └──────┬───────┘
               ▼
    ┌──────────────────────────────┐
    │      PostToolUse Hook        │ (Sensory Phase)
    │  1. tree-sitter parse file   │
    │  2. derive edge set          │
    │  3. drop lines src==file     │
    │  4. atomic rewrite + hash    │
    └──────────────┬───────────────┘
                   ▼
        ┌──────────────┐
        │  Next Action │ (git commit)
        └──────┬───────┘
               ▼
    ┌──────────────────────────────┐
    │      PreToolUse Hook         │ (Enforcement Phase)
    │  1. staleness check (§3.4)   │
    │  2. hydrate facts by relation│
    │  3. evaluate rules.json      │
    │  4. warn (block only when    │
    │     fresh + rule promoted)   │
    └──────────────────────────────┘
```

---

## 7. Explicitly Out of Scope for Phase One

* Non-Rust languages.
* Blocking (as opposed to warning) on any structural rule.
* Transitive/recursive rule evaluation in the engine; cycles are precomputed (§4.5).
* Any change to `crates/phronesis` engine internals — including `TripleWme`, which is **cut**.
* Auditability claims for `graph.jsonl`.

---

## 8. Phase-One Implementation Requirements

1. **Graph reader/writer:** JSONL stream parse + provenance-keyed rewrite compaction with atomic rename, in `crates/phronesis-mcp/src/graph/`.
2. **Staleness index:** per-file content hashes in `.phronesis/graph.index`; freshness check on hydrate; `phr-mcp graph rebuild` for full scan.
3. **Rust extractor:** `tree-sitter-rust` over the closed relation set (§1.2), with the watched-API list, test mapping, Tarjan SCC, and skip-and-count on ambiguity.
4. **Hydration:** load graph lines as `Fact { predicate, args }` at PreToolUse. No engine changes.
5. **Derivation pass:** `untested` by set difference and `in_cycle` by Tarjan SCC over the full edge set, run on every save (§4.5), with derived edges flagged and regenerated wholesale.
6. **Benchmark:** measure (a) hydrate latency at 15k edges against the PreToolUse budget and (b) the per-save parse+compact+derive round trip against the PostToolUse budget. Acceptance gates, not assumptions.
7. **False-positive measurement:** run the extractor over this repo and at least one additional Rust codebase of comparable size; report `untested` and `in_cycle` precision by hand-audit. Gates any future promotion from warn to block. **First corpus measured — see §10.**
8. **Integration tests:** cycle detection and untested-call detection fire deterministically, without LLM invocation; stale graph downgrades to warn.

---

## 10. First-corpus measurement (this repository)

138 Rust files, 7,161 base edges, 699 derived, 0 items skipped by the extractor.

| Relation | Count | Notes |
|---|---|---|
| `defines_fn` | 1,138 | |
| `tested_by` | 5,577 | direct-call edges from test bodies |
| `calls_api` | 167 | watchlist: unwrap/expect/panic/todo/unimplemented |
| `imports` | 98 | intra-crate only |
| `untested` | 695 | 61% of functions |
| `in_cycle` | 4 | 2 cycles of 2 modules each |

**`in_cycle`: 2/2 genuine.** `crate::hook` ↔ `crate::hook_facts` and `crate::variable_binding` ↔ `crate::wme` are both real mutual imports, confirmed by reading the `use` statements.

**`untested` alone is too noisy to be a rule.** At 61% of all functions it mostly reports "covered only transitively", which is true but not actionable — the direct-call heuristic is working as specified, and the specification is what makes it noisy.

**The composed rule is the usable signal.** `untested` ∧ `calls_api` ∧ `file_type=production` fires **6 times across the whole repository**. Spot-audit of `crate::init::install_one_target`: contains `.unwrap()` at `init.rs:220` and `:236`, and its only caller is production code at `:359` — no direct test. True positive.

**Conclusion:** `in_cycle` and the composed untested-risky-call rule are precise enough to ship as `warn`. Bare `untested` should not be given a rule of its own. Promotion of anything to `block` still awaits a second corpus (§8 task 7).

---

## 11. Revision History

**Revision 3** — Phase One implemented (`crates/phronesis-mcp/src/graph/`), 92 unit tests:

* Recorded measured latency for both hook paths and made hydration demand-gated (§2.3, §5.5).
* Corrected §4.4's error-direction claim: short-name matching over-approximates coverage, so the heuristic errs toward silence, not toward false accusation. The earlier text claimed the opposite.
* Added `graph_fresh` as an asserted fact so the stale-downgrade policy lives in `rules.json` (§5.4).
* Measured the first corpus; `untested` alone is too noisy for a rule of its own (§10).

**Revision 2** — applied engine cross-check review (`wme.rs`, `engine_types.rs`, `beta_network.rs`, `variable_binding.rs`):

* Dropped `TripleWme` and the `"triple"` predicate wrapper; relation-as-predicate instead, preserving the predicate index and avoiding quadratic joins (§1).
* Added mandatory `src` provenance; compaction keyed on it rather than on subject (§3.2).
* Promoted the sensor to the centerpiece: language scope, entity naming, watched-API list, test mapping, failure behavior (§4).
* Moved cycle detection and negation into the extractor, since neither is expressible in a flat, non-recursive `when` (§4.5).
* Split extraction into parse (edited file only) and derive (whole graph) tiers, so `untested` and `in_cycle` are recomputed on every save without reparsing the repo (§4.5).
* Added a staleness/resync story for edits outside the hook path (§3.4).
* Resolved the goals-vs-§2 contradiction: `graph.jsonl` is gitignored derived state; the auditability claim now applies only to `rules.json`.
* Renamed "LSM compaction" to "rewrite compaction" (§3.3).
* Downgraded all Phase-One rules from block to warn pending measured false-positive rates (§5.3).
* Replaced asserted latency numbers with a benchmark acceptance gate (§2.2, §8).
