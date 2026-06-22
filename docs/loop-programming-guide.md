# Phronesis for Loop-Based Agent Programming

**A guide to governing the iterative agentic loop so it doesn't drift.**

This guide is for people running Claude (or any hook-capable LLM agent) in a
long, iterative loop — the *propose → act → observe → propose again* cycle that
drives real coding work — and who want that loop to stay on the rails from the
first turn to the thousandth. It assumes you've read the [README](../README.md)
and have `phr-mcp` installed. For the full CLI surface, see the
[Command Reference](../crates/phronesis-mcp/CLAUDE.md).

---

## 1. The problem: the loop forgets

The agentic loop is simple to describe and hard to keep honest:

```
   ┌────────────────────────────────────────────┐
   │                                              │
   ▼                                              │
 propose an action ──► tool runs ──► observe result
   (Edit, Bash, …)                                │
   │                                              │
   └──────────────── next turn ───────────────────┘
```

Every turn appends to the context window: the diff you wrote, the compiler
output, the test log, the conversation. Your project guidance — `CLAUDE.md`,
the architectural decisions, the "always run the build before claiming done"
rule — is read carefully at the *start* of the loop and then steadily buried.
By iteration two hundred, the directive you most need was last seen clearly
around token eight hundred, and auto-compaction may have dropped it entirely.

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
  matches, the hook exits 2 and the action is **blocked** — Claude sees the
  message and adjusts before any damage is done.
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
for free — a `changed_auth_3x` condition is true on the call where the window
holds three auth edits and simply isn't asserted once the window slides past
them. (Full design: [SPEC-journey-facts](specs/SPEC-journey-facts.md).)

Because a `pre-check` runs *before* the current call is journaled, a journey
rule can **block the current action based on the trajectory that led to it** —
"have you done X before" (journey) cleanly separated from "are you doing X now"
(the diff). That is the headline capability for loop programming.

### Layer 3 — honest closure (ending the loop)

A loop's most dangerous moment is when it decides it's *done*. The model is
optimistic; the build may be red. The `confidence` pack gates `git commit` on
**grounded signals** — actual build, test, and known-bug outcomes read from a
per-toolchain adapter (cargo first) — not on three syntactic checks:

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
permissions, hooks, and gitignore lines are preserved. After running it, restart
Claude Code so it picks up the hooks and the MCP server.

---

## 4. Defining your loop's risk surface

Journey rules don't match on hardcoded concepts like "sql" or "auth" — the
engine is domain-neutral. Instead, *you* define your loop's risk surface in
`.phronesis/journey.json`, reusing the same predicate vocabulary the syntactic
rules already use. A **tagger** is a mini-rule whose effect is "stamp this tag
on the journal record" instead of "block."

```json
{
  "version": 1,
  "taggers": [
    { "tag": "build", "when": [ { "bash_command_matches": "cargo (build|check|test)" } ] },
    { "tag": "auth",  "when": [ { "file_path_matches": "src/auth/" } ] },
    { "tag": "tests", "when": [ { "file_path_matches": "tests/" } ] },
    { "tag": "sql",   "when": [ { "or": [
                                   { "new_content_contains": "INSERT INTO" },
                                   { "new_content_contains": "DELETE FROM" },
                                   { "file_path_matches": "migrations/" } ] } ] }
  ],
  "modules": [
    { "name": "auth", "paths": [ "src/auth/**" ] }
  ]
}
```

Pick the handful of surfaces where churn or absence actually hurts in *your*
loop: the module you keep re-touching, the test directory you keep forgetting,
the build command, the destructive operations. Start small — you can add taggers
as you learn where the loop goes wrong.

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
loop failure mode.

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

```json
{
  "id": "build-staleness",
  "phase": "pre",
  "when": [ { "__script__": "facts_count('journey_since_ge', ['build','8']) >= 1" } ],
  "then": { "warn": "8+ tool calls since the last build/test. Run the build before reporting done." }
}
```

> **Selector validation is your typo guard.** A rule referencing a tag the
> project's `journey.json` doesn't define is **rejected at load time**, not
> silently treated as "zero occurrences" (which would make an `== 0` absence
> rule fire constantly). Keep your rule selectors and tagger names in sync.

> **Threshold rules fire once per session.** Pure-script rules dedupe on rule
> id, which is the right semantics for "≥3 occurrences" — you get one warning
> per condition, not one per matching call.

---

## 6. Watching the loop

Phronesis gives you read-only surfaces to see what the loop is doing and tune
your rules:

```sh
phr-mcp journey                    # what journey facts assert right now (live trajectory)
phr-mcp journey --explain <rule>   # which journey facts a rule depends on + current values
phr-mcp confidence                 # current confidence band + grounded signals
phr-mcp stats                      # per-rule blocked/warned counts, last-fired time
phr-mcp stats --since 7d
phr-mcp audit                      # whole-tree sweep (the hook only sees diffs)
phr-mcp audit --fail-on block      # CI gate: exit 1 on any blocked violation
phr-mcp trend                      # is debt going up or down across the loop's lifetime?
```

`get_journey`, `get_confidence`, and the other MCP tools expose the same views
*in-conversation*, so the agent can ask "what does my trajectory look like"
mid-loop and self-correct.

Use `phr-mcp stats` to spot noisy rules (silence them with `silent: true`),
dead rules (delete them), or to confirm a tuning change had the effect you
wanted. Use `phr-mcp trend` after a cleanup sweep to confirm the pile actually
shrank.

---

## 7. Inner loop vs. outer loop

"Loop-based programming" has two meanings, and phronesis serves both:

- **The inner agentic loop** — the per-turn propose/act/observe cycle described
  throughout this guide. This is phronesis's home turf: per-call hooks,
  trajectory-aware journey rules, confidence-gated closure.

- **An outer recurring loop** — running a whole task repeatedly on an interval
  (for example, an unattended job that keeps grinding on a backlog). Phronesis
  composes cleanly here too: end each iteration with `phr-mcp audit --fail-on
  block` to gate it, so the recurring loop can't drift past your rules even
  when nobody is watching. The deep integration, though, is the inner loop —
  that is the cycle phronesis exists to keep honest.

---

## 8. A worked example: a refactor loop that stays honest

Imagine Claude is refactoring the auth module over a long session. Without
phronesis, the loop tends to: edit auth, edit auth again, chase a bug, edit
auth a third time, forget the tests entirely, stop running the build, and
finally announce "done — the refactor is complete" with a red build.

With the layers above wired up:

1. **Turn 1–3 (layer 1):** every auth edit is checked for `.unwrap()` and
   friends as it lands. Syntactic slips never accumulate.
2. **Turn 4 (layer 2):** `auth-churn-without-tests` fires on the *fourth* auth
   edit — "you've edited auth 3+ times this session without touching its tests."
   The warning steers the loop toward coverage *before* the bug count grows.
3. **Turn 12 (layer 2):** twelve edits deep with no recompile, `build-staleness`
   fires — "8+ tool calls since the last build." The loop rebuilds and catches a
   break early instead of at the end.
4. **Closure (layer 3):** Claude reaches for `git commit -m "refactor done"`.
   The build is red, so the confidence band is low and the commit is **blocked**;
   the `llm` pack's unverified-completion-claim rule blocks the "done" framing.
   The loop is forced back to green before it can close.

None of these fired from a `CLAUDE.md` paragraph that might have been compacted
away. They fired from disk, from outside the context window, identically at
turn 12 as they would have at turn 1,200.

---

## Further reading

- [README](../README.md) — project overview and the premise
- [Command Reference](../crates/phronesis-mcp/CLAUDE.md) — full CLI + pack details
- [SPEC-journey-facts](specs/SPEC-journey-facts.md) — the trajectory layer in depth
- [SPEC-confidence-scoring](specs/SPEC-confidence-scoring.md) — grounded closure gating
- [The Explainer](https://awaterma.github.io/phronesis/explainer.html) — the RETE engine and design intent
- [The Catalogue](https://awaterma.github.io/phronesis/catalogue.html) — visual reference of starter rules
