# Phronesis for Loop-Based Agent Programming

**A guide to governing the iterative agentic loop so it doesn't drift.**

This guide is for people running Claude Code, OpenAI Codex, Gemini CLI, or
another hook-capable LLM agent in a
long, iterative loop — the *propose → act → observe → propose again* cycle that
drives real coding work — and who want that loop to stay on the rails from the
first turn to the thousandth. For the conceptual grounding, see the
[documentation site](https://awaterma.github.io/phronesis/) — the explainer
essay and the rule catalogue live there. It assumes you've read the
[README](../README.md)
and have `phr-mcp` installed. For the full CLI surface, see the
[Command Reference](../crates/phronesis-mcp/CLAUDE.md).

---

## 1. The problem: the loop forgets

The agentic loop is simple to describe and hard to keep honest:

```
   ┌── propose ──► tool runs ──► observe ──┐
   │   (Edit, Bash…)                       │
   │                                       │
   └────────── next turn ◄─────────────────┘
```

Every turn appends to the context window: the diff you wrote, the compiler
output, the test log, the conversation. Your project guidance — `CLAUDE.md`,
the architectural decisions, the "always run the build before claiming done"
rule — is read carefully at the *start* of the loop and then steadily buried.
By iteration two hundred, the directive you most need was last seen clearly
hundreds of thousands of tokens ago, and auto-compaction may have dropped it
entirely.

This is **contextual drift**, and it is structural: the longer and more
productive the loop, the worse it gets. You cannot fix it by writing a better
`CLAUDE.md`, because the problem is not *what* the guidance says — it's that the
guidance lives *inside the very context window the loop is filling up*.

Phronesis fixes it by moving enforcement **out of the conversation entirely**.
Rules live on disk in `.phronesis/rules.json`, are re-read by lightweight hooks
at every single tool call, and fire from *outside* the context window. They
cannot be compressed away, because they were never loaded into context to begin
with. A rule fires the same in token nine hundred thousand as it does in token
eight hundred.

---

## 2. Three layers of loop governance

Phronesis governs the loop at three increasingly powerful layers. Most projects
start at layer 1 and add the others as the loop gets longer.

| Layer | What it sees | What it catches |
|-------|--------------|-----------------|
| **1. Per-iteration** | This one tool call | "Don't write `.unwrap()` in `src/`" — syntactic violations on the current edit |
| **2. Trajectory** | The accumulated journey across iterations | "You've edited `auth` 3× this session and never ran the tests" — temporal patterns |
| **3. Honest closure** | Grounded build/test/bug signals | "Don't commit — the build is red and you said it was done" — gated completion |

### Layer 1 — per-iteration enforcement (the inner loop)

Two hooks wrap each turn of the loop:

- **`pre-check`** runs *before* a tool call lands. If a rule with `phase: "pre"`
  matches, the adapter returns the host's blocking decision — the agent sees
  the message and adjusts before any damage is done.
- **`post-check`** runs *after* the action. A `phase: "post"` rule exits 1 and
  **warns** — advisory, the action already happened.

Each hook is a *fresh, stateless process* that reads the rules from disk,
asserts facts about this one call, fires, and exits. That statelessness is the
whole point: there is no in-memory state to drift, so iteration 900 is checked
exactly as rigorously as iteration 1.

This is the layer the starter packs target. `phr-mcp init --packs llm,rust`
gives you a working set immediately: the `rust` pack blocks `.unwrap()` /
`panic!()` / `todo!()` in `src/`, `Result<_, String>` returns, and more; the
`llm` pack blocks blame-shifting and **unverified completion claims**, and warns
on bare `git commit -m` to nudge end-to-end verification. (See the
[Command Reference](../crates/phronesis-mcp/CLAUDE.md) for the full pack
contents.)

Layer 1 is extensible without recompiling Phronesis. Sandboxed Rhai providers
under `.phronesis/predicates/*.rhai` receive a normalized `event` and call
`emit_fact(predicate, args)` to add project vocabulary before RETE matching.
For a multi-file Codex `apply_patch`, providers first receive the complete
`event.files` change set and then one `event.file_path` view per file. Use the
MCP's `test_predicate_provider` before installing a provider. This repository's
`change_set.rhai` provider demonstrates the pattern by classifying production
Rust paths, test paths, and production-only change sets.

### Layer 2 — trajectory awareness (across iterations)

Layer 1 only sees the *current* edit. But the patterns that actually wreck long
loops are **temporal and cross-file**:

- You've edited the auth module three times this session but never touched its
  tests.
- You ran a destructive migration command in the last five tool calls.
- You changed the public API fifteen edits ago and still haven't rebuilt.

These are invisible to a point-in-time rule. **Journey facts** make them
first-class. Phronesis keeps an append-only journal of every executed tool call
under `.phronesis/journey/`, and on every hook invocation it *recomputes*
aggregate `journey_*` facts from a bounded suffix of that journal and asserts
them into the same network your syntactic rules use.

The crucial property: journey facts are **never accumulated in memory**. They
are recomputed from the durable journal every call, over the window your rules
actually ask for. So they survive compaction, stay deterministic, and "decay"
for free — a `journey_occurrence` count of 3 holds while the window covers
three auth edits, and quietly drops to 2 once the window slides past one of
them. (Full design: [SPEC-journey-facts](specs/SPEC-journey-facts.md).)

Because a `pre-check` runs *before* the current call is journaled, a journey
rule can **block the current action based on the trajectory that led to it** —
"have you done X before" (journey) cleanly separated from "are you doing X now"
(the diff). That is the headline capability for loop programming.

### Layer 3 — honest closure (ending the loop)

A loop's most dangerous moment is when it decides it's *done*. The model is
optimistic; the build may be red. The `confidence` pack gates `git commit` on
**grounded signals** — actual build, test, and known-bug outcomes read from a
per-toolchain adapter (cargo first) — not on syntactic proxies:

- **low** confidence → blocks the commit
- **medium** → warns
- **high** → passes clean

Pair it with the `llm` pack's "unverified completion claim" rule and you have a
loop that physically cannot declare victory before the work compiles and passes.
Opt in by writing `.phronesis/confidence.json`; inspect the current band with
`phr-mcp confidence`. (Full design:
[SPEC-confidence-scoring](specs/SPEC-confidence-scoring.md).)

---

## 3. Setup

```sh
# 1. Install the binary (once per machine)
cargo install --path crates/phronesis-mcp

# 2. Register the MCP server at user scope (once per machine)
phr-mcp install

# 3. Initialize the project with the loop-relevant packs
cd /your/project
phr-mcp init --packs llm,rust,confidence,journey
```

`journey` is the pack that unlocks layer 2, and is the one most specific to
loop-based work. `init` is idempotent and only adds its own entries — existing
permissions, hooks, and gitignore lines are preserved. After running it,
restart the agent host so it picks up the hooks and MCP server. Codex users
must also review changed project hooks with `/hooks`; Phronesis does not bypass
that trust boundary.

You can sanity-check the install at any time:

```sh
$ phr-mcp --version
phr-mcp 0.22.0
```

Everything in the rest of this guide is drawn from the actual state of this
repository at the time of writing — the configs, rules, stats, and confidence
band shown below are what `phr-mcp` reports on this machine, not synthetic
examples. The point is that you can run the same commands against your own
project and see equivalent output.

---

## 4. Defining your loop's risk surface

Journey rules don't match on hardcoded concepts like "sql" or "auth" — the
engine is domain-neutral. Instead, *you* define your loop's risk surface in
`.phronesis/journey.json`, reusing the same predicate vocabulary the syntactic
rules already use. A **tagger** is a mini-rule whose effect is "stamp this tag
on the journal record" instead of "block."

The minimum useful surface is one tag. Here is the actual `journey.json` from
this repository — it is exactly what the `journey` pack ships:

```json
{
  "version": 1,
  "taggers": [
    { "tag": "build", "when": [ { "bash_command_matches": "cargo (build|check|test)" } ] }
  ],
  "modules": []
}
```

That single tag is enough to power the build-staleness rule in §5, which on
this repository fired **125 times in the last 7 days** — by far the most active
warning. Most loop pathologies you'll hit early are *temporal* (you forgot to
rebuild) rather than *cross-module* (you touched auth without tests). Start
there.

Once a tag earns its keep, grow the surface. Plausible next additions for a
Rust project look like this:

```json
{
  "taggers": [
    { "tag": "build", "when": [ { "bash_command_matches": "cargo (build|check|test)" } ] },
    { "tag": "tests", "when": [ { "file_path_matches": "tests/" } ] },
    { "tag": "sql",   "when": [ { "or": [
                                    { "new_content_contains": "INSERT INTO" },
                                    { "new_content_contains": "DELETE FROM" },
                                    { "file_path_matches": "migrations/" } ] } ] }
  ],
  "modules": [
    { "name": "engine", "paths": [ "crates/phronesis-mcp/src/engine/**" ] }
  ]
}
```

Pick the handful of surfaces where churn or absence actually hurts in *your*
loop: the module you keep re-touching, the test directory you keep forgetting,
the destructive operations. Adding a tag costs nothing until a rule references
it — the derivation pass only computes aggregates the loaded rules consume.

> **Selector validation is your typo guard.** A rule that references a tag
> your `journey.json` doesn't define is **rejected** — the next pre-check
> exits non-zero with `phronesis: BLOCKED — rule \`<id>\` references
> undefined selector \`<sel>\` — not in journey.json taggers or modules`,
> naming both the offending rule and the missing tag. This is safe by
> default: the dangerous case (absence rules of the form `== 0`) would
> otherwise fire constantly when the tag is missing, so rejecting at config
> time keeps that failure mode out of the loop entirely. Wire the tagger up
> first, then add the rule that depends on it.

---

## 5. Writing loop-aware rules

Journey rules live in `.phronesis/rules.json` alongside your syntactic rules and
use the same v2 `when`/`then` shape. The derivation pass scans your loaded rules
for `journey_*` conditions and computes *exactly those aggregates and nothing
more* — you never pay to derive a fact no rule consumes.

The aggregator family:

| Predicate | Args | Use for |
|-----------|------|---------|
| `journey_occurrence` | `[selector, window]` | counting (one fact per match) — feeds `facts_count(...) >= N` and `== 0` |
| `journey_seen` | `[selector, window]` | presence (≥1) — a plain boolean |
| `journey_since_ge` | `[selector, k]` | distance since last occurrence ≥ k |
| `journey_count` | `[selector, window, count]` | the count as a bindable value |
| `journey_distinct` | `[field, window, count]` | distinct values of a field in a window |

**Window tokens:** `5c` = last 5 calls · `30m`/`2h`/`7d` = wall time · `s` =
current session.

### The headline four

These are the patterns worth starting with — each maps to a recognizable
loop failure mode. Only the fourth (`build`) works with the minimum
`journey.json` from §4; the `auth`, `tests`, and `sql` selectors require the
expanded taggers from the same section.

**Auth churn (count over a session):**

```json
{
  "id": "auth-churn-session",
  "phase": "pre",
  "priority": 20,
  "when": [ { "__script__": "facts_count('journey_occurrence', ['auth','s']) >= 3" } ],
  "then": { "warn": "You've edited the auth module 3+ times this session. Does this need test coverage or a review before the next change?" }
}
```

**Churn without tests (count + absence — composable through ordinary `when`):**

```json
{
  "id": "auth-churn-without-tests",
  "phase": "pre",
  "priority": 25,
  "when": [
    { "__script__": "facts_count('journey_occurrence', ['auth','s']) >= 3" },
    { "__script__": "facts_count('journey_occurrence', ['tests','s']) == 0" }
  ],
  "then": { "warn": "You've edited auth 3+ times this session without touching its tests. Add or update coverage before continuing." }
}
```

**Recent destructive SQL (presence in a call window):**

```json
{
  "id": "sql-recent-call-window",
  "phase": "pre",
  "when": [ { "journey_seen": ["sql", "5c"] } ],
  "then": { "warn": "A SQL/migration edit happened in the last 5 tool calls — double-check it ran against the right database." }
}
```

**Build staleness (distance-since-last — the classic loop pathology):**

This is the rule that actually ships in this repository. It's worth showing in
full because the production version is more nuanced than the textbook form
above:

```json
{
  "id": "build-staleness",
  "phase": "pre",
  "priority": 5,
  "when": [
    { "__script__": "facts_count('journey_since_ge', ['build','8']) >= 1" },
    { "or": [
        { "change_type": "edit" },
        { "change_type": "write" },
        { "change_type": "multiedit" },
        { "change_type": "replace" },
        { "change_type": "write_file" }
    ] }
  ],
  "then": { "warn": "8+ tool calls since the last build/test. Run the build before reporting done." }
}
```

The trajectory clause (`journey_since_ge`) is only half the rule. Without the
`change_type` filter on the current call, the warning would fire on every Read,
Grep, and idle Bash call after the eighth tool call since the last build — the
agent would be drowning in noise and would learn to ignore it. Restricting to
edit-shaped tool calls means the warning fires *when it can act on it*:
"you're about to write more code without rebuilding."

This is the general shape of a good journey rule:

> `<trajectory condition>` **and** `<the current action this trajectory matters
> for>`.

The trajectory tells you the loop is in a precarious state; the current-call
clause tells you *this* is the call where intervention helps.

> **Threshold rules fire once per session.** Pure-script rules dedupe on rule
> id, which is the right semantics for "≥3 occurrences" — you get one warning
> per condition, not one per matching call.

---

## 6. Watching the loop

Phronesis gives you read-only surfaces to see what the loop is doing and tune
your rules. The outputs below are real captures from this repository at the
time of writing.

**`phr-mcp stats --since 7d`** — per-rule activity over the last week. This is
where you discover which rules are pulling their weight and which are dead
code:

```
Rule                                 Blocked  Warned  Last fired
build-staleness                            0     125  2d ago
warn-rust-function-param-count-high        0     115  1h ago
nudge-verify-before-commit                 0      57  1m ago
build-staleness#or0                        0      44  1h ago
confidence-medium-warns-commit             0      22  1m ago
warn-cargo-build-without-workspace         0      18  1h ago
confidence-low-blocks-commit              12       0  3m ago
build-staleness#or1                        0       5  1h ago
enforce-no-result-string-error             2       0  3d ago
warn-clone-heavy                           0       2  3d ago
llm-warn-git-add-all                       0       1  3d ago

Total: 14 blocked, 389 warned across 11 rules (window: 1w)
```

Three things to read from that table:

1. **`build-staleness` is the workhorse** (125 warns). The single tagger in
   §4 is doing real work — the loop genuinely does forget to rebuild.
2. **`confidence-low-blocks-commit` actually blocked 12 commits.** Layer 3 is
   not theoretical; it intercepted closure 12 times this week alone — the
   most recent of those, three minutes before this capture, was the commit
   that landed this very guide (see §8).
3. The `#or0..#or4` siblings under `build-staleness` are the per-branch fire
   counts for the `or` clause from §5 — useful when debugging which arm of a
   multi-branch rule is doing the matching.

**`phr-mcp confidence`** — current band and grounded signals. Three real
captures from this session, in temporal order:

```
$ phr-mcp confidence --json   # build was stale; commit attempt would block
{ "band": "low",    "signals": ["compile"],          "subject": "unit-…" }

$ phr-mcp confidence --json   # after cargo check + cargo test refreshed signals
{ "band": "medium", "signals": ["compile", "tests"], "subject": "unit-…" }

$ phr-mcp confidence --json   # after the commit settled the unit
{ "subject": null }
```

The signal isn't a heuristic vibe — it's the actual exit status of the last
`cargo check` and the test summary line from the last `cargo test`, captured
by the post-check hook and persisted to `.phronesis/outcomes/`. A `git commit`
both settles the open work unit (subject goes `null` between units) and
opens the next one on the next tool call.

**`phr-mcp journey`** — what journey facts the engine *would* assert against
the current journal, before any rule fires:

```
PREDICATE         ARGS       RULES
journey_since_ge  build | 1  build-staleness#or0..or4
journey_since_ge  build | 2  build-staleness#or0..or4
…
journey_since_ge  build | 8  build-staleness#or0..or4
```

This is the "why did this fire" view. Add `--explain <rule-id>` to filter to a
specific rule's dependencies.

**`phr-mcp audit`** — whole-tree sweep (the hook only sees per-call diffs).
**`phr-mcp audit --fail-on block`** turns it into a CI gate.

**`phr-mcp trend`** — debt-over-time, diffing successive audit snapshots:

```
Rule                                 2026-06-20  Δ
audit-rust-let-binding-count-high            45  0 ·
audit-rust-let-mut-count-high                29  0 ·
warn-rust-function-param-count-high          16  0 ·
…
```

`get_journey`, `get_confidence`, and the other MCP tools expose the same views
*in-conversation*, so the agent can ask "what does my trajectory look like"
mid-loop and self-correct.

Use `phr-mcp stats` to spot noisy rules (silence them with `silent: true`),
dead rules (delete them), or to confirm a tuning change had the effect you
wanted. Use `phr-mcp trend` after a cleanup sweep to confirm the pile actually
shrank.

---

## 7. Participatory governance: the loop helps evolve its own rules

So far the loop has been a *subject* of governance — rules constrain it from
outside. But the same loop is also the cheapest place to *discover* new rules:
it has live evidence of where it slips, where existing rules over-fire, and
where guidance lives as prose that nobody can enforce.

Phronesis exposes three drift surfaces that close this feedback loop:

- **`phr-mcp claude-md-drift`** — bullets in `CLAUDE.md` that no current rule
  covers. Candidates for porting to a rule, or for explicitly marking
  "non-lintable by design."
- **`phr-mcp memory-drift`** — entries in the agent's auto-memory store that
  have no matching rule or `durable.md` paragraph. Actionable memories
  (named commands, named tool calls) should become rules; ambient ones
  (shared prose) belong in `durable.md`.
- **`phr-mcp wiki-drift`** — ADR-style decisions under `.phronesis/wiki/decisions/`
  that no rule enforces. Decisions with explicit `enforces: [rule-id]`
  frontmatter resolve deterministically; others fall through to a token-overlap
  fallback.

A snippet of real output from `wiki-drift` on this repo:

```
Decision                      Bucket          Match
journey-derivation-scaling    uncovered       (no match)
no-panic-in-production        covered         → rule enforce-no-todo-in-src
no-llm-deflection             covered         → rule enforce-no-pre-existing-issue
borrow-ergonomic-apis         covered         → rule warn-rust-public-fn-takes-string-ref
…
```

The "uncovered" rows are the actionable signal: each one is a design decision
that lives only as prose, with no enforcement teeth. Some of those *should*
become rules; others are inherently non-lintable and that's fine — but the
list is concrete instead of speculative.

When the loop hits a real moment of friction or insight, it can participate in
closing the gap:

1. **Remember → decide → enforce.** When the human says "remember X" or "make a
   rule for X", check the drift tools first, then scaffold a decision with
   `phr-mcp decision new <slug>`. Fill in Context / Decision / Enforcement /
   Consequences. If the decision is mechanically enforceable, propose a rule in
   `.phronesis/rules.json` and wire `enforces: [rule-id]` into the decision's
   frontmatter so the next `wiki-drift` run picks it up as covered.
2. **Friction-driven proposals.** When a rule blocks the loop three or more
   times in the same session for what looks like a legitimate pattern, pause:
   either the rule's scope is too broad (propose a narrower `file_path_matches`
   or an exclusion) or the loop is doing something it shouldn't (adjust the
   approach, don't weaken the rule).
3. **Cross-session knowledge transfer.** A discovery that warrants permanence
   — a bug pattern, a rollout lesson — becomes a decision page. ADRs travel
   with the repo and outlive any one session's context window.

The bidirectionality is the point: the rules govern the loop *and* the loop
proposes new rules, with the human ratifying. Drift surfaces are how that
feedback channel stays in sync.

---

## 8. A worked example: this repository, last week

Pull the trail from §6 together with a concrete narrative. Everything below
is what `phr-mcp` actually reports about this repository over the last 7 days
— there is no hypothetical.

**Layer 1 (per-iteration)** kept syntactic discipline cheap and quiet. Two
`enforce-no-result-string-error` blocks intercepted `Result<_, String>`
returns before they landed; one `llm-warn-git-add-all` nudge caught a
`git add -A`; a handful of `warn-clone-heavy` and `warn-dbg-in-src` warnings
flagged small slips on the way past.

**Layer 2 (trajectory)** was, by volume, the dominant voice in the loop.
`build-staleness` fired **125 times** — every time the trajectory accumulated
eight tool calls without a `cargo build|check|test`, and the *current* call
was an edit. Each fire was a quiet course-correct: "rebuild before you write
more." `warn-rust-function-param-count-high` chimed in 106 times on the
syntactic side, but the *temporal* warning was the one preventing late-loop
collapse.

**Layer 3 (closure)** intercepted closure 12 separate times.
`confidence-low-blocks-commit` blocked `git commit` because the
build-or-test signal in `.phronesis/outcomes/` was red. The companion
`confidence-medium-warns-commit` warned 22 more times — yellow band, "commit
if you mean to, but the signal is unstable." Across the same window
`nudge-verify-before-commit` fired 57 times reminding the loop to actually
run the verification before reaching for the commit verb.

The cleanest illustration is the most recent one of those 12 blocks, three
minutes before the stats capture above: **the commit that landed this very
guide.** The loop tried `git commit` with a stale red compile signal in
`.phronesis/outcomes/`, and `confidence-low-blocks-commit` refused:

```
[phr-mcp pre-check]: phronesis: BLOCKED — Low confidence — compile/tests/
known-bug not all green. Run the build and tests and resolve failing signals
before committing.
```

The agent (running this session) refreshed the signals with `cargo check
--workspace` and `cargo test -p phronesis-mcp`, which stamped
`outcome:compile_ok` and `outcome:test_pass` against the open subject. The
band rose to `medium`, the block fell back to a warn, and the commit landed:

```
$ git log --oneline -1
5f5e84c docs(loop-guide): rewrite with real captures from this repo
```

None of this fired from a `CLAUDE.md` paragraph that might have been
compacted away. It fired from disk, from outside the context window, against
the recorded exit status of the last `cargo check`. The same mechanism would
fire identically at turn 12 or at turn 1,200.

That is the entire premise of the guide, captured live — the guide demonstrates
itself.

---

## 9. Inner loop vs. outer loop

"Loop-based programming" has two meanings, and phronesis serves both:

- **The inner agentic loop** — the per-turn propose/act/observe cycle this
  guide has been describing throughout. This is phronesis's home turf: per-call
  hooks, trajectory-aware journey rules, confidence-gated closure.

- **An outer recurring loop** — running a whole task repeatedly on an interval
  (for example, an unattended job that keeps grinding on a backlog). Phronesis
  composes cleanly here too: end each iteration with `phr-mcp audit --fail-on
  block` to gate it, so the recurring loop can't drift past your rules even
  when nobody is watching. The deep integration, though, is the inner loop —
  that is the cycle phronesis exists to keep honest.

---

## Further reading

- [README](../README.md) — project overview and the premise
- [Command Reference](../crates/phronesis-mcp/CLAUDE.md) — full CLI + pack details
- [SPEC-journey-facts](specs/SPEC-journey-facts.md) — the trajectory layer in depth
- [SPEC-confidence-scoring](specs/SPEC-confidence-scoring.md) — grounded closure gating
- [The Explainer](https://awaterma.github.io/phronesis/explainer.html) — the RETE engine and design intent
- [The Catalogue](https://awaterma.github.io/phronesis/catalogue.html) — visual reference of starter rules
