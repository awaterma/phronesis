# Downstream Configuration Lifecycle Rules

Status: deferred handoff specification; query-only pending precision validation.

## Objective

Investigate project-local configuration lifecycle gaps without shipping those
opinions in Phronesis starter packs or enabling audit warnings prematurely:

- a generated artifact has no indexed consumer;
- a consumed artifact has no indexed producer.

These findings are evidence of a review opportunity, not proof of dead or
invalid configuration. Deployment outputs and intentionally hand-authored
configuration are legitimate exceptions.

## Prerequisite graph evidence

Use a Phronesis build that exposes these derived relations:

```text
generated_without_consumer(artifact_module)
consumed_without_producer(artifact_module)
```

They are computed by whole-graph set difference over exact `generates` and
`consumes_data` edges. Do not emulate absence with a text search or a RETE
condition: RETE conditions do not implement negation-as-failure.

Current limitation: `.phronesis/graph.toml` requires a producer for every
`[[generated_artifacts]]` binding. Consequently, `consumed_without_producer`
will remain dormant until Phronesis obtains consumer evidence independently
(for example from a bounded language adapter) or supports a separate
consumer-only binding. Do not invent a producer or consumer to make the rule
fire.

## Project-local rules (deferred)

Do not enable the rules below yet. Both generated-artifact findings observed in
the downstream consumer were false positives caused by missing consumer evidence. Phronesis
therefore classifies these relations as query-only: rules that depend on them
are excluded from whole-tree audit until reviewed corpus measurements justify
promotion. Preserve these examples as the intended policy once that gate is
met.

After promotion, merge the following rules into the downstream consumer's
`.phronesis/rules.json`; do not add them to a distributed Phronesis starter
pack.

```json
{
  "id": "downstream-generated-config-without-consumer",
  "phase": "audit",
  "priority": 5,
  "audit": true,
  "when": [
    { "edited_file": "?file" },
    { "declares_module": ["?file", "?artifact"] },
    { "generated_without_consumer": ["?artifact"] }
  ],
  "then": {
    "warn": "Generated configuration artifact ?artifact has no indexed consumer. Confirm that it is a deployment/export output, add the exact consumer binding, or remove obsolete generation. This is closed-world graph evidence, not proof that no external consumer exists."
  }
}
```

```json
{
  "id": "downstream-consumed-config-without-producer",
  "phase": "audit",
  "priority": 5,
  "audit": true,
  "when": [
    { "edited_file": "?file" },
    { "declares_module": ["?file", "?artifact"] },
    { "consumed_without_producer": ["?artifact"] }
  ],
  "then": {
    "warn": "Configuration artifact ?artifact has an indexed consumer but no indexed producer. Confirm that it is intentionally hand-authored or externally supplied, add exact producer evidence, or remove a stale consumer binding. This is closed-world graph evidence, not proof that no external producer exists."
  }
}
```

Use `phase: "audit"`; these are repository-maintenance findings and must not
block ordinary edits. The `edited_file`/`declares_module` joins prevent every
gap from being re-reported on unrelated edits.

## Validation

1. Preserve the live checkout's initial status.
2. Rebuild the graph with the Phronesis version containing the prerequisite
   relations.
3. Query both relations directly:

   ```bash
   phr-mcp graph query generated_without_consumer '*' --json --limit 0
   phr-mcp graph query consumed_without_producer '*' --json --limit 0
   ```

4. Review every result and record confirmed/false-positive counts. Require a
   reviewed, non-zero true-positive rate before removing either relation from
   Phronesis's query-only audit gate.
5. Only after promotion, run `phr-mcp audit` and confirm the findings agree
   with the direct graph queries.
6. Run the downstream consumer's build and test gates required by that repository.
7. Confirm the final status contains only the intended rules change and no
   generated graph, cache, or build output.

## Acceptance criteria

- Both rules remain project-local, disabled until promotion, and audit-only
  after promotion.
- Findings name exact language-qualified artifact modules.
- No producer or consumer edge is guessed.
- Legitimate external/deployment flows are documented rather than hidden by
  broad exclusions.
- An empty result is reported as “no indexed gap found,” never as proof that
  every runtime configuration path is connected.
