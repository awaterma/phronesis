# phronesis-governance

An embeddable policy core over the `phronesis` RETE engine. It compiles caller-supplied policies, composes ordered layers, evaluates explicit fact snapshots, and returns decisions with original consequences, provenance, and diagnostics.

```text
phronesis-mcp -> phronesis-governance -> phronesis
phronesis-rhai ----------------------> phronesis
```

The core has no MCP, repository-discovery, grammar, storage, environment, process-exit, or mandatory Rhai dependency. Applications own collection of facts, policy loading, clocks, reservations, and evaluator selection. No public persistent governance session API is provided.

## Embedding

1. Parse an in-memory v1/v2 document with `parse_source(text, source_id)` and retain its diagnostics.
2. Use `CompiledPolicy::compile_source(&parsed.rules)` to validate and expand rules while retaining source-only `binds` metadata and OR origins. Query `source_origin(flat_rule_id)` for that metadata.
3. Supply `Selection`, a `FactSnapshot { host, extensions }`, the host's reserved predicate set, exact rule IDs to demote, and a boxed `phr::ScriptEval` to `evaluation::evaluate`.
4. Handle operational `EvaluationError`s separately from a successful `Decision` whose verdict is `Allow`, `Warn`, or `Deny`.

`parse` and `CompiledPolicy::compile(Vec<DiskRule>)` support flat-rule consumers. Flat rules do not carry source-only `binds`; that constructor therefore has no source-origin sidecar. `DiskRule` keeps its existing public field shape. Parsing and compilation diagnostics are returned as values, never printed by the core.

`composition::compose` accepts already-loaded, validated `LoadedLayer`s in precedence order. Later definitions replace earlier definitions at their original position. Results retain winning origins and every override, including identical replacements. Source identifiers are opaque strings; adapters retain original path objects separately. Composition does not validate rules or infer layer order. Compile its winning flat rules before evaluation. Source-only metadata must be retained separately when using this flat composition interface.

Each snapshot evaluation has independent working memory. All extension reservations and duplicate fact IDs are checked before asserting any facts. `Fact.source` and source identifiers provide attribution, not authentication. Host facts may use reserved predicates; extension facts may not, even when they claim a host source. Define reservations independently of which host facts happen to be present in one snapshot.

`evaluation::builtin_evaluator()` supplies the engine's built-in guard evaluator. Applications may inject Rhai or another `phr::ScriptEval`; there is no implicit evaluator selection. A guard error follows the engine's fail-closed matching behavior and is retained as a diagnostic; the rule's action still determines severity. Explicit exact-ID demotion also applies to a guard-error block. Demotion changes effective severity, never the original consequence payload or provenance.

## Phase and compatibility semantics

- `Selection::Hook(Pre | Post | Audit)` selects the exact phase. `Hook(None)` selects nothing.
- `Selection::Audit` selects exactly `audit: true`, including a parked rule explicitly marked for audit. This preserves the existing audit implementation; older guide prose described parked rules differently.
- Unknown actions or phases in policy input fail validation. Unknown action payloads in raw engine consequences are retained as informational reasons.
- `override_facts_at` uses a caller-supplied timestamp. The convenience `override_facts` uses zero and never reads a clock.
- Raw OR helpers retain existing tagger behavior; use validated compilation at the embedding boundary. Existing `base_rule_id` handling of literal `#` characters is unchanged.

MCP uses shared policy parsing/conversion, composition, capsule validation, and hook/audit severity classification. Its compatibility adapter retains existing Rust paths and error signatures, file I/O, pack loading, atomic persistence, graph reconciliation, and shadowed-rule ownership. Hook JSON, logs, exit handling, provider reservations, and lifecycle behavior remain host responsibilities. The MCP server keeps its persistent RETE network and raw-consequence transport; it shares compilation helpers without replacing its session with snapshot evaluation.

## Runnable consumers

```sh
cargo run --offline -p phronesis-governance --example consumer_flow
cargo run --offline -p phronesis-governance --example operator_flow -- \
  examples/governance-consumer/fixtures/expense-rules.json \
  examples/governance-consumer/fixtures/expense-events-approve.json
sh examples/governance-consumer/run.sh
```

The coding example demonstrates decisions, provenance, demotion, reservations, and snapshot isolation. The expense-approval workflow demonstrates a non-software domain with explicit input paths and reserved predicates. The [consumer acceptance script](../../examples/governance-consumer/README.md) packages and extracts both Rust crates, builds an external consumer against those archives, and runs the expense workflow from an empty working directory. It performs no publication. Local dependency patches let the unpublished matching engine archive resolve offline; this is not evidence of a crates.io release.
