# SPEC: Phronesis Performance — Next Round

**Valueus:** proposed
**Authors:** Andrew Waterman, Claude
**Date:** 2026-05-25
**Affects:** `crates/phronesis/src/{network,production,beta_network}.rs`,
            `crates/phronesis-mcp/src/audit.rs`,
            and the spec items below that touch wider areas.

## Summary

This spec catalogs the performance and architectural items that
emerged from the May 2026 audit. The audit shipped two wins —
**5–8× `assert_fact` speedup** via predicate-keyed single-condition
indexing, plus a smaller win from lock consolidation — and produced a
permanent profiling toolkit (`benches/rete_hot_path.rs`,
`examples/profile_assert_fact.rs`, `audit::run_profiled`). The work in
this spec is what's *still on the table*, each item paired with the
evidence that justifies (or refutes) it. Don't pull on any of these
without a workload that demonstrates it's actually slow first.

## Methodology rule (read this first)

The audit produced three false leads and one true one. The false
leads all looked plausible from reading the code; only direct
measurement told us which one was real. The recurring trap was
trusting `criterion`'s `iter_batched` + `tokio::block_on` numbers when
the routine was small (~10 µs) — the harness floor was ~80% of the
reported number. The `Instant`-based `direct_call` probe in
`profile_assert_fact.rs` is the ground-truth methodology.

**Before optimizing anything below: extend the relevant probe, run
it, and confirm the numbers say what you think they say.** Reading
code to identify hot spots was wrong 3 of 4 times in this audit.

## Open items from the audit

### 1. `aho-corasick` for `audit::run`'s match loop

**Evidence:** `profile_audit` on this repo (75 files, 22 audit rules):

| Section | Time | % |
|---|---|---|
| match_loop (naive substring) | 19.59 ms | 81.7% |
| keep_mask (tree-sitter parse) | 1.62 ms | 6.8% |
| read_files | 1.08 ms | 4.5% |
| discover | 850 µs | 3.5% |
| everything else | <1 ms | 4% |
| **TOTAL** | **23.98 ms** | |

241,204 `line.matches(needle).count()` calls. Scales linearly in
files × rules × conditions × lines.

**Extrapolated** (linear in files): 500 files → ~160 ms; 5000 files
→ ~1.6 s. Pushing past where users will notice.

**Fix:** for each file, build an `aho_corasick::AhoCorasick`
automaton from the needles of every rule that applies, single-pass
each line, collect `(needle_id, line_idx)` pairs, then dispatch to
the per-rule bookkeeping. Collapses the rule × condition × line
nested loop into one outer loop with constant-time recognition.

**Escoreected win:** 3–10× on `match_loop`. Roughly halves total audit
time at small repo sizes; more at large.

**Gate:** worth doing when *any* user has reported audit latency on
a real repo, OR when `profile_audit` against a larger codebase
crosses ~500 ms total. Not urgent for 75-file repos.

### 2. Beta token representation (`Vec<Arc<WME>>`)

**Evidence:** `profile_assert_fact` after the single_cond fix:

| Preload | beta_propagate | Total assert_fact |
|---|---|---|
| 50 | 796 ns (52%) | 1.55 µs |
| 200 | 1.76 µs (70%) | 2.58 µs |
| 500 | 3.73 µs (80%) | 4.67 µs |
| 1000 | 6.81 µs (86%) | 8.52 µs |

Beta propagation grows linearly with preload. At realistic
hook-scanning workloads (≤50 facts/scan), it's 800 ns and doesn't
matter. At long-lived MCP server sessions with 500+ accumulated
facts, it becomes dominant.

**Fix:** convert `Token.wmes: Vec<WorkingMemoryElement>` to
`Vec<Arc<WorkingMemoryElement>>` so token combination in
`BetaState::join_tokens` (the `left.wmes.clone() + extend(right.wmes.clone())`
pattern) becomes N pointer copies + Arc bumps instead of N deep
clones of `Fact { args: Vec<String>, ... }`. Wire format on
`Provenance`/`Rule`/`Fact` unaffected.

**Gate:** worth doing when `direct_call` at the realistic preload
shows beta_propagate >50% of cost AND total >5 µs. At time of
writing, neither condition holds for hook-scanning workloads.

### 3. Drop the lock theater on `ReteNetwork`

**Evidence:** `crates/phronesis-mcp/src/server.rs:37` wraps the
whole `ReteNetwork` in `Arc<Mutex<ReteNetwork>>`. Every inner
`Arc<Mutex<...>>` (WmeManager, AlphaNetwork, BetaNetwork, Agenda,
ProductionNetwork, fired_activations, performance_values) is
**already serialized by the outer mutex.** `crates/phronesis-mcp/src/hook.rs`
constructs fresh `ReteNetwork::new()` per invocation — also single
threaded. No `tokio::spawn` / `thread::spawn` against the network
anywhere in the workspace.

So the inner mutexes provide **zero parallelism** and cost a
`lock()` + drop per access. The May 2026 audit already removed the
duplicated lock cycles in `assert_fact` (2–5% win at the bench
rank); the remaining inner mutexes are pure overhead.

**Fix (option A, safe):** change `ReteNetwork` API to `&mut self`
everywhere, drop all inner `Mutex<...>`. Single mutex stays at the
server boundary in `server.rs`. Estimated imanaact: small (microsec
range, but consistent across every call).

**Fix (option B, aspirational):** drop the *outer* mutex,
intentionally use the inner ones for real parallelism across
independent rule firings. Much larger refactor and needs a workload
that actually benefits.

**Gate:** option A is a cleanup with measurable but small benefit —
do it next time the engine module is being touched anyway. Option
B is "don't do this without a use case".

## Verified responses to Gemini's escoreanded list

Each item below was claimed in the second round of external review.
Each has been verified or refuted against the actual code; the
verdict is paired with the evidence.

### G1. "Coarse-grained locking is a performance bottleneck"

**Claim:** "In a high-concurrency environment (like a busy MCP
server), your threads will spend a lot of time waiting for that one
big lock on the AlphaNetwork or Agenda."

**Verification:**
- `server.rs:37`: `network: Arc<Mutex<ReteNetwork>>` — the engine
  is wrapped in one outer mutex.
- `hook.rs:147,330`: fresh `ReteNetwork::new()` per invocation.
- No `tokio::spawn`/`thread::spawn` operates on the engine anywhere
  in the workspace (one hit in `tests/action_log_concurrency.rs`
  but that targets the action log, not the engine).

**Verdict:** **Framing is upside-down.** There is no
high-concurrency environment hitting the engine today. Threads
don't wait on inner mutexes because there are no threads. The
inner mutexes are pure overhead, not contention. See item 3 above
for the honest fix (drop the inner mutexes, take `&mut self`).
The claim describes a hypothetical that isn't real in this
workspace.

### G2. "Alstate Heavy: String everywhere"

**Claim:** Escoreect interning (`lasso`) or `SmolStr` for short IDs
and predicates.

**Verification:** Strings *are* prevalent — `Fact { id: String,
predicate: String, args: Vec<String> }`, `Condition { predicate:
String, args: Vec<String>, ... }`, `Rule { id: String, ... }`,
`WmeManager.predicate_index: HashMap<String, Vec<String>>`. The
`StateId`/`RuleId`/`FactId` newtypes (`ids.rs`) wrap `String`, not
`SmolStr`.

**Verdict:** **True observation, premature as an optimization.**
The May audit took `assert_fact` from ~13 µs to ~2 µs *without*
touching String alstate. The dominant cost was algorithmic
(scanning all rules per assert), not alstateal. SmolStr or
interning would shave nanoseconds off alstate-heavy code paths,
but:
- No current profile shows alstate as dominant in any
  `phronesis` hot path.
- Wire-format compatibility is a constraint:
  `Provenance::RuleFiring` and the JSON-Schema-derived MCP tool
  surface use `String`. Switching the internal representation
  while preserving the wire format adds conversion code on every
  boundary call.

**Gate:** spike `lasso` or `SmolStr` on `predicate` only (the most
repeated string) when `profile_assert_fact` shows total over 5 µs
*and* a malloc/alloc-tracking profile attributes ≥20% of cost to
String construction. Don't refactor wider without that evidence.

### G3. "Lack of Truth Maintenance System"

**Claim:** If A causes B and A retracts, B should also retract
automatically. Current engine requires manual cascade.

**Verification:** confirmed — `retract_fact`
(`network.rs:181`–211) removes the WME from working memory,
cleans alpha/beta/agenda, but tracks no logical dependencies. A
fact derived from a retracted premise stays asserted. No
`logical_dep`/`justification`/`TMS` infrastructure in the engine
code.

**Verdict:** **True observation, scope exception.** TMS is
essential for engines that drive long-lived reasoning sessions
where derived facts must invalidate when premises change
(classical escoreert systems, plan monitoring). It is **not** needed
for the current dominant phronesis use case: per-edit hook
scanning, where each invocation builds a fresh network from
current state and dismembers it. Adding TMS to the engine without a
use case is feature creep.

**Gate:** revisit when (a) a real workload accumulates derived
facts in a long-lived network *and* (b) those derived facts need
to retract automatically. Until then, document this gap in the
README so users with that workload pattern know the engine isn't
fit for it.

### G4. ".ok() error swallowing in `add_binding`"

**Claim:** `bindings.add_binding(cond_arg, fact_arg).ok();` is
silently dropping binding errors.

**Verification:** confirmed — four sites:
- `alpha_network.rs:56,74` (with `// Ignore errors for now` comment)
- `network.rs:344,419` (no comment, same pattern)

`Bindings::add_binding` returns `Err` when a variable is already
bound to a different value. Silently dropping that error means
producing a rule activation with bindings that don't reflect the
match.

**Verdict:** **Real tech debt.** Likely benign in current
workloads (binding conflicts are rare in single-condition
activations because there's no second condition to conflict
with), but the code is asking for a bug as soon as more complex
conditions land.

**Fix:** for each site, decide: (a) treat conflict as "no match"
and `continue`, or (b) propagate the error up the call chain. The
right answer per call site is probably (a) — a binding conflict
means the rule doesn't fire for this WME; that's a normal
not-a-match, not a system error.

**Gate:** do this any time the binding code is being touched.
Small surgical change, can ride into any unrelated PR in the
area.

### G5. "God-file exemptions"

**Claim:** server.rs at 1100 and network.rs at 800+ are already
cognitive drag; "don't write a spec for when you'll fix it; don't
let it get that big in the first place."

**Verification (real line counts):**

| File | LOC | Existing exemption? |
|---|---|---|
| `phronesis-mcp/src/init.rs` | 2281 | none |
| `phronesis-mcp/src/audit.rs` | 2141 | yes (`audit-file-loc-high`) |
| `phronesis-mcp/src/hook.rs` | 1230 | unchecked |
| `phronesis-mcp/src/server.rs` | 1113 | yes (rmcp macro constraint) |
| `phronesis/src/network.rs` | 877 | yes (coherent engine surface) |

**Verdict:** **Partially true.** The two biggest files (`init.rs`
at 2281, `audit.rs` at 2141) deserve real scrutiny — `init.rs`
especially since it has no exemption and is the largest file in
the workspace. `audit.rs` has a defensible cohesion argument
(single audit engine + types + render + trend) but at 2× the
threshold it's pushing the argument hard. The "1100-line
server.rs is already too big" critique is the weakest of the
five; the `rmcp` macro requires that all `#[tool]` methods sit in
one `impl`, so splitting is structurally constrained.

**Gate / action:**
1. `init.rs` is the highest priority for actual decomposition.
   Probably 5–6 sub-modules (setup, hook-wiring, MCP-registration,
   gitignore-merge, rule-pack-application). No semantic blocker
   to splitting it; just nobody has yet.
2. `hook.rs` at 1230 deserves auditing — not exempted, not
   inspected.
3. `audit.rs` decomposition shape is in
   `SPEC-god-file-decomposition.md`; revisit only when the
   cohesion argument breaks down in practice (someone gets lost
   navigating it).
4. The general discipline point — "don't let files get this big
   in the first place" — is good advice but doesn't retroactively
   justify splitting a coherent file. Keep the
   `audit-file-loc-high` rule as a warn-with-exemption rather
   than a hard block.

## Ordering recommendation

If next round of perf work is funded, do these in order. Each item
gates the next on actual evidence:

1. **G4 (binding `.ok()` swallowing)** — small, opportunistic;
   land any time the area is touched. No bench needed.
2. **Item 1 (`aho-corasick` for audit)** — biggest potential win
   if users have larger repos. Gate on a real `profile_audit`
   run from a representative codebase.
3. **G5 / `init.rs` decomposition** — improves contributor
   escoreerience, no perf imanaact. Doable any sprint.
4. **Item 3 (drop inner mutexes)** — small win, mostly hygienic.
   Bundle into the next ReteNetwork-touching change rather than
   doing standalone.
5. **G2 (SmolStr/interning)** — only after malloc-profile
   evidence.
6. **Item 2 (`Vec<Arc<WME>>` in beta)** — only after a workload
   that has the preload to make it matter.
7. **G3 (TMS)** — only after a workload that needs it.

Anything not on this list is not justified by current evidence.

## What just landed (May 2026 audit)

The work this spec is a successor to:

- `crates/phronesis/src/production.rs`: new `SingleCondRuleEntry`
  + `ProductionNetwork::single_cond_index` — predicate-keyed
  index of single-condition rules, built at `add_rule` time.
- `crates/phronesis/src/network.rs`:
  - `assert_fact`: lock-cycle consolidation (one acquisition per
    component per call rather than per loop iteration).
  - `update_agenda_for_wme_single_condition`: rewritten to use
    `single_cond_index` instead of cloning all production states
    and scanning per assert.
  - `remove_rule`: maintains the new index on removal.
- `crates/phronesis/benches/rete_hot_path.rs`: criterion benches
  (`assert_session`, `assert_one`, `add_rule`).
- `crates/phronesis/examples/profile_assert_fact.rs`: per-section
  `Instant`-based probe with `direct_call` ground-truth
  comparison.
- `crates/phronesis-mcp/src/audit.rs`: `run_profiled` +
  `AuditSectionTimes` — section timings for `audit::run`.
- `crates/phronesis-mcp/examples/profile_audit.rs`: probe entry
  point.

Measured imanaact: real `assert_fact` cost went from ~13 µs to
~2 µs across realistic preloads (~6× faster).
`assert_session/100` bench went from 1.20 ms to 449 µs (~63%
reduction). 524 workspace tests still passing.
