# SPEC: Dynamic coverage evidence and change-relevant test selection

**Status:** Draft for review — not approved for implementation
**Author:** awaterma (agent-drafted, adapted from `REQUIREMENTS-phronesis-coverage-verification-rust-sketch.md`, which was co-authored with ChatGPT/Codex)
**Created:** 2026-09-23
**Scope:** `crates/phronesis-mcp` host only — **no engine changes**
**Derives from:** `REQUIREMENTS-phronesis-coverage-verification-rust-sketch.md` §1–§9, §11–§12
**Companions:** `SPEC-property-ontology.md` (B), `SPEC-verification-artifact-generation.md` (C)
**Reuses:** `SPEC-triple-store-rete.md` (graph discipline, demand-gated hydration, drift demotion), `SPEC-fact-provenance.md` (fact source attribution), `SPEC-rust-ownership-evidence.md` (stable site identity), `SPEC-confidence-scoring.md` (signal gates)

## Summary

Add a durable, opt-in **coverage evidence store** recording which test executed which code region, at which revision, at which granularity — then hydrate a bounded subset of those edges into the RETE network at hook fire and join them against derived change facts, so rules can answer:

> Which tests exercise the code I changed, and where is the evidence missing?

This is the **dynamic complement to the structural graph**. `SPEC-triple-store-rete.md` already ships static edges — `tested_by`, `test_reaches`, `no_direct_test` — what a test *could* reach. This spec adds what tests *actually executed*, keyed to the revision that produced the observation.

As with the structural graph, **the centerpiece is the importer and the store, not RETE plumbing**. Relational matching over `Fact { predicate, args }` with `?var` binding and beta joins already exists in `crates/phronesis` and is proven in production (`warn-untested-risky-call` joins five conditions on shared variables). Everything here reuses it.

## Problem

Today a rule can say "no test calls this function" (host-derived `no_direct_test`), but nothing can say "no test has ever executed the branch you just changed." An agent editing a branch has no cheap way to ask "which tests provide evidence for this behavior?" — the only answer available is "re-run everything."

The requirements sketch's answer — per-test coverage edges joined against change facts, with evidence-gap detection instead of coverage percentages — is correct. Its *mechanics* need translation to what the engine actually supports (§3).

## Goals

1. **Zero-token, zero-LLM** reasoning about dynamic coverage at the hook boundary.
2. **Evidence-directed test selection**: a query answering "which tests exercise what I changed," with per-test provenance.
3. **Evidence-gap statements a human can act on** ("the changed behavior has neither dynamic test evidence nor formal proof evidence"), not coverage percentages.
4. **Bounded footprint**: store proportional to code under test; hydrated facts bounded by the current change, not the store.
5. **No engine changes**: reuse the `Fact` shape, `?var` beta joins, demand-gated hydration, and the hook-time environment-fact producer pattern (`clock_facts.rs`).

## Non-goals

1. Not a coverage dashboard. No percentage targets.
2. Not a replacement for the full test suite. Selection only; the sketch §8 policy tiers (low risk → minimal set, release candidate → full suite) remain **project policy**, not engine behavior.
3. No engine negation node. Gap detection is host-derived closed-world reasoning (§7).
4. No set-valued rule heads. Selection is a host query (§8).
5. No artifact generation or verifier execution (`SPEC-verification-artifact-generation.md`).
6. No property ontology beyond one formal-evidence lookup seam (`SPEC-property-ontology.md`).
7. Rust first. JaCoCo / coverage.py adapters are Phase 3.

## 1. Engine feasibility — what this spec builds on, and what it refuses

Audited against `crates/phronesis` on 2026-09-23:

| Sketch mechanism | Engine reality | Design here |
|---|---|---|
| Relational facts `test_hits_region(Test, Region)` | `Fact { predicate, args: Vec<String> }` — flat positional strings only (`crates/phronesis/src/engine_types.rs`); no named/typed fields | Region is a minted ID string (§4.2); line numbers are payload, never identity |
| Join `changed_region ⋈ test_hits_region` (sketch §5–§6) | `?var` equi-join via beta network; proven by production rules | Used as-is (§6.1) |
| `AND NOT exists test_hits_region(Test, Region)` (sketch §7) | No pattern-level negation. `__script__` guards (`facts_count(...) == 0`, used by `nudge-verify-before-commit`) are per-activation and do not react to later assertions | Host-derived closed-world facts (§7), the `no_direct_test` precedent (`graph/derive.rs`) |
| `minimal_relevant_test_set(Change, tests=[...])` (sketch §8) | No aggregation. Firings produce per-match consequences; `journey_count`/`journey_distinct` are computed host-side | Host-side union query (§8). "Minimal" is dropped: the sketch computes a **union**, not a hitting-set optimization |
| Facts persisting across runs | Working memory is in-memory; the hook builds a fresh network every fire; `MAX_FACTS = 100_000` (`security.rs`) | Durable on-disk store + demand-gated, change-scoped hydration (§4.1, §5) |
| `stale because dependent code changed` (sketch §9) | The engine cannot order revision SHAs; `detect_commit` knows HEAD at hook time but asserts nothing | Host-side revision comparison; `head_revision` / `coverage_revision` facts + `coverage_stale` marker (§5) |

## 2. Module layout

New module `crates/phronesis-mcp/src/coverage/` mirroring `graph/`:

- `store.rs` — the JSONL store and index (§4.1)
- `import.rs` — normalized-export parsing and validation (§9)
- `hydrate.rs` — demand-gated, change-scoped fact assertion (§5)
- `region_map.rs` — diff hunks → region/function change facts (Phase 2)
- `gap.rs` — closed-world evidence-gap resolution (§7)

## 3. Data representation

### 3.1 Store

`.phronesis/coverage.jsonl` — **derived, gitignored, rebuildable** by re-running the covered suite. Same discipline as `graph.jsonl` (see `SPEC-triple-store-rete.md` §"Note"): no auditability guarantee; git-auditability belongs to version-controlled inputs.

Companion index `.phronesis/coverage.index`:

```json
{ "format": 1, "revision": "<40-hex>", "imported_at": 1715717111, "tool": "cargo-llvm-cov",
  "records_fnv1a64": "<16-hex>", "record_count": 3 }
```

The store holds **only the latest imported revision** (import replaces prior content; idempotent per revision). Historical outcomes are the journey journal's job, not the store's.

**Integrity (commit marker).** Import writes the records file, then the index, each by atomic rename. The index is the commit marker: it records the FNV-1a 64 digest and the count of the exact records bytes it belongs to. Every read recomputes both and also requires each record's `revision` and `tool` to equal the index's. A crash between the two renames (new records behind the old index — whose revision may still equal HEAD) therefore reads as **corrupt**, never as fresh evidence for the old revision; records without an index, an index without records, and an index predating the digest fields are corrupt too. The digest guards against torn or mismatched writes, not tampering (whoever can rewrite one file can rewrite both). Import holds an exclusive advisory lock (`.phronesis/coverage.lock`, flock — released on process exit) across both renames and readers hold it shared across both reads, so a hook firing during an import sees the old store or the new one, never a transient `digest_mismatch`; if the lock cannot be taken (read-only checkout), a read whose index changed while it ran is retried.

**Store states.** A reader sees exactly one of: *missing* (neither file — nothing imported), *loaded* (verified), or *corrupt* with a stable reason code: `index_unreadable`, `records_unreadable`, `missing_index`, `missing_records`, `unverifiable_index`, `unsupported_format`, `invalid_index`, `digest_mismatch`, `count_mismatch`, `invalid_record`, `revision_mismatch`, `tool_mismatch`. Consumers treat *corrupt* as "no evidence" and say so (§4 `store_corrupt`, §7).

### 3.2 Region identity

Region identity is **not** `(file, start_line, end_line)` — raw line numbers drift across edits, and the sketch's §4 change (error-message text swap) would silently break region joins. Identity anchors to structural graph elements:

Every region id names exactly one code site within a revision. A leaf function name is not enough: the same `new` exists in many files, and `if x == 0` can appear twice in one function; joining on either would let a hit on one site stand as evidence for, or select tests of, another.

Grammar (`coverage/region_map.rs` is the single implementation; every id stays within the importer's identifier charset `[A-Za-z0-9_:./-]` and ≤256 bytes):

```text
fn-id      = "fn:" file "::" item-path
branch-id  = "branch:" file "::" item-path ":" anchor [ "." ordinal ]
file       = repo-relative path; a char outside [A-Za-z0-9_./-] becomes "_"
             and the segment gains ".h" + 12 hex of FNV-1a(original path)
item-path  = segment *( "::" segment )      ; outermost scope first
segment    = name                            ; mod, trait, or fn name
           | type [ ".as." trait ]           ; impl block (inherent / trait impl)
           | "_"                             ; branch outside any fn
anchor     = 12 hex of FNV-1a over the whitespace-normalized condition text
```

- `name`, `type`, and `trait` are the source text with whitespace removed, `::` rewritten to `.`, and every other character outside `[A-Za-z0-9_]` — generic brackets, `&`, `'`, `,`, non-ASCII — rewritten to `-`. Generics stay in the id, so `impl From<u8> for X` and `impl From<u16> for X` give `X.as.From-u8-::from` and `X.as.From-u16-::from`.
- The file is the repo-relative path rather than a Rust module path: it is unique per file where a module path is not (`lib.rs` and `main.rs` are both crate roots; `src/bin/*.rs` are separate crates), and every producer already holds it.
- Function ordinal: the encoding above is not injective, and cfg variants legitimately repeat an item path in one file, so a repeated item path gets `.2`, `.3`, … on its last segment in source order (`f`, `f.2`).
- Branch ordinal: sites with the same enclosing function and the same anchor are numbered in source order; the first carries no suffix, later ones `.2`, `.3`, …. Whitespace-only reflow moves neither the anchor nor the order.
- Known limitation: both ordinals follow source order, so inserting a same-path item (a new `#[cfg(...)] fn f` variant) or a same-condition branch *above* an existing one renumbers the later sites — their ids shift, and evidence for them reads as absent until the next collect. The cfg predicate is not folded into the segment.
- Each segment is capped on its own, so an id stays ≤256 bytes and never loses its `::` (a capped id is still qualified, still imports, and still carries its file qualification): a `file` over 120 bytes keeps its first 106 bytes and appends `.h` + 12 hex of FNV-1a(path); an `item-path` over 100 bytes becomes `_h` + 12 hex of FNV-1a(item path) + `::` + its last segment (the fn name and ordinal, dropped as well only when it alone would not fit).
- Example: the §1 fixture's zero-denominator branch is `branch:src/lib.rs::safe_divide:cd6054b02dde`; its function is `fn:src/lib.rs::safe_divide`.
- Line spans ride along as display payload in the store record only.

**Older stores must be re-collected.** Versions before per-site ids wrote leaf-name ids (`fn:new`, `branch:safe_divide:cd6054b02dde`). The importer rejects them, and also rejects an id that disagrees with its record's `hit_kind` or is qualified with a different file than the record's. In practice a store written before this release also lacks the index records digest (§3.1) and reads as `store_corrupt(coverage, unverifiable_index)` first. A digest-valid store with leaf-name ids (possible only from a hand-built or intermediate writer) is treated as stale — the shared read validator deliberately checks only the `hit_kind` prefix, not the qualified grammar, so such a store is stale evidence, not corruption: hydration asserts `coverage_stale` (whatever the revision), never asserts `test_hits_region`/`test_hits_branch` for those hits, and never lets them suppress `region_without_dynamic_evidence`; `coverage select` cannot match them and reports a `coverage_note` naming the remedy. Re-run `phr-mcp coverage collect`. A property store (`SPEC-property-ontology.md`) whose `depends_on` still lists leaf-name ids keeps working conservatively — a leaf-name reference matches every changed site with that leaf name (and anchor) — until it is rewritten with qualified ids.

The static half of `coverage select` pairs a graph function with a changed function region only when the graph's `defines_fn` places it in the region's file under the same name.

Precedent: ownership sites (`SPEC-rust-ownership-evidence.md`) record spans under stable, queryable IDs.

### 3.3 Normalized hit record

```json
{
  "v": 1,
  "kind": "hit",
  "test": "rejects_zero_denominator",
  "region": "branch:src/lib.rs::safe_divide:cd6054b02dde",
  "file": "src/lib.rs",
  "start_line": 3,
  "end_line": 5,
  "hit_kind": "branch",
  "revision": "<40-hex>",
  "tool": "cargo-llvm-cov"
}
```

`hit_kind` ∈ `region` | `branch`, and the region id must agree with it: `region` ⇒ `fn:…`, `branch` ⇒ `branch:…`. One validator (`store::validate_record`) runs on import **and** on every read, so a hand-edited store cannot carry a record the importer would have refused. On import, revisions are canonicalized to lowercase (git prints lowercase; an uppercase import would otherwise be permanently stale, and mixed-case spellings of one sha are one revision), exact duplicate records are collapsed before counting, and `tool` must be non-empty and identical across the export — the index records one tool and region identity is tool-specific, so a mixed-tool export is rejected rather than attributed to the first record's tool. Importer validation: test/region strings must survive the `security.rs` validators; `file` must be repo-relative in the graph's `file_rel` form so coverage facts join graph facts on paths (the join-key discipline `predicate_provider.rs` already documents); record and file sizes capped per `security.rs`.

### 3.4 Change identity

Hooks see the **working tree, not commits**. The `change` id minted at hook fire is `head:<short-sha>` — the revision the edit rides on. Commit correlation flows through the existing lifecycle `detect_commit` post-check recording (`lifecycle/outcome.rs`). The sketch's `commit_abc123` framing is commit-time; our joins run at edit-time. Stated, not hidden.

## 4. Fact vocabulary and producers

| Fact | Args | Producer | When |
|---|---|---|---|
| `test_hits_region` | `[test, region]` | coverage hydrator | hook fire; demand-gated + change-scoped |
| `test_hits_branch` | `[test, branch_site]` | coverage hydrator | hook fire; demand-gated + change-scoped |
| `changed_region` | `[change, region]` | region mapper (Phase 2) | hook fire, from diff hunks ⋈ graph spans |
| `changed_function` | `[change, function]` | region mapper | hook fire; any overlapped region ⇒ function changed (added/removed already covered by `diff_extract.rs`) |
| `region_without_dynamic_evidence` | `[region]` | gap resolver (host) | hook fire; changed regions only |
| `region_without_formal_evidence` | `[region]` | gap resolver (host) | hook fire; changed regions only (see §7) |
| `coverage_revision` | `[sha]` | coverage hydrator | hook fire, from store index |
| `head_revision` | `[sha]` | revision probe (new) | hook fire, `clock_facts` pattern |
| `coverage_stale` | `[]` (zero-arg presence) | coverage hydrator | hook fire, when the verified index revision ≠ HEAD, or the verified store holds leaf-name (pre-§3.2) ids |
| `store_corrupt` | `["coverage", reason]` | coverage hydrator | hook fire, when the store is corrupt (§3.1 reason codes); demand-gated |

Notes:

- **Demand-gated** exactly as graph hydration: a relation is asserted only when a loaded rule mentions it (`graph/hydrate.rs` precedent).
- **Change-scoped**: coverage facts assert only for regions/functions touched by the current event's edited files. Bounded per fire regardless of store size.
- **Stale evidence never closes a gap**: hits from a store whose revision differs from HEAD still assert `test_hits_region` / `test_hits_branch` (the join is useful, and `coverage_stale` marks it), but they never suppress `region_without_dynamic_evidence` — the store says nothing about the code at HEAD. Staleness is decided once (`store::is_stale`): an unknown HEAD (no git) cannot prove staleness, matching when `coverage_stale` asserts.
- **Corrupt store**: the hook prints a stderr warning and asserts `store_corrupt(coverage, <reason>)` when a rule mentions it (so a rule can warn or block on it); no hit, `coverage_revision`, or `coverage_stale` facts assert, and gap facts are derived as if the store were empty. The corrupt store no longer fails the whole hydration open. `coverage_stale` is not asserted for a corrupt store: it means "verified evidence at another revision"; corruption has its own fact.
- **Staleness**: mismatch between `coverage_revision` and `head_revision` asserts `coverage_stale` and **demotes enforcement** block→warn through the existing drift-demotion path (`hook_logged.rs`) — the same contract as graph drift. Rules may also match `coverage_stale` directly (§6.3).
- Every fact carries `Fact.source = "coverage"` / `"diff"` / `"git"` so `Provenance::RuleFiring.fact_sources` shows origin (`SPEC-fact-provenance.md`).

## 5. Rules (exact v2 on-disk format)

### 5.1 The relevant-test join — works today, unmodified

```json
{
  "id": "log-relevant-test-for-change",
  "phase": "post",
  "priority": 30,
  "audit": true,
  "when": [
    { "changed_region": ["?change", "?region"] },
    { "test_hits_region": ["?test", "?region"] }
  ],
  "then": { "log": "change ?change touches region ?region exercised by test ?test" }
}
```

This is sketch §5 verbatim, translated: `changed_region(?change, ?region) ⋈ test_hits_region(?test, ?region)`. The sketch's distinction `branch_relevant != function_relevant` falls out naturally — a test hits `fn:src/lib.rs::safe_divide` without hitting `branch:src/lib.rs::safe_divide:cd6054b02dde`.

### 5.2 The evidence gap — sketch §7, without engine negation

```json
{
  "id": "warn-evidence-gap",
  "phase": "post",
  "priority": 20,
  "audit": true,
  "when": [
    { "changed_region": ["?change", "?region"] },
    { "region_without_dynamic_evidence": ["?region"] },
    { "region_without_formal_evidence": ["?region"] }
  ],
  "then": { "warn": "changed region ?region has neither dynamic test evidence nor formal proof evidence" }
}
```

Both negative conditions are host-derived closed-world facts (§7). The message is the sketch's §7 risk statement, not a coverage delta.

### 5.3 Policy example — stale coverage gates a commit warning

```json
{
  "id": "warn-commit-on-stale-coverage",
  "phase": "pre",
  "priority": 20,
  "audit": true,
  "when": [
    { "bash_command_matches": "git (commit|merge|rebase|cherry-pick|revert|pull)" },
    { "coverage_stale": true }
  ],
  "then": { "warn": "committing with stale coverage evidence; run `phr-mcp coverage collect` to refresh" }
}
```

Mirrors the live `confidence-low-blocks-commit` gate shape. Pack placement (llm/rust/new `evidence` pack) is a review decision, not this spec's.

## 6. Host-derived closed-world facts (why not `__script__` guards)

The engine has no negation-as-failure at the pattern level (`graph/derive.rs` states this in its header). `__script__` guards (`facts_count('test_hits_region', ['?test','?region']) == 0`) exist but are evaluated **per activation** — they don't react to later assertions — and unscoped absence checks over a whole store are exactly the unbounded scan the demand-gating design forbids.

So gap detection follows the `no_direct_test` precedent: the resolver computes, **for changed regions only** (bounded by the change), whether any dynamic hit and any passing formal result cover the region, and asserts `region_without_dynamic_evidence` / `region_without_formal_evidence`. Until `SPEC-property-ontology.md` lands its results index, the resolver asserts the formal-absent fact unconditionally and only rule 5.2's dynamic half has teeth — the seam is explicit, not silent.

## 7. Selection — a host query, not a rule head

Sketch §8's `minimal_relevant_test_set` cannot be a rule: no set-valued derivation exists in the engine, and "minimal" overpromises (the sketch computes a union). Instead:

`phr-mcp coverage select [--change <id>]` (CLI first, like `stats`/`audit`; MCP tool `select_relevant_tests` in Phase 3):

- Union of tests hitting changed regions, plus tests statically reaching changed functions (`tested_by` / `test_reaches` edges), each entry labeled by evidence kind: `coverage_observation` vs `static_reach`. Hits from a stale store (index revision ≠ HEAD) are labeled `coverage_observation_stale` and listed in their own table section.
- A stale or corrupt store sets `coverage_note` (a table line and a `--json` key present only when set, so a fresh store's output is unchanged). A corrupt store contributes no dynamic entries and the empty-selection message names the corruption instead of "the coverage store is empty".
- Deduplicated; each entry carries its justifying regions (provenance), preserving the sketch's `affected_test != test_that_calls_function` distinction.
- Output: human table + `--json`.

If a later rule needs to count relevant tests, the host re-asserts per-element `relevant_test(change, test)` facts and rules use `facts_count` — the `journey_count` pattern. Not in initial scope.

## 8. Importer

`phr-mcp coverage import <export.json>` consumes the normalized format (§3.3). The Rust adapter documents producing it via **cargo-llvm-cov**; per-test attribution requires its cargo-nextest integration, and the per-binary aggregate fallback is documented as degraded granularity (region hits without test identity are recorded against a synthetic `test:<binary>` subject — explicitly, never dropped).

- Import is CLI-first; an `import_coverage` MCP tool is Phase 3.
- The fixture's committed export (A1) must be regenerable by a documented command, and an opt-in integration test runs the real toolchain end-to-end so the documented command cannot rot. No simulated exports in the default test path.

## 9. Acceptance criteria

**A1 — Fixture.** `crates/phronesis-mcp/tests/fixtures/coverage-sample/`: the sketch's `safe_divide` + three tests verbatim; a committed, hand-checkable normalized export; a documented regeneration command.

**A2 — Golden trace.** Hydrate → apply the sketch §4 edit (zero-denominator message change) → fire: rule 5.1 names `rejects_zero_denominator` for the branch site; `divides_positive_values` / `divides_negative_values` are relevant to `fn:src/lib.rs::safe_divide` but **not** to the branch site. The edit must also not break region identity (this pins §3.2).

**A3 — Gap.** With an empty store, rule 5.2 fires for the changed region with the §7 message.

**A4 — Demand-gating proof.** With no loaded rule mentioning coverage relations, zero coverage facts assert (footprint bounded by construction).

**A5 — Staleness.** Index revision ≠ HEAD ⇒ `coverage_stale` asserts and a block-phase rule demotes to warn through the existing drift path.

**A6 — Selection.** `phr-mcp coverage select` on the fixture change returns exactly `rejects_zero_denominator` at branch granularity, with function-level reach listed separately.

**A7 — BDD.** `crates/phronesis-mcp/tests/features/coverage-evidence.feature` scenarios mirroring A2–A5.

## 10. Phasing

- **Phase 1:** store + importer + demand-gated/change-scoped hydration + `head_revision`/`coverage_revision`/`coverage_stale` facts + rule 5.1 + A1/A2/A4/A7 (partial feature file).
- **Phase 2:** region mapper (`changed_region`/`changed_function` from real diff hunks; overlap policy: any line overlap ⇒ changed) + gap resolver + rule 5.2 + `coverage select` + A3/A6.
- **Phase 3:** staleness demotion wiring + block-phase opt-in pack + MCP tools (`import_coverage`, `select_relevant_tests`) + JaCoCo / coverage.py adapters.

## 11. Open questions

1. Branch-site anchor scheme (condition-text hash vs tree-sitter node identity) — Phase 1 spike, decided by A2.
2. Should `changed_region` persist across the pre→post hook pair via in-flight state for commit correlation?
3. Should import record a journey tag (`coverage:imported`) so confidence can count fresh coverage as a signal? (lean yes, Phase 3)