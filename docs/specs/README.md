# Specs

Proposed or evolving designs. Every spec carries an explicit lifecycle-status header derived from code and test inspection, never invented and never inferred from changelog topics alone. A `needs evidence` spec must not be cited as shipped.

## Lifecycle statuses

| Status | Meaning |
|---|---|
| `shipped` | The designed behavior exists in code and is covered by named tests where applicable; the header names the implementation files (clickable relative paths) and a tag-derived first release where one could be verified from Git tags. |
| `partially implemented` | Some of the design exists in code; the header lists what shipped, what was not found (gaps), and the evidence for both. |
| `needs evidence` (proposed) | No matching implementation was identified on the last reconciliation pass; the header records what was searched and what the original draft label said. |
| `shipped`/`partial` (companion record) | Inventories, decision appendices, and other records belonging to a parent spec; they inherit the parent's evidence and link to it. |

Statuses are reconciled by code inspection (module + test paths) and, for release claims, by Git tags: a "first release" is stated only when the introducing commit of the primary module is verifiably contained in a release tag (`git tag --contains`). Changelog topic matching is not used as implementation evidence. How to update a status: [contributing/documentation.md](../contributing/documentation.md).

## Index

| Spec | Status | First release (tag-derived) |
|---|---|---|
| [SPEC-0.14.0-dogfooding-polish](SPEC-0.14.0-dogfooding-polish.md) | shipped | migrate-rules: 0.17.0 |
| [SPEC-agent-lifecycle-events](SPEC-agent-lifecycle-events.md) | shipped | 0.35.0 |
| [SPEC-audit-script-conditions](SPEC-audit-script-conditions.md) | shipped | 0.36.0 |
| [SPEC-call-graph-and-suppression-rules](SPEC-call-graph-and-suppression-rules.md) | partial (calls substrate; suppression/worktree gaps) | 0.23.0 (substrate) |
| [SPEC-codex-hooks-integration](SPEC-codex-hooks-integration.md) | shipped | 0.22.0 |
| [SPEC-confidence-scoring](SPEC-confidence-scoring.md) | shipped | get_confidence: 0.16.1 |
| [SPEC-coverage-evidence](SPEC-coverage-evidence.md) | shipped | 0.36.0 |
| [SPEC-downstream-config-lifecycle-rules](SPEC-downstream-config-lifecycle-rules.md) | needs evidence (proposed) | — |
| [SPEC-drift-consolidation](SPEC-drift-consolidation.md) | shipped | consolidated registry: 0.25.1 |
| [SPEC-dynamic-language-boundaries](SPEC-dynamic-language-boundaries.md) | partial (imports edges; wider design unverified) | — |
| [SPEC-extensible-predicates](SPEC-extensible-predicates.md) | shipped | 0.22.0 |
| [SPEC-extract-rules-defaults](SPEC-extract-rules-defaults.md) | shipped | module since 0.9.1; refinement version unverified |
| [SPEC-fact-provenance](SPEC-fact-provenance.md) | shipped | since 0.9.1 (initial release) |
| [SPEC-gate-merge-commits](SPEC-gate-merge-commits.md) | shipped | commit-detection module: 0.35.0 |
| [SPEC-god-file-decomposition](SPEC-god-file-decomposition.md) | partially implemented | — |
| [SPEC-journey-facts](SPEC-journey-facts.md) | shipped | journey module: 0.16.1 |
| [SPEC-journey-filtered-since](SPEC-journey-filtered-since.md) | shipped | 0.16.1 |
| [SPEC-memory-to-rules](SPEC-memory-to-rules.md) | partial (detector shipped; automated loop not found) | — |
| [SPEC-next-round-perf](SPEC-next-round-perf.md) | partially implemented (active ledger) | — |
| [SPEC-non-code-span-masking](SPEC-non-code-span-masking.md) | partial (Rust test-block mask; general masking not found) | — |
| [SPEC-pack-opt-in-facts](SPEC-pack-opt-in-facts.md) | shipped | current module: 0.36.0 |
| [SPEC-participatory-governance](SPEC-participatory-governance.md) | partial (scaffolding shipped; loop not automated) | — |
| [SPEC-property-ontology](SPEC-property-ontology.md) | shipped | set_property_status: 0.36.0 |
| [SPEC-python-pack-expansion](SPEC-python-pack-expansion.md) | shipped | current module: 0.36.0 |
| [SPEC-python-pytest-pack](SPEC-python-pytest-pack.md) | needs evidence (proposed; no pytest rule pack found) | — |
| [SPEC-resolver-dispatch](SPEC-resolver-dispatch.md) | needs evidence (proposed) | — |
| [SPEC-rhai-code-graph-boundary](SPEC-rhai-code-graph-boundary.md) | shipped | graph boundary: 0.28.0 |
| [SPEC-rule-schema-v2](SPEC-rule-schema-v2.md) | shipped | migrate-rules: 0.17.0 |
| [SPEC-rule-staleness](SPEC-rule-staleness.md) | shipped | bindings module: 0.26.0 |
| [SPEC-rust-ownership-evidence](SPEC-rust-ownership-evidence.md) | shipped | 0.29.0 |
| [SPEC-rust-ownership-evidence-DECISIONS](SPEC-rust-ownership-evidence-DECISIONS.md) | shipped (companion record) | 0.29.0 (parent) |
| [SPEC-starter-rule-evidence-modernization](SPEC-starter-rule-evidence-modernization.md) | partial (evidence-named ids observed; ten-rule audit incomplete) | — |
| [SPEC-structural-rule-migration](SPEC-structural-rule-migration.md) | shipped | migrate-extracted-rules: 0.16.2 |
| [SPEC-test-evidence](SPEC-test-evidence.md) | shipped | tested_by 0.23.0; hydration 0.36.0 |
| [SPEC-triple-store-rete](SPEC-triple-store-rete.md) | shipped | 0.23.0 |
| [SPEC-verification-artifact-generation](SPEC-verification-artifact-generation.md) | partial (Verus harness shipped; host pipeline unverified) | — |
| [SPEC-wiki-drift](SPEC-wiki-drift.md) | shipped | detector: 0.9.1 |
| [INVENTORY-structural-rule-migration](INVENTORY-structural-rule-migration.md) | shipped (companion inventory) | with parent |
| [REQUIREMENTS-phronesis-coverage-verification-rust-sketch](REQUIREMENTS-phronesis-coverage-verification-rust-sketch.md) | partially implemented (vision doc) | — |

Companion records for shipped specs (plans, validation reports, reviews) were moved to the [Archive](../archive/README.md) with historical notices; the table above links each spec's canonical page.
