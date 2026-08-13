# SPEC: Structural Graph Facts for the RETE Engine

**Status:** Phase One implemented on `feat/structural-graph-facts`; measurements in §2.3 and §10
**Target release:** 0.23.0
**Revision:** 8 — glob queries and explicit cross-language data contracts specified

> **Revision 7 (2026-08-11)** generalizes `imports` from an intra-crate edge to
> a static build/evaluation dependency between language-qualified graph nodes,
> permits multiple modules in one physical file, adds generic non-callable
> definition facts, and defines pre-dispatch ownership arbitration for embedded
> languages such as Helm templates over YAML. These changes are required by the
> Lua, CUE, JSON, YAML, and Helm 3 language-pack designs dated 2026-08-11.

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

### 1.2 Closed Relation Set

| Relation | args | Meaning |
|---|---|---|
| `file_type` | `[file, kind]` | `production` \| `test` \| `example` \| `build` |
| `defines_fn` | `[file, func]` | Fully-qualified function path defined in file |
| `calls_api` | `[func, callee]` | Call edge to a watched API (closed watchlist, §4.3) |
| `imports` | `[module, module]` | Static build/evaluation dependency between tracked, language-qualified graph nodes. Cross-language edges require explicit dialect, manifest, trace, or project binding evidence; names or proximity alone are insufficient. |
| `tested_by` | `[func, test_func]` | Test coverage edge (§4.4) |
| `in_cycle` | `[module, cycle_id]` | Graph node participates in a static build/evaluation dependency cycle (§4.5) |
| `declares_module` | `[file, module]` | Links a file to each module it declares or contains. A file may declare zero or more modules, so file-scoped joins may bind more than once (§5.7). |
| `edited_file` | `[file]` | The file the current call is touching, repo-relative. Not stored on disk — asserted per invocation (§5.7) |
| `graph_file` | `[file]` | A source file known to the graph, including an otherwise-empty file (§9.1) |
| `graph_module` | `[module]` | A language-qualified module identity (§9.1) |
| `graph_function` | `[function]` | A callable function or method identity, including tests (§9.1) |
| `graph_test` | `[test]` | A test function identity. Tests are also functions (§9.1) |
| `graph_definition` | `[definition]` | A stable, named non-callable definition such as a CUE definition, schema resource, anchor, or Helm named template (§9.1) |
| `defines` | `[file, definition]` | Links a physical file to a named non-callable definition it declares (§9.1) |
| `element_in_file` | `[element, file]` | Exact containment used to scope any element kind to a file (§9.1) |
| `element_in_module` | `[element, module]` | Exact containment used to roll function/test evidence up to a module (§9.1) |
| `generates` | `[producer, artifact_module]` | An exact producer graph element generates a uniquely indexed tracked artifact. Structured key analysis is limited to JSON/YAML. |
| `consumes_data` | `[consumer, artifact_module]` | An exact indexed consumer reads a configured or narrowly inferred artifact. Explicit bindings support every indexed language pack. |
| `deserializes` | `[consumer_type, artifact_module]` | Stronger Rust/Serde-specific evidence that a `Deserialize` type consumes the artifact. |
| `data_flows_to` | `[artifact_module, consumer]` | Derived inverse of `consumes_data` (and compatibility `deserializes` evidence), providing forward Config → consumer navigation across languages. |
| `data_key` | `[artifact_module, pointer]` | A bound JSON/YAML artifact's top-level key, encoded as a JSON pointer. |
| `serde_field` | `[type, field, wire_name]` | A `Deserialize` field and one accepted wire name. |
| `emits_key` | `[producer, artifact_module, pointer]` | A producer definition supplies a bound artifact key. |
| `maps_data_key` | `[artifact_module, pointer, rust_field]` | An artifact key maps to an accepted Rust field. |
| `unconsumed_data_key` | `[artifact_module, pointer, consumer_type]` | A bound key has no accepted field and can be silently dropped. |
| `cue_import_diagnostic` | `[file, import_path, kind]` | A repository-local CUE import was `unresolved` or `ambiguous`; no dependency edge is guessed. Built-ins are not diagnostics. |
| `generated_artifact_diagnostic` | `[kind, reference]` | A configured generated-artifact reference is missing, malformed, invalid, or ambiguous; no seam edge is guessed. |
| `generated_without_consumer` | `[artifact_module]` | Derived closed-world evidence that a generated artifact has no indexed consumer. Policy is project-specific. |
| `consumed_without_producer` | `[artifact_module]` | Derived closed-world evidence that a consumed artifact has no indexed producer. Policy is project-specific. |

Provenance is **not** a relation. It is the `src` field on every stored edge (§2.1), and is never asserted into working memory.

The set is deliberately closed. Adding a relation is a spec change, not an extractor implementation detail, because each relation is a promise the enforcement layer makes to the user. Revision 8's data-contract relations, corrected CUE package identities, forward Config → consumer navigation, and lifecycle-gap evidence require graph format 8: an unchanged worktree built by an older binary otherwise appears fresh while lacking the promised seams or retaining obsolete identities.

---

## 2. On-Disk Representation: `graph.jsonl`

Stored at `.phronesis/graph.jsonl`, gitignored. Each line is one standalone JSON object, one edge, with a mandatory `src` provenance field.

### 2.1 File Schema

```json
{"p": "file_type",  "a": ["crates/phronesis/src/network.rs", "production"],        "src": "crates/phronesis/src/network.rs"}
{"p": "defines_fn", "a": ["crates/phronesis/src/network.rs", "rust:phronesis::network::fire_all_consequences"], "src": "crates/phronesis/src/network.rs"}
{"p": "calls_api",  "a": ["rust:phronesis::network::fire_all_consequences", "unwrap"], "src": "crates/phronesis/src/network.rs"}
{"p": "tested_by",  "a": ["fire_all_consequences", "rust:phronesis::tests::test_fire_all"], "src": "crates/phronesis/tests/network_test.rs"}
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
* **Stale graph downgrades enforcement to warn**, never block. The downgrade is applied by the harness to the rules that read graph relations (§5.4), with a one-line stderr notice naming the resync command.
* **`phr-mcp graph rebuild`** performs a full scan. Recommended as a `post-checkout` / `post-merge` git hook; documented, not auto-installed.

---

## 4. The Sensor (Extractor)

This is the hard part of the spec and the main Phase-One risk. A structural enforcement layer is only as trustworthy as its facts; a single false block destroys confidence in the whole system.

### 4.1 Language scope

**Extraction is AST-based, per language.** Rust uses `tree-sitter-rust`, Python
uses `tree-sitter-python`, and TypeScript uses `tree-sitter-typescript`. Regex
extraction is explicitly rejected — it cannot distinguish a call inside a
string literal, a comment, or a disabled block, and each of those is a
false-block generator.

The three extractors share the edge vocabulary of §1.2, `::` as the segment
separator, and nothing else. Their module systems and test shapes differ, so
a shared "generic extractor" would hide the semantics that determine whether
an edge is trustworthy. Each extractor is written explicitly.

#### 4.1.1 Dispatch and ownership arbitration

Extension dispatch is insufficient when one language is embedded in files
normally associated with another. Before per-file extraction, rebuild and
incremental synchronization construct an ownership map from repository
manifests and explicit graph configuration. Each physical file has exactly one
syntax owner for a generation. Metadata consumers may still read that file,
but only the syntax owner emits its syntax-derived subgraph.

Ownership uses ordered, explicit claims. A Helm 3 chart's valid `templates/`
boundary, for example, owns `.yaml`, `.yml`, `.tpl`, and `NOTES.txt` template
source before generic YAML dispatch; `Chart.yaml`, `values.yaml`, and
`values.schema.json` remain YAML/JSON syntax. The map is complete before any
extractor runs, deterministic under traversal-order changes, and included in
freshness. An ownership-changing manifest edit routes to `rebuild()` because
per-file compaction cannot retract facts produced under a former owner.

When two claims at the same precedence own one file, extraction skips it and
reports `owner_ambiguous`; it never lets whichever extractor ran last win.

### 4.2 Entity naming

Entities are identified by
`<language>:<package>[#<target-kind>:<target-name>]::<module-path>`. A Cargo
package's library uses the unsuffixed package identity. Its other compilation
targets use an explicit suffix, so both workspace members and targets within a
member remain distinct:

```text
rust:phronesis::wme
rust:phronesis-mcp::wme
rust:phronesis-mcp#bin:phronesis-mcp
rust:phronesis-mcp#test:hook_integration
```

The extractor discovers Cargo packages from their manifests and resolves each
source file to the innermost containing package and then to its library,
default or named binary, integration test, example, benchmark, or build-script
target. Dependency aliases are mapped to sibling package identities, including
aliases inherited from `[workspace.dependencies]`. Non-library targets can
also resolve their own package's library through its normalized Rust extern
name. Third-party dependencies are ignored because the project graph has no
definitions for them.

Functions append their item path
(`rust:phronesis::network::fire_all_consequences`). Trait impl methods append
the impl type (`rust:phronesis::network::Network::fire`). A Rust file not
claimed by a discoverable package uses the explicit fallback unit
`rust:crate`; it never falls back to the ambiguous bare `crate` namespace.
Ambiguity (macro-generated items, `#[path]` attributes) causes the item to be
**skipped and counted**, not guessed.

The language prefix is part of identity even though Phase One has only a Rust
extractor. This lets later extractors coexist in one graph without a
repository-wide identity migration.

#### 4.2.1 Migration

The graph and its index are derived, gitignored state. Revision 4 changes every
Rust entity identity from `crate::…` to `rust:<package>::…`, so an existing
graph must be regenerated.

Nothing about the *files* changes across such a revision, so the content-hash
index cannot detect it: every hash still matches and the graph reports itself
fresh while half its edges are unjoinable. The index therefore carries the
identity scheme it was built under, as a `# format <n>` header:

| format | identity scheme |
|--------|-----------------|
| 0 | pre-versioning; bare `crate::…` |
| 4 | `<lang>:<package>[#<target>]::<module path>` |
| 5 | format 4 identities plus revision 7 multilingual `imports`, multi-module files, `graph_definition`, and `defines` contracts |
| 6 | format 5 plus package-shaped CUE identities, glob queries, and generated-artifact data seams |

A non-empty index whose format differs from the running binary's is reported
as `Outdated` — distinct from `Stale`, because no file drifted and only a
rebuild resolves it. Enforcement downgrades to warn exactly as it does for
drift, and both `phr-mcp graph status` and the pre-hook name the older format
rather than reporting "0 files changed".

Recovery is automatic: the next `PostToolUse` save into an outdated graph runs
a full rebuild before applying the edit. Incremental compaction alone is not a
migration, because it only replaces edges belonging to the edited file — the
rest would keep the old naming and never join to the new. `phr-mcp graph
rebuild` does the same thing on demand.

An empty index is never "outdated": a project that has never built a graph has
no old edges to migrate, and treating it as a migration would demand a rebuild
of nothing.

#### 4.2.2 Module-path anchoring

The module part of an identity is anchored at the owning target's crate-root
file, not at the repository root. `crates/app/benches/sync.rs` is
`rust:app#bench:sync`, not
`rust:app#bench:sync::crates::app::benches::sync` — the prefix already states
the package and target, and restating the path would also make identity
depend on where the package sits in the repo. Modules resolve against the
crate-root file's *directory*, matching Rust: a target root
`tests/hooks.rs` declaring `mod helper;` means `tests/helper.rs`. A file no
manifest claims keeps the older `src/`-anchored heuristic, since there is no
known prefix to strip.

#### 4.2.3 Python identity

A Python entity is `python:<distribution>::<module path>`, with **no target
suffix**. Cargo's `#bin:` / `#test:` split exists because those genuinely
compile as separate crates with separate `crate` roots; Python has no
equivalent, and a test module is an ordinary module. Inventing a suffix by
analogy would split one namespace into several that can never join.

The distribution name comes from the nearest `pyproject.toml`, under PEP 621's
`[project]` or Poetry's `[tool.poetry]`. Import paths are rooted at the
layout's import root, read from disk rather than guessed: a `src/` directory
beside the manifest means the src layout, otherwise the flat layout. A `.py`
file no manifest claims falls back to `python:project` — its own language's
word for an unnamed root, never Rust's `crate`.

Segments join with `::`, not `.`, even though Python source writes dots. The
separator belongs to the graph's data model: `derive::untested` bridges
`tested_by`'s bare callee names to `defines_fn`'s qualified ones by splitting
on it, so a dotted identity would make every tested Python function report as
untested.

Unit resolution filters by the file's own language. A repository can hold a
`pyproject.toml` at the root and a `Cargo.toml` under `crates/`, and
innermost-root alone would hand a `.py` file beside the Rust code a Cargo
package.

**`calls_api` is deliberately empty for Python.** The Rust watchlist works
because `unwrap`/`expect`/`panic!` are a closed, idiomatic set of
panic-introducing calls. Python has no trustworthy equivalent — `open`,
`int`, and dictionary access all raise, and flagging them would bury the
signal. Inventing one would undermine the precision gate that earns these
rules the right to block. The consequence is that the untested-risky-call rule
does not fire on Python; of the two shipped structural rules, only
`warn-import-cycle` can fire in a Python project. `warn-untested-risky-call`
is Rust-only.

Adding Python needed no `GRAPH_FORMAT` bump. Rust identities are unchanged,
and `.py` files were previously untracked, so they are absent from the index
and the ordinary drift path reports them — warn, then rebuild.

#### 4.2.4 TypeScript identity and measured corpus

A TypeScript entity is `typescript:<package>::<module path>`, with no target
suffix. The nearest `package.json` supplies the package name; unclaimed files
fall back to `typescript:project`. Module paths are relative to
`compilerOptions.baseUrl` when configured, otherwise to the package root.
Relative imports probe TypeScript extensions and `index` modules; non-relative
imports try `paths` aliases and then `baseUrl`. Third-party imports produce no
edge. An unresolved relative or cross-unit import increments `skipped` rather
than disappearing silently.

The real-project merge gate used tough-cookie at commit `e8c27e3`: 47 tracked
TypeScript files and 11,296 lines. A rebuild completed in 3.3 seconds wall time
and produced 1,221 base edges, 54 derived edges, and **0 skipped items**. The
base graph included 120 function definitions, 899 direct test-call edges, 105
resolved imports, and 3 non-null-assertion watchlist edges. Derivation found
50 untested functions and four `in_cycle` members in one genuine cycle:
`cookie::cookieJar`, `cookie::index`, `memstore`, and `store`. Inspection of
their import edges confirmed the cycle rather than merely trusting the SCC
output.

TypeScript's `calls_api` watchlist contains only the non-null assertion `!`.
Unlike Rust's panic-at-call-site APIs, it means "this function makes an
unchecked type assumption"; it does not claim the failure occurs at that
source location. Both structural warning rules can therefore fire for
TypeScript, while Python remains import-cycle-only.

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

Parse failure preserves the file's previous edges and leaves its old content
hash in the index, which makes freshness report that file as drifted. The
extractor never emits partial edge sets for a file it could not fully parse.
Treating "could not parse" as an empty successful extraction would erase
trusted structure and then certify the resulting graph as fresh.

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
        { "predicate": "edited_file", "args": ["?file"] },
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
    { "predicate": "edited_file", "args": ["?file"] },
    { "predicate": "declares_module", "args": ["?file", "?module"] },
    { "predicate": "in_cycle", "args": ["?module", "?cycle"] }
  ],
  "then": {
    "action_type": "warn",
    "params": ["Graph node `?module` participates in static dependency cycle `?cycle`."]
  }
}
```

### 5.3 Warn before block

All Phase-One structural rules ship as `warn`. Promotion to `constraint_violation` requires a measured false-positive rate from real use (§8, task 6). Blocking on a heuristic that has never been measured is how an enforcement layer loses the user's trust permanently.

### 5.4 Freshness is harness state, never a fact

**Only facts about the code are asserted.** Working memory holds the closed relation set of §1.2 and nothing else. Whether the graph is currently trustworthy is a property of the *enforcement machinery*, not of the codebase, and it does not belong in the fact base.

An earlier revision proposed asserting `graph_fresh(true|false)` and requiring every blocking rule to carry a `graph_fresh true` condition. **That design is rejected.** It conflates two different kinds of statement — "this function is untested" describes the world; "my index is current" describes the tool — and it taxes every rule author with boilerplate about machinery they did not ask to know about. A fact base that mixes domain facts with self-diagnostics stops being a model of the codebase.

Instead, `hydrate` *returns* freshness to the caller along with the set of rule ids that read graph relations. When the graph has drifted, the hook demotes those rules' violations to warnings (`hook_logged::demote_violations_from`) before deciding the exit code. The rule is untouched and still says `block`; the harness declines to act on a verdict whose evidence it cannot vouch for, and says so on stderr.

The same seam generalizes: any future evidence source that can go stale gets a downgrade, without inventing a fact per source.

### 5.5 Demand-gated hydration

The graph is loaded only if some rule in `rules.json` names one of the relations in §1.2. A project with no structural rules performs no file I/O and asserts no facts (measured at 6.7 ns, §2.3). This is what makes the feature safe to ship enabled.

### 5.7 Every structural rule must be scoped to the edited file

Graph relations describe the **whole repository**. A rule written over them alone therefore matches repo-wide state and fires on *every* tool call, regardless of what is being edited — the same warnings, every time.

This was found by running the shipped pack against a real project, not by reading the spec: earlier drafts of §5.1 and §5.2 both had this flaw. On this repository the unscoped pair would have emitted 6 untested-call warnings plus 4 cycle warnings on every single edit, none of them about the file in front of the user. That is not a tuning problem; it is the fastest possible way to get a pack switched off.

**Every structural rule opens with `edited_file`:**

```json
{ "when": [
    { "edited_file": "?file" },
    { "defines_fn": ["?file", "?func"] },
    { "untested": ["?func"] }
] }
```

For module-keyed relations, `declares_module` bridges file to module:

```json
{ "when": [
    { "edited_file": "?file" },
    { "declares_module": ["?file", "?module"] },
    { "in_cycle": ["?module", "?cycle"] }
] }
```

`edited_file` exists because the pre-existing `file_path` fact cannot serve this purpose: hosts send **absolute** paths while the graph is keyed **repo-relative**, so joining the two silently never matches — a failure that looks exactly like "no violations found". `hydrate` normalizes the host's path into the graph's form, and drops paths outside the project.

### 4.7 Relative import anchors

`use super::` accounts for roughly 40% of this repository's intra-crate use statements. An earlier revision recorded only `crate::`-anchored paths, which silently dropped them: fan-in was understated, orphan-module counts were meaningless, and — most seriously — **cycle detection could not see any cycle formed through a relative import**. That is a recall failure, invisible by construction, in one of the two shipped rules.

Anchors now resolve against the module the statement is *written in*, not the file:

* `crate::a::b::Item` → `crate::a::b`
* `super::x::Item` from `crate::a::b` → `crate::a::x`
* `self::x` from `crate::a::b` → `crate::a::b::x`
* repeated `super::` climbs one level each; climbing past `crate` yields no edge

Resolving against the enclosing scope rather than the file matters for the single most common use statement in Rust: `#[cfg(test)] mod tests { use super::*; }`. Against the file it would invent an edge to the file's *parent*; against the scope it correctly reduces to a self-import and is dropped.

Effect on this repository: `imports` rose from 99 to 138 edges (+39%). Cycle count was unchanged at 2, which retroactively confirms the earlier finding was complete — but that was luck, not design.

---

### 5.9 Whole-tree audit

`phr-mcp audit` sweeps the repository rather than one edit. Graph rules are invisible to its file-scanning loop — their conditions join relations across the whole project instead of matching text in one file — so they are evaluated separately and merged.

The evaluation is one line of insight: **assert every file as the `edited_file`, then fire once.** A hook asks "is there a problem in this file?"; an audit asks the same question with that binding freed.

Critically, this runs the *real* network with the *same* rules. A bespoke matcher for audit would be a second implementation of joins and variable binding, free to drift — and a rule that blocks at the hook while reporting clean in the audit is worse than having no audit. Reusing the engine makes disagreement impossible by construction.

Structural rules therefore set `audit: true`. Findings carry no line number (the graph records *that* a function is untested, not where), so they use the line-1 placeholder plus detail string that AST hits already use. The detail is the bound entities, not the rule's prose: audit lists many hits per rule, and repeating a paragraph of guidance for each buries the only part that differs.

### 5.8 Evaluation pipeline

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

* Languages beyond Rust, Python, and TypeScript. Python remains
  import-cycle-only; TypeScript supports both warnings through its narrow `!`
  watchlist (§4.2.4).
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

## 9. Phase Two: First-Class Elements and Grounded Evidence

Phase One answers structural questions over relations. Phase Two makes the
things at the ends of those relations independently addressable and joins
them with evidence of actions that actually happened. The graph remains the
durable structural model; the journey journal remains the durable observation
log; RETE combines fresh snapshots of both and emits consequences.

This phase does not add a new engine primitive. Element declarations,
relations, observations, and evaluation-local conclusions all use the existing
`Fact { predicate, args }` shape.

### 9.1 Element predicates and containment

Elements are positive unary facts:

```text
graph_file("crates/phronesis/src/network.rs")
graph_module("rust:phronesis::network")
graph_function("rust:phronesis::network::fire")
graph_test("rust:phronesis::network::tests::fire_works")
graph_definition("cue:example.com/mod@v0::schema::#Request")
```

Dedicated predicates are intentional. A generic
`element(id, kind)` predicate would put every code entity in one alpha-memory
bucket and give up the predicate index described in §1.1.

Classifications may overlap. Every `graph_test(T)` also has
`graph_function(T)` because a test is a function with an additional role.
`graph_function` therefore means "callable function or method"; rules that
mean production callable add a
`file_type(File, "production")` join. Future kinds such as `graph_class` and
`graph_type` follow the same additive rule rather than creating a disjoint
type hierarchy.

`graph_definition` is the shared kind for stable, named, non-callable
definitions. It covers CUE definitions, JSON/YAML schema resources and anchors,
and Helm named templates. It excludes arbitrary object keys and fields:
extractors emit it only where the hosted language assigns a stable addressable
identity. `defines(File, Definition)` is its declaration edge, parallel to
`defines_fn` for callables. Language-specific attributes such as "open CUE
struct" remain syntax facts unless separately admitted to the closed set.

Containment is explicit:

```text
element_in_file(Element, File)
element_in_module(Element, Module)
```

`defines_fn(File, Function)` and `declares_module(File, Module)` remain in the
v1 vocabulary for compatibility and relation-specific queries. The extractor
emits the corresponding generic containment facts from the same AST node; the
two representations must agree in contract tests.

The identities are exactly those of §4.2. Files remain normalized,
repo-relative paths. Modules, functions, and tests carry their
language-specific unit prefix. No display name, line number, or short test
name is an identity.
Renaming an element is deletion plus addition; Phase Two does not attempt
heuristic rename detection.

An otherwise-empty parseable source file still emits `graph_file(File)`.
Likewise, a module or test with no qualifying relation remains queryable
through its unary predicate. Every element and containment fact is a base edge
whose `src` is the file that declared the element.

### 9.2 Fact classes and their authority

The predicate name alone does not imply how a fact became true. Phase Two
defines four classes with distinct persistence:

| Class | Examples | Authority | Persistence |
|---|---|---|---|
| Extracted structure | `graph_function`, `tested_by`, `element_in_file` | AST sensor | `graph.jsonl`, provenance-keyed |
| Graph-derived structure | `untested`, `in_cycle` | Pure computation over the complete graph | Regenerated in `graph.jsonl`, `d: true` |
| Observed evidence | `test_executed`, `test_result`, `run_state` | Post-hook outcome adapter parsing an executed command | Journey journal |
| Evaluation-local inference | `introduced_element`, `verified_at_state`, `verification_missing` | Host derivation over graph + baseline + evidence + current state | Fresh network only |

Only the observation adapter may assert that a test executed or passed.
Neither the presence of `graph_test(T)` nor the structural edge
`tested_by(F, T)` is evidence that `T` ran. Absence of failure evidence is
`unknown`, never `pass`, following `SPEC-confidence-scoring.md`.

RETE RHS actions continue to emit consequences; they do not persist inferred
graph edges and do not forward-chain. Conclusions needed by an LHS are
precomputed by a pure host derivation before `update_agenda`, just as
`untested`, `in_cycle`, journey aggregators, and confidence signals are today.

### 9.3 Incremental save lifecycle

On every successful source-file PostToolUse/save:

1. Parse the saved file completely.
2. Produce its complete base subgraph: unary elements, containment, and
   language relations.
3. If parsing failed, preserve the previous subgraph and index hash; freshness
   becomes false for that file.
4. Otherwise remove every base edge with `src == saved_file`, insert the new
   subgraph, and record the new file hash.
5. Recompute all whole-graph derived edges.
6. Compute the graph state id (§9.5).
7. Atomically replace `graph.jsonl` and its index.

Deleting a file removes every base edge with that file's provenance, removes
its index entry, and reruns derivation. A test file owns the `tested_by` edges
inferred from its test bodies, even when their subject is a function declared
elsewhere; saving or deleting the test therefore replaces that evidence
correctly.

This is subgraph replacement, not an append-only graph. It makes removal and
rename semantics deterministic without a truth-maintenance system.

### 9.4 “Introduced” means relative to the open work-unit baseline

Transient comparison with the immediately previous save is useful for
diagnostics but is not the meaning of "new in this change": after a second
save, the element would incorrectly stop being new.

The normative baseline is the structural graph state captured when the
current outcomes/journey subject opens. The pre-hook that observes the first
mutating call must open the subject and snapshot the graph *before* that call
executes; capturing it at post-hook time would already include the first new
element.

The snapshot is a compact, sorted set of unary element identities plus the
baseline state id, stored under
`.phronesis/outcomes/baselines/<subject>.json`. It does not duplicate
relations: introduction is an identity-set comparison, while current
containment and test relationships come from the current graph. Subject
creation and baseline writing are one lock-serialized operation; a subject is
not considered open until its baseline has been atomically renamed into place.

Host derivation emits:

```text
introduced_element(Subject, Element)
introduced_file(Subject, File)
introduced_function(Subject, Function)
introduced_module(Subject, Module)
introduced_test(Subject, Test)
```

Specific predicates are conveniences derived from the unary element kind plus
`introduced_element`; they are not separately extracted. An element is
introduced when its stable identity is present in the current fresh graph and
absent from the baseline graph. A rename is therefore one removed identity
and one introduced identity.

If no subject is open, the baseline cannot be loaded, or either graph is stale, no
`introduced_*` fact is asserted. Rules depending on introduction are
downgraded through the same harness-health seam as stale structural rules;
missing baseline evidence must not become a blocking negative claim.

### 9.5 Structural state identity

Passing evidence is valid only for the source state against which the command
ran. Phase Two uses a conservative project-wide structural state id:

```text
state_id = sha256(
    GRAPH_FORMAT || "\n" ||
    sorted(repo_relative_path || US || content_hash || "\n")
)
```

The file hashes are the same hashes maintained by `graph.index`; `US` is the
U+001F separator already used in stable graph fact identities. The state id is
available only when the index is fresh. This deliberately invalidates all
structural verification after any tracked source file changes. Per-element
dependency fingerprints may narrow invalidation later, but cannot replace
this conservative contract without a spec revision.

`current_state(State)` is invocation-local machinery asserted only for the
evidence join. Unlike the rejected `graph_fresh` condition (§5.4), rule
authors do not use it to decide whether stale machinery is acceptable:
freshness demotion remains the harness's responsibility.

### 9.6 Journey evidence schema

The journey journal remains one record per executed tool call. Its next schema
version adds optional `run`, `state`, and `atoms` fields:

```json
{
  "v": 2,
  "ts": 1718700000,
  "sid": "s-2026-06-18-a1b2",
  "seq": 4137,
  "tool": "Bash",
  "tags": ["test", "outcome:test_pass"],
  "subject": "auth-fix-3",
  "run": "s-2026-06-18-a1b2:4137",
  "state": "sha256:…",
  "atoms": [
    {"p": "test_result", "a": ["rust:phronesis::network::tests::fire_works", "pass"]}
  ]
}
```

`run` is deterministically `<sid>:<seq>`. `state` is the fresh graph state
observed when the command completes. If graph state is unavailable, the event
may still carry aggregate outcome tags, but it cannot verify an element.

`atoms` revives the deliberately deferred atom seam in
`SPEC-journey-facts.md` for this concrete consumer. Adapters emit only
normalized, bounded facts:

```text
test_result(Test, "pass" | "fail")
```

The derivation expands a record into:

```text
test_executed(Run, Test)
test_result(Run, Test, Status)
run_state(Run, State)
run_subject(Run, Subject)
```

The on-disk atom omits `Run` because the containing record supplies it.
Aggregate summaries remain `test_outcome`; they prove that a test command ran
but cannot prove that a particular graph test passed.

Adapters must resolve runner names to the exact graph test identity from
§9.1. Rust adapters account for package/target identity; pytest adapters map
node ids to Python module/function identities. Ambiguous, truncated, filtered,
or unresolvable names produce no element-level atom. Short-name matching is
acceptable for the warning-oriented structural `tested_by` heuristic (§4.4)
but is forbidden for passing evidence.

To bound journal growth, an adapter records at most the configured atom cap
per run and marks truncation in the record. A truncated run can verify the
tests whose atoms are present; it makes no claim about omitted tests.

### 9.7 Joining structure to observed evidence

Before agenda construction, a pure derivation joins exact identities:

```text
tested_by(Function, Test)
test_result(Run, Test, "pass")
run_state(Run, State)
current_state(State)
    => verified_at_state(Function, Test, Run, State)
```

The arrow describes host derivation, not RETE forward chaining. The resulting
fact is asserted into the fresh network so ordinary rules can consume it.

A rule governing introduced functions can then be flat:

```json
{
  "id": "introduced-function-needs-current-passing-test",
  "phase": "pre",
  "conditions": [
    {"predicate": "current_subject", "args": ["?subject"]},
    {"predicate": "introduced_function", "args": ["?subject", "?function"]},
    {"predicate": "element_in_file", "args": ["?function", "?file"]},
    {"predicate": "edited_file", "args": ["?file"]},
    {"predicate": "verification_missing", "args": ["?function"]}
  ],
  "actions": [
    {
      "action_type": "constraint_violation",
      "params": ["Introduced function ?function has no observed passing test for the current source state."]
    }
  ]
}
```

Because the engine has no negation-as-failure, the host derivation emits
`verification_missing(Function)` by set difference over the in-scope
introduced functions and `verified_at_state` facts. It emits
`verification_stale(Function, Run)` only when a linked passing run exists at a
different state; stale is explanatory evidence, not a substitute for
`verification_missing`.

File and module verification roll up from their contained introduced
functions:

* `verified_file(File)` iff every in-scope introduced function in `File` has a
  `verified_at_state` fact.
* `verified_module(Module)` iff every in-scope introduced function in `Module`
  has a `verified_at_state` fact.
* A newly introduced empty file/module is not silently considered tested.
  Projects must choose an explicit policy rule (warn, exempt, or require a
  module-level smoke test).

These are evidence claims, not correctness claims. They mean an exactly
identified linked test was observed passing at the current structural state;
they do not establish behavioral adequacy or runtime coverage of every path.

### 9.8 Cross-store hydration and freshness

The hook evaluation order is normative:

1. Load rules and determine demanded graph, journey, and evidence predicates.
2. Hydrate the fresh structural relations needed by those rules.
3. Load the open subject and its baseline.
4. Read the bounded relevant journey suffix and expand evidence atoms.
5. Assert `current_subject` and the current structural state.
6. Host-derive introduction, verification, and missing/stale facts.
7. Assert those facts, call `update_agenda`, and fire once.
8. Discard the network and all evaluation-local facts.

Demand gating remains predicate-driven: projects with no element/evidence
rules do not read the additional journey data or compute baselines.

Each source reports freshness separately. A blocking consequence whose bound
facts depend on a stale graph, missing baseline, truncated evidence for the
claimed test, or unavailable current state is demoted to a warning. Positive
evidence facts are never synthesized to make evaluation proceed.

### 9.9 Phase-Two implementation requirements and acceptance tests

1. Extend the closed relation set, hydration allowlist, graph query inventory,
   Rust extractor, and Python extractor with unary elements and containment.
2. Preserve the existing `Edge` JSONL shape; the new graph predicates require
   a `GRAPH_FORMAT` bump because full rebuild is required to populate them for
   previously indexed files.
3. Add pre-mutation, atomic baseline capture to the open-subject lifecycle and
   deterministic introduction derivation.
4. Add graph state-id computation over the freshness index.
5. Version the journey record and add bounded outcome atoms without changing
   the existing tag aggregators' behavior.
6. Normalize exact per-test identities in Cargo and pytest adapters; unresolved
   names remain aggregate-only evidence.
7. Add demand-gated cross-store derivation before agenda construction.
8. Ship new enforcement rules as warnings until false-positive and
   false-negative behavior is measured on at least two corpora.

Acceptance tests must prove:

* An isolated file, module, function, and test remains queryable.
* Saving a file removes deleted/renamed elements and inserts new elements
  without reparsing unrelated files.
* Parse failure preserves prior edges and makes graph-dependent blocking
  verdicts fail open.
* A function remains introduced across repeated saves in the same subject.
* A directly linked exact-name test passing at the current state verifies its
  function.
* An aggregate passing summary, unresolved test name, failed test, stale-state
  pass, or missing state does not verify a function.
* Editing any tracked source file invalidates project-wide verification until
  a new matching run is observed.
* Saving/deleting a test replaces its cross-file `tested_by` edges.
* File/module rollups require all introduced contained functions and do not
  vacuously verify empty elements.
* Replaying identical graph bytes, baseline, journal bytes, and invocation
  timestamp produces identical facts and consequences.
* Projects whose rules mention no Phase-Two predicate do no Phase-Two I/O.

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

## 10.1 Query surface

`phr-mcp graph query <relation> [arg...]` and the `query_code_graph` MCP tool expose the graph directly. The pattern language mirrors the fact shape — relation plus positional arguments — rather than inventing a vocabulary of named questions: one concept to learn, and it composes with any relation added later instead of needing a new verb per question.

Ordinary tokens match exactly. A token consisting solely of `*` or `?`
remains a whole-position wildcard for compatibility. Within any relation or
argument token, `*` matches zero or more characters and `?` matches exactly
one character; all other characters are literal. CLI and MCP use the same
matcher, limit, and unlimited-count semantics. Thus
`defines '*' '*rete_rules*'` keeps the first argument unconstrained while
selecting definition identities containing `rete_rules`.

Both always report the unlimited total alongside a truncated result set. A capped list that reported only its own length would read as a complete answer.

Omitting the relation returns the relation inventory with edge counts, so the vocabulary is discoverable without the spec.

---

## 11. Revision History

**Revision 8** — corrected CUE identities and added data seams:

* Required graph format 8 for package-shaped CUE nodes, the shared
  generated-artifact relation set, forward Config → consumer navigation, and
  lifecycle-gap evidence.
* Specified exact `.phronesis/graph.toml` bindings and bounded inference.
* Added compatible embedded-glob query semantics shared by CLI and MCP.

**Revision 7** — added the TypeScript extractor:

* Defined package-qualified TypeScript identities and filesystem-backed
  resolution for relative imports, `baseUrl`, and `paths` aliases (§4.2.4).
* Added the narrow non-null-assertion watchlist and kept its claim distinct
  from Rust's panic-at-call-site watchlist.
* Recorded the required tough-cookie real-corpus validation: 1,221 base, 54
  derived, 0 skipped, with 105 resolved imports and one confirmed cycle.

**Revision 6** — specified first-class elements and grounded evidence:

* Added unary file/module/function/test predicates plus generic containment,
  preserving per-predicate alpha indexing and isolated elements (§9.1).
* Separated extracted, graph-derived, observed, and evaluation-local facts;
  only post-hook outcome adapters may claim execution or success (§9.2).
* Made the open work-unit graph the normative baseline for
  `introduced_*` facts and defined conservative fallback/fail-open behavior
  (§9.4).
* Bound per-test journey evidence to an exact graph identity and a
  project-wide structural state id; stale or ambiguous evidence cannot verify
  an element (§9.5–§9.7).
* Defined cross-store hydration order, demand gating, derivation ownership,
  freshness demotion, format migration, and acceptance tests (§9.8–§9.9).
* Corrected §4.6 to match the implemented safe behavior: parse failure
  preserves prior edges and leaves the graph stale rather than erasing the
  file's subgraph.

**Revision 5** — added the Python extractor:

* Defined distribution-qualified Python identities and `::` normalization so
  Python facts join the language-neutral graph vocabulary (§4.2.3).
* Kept `calls_api` Rust-only rather than inventing a noisy Python watchlist.
* Required language-aware unit resolution in mixed Cargo/Python repositories.

**Revision 4** — made identity compilation-unit-aware:

* Qualified Rust identities by package and target so workspace crates,
  integration tests, examples, and benches cannot collapse into one node.
* Defined target-root-relative module paths and a graph-format migration for
  identities generated by the earlier scheme (§4.2).

**Revision 3** — Phase One implemented (`crates/phronesis-mcp/src/graph/`), 92 unit tests:

* Recorded measured latency for both hook paths and made hydration demand-gated (§2.3, §5.5).
* Corrected §4.4's error-direction claim: short-name matching over-approximates coverage, so the heuristic errs toward silence, not toward false accusation. The earlier text claimed the opposite.
* Kept machinery health out of working memory: staleness is returned to the harness, which demotes affected rules' violations to warnings. An intermediate draft asserted a `graph_fresh` fact and was rejected — the fact base models the codebase, not the tool's self-diagnostics (§5.4).
* Measured the first corpus; `untested` alone is too noisy for a rule of its own (§10).
* Added `edited_file` + `declares_module` and required every structural rule to scope to the edited file. Found by running the shipped pack, not by review: the §5.1/§5.2 rules as written matched repo-wide state and fired on every tool call (§5.7).
* Shipped the `structural` pack (`phr-mcp init --packs structural`) with both measured rules, warn-only, and made them auditable — `phr-mcp audit` sweeps the whole tree by asserting every file as the edited file and firing the real network once (§5.9). `init` builds the graph itself: without it the pack installs silent, and a rule that cannot fire is indistinguishable from one that found nothing. The build is non-fatal — writing config is what `init` is for, and a missing graph is recoverable with `graph rebuild` while a failed `init` leaves no enforcement at all.
* Fixed `use super::`/`self::` resolution — 40% of intra-crate imports were being dropped, leaving cycle detection blind to any cycle formed through a relative import (§4.7).
* Added a query surface: `phr-mcp graph query` and the `query_code_graph` MCP tool (§10.1).
* Deleted the `provenance` row from §1.2 — provenance is the `src` field on each edge, never a relation.

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
