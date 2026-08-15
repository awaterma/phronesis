# Ownership evidence: graph size and latency measurements

**Spec:** [`../specs/SPEC-rust-ownership-evidence.md`](../specs/SPEC-rust-ownership-evidence.md) §4.2, §13.3, §15
**Date:** 2026-08-14
**Branch:** `feat/rust-ownership-evidence` (worktree `.worktrees/ownership-evidence`)
**Corpus:** Phronesis itself (§14.10 dogfooding). No sibling repository was measured.

## 1. Verdict up front

| Question | Answer |
|---|---|
| §13.3 gate — is **ownership disabled** within measurement noise of the current graph path? | **PASS.** The graph produced is *byte-identical* to the pre-feature binary's, and every timing differs by less than run-to-run variance (largest delta 1.1%, and it favours the feature build). |
| Enabled numbers | Reported in §4 as data. §13.3 says these are "reported, not assumed acceptable", so no verdict is offered here. |
| Would enabled approach `security::MAX_FACTS` (100,000) on this repository? | Not on its own: **66,695** total edges enabled vs **46,898** disabled — 67% of the ceiling vs 47%. It does not cross it, but it consumes 40% of the remaining headroom in one enrichment, on a repository that is not large. See §7. |

## 2. Environment and profile

| | |
|---|---|
| Machine | Apple M4 Max, 16 cores, 128 GiB RAM |
| OS | macOS 26.6 (Darwin 25.6.0) |
| Toolchain | rustc 1.96.1 (31fca3adb 2026-06-26) |
| **Build profile** | **`--release` for every binary and every number in this document.** No debug-profile number appears anywhere below. |
| Timer | `time.perf_counter()` around `subprocess.run`, so every CLI figure includes process spawn + dynamic linking (~8 ms floor, see §5). In-process figures (`store::load`, `hydrate`) use `std::time::Instant` inside a release example binary. |
| Peak memory | `/usr/bin/time -l` "maximum resident set size", one sample per configuration. |

Three binaries were compared:

| Label | What it is |
|---|---|
| **A — feature, disabled** | This branch's working tree, no `[ownership.rust]` section on disk. |
| **B — feature, enabled** | Same binary, `.phronesis/graph.toml` = `[ownership.rust] enabled = true, provider = "ast"`. |
| **C — pre-feature** | `git archive HEAD` (ab6d59b, the branch point) built separately in a scratch tree. This is "the current graph path" the §13.3 gate refers to. |

## 3. Methodology, and what it controls for

### 3.1 The corpus is a frozen snapshot, not the live worktree

The first measurement round ran against the live worktree and produced an
inconsistency: two rebuilds of the "same" disabled configuration differed by
97,893 bytes. The cause was **a concurrent agent session writing new test files
into the same worktree** (`crates/phronesis-mcp/tests/ownership_corpus.rs`,
`tests/zz_probe.rs` appeared mid-run). A `.rs` file full of `#[test]` functions
moves `test_reaches` by hundreds of edges, so the corpus was not constant
across configurations.

All numbers below therefore come from a **frozen snapshot**:

```
rsync -a --exclude 'target/' --exclude '.git' --exclude '.phronesis/' \
  /Users/…/.worktrees/ownership-evidence/ /…/scratchpad/corpus/
```

201 Rust files, 46,898 edges disabled. Determinism was verified before
measuring: two consecutive rebuilds of the snapshot produced byte-identical
`graph.jsonl`.

Deviation to be aware of: `.phronesis/` was excluded, so the snapshot lacks the
repo's `wiki/decisions/` pages. The frozen corpus therefore has no
`graph_decision` / `decision_missing_rule` edges (71 edges in the live repo).
This affects both configurations identically and is irrelevant to ownership.

### 3.2 A format-version artifact that had to be corrected

The first pre-feature hydration numbers looked like a 2× regression on the
disabled path (9.9 ms vs 18.7 ms). They were wrong. This branch bumps
`GRAPH_FORMAT` 17 → 18; the pre-feature binary read the format-18 index, saw
`Freshness::Outdated`, and **skipped the whole-tree freshness hash** — it was
fast because it did less work. Every baseline measurement below is taken after
rebuilding the index *with the baseline binary*, so both sides do the same
freshness work. Format mismatch of this kind silently flatters whichever binary
is older; it is worth remembering for future comparisons.

### 3.3 Other controls

- Each timing is 5 samples (rebuild, pre-check) or 7 (post-check, in-process),
  each preceded by a discarded warm-up run, reported as **median (min–max)**.
- Page cache is warm for all configurations; none of these numbers describe a
  cold-start read.
- The `post-check` subject files were chosen to avoid `on_save`'s full-rebuild
  escape hatches: `graph/sync.rs` contains the literal `#[path`, which routes
  `on_save` to `rebuild()` and would have made the "incremental" measurement a
  full rebuild in disguise. The subjects used are
  `crates/phronesis-mcp/src/audit.rs` (3,375 lines, "big") and
  `crates/phronesis-mcp/src/graph/query.rs` (260 lines, "small").
- A/B/C rounds were also run twice on the live repo before the snapshot was
  taken; the ordering of the three configurations was consistent across rounds
  (§8).

### 3.4 What these numbers do *not* cover

- **RETE assertion cost is only partly covered.** `hydrate` builds `Fact`
  values; it does not insert them into the network. The `pre-check` figures in
  §5 do include insertion, and are the closest thing here to real hook latency.
- **Peak memory is a single sample** per configuration, from an allocator whose
  high-water mark is not perfectly reproducible. Treat the RSS column as
  ±few MB, and read it for its shape (+25 MB enabled), not its last digit.
- **No MSRV workflow run.** §13.3 also asks for that; it is not a measurement
  and is not attempted here.
- **`provider = "rust-analyzer"` was not measured.** It is availability-only
  (it stats `PATH`, executes nothing), so its cost is a handful of edges and no
  meaningful time — but that is an argument, not a measurement.

## 4. Results — frozen corpus, release profile

### 4.1 Every §13.3 metric

| Metric | A: feature, **disabled** | C: **pre-feature** (current path) | B: feature, **enabled (ast)** | B vs A |
|---|---|---|---|---|
| Base edges | 11,566 | 11,566 | 31,363 | **+171%** |
| Derived edges | 35,332 | 35,332 | 35,332 | 0% |
| Total edges | 46,898 | 46,898 | 66,695 | **+42.2%** |
| Serialized graph bytes | 7,833,010 | 7,833,010 | 11,746,499 | **+50.0%** (+3.91 MB) |
| Graph byte-identical to C? | **yes (`cmp` clean)** | — | no | — |
| Full rebuild | 811.8 ms (808.6–813.3) | 820.8 ms (814.4–823.9) | 875.6 ms (869.8–879.4) | +7.9% |
| Incremental update, big file (`post-check`, 3,375-line file) | 266.2 ms (262.4–269.2) | 269.1 ms (266.4–271.9) | 296.1 ms (291.4–303.0) | +11.2% |
| Incremental update, small file (`post-check`, 260-line file) | 247.0 ms (244.0–253.6) | 248.6 ms (247.0–253.8) | 278.8 ms (277.0–282.4) | +12.9% |
| `store::load` (whole-file parse) | 7.3 ms (7.3–8.4) | 7.5 ms (7.4–8.4) | 11.5 ms (11.4–12.6) | +57.5% |
| `hydrate`, rule demands `defines_fn` only | 16.8 ms (16.5–17.3) | 16.5 ms (16.4–18.0) | 22.6 ms (22.1–23.1) | +34.5% |
| `hydrate`, rule demands every graph relation | 25.3 ms (24.9–26.7) | 25.1 ms (24.7–25.9) | 35.8 ms (35.4–37.5) | +41.5% |
| `hydrate`, rule demands `ownership_site` | 16.5 ms → 0 facts | n/a (relation unknown to that binary; returns instantly) | 23.6 ms → 7,584 facts | — |
| Peak RSS, rebuild | 68.9 MB | 65.5 MB | 93.4 MB | +24.5 MB |
| Peak RSS, post-check | 71.7 MB | 74.2 MB | 98.7 MB | +27.0 MB |

Disabled-vs-pre-feature deltas: rebuild −1.1%, post-check big −1.1%, post-check
small −0.6%, `store::load` −2.7%, hydrate +1.8% / +0.8%. All are smaller than
the min–max spread of the samples themselves, and they do not point the same
way — that is the signature of noise, not of a regression. Combined with the
byte-identical graph, **the disabled path is unchanged.**

### 4.2 Where the volume goes (enabled)

Ownership contributes **19,797 edges** (29.7% of the enabled graph) and
**3,913,489 bytes** (33.3% of the file). Full relation census:

| Relation | Edges | Share of ownership | Note |
|---|---|---|---|
| `ownership_site` | 3,792 | 19.2% | one per site |
| `ownership_site_in_function` | 3,792 | 19.2% | one per site |
| `ownership_site_span` | 3,792 | 19.2% | one per site |
| `ownership_evidence` | 3,792 | 19.2% | one per site |
| `clone_site` | 2,048 | 10.3% | site kind |
| `mutation_site` | 1,153 | 5.8% | site kind |
| `filter_site` | 268 | 1.4% | site kind |
| `await_site` | 256 | 1.3% | site kind |
| `sync_lock_site` | 67 | 0.3% | site kind |
| `clone_before_await` | 384 | 1.9% | derivation |
| `filter_before_clone` | 219 | 1.1% | derivation |
| `read_before_mutation` | 21 | 0.1% | derivation |
| `lock_scope_ends_before_await` | 12 | 0.06% | derivation |
| `ownership_analysis_status` | 201 | 1.0% | one per in-scope Rust file |
| **total** | **19,797** | | |

Reading of that table:

- **The volume is not in the interesting relations.** The four ordering/scope
  derivations — the entire analytical product of the feature — are 636 edges,
  3.2% of what ownership adds. The other 96.8% is site bookkeeping.
- **Every site costs 5 edges**: one kind edge plus the four per-site attribute
  edges. 3,792 sites × 5 = 18,960. The multiplier, not the site count, is what
  makes this expensive: a 20% reduction in edges per site would save more than
  deleting every `mutation_site` in the repository.
- `lock_scope_may_cross_await` does not appear anywhere in this corpus.
- Site density: 3,792 sites / 201 Rust files = 18.9 per file; 3,792 sites /
  2,373 functions (1,863 `defines_fn` + 510 `defines_method`) = 1.6 per
  function. The heaviest single file (`src/init.rs`) carries 896 ownership
  edges (~172 sites) — **well under the 2,000-site-per-file default cap, which
  never fired anywhere in this corpus** (all 201 statuses are
  `available / complete`; zero `site_cap`).
- **`derived_edges` stays at 35,332 in both configurations.** The ownership
  "derivations" are emitted as *base* edges carrying source provenance, so the
  rebuild summary's derived count is not a place ownership growth shows up.
  Do not read that column as evidence of no growth.

### 4.3 Ownership evidence is per-file bounded, per-repository unbounded

`max_sites_per_file` bounds a file. It does not bound the graph. On this
corpus, the observed rate is **~98 ownership edges per Rust file** (19,797 /
201) and **~19.5 KB of serialized graph per Rust file**. Under the current
default cap a single file may legitimately contribute 2,000 sites ≈ 10,000
edges ≈ 2 MB — more than a quarter of the entire pre-feature graph, from one
file, without the cap ever reporting partial analysis.

## 5. Hook latency, end to end

`pre-check` is the path that hydrates the graph into RETE and fires rules; it
is the closest measurement here to what a user feels. Frozen corpus, release
profile, 5 samples each, one synthetic rule on disk:

| Rule on disk | Ownership disabled | Ownership enabled |
|---|---|---|
| No graph relation demanded (`new_content_contains`) | — | **8.7 ms** (8.2–8.8) |
| `defines_fn` demanded (1,863 facts asserted) | **42.8 ms** (42.4–43.8) | **47.2 ms** (46.0–48.1) |
| `ownership_site` demanded | 28.4 ms (28.3–29.6) → 0 facts | 38.1 ms (37.9–38.8) → 3,792 facts |

The 8.7 ms row is the floor: when no rule names a graph relation, `hydrate`
returns before loading anything, and ownership is free. The important row is
the second: **a rule that has nothing to do with ownership pays +4.4 ms
(+10%)** simply because `store::load` parses the whole 11.7 MB file to reach
`defines_fn`. That is §4.2's conflict made concrete — the cost is not paid by
the queries that want ownership, it is paid by every hook that wants anything.

Two amplifiers worth stating plainly, both pre-existing and both multiplied by
this feature:

- `store::compact` rewrites the entire `graph.jsonl` on every save. Enabled,
  that is 11.7 MB written per edit instead of 7.8 MB.
- `sync::on_save` calls `store::load` **twice** (once for the `#[path` /
  `includes_file` owner check, once for `existing`), so the enabled parse cost
  is paid twice per incremental save.

Repeated `post-check` runs on the same file do not grow the graph
unboundedly: the D9 stale markers added +668 bytes once and the file size then
stayed flat across every subsequent save.

## 6. Reproduction

Release build of both binaries:

```sh
cargo build --release                                    # feature binary
git -C <worktree> archive HEAD | tar -x -C /tmp/baseline-src
cargo build --release --manifest-path /tmp/baseline-src/Cargo.toml --bin phr-mcp
```

Frozen corpus, per configuration:

```sh
# disabled
rm -f .phronesis/graph.toml
phr-mcp graph rebuild --path . --json     # x5, timed; also under /usr/bin/time -l
echo '{"tool_name":"Edit","tool_input":{"file_path":"crates/phronesis-mcp/src/audit.rs"}}' \
  | phr-mcp post-check                    # x7, timed
# enabled
printf '[ownership.rust]\nenabled = true\nprovider = "ast"\n' > .phronesis/graph.toml
# …same three commands…
```

Edge census (`breakdown.py`, counts `{"p": …, "d": …}` per line):

```sh
python3 breakdown.py .phronesis/graph.jsonl
```

`store::load` and `hydrate` were timed by a throwaway release example
(`crates/phronesis-mcp/examples/ownership_measure.rs`, deleted after the run —
it is not part of the feature). Its body:

```rust
let path = store::graph_path(&root);
let t = Instant::now();
let edges = store::load(&path).unwrap_or_default();   // store::load sample
let ms = t.elapsed();

let rules = vec![rule_using(&["defines_fn"])];        // or GRAPH_RELATIONS, or ownership_site
let t = Instant::now();
let h = hydrate::hydrate(&root, &rules, None);        // hydrate sample
let ms = t.elapsed();
```

## 7. `MAX_FACTS`, and what §16 should take from this

Plainly stated:

- `security::MAX_FACTS` is **100,000**. The enabled graph on this repository is
  **66,695 edges**, i.e. a full hydration is 67% of that ceiling; disabled it is
  47%. **Enabled does not reach the cap on this repository.**
- **The cap would not stop it if it did.** `MAX_FACTS` is enforced in exactly
  one place — the `assert_fact` MCP tool (`server.rs`). Neither
  `hook/pre.rs` nor `codex_hook.rs` checks it when asserting hydrated graph
  facts. So the number above is a design ceiling being consumed, not a guard
  that will fire. Anyone reasoning "the cap protects us" is wrong today.
- Headroom arithmetic: pre-feature, 53,102 edges of headroom remained. Enabling
  ownership for Rust alone consumes 19,797 of them — **37% of the remaining
  headroom for one opt-in enrichment on a 201-file Rust corpus**. A repository
  3× this size, or the same repository with ownership extended past Rust,
  crosses 100,000.

For **§16's deferred sidecar-graph question**, this measurement supports the
following, and nothing stronger:

1. The core graph budget concern in §4.2 is real and now quantified: ownership
   is 30% of edges and 33% of bytes for 3.2% of the analytical product.
2. The cost lands on the wrong consumers. Because `hydrate` and `on_save` parse
   the whole file, ownership taxes every rule that touches any graph relation
   (+10% pre-check, +12% post-check here) whether or not it wants ownership. A
   sidecar loaded only when an ownership relation is demanded would move that
   cost onto the queries that actually asked for it, and would restore the
   8.7 ms floor for everyone else.
3. A cheaper alternative exists inside the current design and should be
   evaluated before a sidecar: fold the four per-site attribute edges into
   fewer, wider edges. That addresses ~76% of the added volume without
   splitting the store.
4. Nothing here forces a decision now. Opt-in default off, on this corpus, is
   affordable. The threshold to watch is not this repository's absolute numbers
   but the rate: ~98 ownership edges and ~19.5 KB per Rust file.

## 8. Corroborating round on the live worktree

Taken before the snapshot, on the live (mutating) worktree. Included because
the ordering and the deltas reproduce; the absolute values are contaminated by
the concurrent writer described in §3.1 and must not be quoted.

| Metric | A disabled (r1 / r2) | C pre-feature (r1) | B enabled (r1 / r2) |
|---|---|---|---|
| Full rebuild | 818.1 / 842.2 ms | 821.6 ms | 873.3 / 898.0 ms |
| post-check, big | 277.3 / 270.3 ms | 265.8 ms | 294.1 / 304.7 ms |
| post-check, small | 250.9 / 256.1 ms | 249.2 ms | 277.2 / 284.2 ms |
| Graph bytes | 7,770,883 | 7,770,883 | 11,668,981 |
| Total edges | 46,602 | 46,602 | 66,332 |
| `store::load` | 7.6 ms | 7.6 ms | 11.6–11.8 ms |
| `hydrate` all-graph | 26.7 ms | 26.7 ms | 35.8 / 37.1 ms |

The disabled feature binary and the pre-feature binary produced byte-identical
`graph.jsonl` here too (`cmp` clean), on a corpus that differs from the frozen
snapshot — two independent confirmations of the §13.3 gate.

## 9. Honest caveats

- **The host was not an idle machine.** A second agent session was active in
  the same checkout throughout (§3.1). CPU contention would inflate, not
  deflate, timings; the tight min–max spreads (typically <2%) suggest no run
  was badly disturbed, but no run is certified clean either.
- **Single machine, single OS, single toolchain.** Nothing here says anything
  about Linux CI, a spinning disk, or a memory-constrained runner. The
  `store::load` / rewrite costs are I/O-shaped and will look worse on slower
  storage.
- **Peak RSS is one sample per configuration** (§3.4).
- **`hydrate` timings include the whole-tree freshness hash**, which is why
  they exceed `store::load` by ~9 ms in every configuration. That component is
  identical across configurations and is not an ownership cost.
- **The `hydrate[ownership]` row for the pre-feature binary reads 0.0 ms.** That
  is not a fast path; that binary does not know the relation, so
  `needed_relations` is empty and `hydrate` returns immediately. It is not
  comparable to the other two columns and is marked n/a in §4.1.
- **The corpus is one repository, and it is the tool's own repository.** Site
  density in Phronesis (1.6 per function) is not evidence about anybody else's
  Rust. §12/§14.10 field testing on a second corpus remains outstanding and is
  still owed against §15.
