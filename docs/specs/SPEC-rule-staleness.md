# SPEC: rule-staleness — rules that outlived the code they describe

**Status:** draft
**Authors:** Claude, Andrew Waterman
**Date:** 2026-08-04
**Target release:** phronesis-mcp 0.25.0 (MINOR — new file surface, new drift source, new hook behavior)
**Depends on:** SPEC-drift-consolidation (must ship first — the report surface
is a source of the consolidated `get_drift` tool, not a standalone tool)
**Affects:** new `crates/phronesis-mcp/src/graph/bindings.rs`;
              `crates/phronesis-mcp/src/graph/{mod.rs, sync.rs}`,
              `crates/phronesis-mcp/src/hook/pre.rs`.
              No `phr` library-crate change.

## Premise

A rule moved out of the context window does not thereby become
correct. It becomes *durable* — which is worse when it is wrong.

Two rules currently shipped in the `rust` pack read:

- `block-await-on-sync-execute-all-agenda-items`
- `block-await-on-sync-fire-all-consequences`

Each pins a fact about a specific function's signature after a
specific refactor: these methods became sync, so `.await` on them is a
compile error, so the rule blocks the edit before the compiler has to.

That is a good rule *while the premise holds*. When the premise stops
holding — the method goes async again, or is renamed, or is deleted —
nothing tells anyone. The rule keeps firing, deterministically, with
full confidence, from outside the context window, blocking correct
code.

This is the cost of the architecture stated in the README: rules "fire
the same in token nine hundred thousand as they do in token eight
hundred." A prompt that has gone stale degrades into ignorable noise.
A *rule* that has gone stale degrades into an authoritative false
block. Durability cuts both ways, and only one side is currently
instrumented.

Phronesis already knows what the codebase contains. The structural
graph (SPEC-triple-store-rete) records `defines_fn` edges for Rust,
Python, and TypeScript, and `graph/sync.rs` keeps them current on
every save. The information needed to notice that a rule's referent
has vanished is already on disk. Nothing reads it for this purpose.

This SPEC connects the two: rules acquire **bindings** to the code
entities they name, bindings are reconciled whenever the graph
changes, and a rule whose binding has vanished is reported as drift
and demoted from `block` to `warn` at hook time.

## Goals

- A `.phronesis/bindings.json` recording which rules name which code
  entities, written only by the graph-sync pipeline.
- Detection of **referent-gone** staleness: a rule named a symbol, that
  symbol resolved against the graph at least once, and it no longer
  resolves.
- Automatic `block` → `warn` demotion of stale rules at pre-check,
  guarded on graph freshness, reusing the existing
  `hook_logged::demote_violations_from` seam.
- A `code` source in the consolidated drift tool, so the model can ask
  why a rule demoted mid-session and a human can triage the list.
- Structural exclusion of false positives: a foreign symbol
  (`.unwrap()`, `panic!()`, `console.log`) must be incapable of being
  reported stale, rather than filtered out after the fact.

## Out of scope

- **Premise-inverted staleness.** Detecting that
  `execute_all_agenda_items` still exists but is async again requires
  the graph to record function shape (`is_async`, `returns_result`,
  arity). That is new vocabulary, new extraction per language, and a
  way for a rule to declare which premise it depends on. It is the
  more valuable detector and the much larger spec. Referent-gone ships
  first because it is free — the edges already exist.
- **Automatic retirement.** Deleting or disabling a stale rule belongs
  to the rule-retirement spec. This SPEC only demotes and reports.
- **Language-specific inference.** Symbol extraction operates on rule
  literals and matches against `defines_fn`, which Rust, Python, and
  TypeScript all emit. No per-language inference code.
- **Non-graph rules.** Rules whose literals name no code entity (the
  `llm` deflection pack, for instance) never bind and are never
  evaluated. That is correct: there is no referent to lose.

## 1. Staleness as a transition, not a state

The naive formulation — "a rule is stale if its literal is absent from
the graph" — is unusable. Most rule literals name foreign symbols.
`.unwrap()` is `std`, `panic!()` is a macro, `console.log` is the DOM;
none appears in a project's `defines_fn` set, and none ever will.
Under the naive rule, essentially the entire `rust` pack reports stale
on day one.

The single-snapshot view also cannot distinguish the two reasons a
symbol is absent:

| Symbol absent because… | Correct verdict |
|------------------------|-----------------|
| it was never a local entity (`std::unwrap`) | not stale — no referent was ever claimed |
| it was a local entity and was deleted | stale |

No amount of scoring separates these from one snapshot. They are
distinguishable only over time.

So staleness is defined as a **transition**:

> A rule is stale when a symbol it names resolved against the graph at
> some earlier point, and no longer resolves.

The earlier resolution is recorded when it happens. A symbol that
never resolves is never recorded, so it can never transition, so it
can never be reported stale. The false-positive class is excluded by
construction rather than suppressed by a threshold — which matters,
because a threshold that silently demotes real rules is a worse
failure than no detector at all.

## 2. Binding

### 2.1 Candidate extraction

For each rule, each string literal in a `new_content_contains` or
`bash_command_matches` clause yields candidate symbols. Decoration is
stripped:

| Literal | Candidate |
|---------|-----------|
| `.execute_all_agenda_items().await` | `execute_all_agenda_items` |
| `.unwrap()` | `unwrap` |
| `fire_all_consequences(` | `fire_all_consequences` |
| `panic!()` | `panic` |

Stripping removes a leading `.`, a trailing `!`, `(`, `()`, and
`().await`. A candidate that is not a plausible identifier (empty,
contains whitespace or path separators, or is a language keyword) is
discarded before resolution.

`bash_command_matches` holds a *regex*, not a plain literal. Its
source is treated as text and any candidate containing regex
metacharacters (`\ | [ ] ( ) { } * + ? ^ $`) after stripping is
discarded rather than guessed at. Regex clauses therefore contribute
candidates only when the pattern happens to contain a bare identifier
run, and contribute nothing otherwise. This is intentional: a wrong
candidate extracted from a regex would bind to an unrelated function
and produce a false stale later.

Extraction is deliberately generous within those limits.
Over-generating candidates is harmless: a candidate that does not
resolve is simply never recorded.

### 2.2 Resolution

A candidate resolves if it equals the **last path segment** of the
symbol argument of any `defines_fn` edge.

```
candidate:  execute_all_agenda_items
edge:       defines_fn("src/engine.rs", "crate::engine::Agenda::execute_all_agenda_items")
            last segment ──────────────────────────────► execute_all_agenda_items   HIT
```

Leaf-segment matching rather than full-path matching is what lets a
rule literal (`.execute_all_agenda_items()`, which carries no module
path) match a fully-qualified graph entity.

`unwrap` normally resolves against nothing, because no local
`defines_fn` edge ends in `unwrap`. It is therefore never recorded.

A candidate resolving to one or more definitions produces one binding
holding **all** of them.

### 2.2.1 Shadowing: the one surviving false-positive path

Leaf-segment matching means a *local* definition sharing a name with a
foreign symbol will resolve. A project defining its own
`fn unwrap(...)` gives the `rust` pack's `.unwrap()` literal a real
binding — and deleting that local function would then report
`no-unwrap-in-src` as stale, which is wrong. The rule is about the
`std` method and never depended on the local one.

The structural exclusion in §1 is therefore precise about what it
excludes: a symbol with **no** local definition can never be reported
stale. A symbol that is *shadowed* by a local definition can be, and
this is the residual false-positive class.

It is accepted rather than solved, for three reasons: it requires a
name collision between a pack literal and a local definition, which is
rare; the consequence is bounded (one rule demotes `block` → `warn`,
and the report names the symbol, so triage is immediate); and solving
it properly needs call-site resolution — knowing which `unwrap` a
given `.unwrap()` refers to — which is a type-resolution problem well
beyond what a leaf-segment graph can answer.

If measurement later shows this occurring in practice, the cheap
mitigation is an opt-out: a `binds: false` field on a rule, suppressing
candidate extraction for rules whose literals are known to name foreign
symbols. That is not shipped now because there is no evidence the case
arises.

### 2.3 File format

`.phronesis/bindings.json`, written only by graph-sync:

```json
{
  "version": 1,
  "bindings": [
    {
      "rule": "block-await-on-sync-execute-all-agenda-items",
      "symbol": "execute_all_agenda_items",
      "definitions": ["crate::engine::Agenda::execute_all_agenda_items"],
      "bound_at": 1754300000,
      "state": "bound"
    }
  ]
}
```

`definitions` holds the full resolution set, which is what makes the
conservative rule in §3.2 expressible. `state` is `bound` or `stale`.
`bound_at` is the first resolution and is never rewritten, so the
report can say how long a binding held before it broke.

The file is derived state. Deleting it costs only the `bound_at`
history: the next sync re-binds everything currently resolvable, and
nothing is reported stale until a symbol subsequently disappears.
Deleting it is therefore the documented recovery for any suspected
corruption, and it is safe — it can only under-report, never
over-report.

## 3. Reconciliation

### 3.1 When

Inside `graph::sync`, after the graph updates and derived edges are
regenerated. This is the only writer of `bindings.json`.

Sync already walks the graph on every save and already maintains the
staleness index, so reconciliation adds a pass over an in-memory edge
set that is already loaded. Nothing is added to the pre-check hot
path: pre-check reads a small file and does no resolution.

Computing the verdict in the same pass that updates the graph also
means the verdict cannot disagree with the graph. There is no window
in which `bindings.json` describes an older edge set.

### 3.2 The conservative rule

On each reconciliation, every recorded binding is re-resolved.

- **Every definition gone** → `state: "stale"`.
- **At least one definition remains** → `state: "bound"`, and
  `definitions` is updated to the surviving set.

A rule binding several same-named definitions goes stale only when the
last one disappears. Moving a function between modules, renaming its
enclosing type, or deleting one of several overloads leaves the rule
armed.

This trades recall for precision on purpose. A missed stale rule
leaves today's behavior unchanged. A false stale silently demotes a
correct rule from `block` to `warn`, which is a regression in
enforcement that no one is likely to notice. The asymmetry justifies
the conservative side.

### 3.3 Recovery

A binding in `state: "stale"` whose symbol resolves again returns to
`state: "bound"`. Reverting a bad refactor, restoring a deleted file,
or checking out a branch where the function still exists re-arms the
rule with no manual step.

## 4. Enforcement

### 4.1 The demotion seam

`hook/pre.rs` already collects `stale_graph_rules: BTreeSet<RuleId>`
and passes it to `hook_logged::demote_violations_from` when non-empty
(`hook/pre.rs:176-179`). Structural rules reading a drifted graph are
demoted through exactly this path.

Stale-rule IDs are unioned into that same set. No new demotion
mechanism is introduced, and stale-by-binding and stale-by-graph
converge on one behavior.

The message accompanying a demotion names the lost symbol, so the
model is told *why* a rule softened rather than observing that it did:

```
phronesis: NOTE — rule `block-await-on-sync-execute-all-agenda-items`
names `execute_all_agenda_items`, which the code graph no longer
defines; this rule will warn, not block. Review or retire it.
```

### 4.2 The freshness guard

Staleness is evaluated **only** when the graph reports
`Freshness::Fresh`.

Under `Stale(_)` or `Outdated { .. }`, the stale set is ignored
entirely and all rules keep full force.

This guard is load-bearing. A graph that has not been rebuilt after a
`git checkout` describes files that no longer exist and functions that
were never removed. Evaluating bindings against it would report mass
symbol deletion and demote much of the rule pack at once — silently
disarming enforcement at exactly the moment (a branch switch) when the
tree is least familiar. The guard makes the failure mode "rules stay
strict," which is the safe direction.

## 5. Reporting

The report is a source of the consolidated drift tool
(SPEC-drift-consolidation), not a standalone surface:

```
phr-mcp drift --source code
```

MCP: `get_drift(source: "code")`.

This follows the existing drift contract. Output is a triage list, not
ground truth: it says a rule names something the graph no longer
defines, which is evidence that the rule needs review — not proof that
the rule is wrong. A rule may legitimately outlive its referent, for
instance one guarding against reintroducing a pattern that was
deliberately removed. Such a rule is *expected* to be permanently
stale, and the honest handling is for a human to see it in the report
and leave it there, or retire it. Automatic action beyond demotion is
out of scope.

Each entry reports the rule ID, the lost symbol, the definitions it
formerly resolved to, and how long the binding held before breaking
(`bound_at` to the reconciliation that broke it).

## 6. Failure behavior

Every failure path fails **open toward full enforcement**, matching
`load_index`, which treats a missing index as "nothing known yet"
rather than failing closed:

| Condition | Behavior |
|-----------|----------|
| `bindings.json` missing | empty stale set; no demotions; sync rebuilds it |
| unreadable or malformed | empty stale set; diagnostic on stderr; file rebuilt next sync |
| `version` unrecognized | file ignored; rebuilt from scratch |
| write fails during sync | logged; the save is not failed; next sync retries |
| graph not `Fresh` | evaluation skipped entirely (§4.2) |

A bug in this feature must never silently weaken the rule pack. Every
degraded state above results in *more* enforcement, not less.

## 7. Module boundaries

**`graph::bindings`** (new). Owns the format and the logic. Core is
pure: `resolve(rules, edges) -> Vec<Binding>` and
`reconcile(persisted, fresh) -> BindingSet`. Neither touches the
filesystem or the hook payload; a thin `load`/`store` edge does the
I/O. Testable with hand-built edge vectors and no fixtures.

**`graph::sync`** (extended). Calls `reconcile` after the graph
updates and persists the result. The only writer.

**`hook::pre`** (extended). Loads the stale set, checks freshness,
unions into `stale_graph_rules`. Reads only; never resolves.

The seam that matters: `bindings` knows nothing about hooks and `pre`
knows nothing about resolution. Inference is testable without a hook
and demotion is testable without a graph.

## 8. Testing

Unit, in `graph::bindings`:

- a candidate resolving to no definition is not recorded
  (`.unwrap()` never appears in `bindings.json`)
- a symbol that resolves, then vanishes, transitions to `stale`
- a symbol losing some but not all definitions stays `bound`, with
  `definitions` narrowed
- a stale binding whose symbol reappears returns to `bound`
- `bound_at` survives reconciliation unchanged
- a malformed or version-mismatched file yields an empty set, not an
  error
- a locally-defined symbol shadowing a foreign one *does* bind
  (§2.2.1) — pinning the known limitation so a future call-site
  resolver has a test to flip rather than a surprise to discover

Integration, through the real binary:

- a rule blocking an edit still blocks it while its referent exists
- after the referent is deleted and the graph resynced, the same edit
  **warns** and the diagnostic names the lost symbol
- with the graph marked stale, the same edit **still blocks** —
  the freshness guard holds

The last assertion protects the fail-open property and is the one to
write first: it is the test that fails if the guard is ever refactored
away.

## 9. Relationship to the other roadmap items

This is one of four items from the context-engineering review
(2026-08-04):

1. **rule staleness** (this SPEC) — the only correctness issue of the
   four; a stale rule blocks correct code.
2. **drift-tool consolidation** — a hard prerequisite here, since the
   report ships as a `code` source rather than a fourth drift tool.
3. **rule retirement** — consumes this SPEC's output. A permanently
   stale binding is one retirement signal among several; `phr-mcp
   stats` already surfaces never-fired rules as another.
4. **subagent hook governance** — independent.

The pairing with (3) is deliberate but deferred. Detection and
retirement are separable, and a detector that only demotes and reports
is safe to ship before the policy that acts on it.
