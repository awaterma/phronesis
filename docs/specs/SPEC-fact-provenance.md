# SPEC: fact provenance — trace consequences to data sources

**Status:** Draft
**Author:** user
**Created:** 2026-08-12
**Scope:** Core engine (`phronesis`) + MCP host (`phronesis-mcp`)

## Problem

When a rule fires and produces a consequence, the `Provenance::RuleFiring` record says *which rule* fired and *what facts* matched. It does **not** say where those facts came from.

Facts enter the engine from multiple origins:
- **Hook diffs** — regex-based diff extraction (`function_added`, `import_added`)
- **Code graph** — AST parsing (`in_cycle`, `defines_fn`, `untested`)
- **Journey journal** — agent trajectory analysis (`journey_*`)
- **Rhai providers** — sandboxed predicate scripts (`change_set_production_rust`)
- **Manual MCP** — `assert_fact` tool calls
- **Clock/context** — wall-clock and session context facts

Without source attribution, a human (or LLM agent) auditing a rule violation can only see: "rule X matched facts [f1, f2]." They cannot distinguish "these facts came from the code graph" from "these facts came from the hook diff." The provenance trail stops one hop short.

For a product whose differentiator is **preventative rule enforcement with traceable quality decisions**, this is a gap. Structural rules (cycle detection, untested risky calls, import policies) rely on the code graph. When those rules fire, the consumer should be able to see that the code graph was the data source.

## Goal

Every `Consequence` produced by a rule firing includes the **source label** of each bound fact. This enables full traceability: "rule X fired because the **code graph** found facts f1 and f2, AND the **hook diff** found fact f3."

Non-goal: making the source part of the rule matching logic. Source is metadata, not a condition.

## Requirements

### R1. Fact carries a `source` field

`Fact` (in `crates/phronesis/src/engine_types.rs`) gains an optional `source` field:

```rust
pub struct Fact {
    pub id: String,
    pub predicate: String,
    pub args: Vec<String>,
    pub timestamp: u64,
    /// Where this fact came from. Set by the assertion site
    /// (hook diff, code graph, journey, Rhai provider, MCP).
    /// Enables "why did this rule fire?" to show the data origin.
    #[serde(default)]
    pub source: Option<String>,
}
```

**R1a.** The field is `Option<String>` with `#[serde(default)]`. Backward-compatible: old facts serialized without this field deserialize to `None`.

**R1b.** Source values are opaque strings — the type does not enforce a fixed enum. The host (phronesis-mcp) assigns semantic values.

### R2. WME exposes the canonical fact source

`WorkingMemoryElement` does not duplicate the source. Callers read
`wme.fact.source`; the WME-owned `Fact` remains the single source of truth.

### R3. Consequence provenance includes fact sources

`Provenance::RuleFiring` gains a `fact_sources` map (in `crates/phronesis/src/consequence.rs`):

```rust
RuleFiring {
    rule_id: RuleId,
    bound_facts: Vec<String>,
    bindings: HashMap<String, String>,
    /// fact_id → source_label for each attributed bound WME.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fact_sources: BTreeMap<String, String>,
},
```

**R3a.** `skip_serializing_if` keeps the JSON lean when there are no facts (e.g., pure-`__script__` rules with zero WMEs).

**R3b.** An absent map entry means that the corresponding `bound_facts` entry
is unattributed. `BTreeMap` keeps serialized provenance deterministic. The
ordered `bound_facts` field remains the canonical list of facts that matched.

**R3c.** `Provenance::RuleDrivenLookup` also gains a `fact_sources` field (same semantics), since it composes a rule firing with a lookup.

### R4. Production network attaches sources at fire time

`ProductionNetwork::fire_agenda_item` (in `crates/phronesis/src/production.rs`) populates `fact_sources` from the agenda item's WME list:

```rust
let bound_facts: Vec<String> = agenda_item
    .wme_list
    .iter()
    .map(|wme| wme.id.clone())
    .collect();
let fact_sources: BTreeMap<String, String> = agenda_item
    .wme_list
    .iter()
    .filter_map(|wme| {
        wme.fact.source.clone().map(|source| (wme.id.clone(), source))
    })
    .collect();
```

The push and rule-driven lookup helpers accept the same provenance inputs.
Compatibility wrappers that receive fact IDs only remain supported and emit an
empty `fact_sources` map.

### R5. Assertion sites populate source

Every call site that constructs a `Fact { id, predicate, args, timestamp, .. }` in the MCP crate sets `source`:

| Call-site group | Files | Source label convention |
|-----------------|-------|------------------------|
| Diff facts | `diff_extract.rs`, `hook_facts.rs` | `"diff:{predicate}"` |
| AST predicates | `syntax/*.rs`, `hook_facts.rs` | `"ast:{language}"` |
| Code graph structural edges | `graph/model.rs::Edge::to_fact()` | `"graph:{edge.src}"`, falling back to `"graph:structural"` |
| Language-pack diagnostics | `graph/audit.rs`, `hook_facts.rs` | `"graph:language_pack"` |
| Journey journal entries | `journey/derive.rs` | `"journey"` |
| Rhai predicate providers | `predicate_provider.rs` | `"rhai:{provider_name}"` |
| Clock facts | `codex_hook.rs`, `clock_facts.rs` | `"clock"` |
| Manual MCP assert | `server.rs::assert_fact` | `"mcp"` |
| Context injection | `context/capsule.rs` | `"context"` |
| Hook boundary facts | `hook/mod.rs` | `"hook"` |

**R5a.** Source labels are stable identifiers, not display text. Their namespace
is reserved for future filtering, but v1 does not expose source to conditions or
scripts.

**R5b.** Labels classify the producing subsystem and, where bounded evidence
exists, its producer. They are origin metadata, not a cryptographic or complete
evidence chain.

### R6. Server params accept source on assert_fact

`AssertFactParams` (in `crates/phronesis-mcp/src/server_params.rs`) gains an optional `source` field, allowing MCP callers to set it manually:

```rust
pub struct AssertFactParams {
    pub id: String,
    pub predicate: String,
    pub args: Vec<String>,
    #[serde(default)]
    pub source: Option<String>,
}
```

### R7. No behavioral change to rule matching

Source is **purely observational**. Rules match on predicate and arguments only. A rule does not need to list conditions to match facts from a particular source. Source is written once at assertion time and flows through to provenance at fire time.

### R8. Duplicate assertion semantics

Fact identity remains `(id, predicate, args)`. Reasserting an identical fact is
an idempotent no-op, including when the new assertion supplies a different
source. The first assertion's source is immutable until retraction. Hosts that
need to replace origin metadata must retract and reassert.

### R9. Tests

**R9a.** Unit test in `production.rs`: `fire_agenda_item_attaches_fact_sources()` — assert that a rule firing with bound WMEs produces `bound_facts` and `fact_sources` from the same WME list, including attributed and unattributed facts.

**R9b.** Unit test in `engine_types.rs`: `fact_serialization_roundtrips_with_source()` — serialize and deserialize a fact with a source value, verify `Option<String>` preserves the string.

**R9c.** Unit test in `engine_types.rs`: `fact_serialization_roundtrips_without_source()` — verify backward compatibility: a fact without a source field deserializes to `source: None`.

**R9d.** Test deterministic provenance serialization, old provenance JSON without `fact_sources`, first-assertion-wins duplicate semantics, push and rule-driven lookup propagation, MCP caller/default source behavior, and graph source preservation.

**R9e.** Integration test: a hook invocation that fires structural rules (e.g., cycle detection) produces a consequence whose provenance includes graph-origin values.

### R10. Architectural-decision traceability

Accepted ADRs under `.phronesis/wiki/decisions/` project their explicit
`enforces` declarations into the graph as
`decision_enforces(decision, rule)` and the navigational inverse
`rule_governed_by(rule, decision)`. Only existing rule IDs produce those
resolved relations.

Unresolved and stale declarations remain visible as evidence rather than being
guessed or silently dropped:

- `decision_missing_rule(decision, rule)`
- `proposed_decision_enforces(decision, rule)`
- `superseded_decision_enforces(decision, rule)`
- `rule_without_decision(rule)`

Rule-firing and rule-driven-lookup provenance carries a deterministic list of
governing decision IDs. Hook action-log projections retain that list so a later
reader can traverse consequence → rule → ADR as well as consequence → bound
fact → source. ADR and rules-file changes participate in graph freshness and
trigger a full rebuild because the relationship is repository-wide.

## Design decisions

### Why `Option<String>` not `enum`?

An enum would be stricter but harder to extend. Rhai providers are user-defineable — they can't be given a fixed enum variant without complicating the provider model. A string with a naming convention ("rhai:{provider_name}") is simpler and sufficient for the use case. If a structured approach proves valuable later, the string values can be parsed.

### Why on `Fact` and not on `WME` only?

`Fact` is the canonical type. `WME` wraps a `Fact`. Putting `source` on `Fact`
means it is always available, even for callers that do not go through `WME`.
Reading `wme.fact.source` is direct field access and avoids divergent metadata.

### Why not tag provenance at the alpha/beta network level?

The alpha and beta networks operate on WMEs and tokens, not on provenance metadata. Tagging there would require threading source through every join state and token operation. The provenance is naturally available at fire time when the `AgendaItem` already carries the complete WME list. Adding it earlier would be premature optimization.

### What about retraction?

Retraction does not need provenance tracking. When a fact is retracted, the WME is removed from working memory and any agenda items referencing it are purged. The consequence that was already produced retains its provenance in the accumulated consequences list or action log. No additional retraction provenance is needed.

## Migration

1. Add `source: Option<String>` to `Fact` — serialized facts remain backward compatible via `#[serde(default)]`.
2. Add `fact_sources` to `Provenance::RuleFiring` — backward compatible via `skip_serializing_if`.
3. Update assertion sites in the MCP crate — each site passes a string literal.
4. Update Rust struct literals and constructors, then run the existing suite and the tests from R9.

No rules.json changes are needed. The serialized JSON change is additive and old
payloads deserialize successfully. Adding fields to public Rust structs and enum
variants is source-breaking for exhaustive constructors and patterns; Phronesis
accepts that pre-1.0 API cost in v0.28 and documents it in the release notes.

## Open questions

**Q: Should source be queryable in rule conditions?**

Not in v1. A rule like `source == "graph:structural"` would be a `__script__` guard or a new built-in predicate (`source_matches`). If the need arises after this ships, a `source_matches(pattern)` predicate can be added to the built-in set.

**Q: What about audit — does it need source in its output?**

Currently, audit output shows per-rule per-file violations. Adding source to audit render would be straightforward (it already has access to facts) but is scope expansion. V1 focuses on the consequence/provenance path. Audit can use the same field later without code changes.

**Q: Should the CLI `phr-mcp stats` show source breakdown?**

Not in v1. The stats view aggregates by rule, not by fact source. This could be added later as `phr-mcp stats --by-source`.
