---
id: journey-derivation-scaling
date: 2026-06-21
status: accepted
enforces: []
superseded_by: null
tags: [perf, journey, benchmarks]
---

# journey-derivation-scaling

## Context

`journey::derive::assert_facts` runs on every pre/post hook invocation
and on every UserPromptSubmit. It reads a bounded suffix of
`.phronesis/journey/events.jsonl` (capped at `SUFFIX_HARD_CAP = 10_000`,
see `crates/phronesis-mcp/src/journey/journal.rs:72`), then emits one
or more `journey_*` aggregator facts per rule selector.

The cap was introduced defensively in commit `2c413fb` (Jun 19, 2026)
to bound a runaway session-floor scan under a misconfigured retention.
It is *not* a measured performance ceiling.

As `journey` rules and aggregators expand (e.g.
`journey_filtered_since_ge` landed in 0.13.x), a recurring question
is "can we raise the cap, and what would it cost?" Without data, we
had been extrapolating from `assert_one` micro-benches that stop at
N=200.

A criterion bench at `crates/phronesis-mcp/benches/journey_derive.rs`
sweeps the work over `N ∈ {1k, 5k, 10k, 25k, 50k, 100k}` events in
the suffix. It replay-and-scales from this repo's seed events
corpus, with seeded RNG (`SEED = 0x5eed_5eed_5eed_5eed`) so results
are reproducible. Two benchmark groups:

- `journey_derive` — just `derive::assert_facts` (fact assertion
  into a fresh `ReteNetwork`, no firing).
- `journey_full` — the full per-turn hook path: `add_rule` ×4 +
  `derive::assert_facts` + `fire_all_consequences`. This is what
  `pre-check` / `post-check` actually do.

## Decision

**The `SUFFIX_HARD_CAP` can safely be raised by an order of magnitude
without perceptible per-turn impact.** When a real workload needs it,
raise the constant directly. Re-measurement is not required below
~200k events.

### Measured curves (release build, 2026-06-21)

Both groups, side-by-side. The "overhead" column is `(full − derive)` —
the marginal cost of registering 4 rules and firing the agenda on top
of derivation.

| N         | derive only | full path  | overhead    | per-event (full) |
|-----------|-------------|------------|-------------|------------------|
| 1,000     | 875 µs      | 875 µs     | ~0          | 0.88 µs          |
| 5,000     | 4.14 ms     | 4.15 ms    | ~10 µs      | 0.83 µs          |
| 10,000    | 8.31 ms     | 8.28 ms    | ~0          | 0.83 µs          |
| 25,000    | 12.59 ms    | 12.65 ms   | ~60 µs      | 0.51 µs          |
| 50,000    | 19.16 ms    | 19.61 ms   | ~450 µs     | 0.39 µs          |
| 100,000   | 33.02 ms    | 33.21 ms   | ~190 µs     | 0.33 µs          |

**Firing cost is negligible across the entire sweep.** At every N, the
full per-turn cost is within measurement noise of derivation alone. The
working-memory pressure from up to ~100k asserted `journey_*` facts at
N=100k does not translate to a meaningful firing cost — most likely
because `__script__` rules (which is how journey rules are expressed)
are evaluated once at agenda population, not per-fact, and even at 100k
WM facts a `facts_count` walk completes in well under 1 ms.

The curve is **sub-linear** past 10k: per-event cost *drops* from
0.88 µs at 1k to 0.33 µs at 100k as fixed setup costs amortize
(rule scan, network construction, file open, JSON parse warm-up).

Every transition is at or below proportional:

| transition | data multiplier | time multiplier |
|------------|-----------------|-----------------|
| 1k → 5k    | 5.0× | 4.7× |
| 5k → 10k   | 2.0× | 2.0× |
| 10k → 25k  | 2.5× | 1.5× |
| 25k → 50k  | 2.0× | 1.5× |
| 50k → 100k | 2.0× | 1.7× |

No inflection point exists in the measured range. The 100k worst case
(~33 ms) is below the imperceptible-latency threshold (~50 ms) and an
order of magnitude below user-noticeable on local (~200 ms).

The full-path measurement includes file I/O of the entire
`events.jsonl`, JSON deserialization, rule registration (4 rules),
selector validation, suffix read, emission of `journey_occurrence` /
`journey_count` / `journey_seen` facts on both session and 100-call
windows, and a complete `fire_all_consequences` pass over the
populated WM. This is the same path the pre/post-check hooks execute
on every tool call.

### Cap-raise guidance

Numbers are full per-turn cost (derive + fire), since that's what
user-perceived latency tracks.

| target cap | expected worst-case per-turn cost |
|------------|-----------------------------------|
| 10k (current) | ~8 ms |
| 25k | ~13 ms |
| 50k | ~20 ms |
| 100k | ~33 ms |
| 200k (extrapolated, linear) | ~66 ms |

Below 100k: no concern. At 200k: enters the "noticeable on local"
band; re-bench before committing. The sub-linear shape past 10k
suggests 200k may actually come in well below 66 ms, but
extrapolation past the measured range is not the basis on which
to ship.

## Enforcement

No automated rule. This is a perf characterization that informs a
future cap-tuning decision, not a code-shape pattern. The bench
itself is the artifact: re-running
`cargo bench -p phronesis-mcp --bench journey_derive` reproduces
both curves against future changes to derivation or firing code,
and criterion's baseline-comparison will flag regressions
automatically. The `journey_full − journey_derive` delta is the
firing-cost signal — if that gap widens in a future run, a new
fact-evaluation path was introduced that *does* scale with WM
pressure.

## Consequences

- The cap stays at 10k for now. No code change today.
- When a real workload pushes against 10k, the answer is "raise the
  constant" — no re-architecture required, no path-index work needed
  in the journey layer.
- The bench (`benches/journey_derive.rs`) becomes the regression
  detector for derivation performance going forward. Future changes
  to `derive::assert_facts` should re-run it and compare; criterion
  will surface any cliff this characterization didn't predict.
- If derivation ever does inflect (new aggregator with quadratic
  behavior, a join we add later), this ADR's measured baseline is
  what surfaces the regression — supersede then with a new measurement
  and a revised cap-raise table.

## See also

- `crates/phronesis-mcp/src/journey/journal.rs:72` — the cap constant.
- `crates/phronesis-mcp/src/journey/derive.rs:461` — the entry point.
- `docs/specs/SPEC-journey-facts.md` §"Cost: how the suffix stays bounded".
- `crates/phronesis-mcp/benches/journey_derive.rs` — this ADR's source bench.
