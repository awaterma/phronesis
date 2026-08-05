# SPEC: drift-consolidation — one drift tool, four sources

**Status:** draft
**Authors:** Claude, Andrew Waterman (design pass by glm-5.2 and codex)
**Date:** 2026-08-04
**Target release:** phronesis-mcp 0.25.0 (MINOR — MCP tool surface change, new file surface)
**Blocks:** SPEC-rule-staleness (which registers the `code` source against
this spec's registry)
**Affects:** new `crates/phronesis-mcp/src/drift/{mod,types,registry,render}.rs`;
              `crates/phronesis-mcp/src/{server.rs, main.rs, init.rs}`,
              `crates/phronesis-mcp/src/{claude_md_drift,memory_drift,wiki_drift}.rs`
              (become source adapters; scoring logic unchanged),
              `crates/phronesis-mcp/CLAUDE.md`, `crates/phronesis-mcp/README.md`.
              No `phr` library-crate change.

## Premise

Phronesis exposes 34 MCP tools. Three of them answer the same question
against different corpora:

| Tool | Corpus | Lines |
|------|--------|-------|
| `get_claude_md_drift` | imperative bullets in `CLAUDE.md` | 506 |
| `get_memory_drift` | Claude Code auto-memory entries | 1026 |
| `get_wiki_drift` | ADR decisions under `.phronesis/wiki/decisions/` | 652 |

Each asks: *what guidance exists here that no current rule enforces?*
Each returns a triage list. Each has its own `DriftItem`,
`DriftReport`, `DriftError`, `render_table`, and `render_json`.

Anthropic's context-engineering guidance is direct about this shape:
tools should have "minimal overlap in functionality," and "if human
engineers cannot definitively identify which tool to use, agents will
struggle similarly." Three tools differing only in which directory
they read is the named failure mode — a bloated tool set with
ambiguous decision points.

**The stronger argument is not the tool count.** `phr-mcp init` ships
a `.phronesis/durable.md` whose drift-discipline section names all
three tools across roughly fifteen lines (`init.rs:820-834`). That
file is re-injected into the model's context at **every SessionStart
and every UserPromptSubmit**. Three tool names, three descriptions,
three sets of guidance on when to call which — paid for on every
single interaction, forever.

Consolidating to one tool with a `source` parameter turns fifteen
re-injected lines into roughly five. The tool registry shrinks by two
entries once; the per-turn context cost shrinks every turn. For a
project whose founding claim is that durable guidance must survive
context compression, spending that budget on three names for one
capability is the wrong trade.

A fourth source is already specced (SPEC-rule-staleness: rules naming
code entities the structural graph no longer defines). Adding it as
`get_rule_staleness` would make four near-identical tools. This SPEC
exists so that it lands as `get_drift(source: "code")` instead.

## Goals

- One MCP tool, `get_drift`, replacing three.
- A **source registry** that new drift sources plug into without
  touching the MCP surface — so SPEC-rule-staleness adds a source, not
  a tool.
- A common envelope that preserves what each source actually knows,
  rather than flattening four vocabularies into a lowest common
  denominator.
- Evidence typing that makes a token-overlap guess distinguishable
  from a graph fact **without reading documentation**.
- A token-bounded default response (§5), because the durable directives
  nudge the model to call this tool routinely.
- A migration that does not leave existing projects re-injecting the
  names of tools that no longer exist (§6).

## Out of scope

- **Rewriting the scoring logic.** `claude_md_drift`, `memory_drift`,
  and `wiki_drift` keep their extraction, Jaccard scoring, bucketing,
  and `suggest_rule` implementations verbatim. They become adapters
  behind the registry. Their existing unit tests keep passing
  unchanged; that is the regression signal for this refactor.
- **Semantic matching.** Replacing Jaccard token overlap with
  embeddings is a real improvement and a separate SPEC. This one moves
  code without changing what it computes.
- **The `code` source.** Defined and registered by SPEC-rule-staleness.
  This SPEC defines the extension point and does not stub the source.
- **A separate crate.** The design pass proposed extracting
  `crates/phronesis-drift`. There is no consumer outside
  `phronesis-mcp`; a crate boundary here would be structure without a
  second consumer to justify it.

## 1. Source registry

Enum dispatch, not trait objects. Sources are a closed set, their
inputs differ structurally, and there is no plugin story to serve. A
trait would force either a god-args struct or `Box<dyn Any>` inputs;
an enum lets the compiler prove exhaustiveness when a source is added.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    ClaudeMd,
    Memory,
    Wiki,
    Code, // registered by SPEC-rule-staleness
}

impl Source {
    pub const ALL: &'static [Source] =
        &[Self::ClaudeMd, Self::Memory, Self::Wiki, Self::Code];
}
```

Inputs are built by the MCP/CLI layer from request parameters and
workspace state, never by the drift core, which stays pure over them:

```rust
pub struct SourceInputs<'a> {
    pub rules: &'a [SourceRule],
    pub durable: Option<&'a str>,       // memory scores against rules + durable
    pub claude_md: Option<&'a Path>,
    pub memory_dir: Option<&'a Path>,
    pub wiki_dir: Option<&'a Path>,
    pub graph: Option<&'a GraphView>,   // supplied by SPEC-rule-staleness
}
```

### 1.1 An absent corpus is data, not an error

Today each tool fails with a bespoke error when its corpus is missing
— `DriftError::ClaudeMdMissing`, `MemoryDirMissing`,
`Wiki(WikiError::DirMissing)`. That is defensible for three separate
tools and wrong for one aggregate tool: on a freshly-`init`ed project
the wiki and memory directories legitimately do not exist yet, and
`source: all` must not hard-fail because two of four corpora are
absent.

Availability is therefore resolved before any scoring runs, and
distinguishes *absent* from *malformed*:

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Availability {
    /// Corpus present and read.
    Present { scanned: usize },
    /// Corpus not present. Expected on a fresh project; not a fault.
    Missing { reason: MissingReason },
    /// Corpus present but unreadable or unparseable. Bug-shaped.
    Errored { detail: String },
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MissingReason { NoFile, NoDir, NoGraph, NotInitialized }
```

`Missing` is reported, never raised. `Errored` is reported per source
and does not abort the other sources (§5.2).

## 2. Envelope types

```rust
#[derive(Debug, Clone, Serialize)]
pub struct DriftReport {
    pub source: Source,
    pub availability: Availability,
    pub uncovered_count: usize,
    pub items: Vec<DriftItem>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DriftItem {
    /// Bullet text, memory entry name, ADR id + title, or rule id.
    pub subject: String,
    pub verdict: Verdict,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>, // draft rule JSON, as today
    pub evidence: Evidence,
}
```

### 2.1 Verdict: union, not intersection

The three sources classify differently — `claude_md` has only a
similarity threshold, `memory` has `{actionable, ambient, personal}`,
`wiki` has `{covered, likely-covered, uncovered, superseded}`.
Collapsing to `{covered, uncovered}` would discard exactly the
information an operator uses to decide what to do: whether an
uncovered memory entry belongs in a rule or in `durable.md`, and
whether an uncovered decision is live or superseded.

So `Verdict` is the union, with a cheap grouping helper so consumers
are not forced into an exhaustive match:

```rust
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Covered,             // claude_md, wiki
    LikelyCovered,       // wiki
    Uncovered,           // claude_md
    ActionableUncovered, // memory — should become a rule
    AmbientUncovered,    // memory — should go in durable.md
    Personal,            // memory — stays in MEMORY.md, not drift
    Superseded,          // wiki
    Moved,               // code (SPEC-rule-staleness §3.2)
    Stale,               // code (SPEC-rule-staleness §3.2)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Family { Covered, Superseded, Uncovered, Broken }

impl Verdict {
    pub fn family(self) -> Family { /* … */ }
}
```

`Family` orders by triage urgency (`Broken > Uncovered > Superseded >
Covered`) and is what the renderer sorts on. Each source emits only
the variants its existing code already produces.

`uncovered_count` counts `Family::Uncovered` and `Family::Broken`
only. It deliberately excludes `Personal` (a memory entry that belongs
in `MEMORY.md` is not drift — nothing is missing) and `Superseded` (a
decision explicitly replaced by a later one needs no rule). Counting
either would inflate the number an operator uses to decide whether
there is work to do, which is the only number in the summary response
that drives a decision.

## 3. Evidence: heuristic and structural do not share a shape

There are **three** kinds of evidence, not two:

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Evidence {
    /// An author wrote down which rules enforce this. Not inferred.
    /// wiki's `enforces: [rule-id]` frontmatter.
    Declared {
        rules: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        superseded_by: Option<String>,
    },
    /// Token-overlap heuristic. A triage hint, not ground truth.
    Heuristic {
        score: f64,
        threshold: f64,
        matched_rules: Vec<String>,
    },
    /// Resolved against the code graph. Either it resolves or it does not.
    Structural {
        symbol: String,
        bound_to: Vec<String>,
        resolves: bool,
    },
}
```

An earlier draft had two variants and folded `enforces:` frontmatter
into `Heuristic` as a `method: FrontmatterDeclared` discriminator.
That was wrong, and the review that caught it put the objection
precisely: a declaration is neither a token-overlap score nor a graph
fact. Keeping it inside `Heuristic` forces a `score` and a `threshold`
onto a record where neither exists — either fabricating `1.0`/`0.0` or
making the fields optional and meaningless. Fabricating `1.0` is
exactly the false comparability this section claims to prevent, one
level down.

Three properties are deliberate:

**No shared field across variants.** A consumer distinguishes them by
the serde `kind` tag with no documentation lookup, which is what
"extremely clear with respect to their intended use" requires.

**No `confidence` on `Structural`.** A graph fact has a boolean, not a
score. Adding one so the variants line up would manufacture false
comparability between a Jaccard 0.71 and a path that either exists or
does not.

**No `score` on `Declared`.** Same reason. An author saying "rule R-022
enforces this" is not a measurement, and rendering it beside a
Jaccard 0.71 as though both were confidences would misrepresent the
strongest evidence the system has.

## 4. Renderers

One `render_table(&[DriftReport])` and one `render_json(&[DriftReport])`
replace six functions. The table flattens items with their owning
source prepended, sorted by `Family` then source:

```
SOURCE     VERDICT      SUBJECT                                   EVIDENCE
code       stale        block-await-on-sync-execute-all-agenda…   structural resolves=false
memory     actionable   parallel-sessions                         heuristic jaccard=0.31 <0.40
claude_md  uncovered    "Prefer ? over manual match arms"         heuristic jaccard=0.22 <0.40
wiki       superseded   ADR-003 "use libc directly"               heuristic frontmatter → ADR-007
```

Evidence rendering is a renderer-local function, not a `Display` impl,
so the pure types never import a formatting concern.

## 5. Aggregation and the token budget

### 5.1 `all` is a summary, not four reports

`source` defaults to `all`. A naive `all` concatenates four full
reports — on a mature project, hundreds of items — into a tool
response the model is nudged to request routinely by `durable.md`.
That would spend on one tool call the budget this SPEC's premise
claims to save.

So the default response is bounded:

- `source: all` returns per-source availability, `uncovered_count`,
  aggregate totals by `Family`, and the **top `limit` items per source**
  ordered by `Family`, with `truncated: true` when items were dropped.
- `limit` defaults to **5** per source and is capped at 50.
- Naming a single source (`source: "wiki"`) returns that source's full
  report, still subject to `limit`.

This is progressive disclosure applied to the response rather than the
tool list: the cheap call says *where* drift is, and a second call says
*what* it is. The truncation is always explicit — a silent cap would
read as "nothing more to see," which is the failure the omission
footer in `context/packing.rs` already exists to prevent elsewhere.

### 5.2 One source failing does not fail the call

Each source runs independently. A source that errors contributes a
`DriftReport` with `availability: Errored` and `items: []`. The
aggregate returns successfully with `sources_errored > 0`.

An operator asking "what drift exists" should not get a hard failure
because one of four corpora is malformed — particularly when the
malformed corpus may be the very thing they are trying to diagnose.

```rust
#[derive(Debug, Clone, Serialize)]
pub struct AggregateReport {
    pub sources: Vec<DriftReport>,
    pub totals: Totals,
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Totals {
    pub sources_present: usize,
    pub sources_missing: usize,
    pub sources_errored: usize,
    pub uncovered_total: usize,
    pub by_family: BTreeMap<Family, usize>,
}
```

## 6. Migration

The three MCP tools are **removed**, not deprecated. Pre-1.0, a MINOR
bump, and leaving them registered would forfeit the entire point.

### 6.1 The durable.md hazard — the load-bearing part

`init.rs:904-910` returns early when `.phronesis/durable.md` already
exists:

```rust
if path.exists() {
    report.steps.push("= .phronesis/durable.md already exists — leaving unchanged …");
    return Ok(());
}
```

That is correct behavior for a file operators are told to edit in
place. It also means **every existing phronesis project keeps a
`durable.md` naming three tools that will no longer exist** — and that
file is re-injected at every SessionStart and every UserPromptSubmit.

Without a migration, upgrading turns a per-turn context cost into a
per-turn context *hazard*: every turn, forever, the model is instructed
to call `get_claude_md_drift`, discovers no such tool, and burns a
failed tool call plus recovery reasoning. The change would make the
context problem it exists to solve strictly worse for every installed
project.

The migration is therefore required in the same PR:

1. Add a `<!-- phronesis:durable-schema=2 -->` marker to the shipped
   template. Files with no marker are schema 1.
2. On `init` (including `--hooks-only` and `--rules-only`, which
   otherwise skip this file), if `durable.md` exists at schema 1 and
   contains the drift-discipline section verbatim as shipped, rewrite
   **only that section** in place, back up to `durable.md.bak`, and
   stamp schema 2.
3. If the section has been edited, do not rewrite. Report a diagnostic
   naming the file and the three dead tool names, so the operator can
   fix a file they own rather than having their edits overwritten.

Rewriting only an unmodified section, and refusing to touch an edited
one, keeps the existing "edit in place to customize" contract intact.

### 6.2 Everything else in the same PR

| Surface | Change |
|---------|--------|
| `server.rs` | remove three `#[tool]` registrations; add `get_drift` |
| `server.rs:1643`, `server.rs:1710` | invert the registration canaries (§8) |
| `server_params.rs:244,251,263` | remove three param structs; add `GetDriftParams` |
| `main.rs` | add `drift --source`; keep three subcommands as aliases |
| `main.rs:147` | **remove `alias = "drift"` from `claude-md-drift`** (§6.3) |
| `init.rs:820-834` | rewrite the durable template's drift section |
| `CLAUDE.md`, `README.md`, `AGENTS.md` | one `get_drift` section replacing three; fix any stated tool count |
| `docs/loop-programming-guide.md` | update tool references |
| action log | one `get_drift` event replacing three, with `selection`, `sources_present`, `sources_missing`, `sources_errored`, `items_total`, `uncovered_total` |

CLI subcommands `claude-md-drift`, `memory-drift`, and `wiki-drift`
are **kept** as thin aliases forwarding to `drift --source X`. CLI
surface costs nothing on the model's attention budget — only the MCP
tool registry does — and removing them would break scripts and muscle
memory for no benefit.

There is deliberately **no MCP deprecation shim.** Keeping the three
tools registered as forwarding stubs would preserve exactly the
overlap this SPEC exists to remove, and would leave the durable.md
line count — the actual cost — unchanged. Callers get a release-note
mapping table and a "method not found" migration example instead.

### 6.3 `drift` is not a free name

`main.rs:147` currently reads:

```rust
#[command(name = "claude-md-drift", alias = "drift")]
```

So `phr-mcp drift` today means *scan CLAUDE.md*. Promoting `drift` to
the multi-source command silently changes what that invocation does —
from one corpus to four — for anyone with it in a script or in muscle
memory. It will not error; it will quietly do more.

The alias is removed in the same PR, and the release notes call the
change out explicitly. This is the one genuinely silent behavior
change in the migration, which is why it gets its own subsection
rather than a table row.

## 7. Module boundaries

**`drift::types`** — the envelope: `Source`, `Availability`,
`DriftReport`, `DriftItem`, `Verdict`, `Family`, `Evidence`. Pure data,
serde only, no I/O, no formatting.

**`drift::registry`** — `run_source(Source, &SourceInputs) -> DriftReport`
and `run_all(&[Source], &SourceInputs) -> AggregateReport`. Dispatch
and availability resolution. No scoring.

**`drift::render`** — table and JSON rendering over `&[DriftReport]`.

**`claude_md_drift`, `memory_drift`, `wiki_drift`** — unchanged
scoring, plus a small `into_items()` adapter mapping their existing
types into the envelope. The adapter is the only new code in these
files.

The seam: adapters depend on `types`, and `types` depends on nothing.
Adding a source touches `registry` and one adapter, never `server.rs`.

## 8. Testing

The per-source scoring tests are untouched and are the regression
signal for the refactor — if `memory_drift`'s existing tests still
pass, its scoring did not change.

New tests cover the consolidation itself:

- `get_drift` is registered and the three old tools are **not**
  (inverting `server.rs:1643` and `server.rs:1710` rather than deleting
  them — they were written to catch a SPEC-vs-code gap, and the
  inverted assertion catches an incomplete removal)
- a missing corpus yields `Availability::Missing`, not an error, and
  `source: all` still succeeds with the remaining sources
- a malformed corpus yields `Availability::Errored` and does not
  suppress other sources' items
- `all` truncates at `limit` per source and sets `truncated: true`
- `limit` is clamped to 50
- each adapter maps its full verdict vocabulary — every `Verdict`
  variant is produced by at least one source
- `uncovered_count` excludes `Personal` and `Superseded`
- wiki's `enforces:` frontmatter maps to `Evidence::Declared`, and its
  Jaccard fallback to `Evidence::Heuristic` — the two must not collapse
- `Evidence::Structural` serializes with no `score`, `Declared` with no
  `score`, and `Heuristic` with no `resolves` (pinning §3's
  no-shared-shape property against a well-meaning future merge)
- each retained CLI alias produces JSON identical to its canonical
  `drift --source X` invocation, and `--suggest` survives the alias
  translation
- `drift` with no `--source` defaults to `all`

Migration tests:

- an unmodified schema-1 `durable.md` is rewritten and stamped schema 2
- an **edited** schema-1 `durable.md` is left untouched and reported
- a schema-2 `durable.md` is not rewritten twice (idempotence)
- after migration, no shipped artifact mentions
  `get_claude_md_drift`, `get_memory_drift`, or `get_wiki_drift` —
  a repo-wide grep test, which is what catches the doc surfaces that a
  type-checker cannot

## 9. Relationship to the other roadmap items

From the context-engineering review (2026-08-04):

1. **rule staleness** — depends on this SPEC; registers the `code`
   source and contributes `Verdict::{Moved, Stale}` and
   `Evidence::Structural`. Ships second.
2. **drift consolidation** (this SPEC) — ships first.
3. **rule retirement** — will likely add a fifth source (rules that
   never fire, per `phr-mcp stats`). The registry is the reason that
   will be a source rather than a sixth tool.
4. **subagent hook governance** — independent.
5. **graph-sync serialization** — independent (SPEC-rule-staleness §10).

The registry earns its keep at item 3: without it, the roadmap adds
two more near-identical tools to a surface that already has too many.
