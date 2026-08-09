# SPEC: rule-staleness — rules that outlived the code they describe

**Status:** draft (revision 2 — adversarial review by glm-5.2 and codex, 2026-08-04)
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
changes, and a rule whose every binding has vanished is reported as
drift and demoted from `block` to `warn` at hook time.

## Goals

- A `.phronesis/bindings.json` recording which rules name which code
  entities, written only by the graph-sync pipeline.
- Detection of **referent-gone** staleness: a rule named a symbol, that
  symbol resolved against the graph at least once, and neither it nor
  any relocation of it now resolves.
- Automatic `block` → `warn` demotion of stale rules at pre-check,
  guarded on graph freshness and generation agreement, reusing the
  existing `hook_logged::demote_violations_from` seam.
- A `code` source in the consolidated drift tool, so the model can ask
  why a rule demoted mid-session and a human can triage the list.
- Exclusion of the dominant false-positive class: a symbol with **no**
  local definition (`.unwrap()`, `panic!()`, `console.log`) can never
  bind, and therefore can never be reported stale. This is a bounded
  claim, not a claim of zero false positives — see §2.5.

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
  TypeScript all emit under a uniform `::`-delimited naming scheme
  (`graph/python.rs:58`, `graph/typescript.rs:86`). No per-language
  inference code, and no keyword filtering — see §2.1.
- **Non-graph rules.** Rules whose literals name no code entity (the
  `llm` deflection pack, for instance) never bind and are never
  evaluated. That is correct: there is no referent to lose.
- **Concurrency hardening of graph-sync.** See §10.

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
> some earlier point, and neither that definition nor any relocation
> of it now resolves.

The earlier resolution is recorded when it happens. A symbol that
never resolves is never recorded, so it can never transition, so it
can never be reported stale. That excludes the dominant false-positive
class by construction rather than by threshold — which matters,
because a threshold that silently demotes real rules is a worse
failure than no detector at all. It does **not** make the detector
false-positive-free; §2.5 states the residual class.

## 2. Binding

### 2.1 Candidate extraction

Candidate extraction is a lexer, not a sequence of prose string
operations. Order-dependent suffix stripping is unspecifiable: given
`foo().await`, stripping `().await` yields `foo` while stripping `()`
first yields `foo.await`, and the spec cannot silently depend on which
a given implementation chose.

The algorithm is:

> Scan the literal for maximal runs matching `[A-Za-z_][A-Za-z0-9_]*`.
> Each distinct run is a candidate.

Nothing is stripped, because nothing needs to be. Decoration
characters (`.`, `(`, `)`, `!`, `:`, whitespace) are not in the
identifier character class, so they terminate runs naturally.

| Literal | Candidates |
|---------|-----------|
| `.execute_all_agenda_items().await` | `execute_all_agenda_items`, `await` |
| `.unwrap()` | `unwrap` |
| `fire_all_consequences(` | `fire_all_consequences` |
| `Result<_, String>` | `Result`, `String` |

The algorithm is deliberately generous and deliberately dumb. It
over-generates — `await`, `Result`, and `String` are all candidates —
and that is harmless, because a candidate that does not resolve
against `defines_fn` is never recorded (§2.2). Over-generation costs
nothing; under-generation silently loses the protection the rule was
supposed to get.

There is **no keyword filter.** An earlier draft discarded candidates
that were "language keywords," which is per-language inference
smuggled into a section that claims not to do any: `match` is a Rust
keyword and a valid TypeScript method name, `class` is a Python
keyword and a valid Rust identifier. Resolution against the actual
graph already answers the only question that matters — does a local
definition with this name exist? — and answers it per-project rather
than per-language.

### 2.1.1 Which predicates contribute

Only `new_content_contains` contributes candidates.

`bash_command_matches` holds a **regex**, and is excluded entirely.
An earlier draft proposed extracting "bare identifier runs" from regex
source while discarding candidates containing metacharacters. Those
two rules contradict each other: `cargo\s+test.*execute_all_agenda_items`
contains metacharacters *and* a bare identifier run, so it is
undefined whether it binds. Distinguishing a literal token from a
character class, an escape, or an alternation branch requires parsing
the regex to an AST, which is disproportionate machinery for a
detector whose motivating rules are all `new_content_contains`.

Excluding regex predicates loses nothing that motivated this SPEC and
removes an entire class of wrong bindings. If a real need appears
later, a regex AST walk can add the capability without changing
anything else here.

### 2.2 Resolution

A candidate **resolves** if it equals the last `::`-delimited segment
of the symbol argument of any `defines_fn` edge.

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

A candidate resolving to one or more definitions produces one binding.
The binding records the **fully-qualified paths** of every definition
it resolved to, not merely the leaf name. Leaf matching *establishes*
a binding; fully-qualified paths *are* the binding. This distinction
is what §3.2 depends on.

### 2.3 Rule identity

A binding is keyed on `(rule_id, rule_hash, symbol)`, where
`rule_hash` is a content hash of the rule's canonical serialized form.

Keying on `rule_id` alone is unsound. Rule IDs are stable names, not
stable content: an author may rewrite a rule's conditions and message
while keeping its ID, and packs are refreshed wholesale by
`init --rules-only --force`. A binding recorded against the old
content would then demote a rule that never named the lost symbol at
all — silently weakening an unrelated, possibly security-relevant
rule.

Any binding whose `rule_hash` does not match the current rule's hash
is **discarded, not migrated**. The rule re-binds from scratch on the
next reconciliation, which means an edited rule starts with no
staleness history. That is correct: its premises are new, and it has
not yet demonstrated that any of them ever held.

### 2.4 File format

`.phronesis/bindings.json`, written only by graph-sync:

```json
{
  "version": 1,
  "generation": 41,
  "bindings": [
    {
      "rule": "block-await-on-sync-execute-all-agenda-items",
      "rule_hash": "9f2c1ab4e70d5583",
      "symbol": "execute_all_agenda_items",
      "bound_to": ["crate::engine::Agenda::execute_all_agenda_items"],
      "surviving": ["crate::engine::Agenda::execute_all_agenda_items"],
      "relocated": [],
      "bound_at": 1754300000,
      "stale_at": null,
      "state": "bound"
    }
  ]
}
```

| Field | Meaning |
|-------|---------|
| `generation` | the graph generation this file was computed from (§3.1) |
| `rule_hash` | content hash of the bound rule (§2.3) |
| `bound_to` | **immutable** — the fully-qualified paths at first resolution |
| `surviving` | the subset of `bound_to` still present in the graph |
| `relocated` | same-leaf definitions at paths *not* in `bound_to` |
| `bound_at` | first resolution; never rewritten |
| `stale_at` | when `surviving` first became empty, else `null` |
| `state` | `bound`, `moved`, or `stale` (§3.2) |

`bound_to` is never rewritten. An earlier draft updated the definition
set on every reconciliation, which allowed a binding to migrate from
`A::foo` to `B::foo` to `C::foo` across unrelated functions while
reporting a continuous history since `bound_at`. Separating the
immutable original (`bound_to`) from the current view (`surviving`,
`relocated`) makes migration impossible to express and makes
`bound_at` mean what §5 claims it means.

`stale_at` exists because §5 promises to report how long a binding
held before it broke, and `bound_at` alone cannot answer that.

The file is derived state, but deleting it is **not** free — see §6.

### 2.5 Residual false positives: shadowing

Leaf-segment matching means a *local* definition sharing a name with a
foreign symbol will resolve. A project defining its own
`fn unwrap(...)` gives the `rust` pack's `.unwrap()` literal a real
binding — and deleting that local function would then report
`no-unwrap-in-src` as stale, which is wrong. The rule is about the
`std` method and never depended on the local one.

The exclusion claimed in §1 is therefore precise about its scope: a
symbol with **no** local definition can never be reported stale. A
symbol *shadowed* by a local definition can be. This is the residual
false-positive class, and the Goals section is worded to claim only
the bounded property.

It is mitigated rather than solved. The `binds: false` rule field
(§4.4) suppresses extraction for rules whose literals are known to
name foreign symbols, and is the documented remedy when a pack rule
collides with a local name. Solving it properly needs call-site
resolution — knowing which `unwrap` a given `.unwrap()` refers to —
which is a type-resolution problem well beyond what a leaf-segment
graph can answer.

## 3. Reconciliation

### 3.1 When, and the generation stamp

Reconciliation runs inside `graph::sync`, after the graph updates and
derived edges are regenerated. This is the only writer of
`bindings.json`.

Sync already walks the graph on every save and already maintains the
staleness index, so reconciliation adds a pass over an in-memory edge
set that is already loaded. Nothing is added to the pre-check hot
path: pre-check reads a bounded file and does no resolution.

**The graph and the bindings are two separate files and cannot be
written atomically.** An earlier draft claimed "the verdict cannot
disagree with the graph." That is false. If the graph write succeeds
and the bindings write fails — or the process is killed between them —
the two files describe different edge sets, and the dangerous
direction is real: a restored function leaves a stale-marked binding
on disk while the graph shows the function present, and pre-check
demotes an armed rule with no justification.

The fix is a **generation counter**, not an ordering trick:

- The graph staleness index carries a monotonically increasing
  `generation`, incremented on every successful graph write.
- `bindings.json` stamps the `generation` it was computed from.
- Pre-check ignores `bindings.json` entirely unless its `generation`
  **equals** the index's current generation.

A failed or interrupted bindings write leaves an older generation on
disk, which pre-check declines to use, which means no demotions —
full enforcement. The bindings file is written *after* the graph, so
the mismatch is always in the ignorable direction. Crash-consistency
therefore does not depend on write ordering being atomic; it depends
only on the stamp being compared.

Both files are written by atomic replace (write temp, fsync, rename),
so a partially-written file is not observable.

### 3.2 The three states

On each reconciliation, every retained binding (§3.3) is re-resolved
against the current graph. `surviving` is recomputed as the subset of
`bound_to` still present; `relocated` as the set of same-leaf
definitions at paths outside `bound_to`.

| Condition | State | Demotes? | Reported? |
|-----------|-------|----------|-----------|
| `surviving` non-empty | `bound` | no | no |
| `surviving` empty, `relocated` non-empty | `moved` | **no** | yes |
| `surviving` empty, `relocated` empty | `stale` | yes | yes |

The `moved` state exists because the two obvious designs are each
wrong in one direction, and review surfaced both:

- **Strict full-path identity** would report a *moved* function as
  stale. Moving `Agenda::execute_all_agenda_items` to
  `Agenda2::execute_all_agenda_items` does not invalidate the rule,
  and demoting it there is a false positive.
- **Pure leaf matching** would miss a *deleted* function whenever any
  unrelated same-named definition exists. Deleting `Agenda::run` while
  `Cleanup::run` remains would leave the rule silently bound to an
  entity it was never about — a false negative on the exact case the
  SPEC exists to catch.

Three states separate the cases instead of trading one error for the
other: a move keeps full enforcement but becomes visible in the
report, and a deletion demotes only when no same-named definition
survives anywhere. Neither outcome is silent, and `bound_to` is
preserved in both, so the report can always say what the rule
originally named.

A `moved` binding is deliberately *not* auto-adopted into `bound_to`.
Adoption is the migration path that this design exists to prevent; if
the move is legitimate, the remedy is to update or re-author the rule,
which produces a new `rule_hash` and a fresh binding (§2.3).

### 3.3 Pruning

Before re-resolution, every binding is discarded whose:

- `rule` no longer exists in `rules.json`, or
- `rule_hash` does not match that rule's current hash (§2.3), or
- `symbol` is no longer among the rule's extracted candidates.

Without pruning, a rule edited to stop naming `foo` keeps `foo`'s
stale binding forever and stays demoted permanently, and deleted rules
accumulate in the drift report.

### 3.4 Recovery

A binding in `state: "stale"` whose `bound_to` paths reappear returns
to `bound` and clears `stale_at`. Reverting a bad refactor, restoring
a deleted file, or checking out a branch where the function still
exists re-arms the rule with no manual step.

Recovery is evaluated against `bound_to`, never against leaf matching.
A stale binding for `Agenda::execute` is **not** re-armed by an
unrelated module later defining `Logger::execute`; that produces
`relocated`, hence `moved`, hence a report rather than a silent
re-arm.

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

### 4.2 Rule-level demotion requires all bindings stale

A rule contributes to the demotion set only when **every** binding it
holds is `stale`. A rule with any `bound` or `moved` binding keeps
full force.

A rule may name several entities — one carrying its premise, others
incidental. Demoting the whole rule because an incidental symbol
disappeared would weaken enforcement on the strength of a name the
rule never depended on. Candidate extraction cannot tell which symbol
carries the premise (that is premise-inverted staleness, out of
scope), so the conservative reading is the only sound one: demote only
when nothing the rule named survives anywhere.

### 4.3 The guards, and their order

Pre-check applies three checks, **in this order**, before any
stale-rule ID enters the demotion set:

1. Graph freshness is `Freshness::Fresh`. Under `Stale(_)` or
   `Outdated { .. }`, stop — no demotions.
2. `bindings.json` parses and its `generation` equals the index's
   current generation (§3.1). Otherwise stop — no demotions.
3. For each rule, every binding is `stale` (§4.2).

The ordering is normative, not descriptive. An implementation that
loads `bindings.json` and unions its stale IDs *before* checking
freshness defeats guard 1 entirely while satisfying every other
sentence in this document. The earlier draft left this unpinned.

Guard 1 is load-bearing for a specific reason. A graph that has not
been rebuilt after a `git checkout` describes files that no longer
exist and functions that were never removed. Evaluating bindings
against it would report mass symbol deletion and demote much of the
rule pack at once — silently disarming enforcement at exactly the
moment (a branch switch) when the tree is least familiar.

### 4.4 The `binds: false` escape hatch

An optional rule field, `binds: false`, suppresses both candidate
extraction and demotion for that rule.

This is required, not optional, because §5 names a legitimate rule
class that this SPEC would otherwise break: a rule that deliberately
guards against *reintroducing* a pattern that was removed on purpose.
Such a rule is permanently referent-gone by design and must keep
blocking to do its job. Automatic demotion would defeat its entire
purpose, and the report telling a human "this is fine, leave it" would
be advising them to accept a rule that has already stopped working.

`binds: false` is also the documented remedy for the shadowing case in
§2.5.

Reading it costs nothing new: reconciliation already parses
`rules.json` to extract candidates.

## 5. Reporting

The report is a source of the consolidated drift tool
(SPEC-drift-consolidation), not a standalone surface:

```
phr-mcp drift --source code
```

MCP: `get_drift(source: "code")`.

This follows the existing drift contract. Output is a triage list, not
ground truth: it says a rule names something the graph no longer
defines at the path it originally resolved to, which is evidence that
the rule needs review — not proof that the rule is wrong.

Each entry reports the rule ID, the symbol, `bound_to`, the state
(`moved` or `stale`), `relocated` paths where applicable, and the
interval from `bound_at` to `stale_at`.

`moved` entries are reported without demotion, and are the more
actionable half of the report in practice: they say "this rule still
matches something, but not the thing it was written against."

## 6. Failure behavior

Every failure path fails **open toward full enforcement**, matching
`load_index`, which treats a missing index as "nothing known yet"
rather than failing closed:

| Condition | Behavior |
|-----------|----------|
| `bindings.json` missing | empty stale set; no demotions; sync rebuilds it |
| unreadable or malformed | empty stale set; file quarantined to `.bak`; diagnostic; rebuilt next sync |
| `version` unrecognized | file ignored; rebuilt from scratch |
| `generation` ≠ index generation | file ignored entirely (§3.1) |
| write fails during sync | logged; the save is not failed; generation mismatch makes the stale file inert |
| graph not `Fresh` | evaluation skipped entirely (§4.3) |

Every degraded state above results in *more* enforcement, not less.

**One honest exception.** Losing `bindings.json` — to corruption,
manual deletion, or a `.phronesis/` clean — loses the *transitions*,
not just the records. On rebuild, every symbol currently resolvable
re-binds as `bound`, and every binding that had been correctly marked
`stale` is silently re-armed to `block` against a referent that is
still missing. That is a return to the pre-feature behavior — an
authoritative false block, the exact thing this SPEC exists to prevent
— reached through the recovery path.

This is accepted rather than solved. Persisting transitions
independently of the derived file would mean a second durable store
with its own corruption modes, and the failure is bounded: it recreates
the status quo ante rather than introducing a new defect, and the next
genuine deletion re-detects it. It is documented here so that
"just delete the file" is never recommended as a casual remedy.

## 7. Module boundaries

**`graph::bindings`** (new). Owns the format and the logic. Core is
pure: `extract(rule) -> Vec<Candidate>`,
`resolve(candidates, edges) -> Vec<Binding>`, and
`reconcile(persisted, rules, edges, generation) -> BindingSet`. None
touch the filesystem or the hook payload; a thin `load`/`store` edge
does the I/O. Testable with hand-built edge vectors and no fixtures.

**`graph::sync`** (extended). Calls `reconcile` after the graph
updates and persists the result with the current generation. The only
writer.

**`hook::pre`** (extended). Applies the §4.3 guards in order and
unions the surviving IDs. Reads only; never resolves.

The seam that matters: `bindings` knows nothing about hooks and `pre`
knows nothing about resolution. Extraction is testable without a hook
and demotion is testable without a graph.

## 8. Testing

Unit, in `graph::bindings`:

- extraction is a pure identifier-run scan: `foo().await` yields
  exactly `{foo, await}` regardless of implementation order
- `bash_command_matches` clauses contribute no candidates (§2.1.1)
- a candidate resolving to no definition is not recorded
  (`.unwrap()` never appears in `bindings.json`)
- a binding whose `bound_to` paths all vanish, with no same-leaf
  definition anywhere, becomes `stale`
- a binding whose `bound_to` path vanishes while a same-leaf
  definition exists elsewhere becomes `moved`, **not** `stale`
- `bound_to` is never rewritten across any state transition —
  pinning the anti-migration property
- a stale binding is **not** recovered by an unrelated same-leaf
  definition appearing (§3.4)
- a binding whose `rule_hash` no longer matches is discarded, not
  reused (§2.3)
- a binding whose symbol left the rule's candidate set is pruned (§3.3)
- `stale_at` is set on the transition into `stale` and cleared on
  recovery
- a locally-defined symbol shadowing a foreign one *does* bind
  (§2.5) — pinning the known limitation so a future call-site
  resolver has a test to flip rather than a surprise to discover

Integration, through the real binary — the safety-property tests come
first, because they are the ones whose failure is silent:

- a rule blocking an edit still blocks it while its referent exists
- after the referent is deleted and the graph resynced, the same edit
  **warns** and the diagnostic names the lost symbol
- with the graph marked stale, the same edit **still blocks**
  (guard 1)
- with `bindings.json` holding a generation older than the index, the
  same edit **still blocks** (guard 2)
- a rule holding one stale and one surviving binding **still blocks**
  (§4.2)
- a rule with `binds: false` whose referent is deleted **still blocks**
  (§4.4)
- a rule whose ID is reused for different content does **not** inherit
  the previous rule's stale binding (§2.3)

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

Review of this SPEC surfaced a fifth item that was not on the original
list: **graph-sync serialization** (§10). It is a pre-existing defect
in our own pipeline that this SPEC makes more consequential, and it
needs its own SPEC rather than a footnote here.

## 10. Known limitations

**Concurrent graph-sync writers — our defect, deferred deliberately.**
Two saves racing into `graph::sync` can interleave: both read
generation N, and the later write can land a bindings set computed
from a partial view. The generation stamp detects graph/bindings
*skew* but not lost updates between two concurrent syncs.

We wrote this pipeline (`graph/sync.rs`, SPEC-triple-store-rete), and
it has had this race since it shipped. This SPEC does not introduce
it, but it does raise the stakes: today a lost update means a slightly
stale graph, and after this SPEC it can also mean a rule demoted or
left armed on the strength of a partial view. Calling that "inherited"
would be a way of not deciding.

The decision is to defer, and the reason is scope rather than
ownership: correct serialization is a lock file or a compare-and-swap
on generation around the whole sync pass, which changes the contract
for every consumer of the graph — the PostToolUse sensor, `graph
rebuild`, `graph status`, and the audit path. Making that change
inside a spec about rule lifecycle would put a pipeline-wide
concurrency revision behind a review aimed at something else, which is
how contracts get changed without anyone noticing.

**This is tracked as its own work item, not left as a note.** It needs
a SPEC covering graph-sync serialization, and it should be written
before this one ships if the demotion behavior is to be trusted under
concurrent saves. If it is not written first, the honest statement at
ship time is that demotion is reliable for serialized saves and
best-effort under concurrent ones.

Until then the consequence is bounded by the same asymmetry as
everywhere else: a lost update leaves an older or partial bindings
set, and a generation mismatch makes it inert.

**Time-of-check/time-of-use at pre-check.** Freshness is observed,
then bindings are loaded; a save landing between them means decisions
from the prior snapshot are applied to a newer tree. The window is
small and the failure is one hook invocation of over- or
under-demotion, self-correcting on the next call. Closing it requires
the same snapshot discipline as the item above, and belongs to the
same follow-up SPEC.
