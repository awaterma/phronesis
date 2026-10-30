# phronesis-governance

Experimental embeddable policy core over the phronesis RETE engine. This is a partial extraction, not a completed MCP migration or accepted release.

- `policy`: strict in-memory v1/v2 parsing, OR expansion, validation, metadata-bearing disk types, opaque validated `CompiledPolicy`, and explicit hook/audit selection.
- `composition`: caller-loaded ordered layers, opaque source IDs, winners and complete override history. `override_facts_at` takes an explicit timestamp; `override_facts` uses zero and never reads a host clock.
- `decision`: Allow/Warn/Deny projection, every original consequence, structured diagnostics and exact-rule severity demotion. Raw consequences remain unchanged.
- `evaluation`: independent working memory for each explicit host/extension `FactSnapshot`, caller-injected `phr::ScriptEval`, reserved extension-predicate rejection and duplicate fact-ID rejection before assertions, typed engine/input failures.
- `capsule`: pure payload validation. Persistence, expiry timestamps, capture and delivery stay in the MCP adapter.

The embedding application owns fact collection, reservation definitions and script evaluator choice. `Fact.source` and policy source IDs are attribution supplied by callers, never authentication. Reserved predicates only protect against extension facts when the host supplies the right reservation set. Host facts may use reserved predicates. A policy denial is a successful evaluation result; operational failures are errors. Explicit demotion changes effective severity even for a guard-error block, while retaining original consequences and diagnostics.

`Selection::Hook(Phase::None)` is empty. `Selection::Audit` preserves the existing audit implementation's `audit == Some(true)` filter, including an explicitly opted-in parked rule; this differs from older guide prose.

## Remaining work

The MCP parser, loader, rule persistence, hooks and persistent server have not yet been migrated to these shared types/helpers. Only capsule validation is wired. Compatibility re-exports, coding/non-coding executable consumers, an external packaged consumer, and final independent architecture acceptance remain pending. `SourceRule.binds` is not retained by the flat `DiskRule`/`CompiledPolicy` path; design a metadata-preserving source compilation path before acceptance. Other inherited helper limitations and review dispositions are recorded in the preserved campaign report. No public session API exists, and MCP keeps its existing persistent network.

Local tests and partial-workspace gates provide behavioral evidence for this snapshot, not final acceptance of the extraction plan.
