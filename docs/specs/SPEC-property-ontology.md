# SPEC: Verification properties as first-class entities

**Status:** Draft for review — blocked on `SPEC-coverage-evidence.md` Phase 2 (region IDs, `head_revision`, `changed_region`)
**Author:** awaterma (agent-drafted, adapted from `REQUIREMENTS-phronesis-coverage-verification-rust-sketch.md`)
**Created:** 2026-09-23
**Scope:** `crates/phronesis-mcp` host only — no engine changes
**Derives from:** `REQUIREMENTS-phronesis-coverage-verification-rust-sketch.md` §3, §13–§14, §17–§18, §20–§21
**Companions:** `SPEC-coverage-evidence.md` (A), `SPEC-verification-artifact-generation.md` (C)
**Reuses:** `SPEC-fact-provenance.md` (`Fact.source`, `fact_sources`), `SPEC-confidence-scoring.md` (signals, Band), `SPEC-journey-facts.md` (journal, tags)

## Summary

Introduce language-neutral **semantic properties** as first-class entities with encodings, results, provenance, and a promotion lifecycle. The sketch (§13) is right: the property is the join point between intent, code, tests, proof tools, and provenance. Nothing represents it today — graph relations cover structure, outcomes cover commands, nothing carries "we claim X should hold, learned from Y, currently believed at level Z."

Central design split, which resolves the sketch's ambiguity where properties, encodings, and outcomes were all flat "facts":

> **Intent is version-controlled; evidence is derived.**

Properties live in a curated, committable `.phronesis/properties.json` — the moral equivalent of `rules.json`. Results live in derived, gitignored stores (the journey journal plus a small results index). Sketch §17's constraint — observed behavior must not silently become intended behavior — becomes an enforceable rule, not a convention.

## Goals

1. Properties, sources, statuses, encodings, and results as **relation-as-predicate facts** (never a generic `triple` wrapper — rejected in `SPEC-triple-store-rete.md` §1 for predicate-index reasons).
2. Promotion is an **explicit, recorded act** — rules report eligibility, never promote.
3. "Observed ≠ intended" is enforceable by rules (§5, B4).
4. Proof outcomes feed the existing confidence machinery through a **minimal, well-marked seam** (§4).
5. Staleness is derivable when code under a property changes (§6).

## Non-goals

1. No artifact generation or verifier execution — `SPEC-verification-artifact-generation.md`.
2. No automated property **discovery** from logs (sketch §18). This spec records `runtime_observation` / `agent_inference` as sources when someone asserts them; it mines nothing.
3. No verifier syntax in core; encodings reference artifacts by path only.
4. No new engine machinery — pattern joins, `facts_count` guards, and provenance fields all exist.

## 1. The property record — `.phronesis/properties.json`

Version-controlled, reviewed in PRs, loaded alongside rules at hook fire. Hydration is demand-gated as with graph relations: property facts assert only when a loaded rule mentions a property relation.

```json
{
  "version": 1,
  "properties": [
    {
      "id": "safe_divide.nonzero_returns_quotient",
      "subject": "safe_divide",
      "kind": "postcondition",
      "depends_on": ["fn:src/lib.rs::safe_divide"],
      "source": "explicit_spec",
      "status": "accepted",
      "corroborated_by": [],
      "encodings": [
        { "language": "rust", "verifier": "kani", "artifact": "verification/safe_divide_nonzero.rs" }
      ]
    },
    {
      "id": "safe_divide.zero_returns_error",
      "subject": "safe_divide",
      "kind": "postcondition",
      "condition": "denominator == 0",
      "guarantee": "result is Error",
      "depends_on": ["branch:src/lib.rs::safe_divide:cd6054b02dde"],
      "source": "explicit_spec",
      "status": "accepted",
      "corroborated_by": ["docs/specs/REQUIREMENTS-phronesis-coverage-verification-rust-sketch.md"],
      "encodings": []
    }
  ]
}
```

Corroboration claims are part of the reviewed record — a corroboration is itself an assertion someone stands behind, which is why `corroborated_by` is curated rather than mined (non-goal 2).

## 2. Relations

| Relation | Args | Meaning |
|---|---|---|
| `property` | `[id]` | The property exists |
| `property_subject` | `[property, function]` | What the property is about (graph element identity) |
| `property_kind` | `[property, kind]` | `postcondition` \| `precondition` \| `invariant` \| … |
| `property_depends_on` | `[property, region]` | Region dependency (SPEC A §3.2 per-site region IDs; a legacy leaf-name id matches every same-leaf changed site) |
| `property_source` | `[property, source]` | `explicit_spec` \| `existing_verifier_contract` \| `test_assertion` \| `documentation` \| `code_inference` \| `runtime_observation` \| `agent_inference` |
| `property_status` | `[property, status]` | `observed` \| `candidate` \| `corroborated` \| `accepted` \| `verified` \| `rejected` \| `superseded` |
| `property_corroborated_by` | `[property, source_entity]` | Independent corroboration for promotion policy |
| `property_encoding` | `[property, language, verifier, artifact]` | Where the claim is encoded (sketch §14) |
| `verification_result` | `[property, verifier, status]` | A **bound** result at the recorded revision (see *Result binding*) |
| `result_revision` | `[property, verifier, sha]` | Commit (40 hex) at which the bound result was produced |
| `result_tier` | `[property, verifier, tier]` | Confinement tier that ran the bound result: `devcontainer` \| `sandbox_exec` \| `raw` — rules can refuse `raw` |
| `unbound_evidence` | `[property, verifier, reason]` | A result record that does not bind; never evidence (reasons below) |
| `stale_evidence` | `[property, verifier]` | Host-derived: a bound result predates a change to a dependent region |
| `property_obligation` | `[property, "first_proof"]` | Host-derived: an accepted/verified property's dependent region changed and no bound result is at HEAD |
| `store_corrupt` | `["properties", reason]` | properties.json or property-results.jsonl could not be read or validated (reasons below) |

**Result statuses** are `passed` \| `failed` \| `inconclusive` \| `timeout` \| `unknown`. The sketch §14's evidence-kind list had no failure semantics; the three-state `unknown` discipline from `outcomes/toolchain.rs` ("never a silent pass") is mandatory here. Evidence *kinds* (`deductive_proof`, `bounded_model_check`, `runtime_unit_test`, …) are a function of the verifier and live in the results record payload, not as RETE args, unless a rule needs them — demand-gated philosophy.

**Result binding (D9).** A results-sidecar record (`.phronesis/property-results.jsonl`, format `v: 2`) is evidence only when it is *bound*:

```json
{"v": 2, "kind": "verification_result", "property": "safe_divide.zero_returns_error",
 "verifier": "verus", "status": "passed", "revision": "<40-hex commit>", "tool": "<verifier command>",
 "tier": "sandbox_exec", "artifact_sha256": "<64-hex SHA-256 of the artifact bytes that ran>"}
```

`properties::execute::execute` fills every field from the run: the property it was asked to prove, the encoding's verifier name, the commit it ran against (a non-commit revision is refused before running), the tier that ran, and the SHA-256 it computed from the on-disk bytes (which must be allowlisted *for that property*). At hydration a record binds only when all of these hold, checked in this order; the first failure is the `unbound_evidence` reason:

| Reason | Failure |
|---|---|
| `legacy_record` | `v: 1` record (written before binding; no tier, no artifact hash) |
| `missing_revision` / `invalid_revision` | revision empty / not 40 hex |
| `missing_tier` / `invalid_tier` | tier absent / not `devcontainer`, `sandbox_exec`, or `raw` |
| `missing_artifact` / `invalid_artifact` | artifact hash absent / not 64 lowercase hex |
| `unknown_property` | no curated property with that id |
| `no_encoding` | the property has no encoding for that verifier |
| `allowlist_unreadable` / `artifact_not_approved` | the artifact hash is not an S3-approved artifact for that property |

An unbound record never asserts `verification_result`, `result_revision`, `result_tier`, or `stale_evidence`, and never satisfies the first-proof obligation. A bound result is "at HEAD" only when HEAD is known and equals its revision, so with no git HEAD every accepted property with a changed dependency stays obligated; staleness is not claimed without a HEAD.

**Store integrity (D8).** Loading validates the closed `kind` and status sets, the format (`v` 1 or 2), and field shapes (identifier charset for `property`; bounded, control-free strings elsewhere). Any failure makes that file corrupt: the hook prints a stderr warning and asserts `store_corrupt(properties, <reason>)` when a rule mentions it, and derives everything else as if the corrupt file were empty — unreadable results are no evidence, so obligations still fire. Reasons: `properties_unreadable`, `invalid_properties`, `invalid_record` (properties.json), `results_unreadable`, `invalid_result` (property-results.jsonl), `unsupported_format` (either). A corrupt properties.json leaves no properties to derive from, so rules that must not pass silently should also match `store_corrupt(properties, ?reason)`.

All facts carry `Fact.source` (`SPEC-fact-provenance.md`) so "why did this rule fire?" shows whether evidence came from a verifier run, the curated record, or an agent assertion.

## 3. Results enter through the existing outcomes seam

Kani / Verus register as **declarative ToolchainDefs** (`.phronesis/toolchains.json`, zero code): `"matches": "^cargo (kani|verus)"` with `per_test` named groups mapping proof output lines to `(property, status)`. This is exactly what the adapter machinery already parses for cargo test output. Kani is registered as a project def (`id: kani`) with `section_start` scoping each harness's `Checking harness …` header to its own `VERIFICATION:-` verdict; a harness is bound to its property by an encoding `{"verifier": "kani", "artifact": "harness:<module::path::name>"}`. An unbound harness result is journaled as `proof_unbound(subject, harness, status)` and counts for no property.

Results journal as `outcome:proof_pass:<property>` / `outcome:proof_fail:<property>` tags. New code — small, well-marked seam:

- `outcomes/facts.rs` + `outcomes/derive.rs` accept a `proof` signal kind: `signal_pass(subject, "proof")` joins `compile` and `tests`.
- `record_signal` accepts `proof` alongside `compile` / `tests`.
- Band derivation counts proof signals like test signals (`SPEC-confidence-scoring.md` extension).

## 4. Promotion — rules report eligibility, never promote

Rules in exact v2 format:

```json
{
  "id": "log-property-promotion-eligible",
  "phase": "audit",
  "priority": 30,
  "audit": true,
  "when": [
    { "property_source": ["?p", "runtime_observation"] },
    { "property_status": ["?p", "candidate"] },
    { "__script__": "facts_count('property_corroborated_by', ['?p', '*']) >= 2" }
  ],
  "then": { "log": "property ?p has 2+ corroborating sources and is eligible for promotion review" }
}
```

```json
{
  "id": "warn-unpromoted-observation-property",
  "phase": "audit",
  "priority": 20,
  "audit": true,
  "when": [
    { "property_source": ["?p", "runtime_observation"] },
    { "__script__": "!facts_contain('property_status', ['?p', 'accepted']) && !facts_contain('property_status', ['?p', 'verified'])" }
  ],
  "then": { "warn": "property ?p is sourced from runtime observation and not promoted; it is evidence, not intent" }
}
```

Promotion itself is `set_property_status` — an MCP tool whose every invocation lands in `log.jsonl` (kind `mcp`) with property, old status, new status, and a required `--because <reason>`. Never rule-driven; this is sketch §17's "observed ≠ intended" given teeth.

*As shipped:* the journal line is written **before** the store is replaced, and a journal failure refuses the transition; `PHRONESIS_NO_ACTION_LOG` does not suppress it. The line is therefore a record of intent: a failed rename appends `set_property_status_aborted`, but a crash between the append and the rename leaves a line for a transition that never landed. `properties.json` is the source of truth for current status; the log is the audit trail of attempts.

## 5. Staleness — sketch §6, honestly named

The engine cannot order revision SHAs, so `verification_required` from the sketch is host-derived: when `changed_region(change, region)` ⋈ `property_depends_on(property, region)` and a bound result's recorded `result_revision` ≠ `head_revision` (SPEC A), the resolver asserts `stale_evidence(property, verifier)`. Unbound records (§2) are never stale — they were never evidence. The evidence is stale; rerunning is the action it recommends — the consequence is a warning, the derivation is host-side, and the name says what it is:

```json
{
  "id": "warn-stale-proof-for-changed-region",
  "phase": "post",
  "priority": 20,
  "audit": true,
  "when": [
    { "changed_region": ["?change", "?region"] },
    { "property_depends_on": ["?p", "?region"] },
    { "stale_evidence": ["?p", "?v"] }
  ],
  "then": { "warn": "property ?p depends on changed region ?region; proof by ?v is stale — reprove" }
}
```

## 6. Acceptance criteria

**B1 — Join.** With the two safe_divide properties hydrated and the sketch §4 edit applied, `warn-stale-proof-for-changed-region` names `safe_divide.zero_returns_error` and its verifier; `nonzero_returns_quotient` is **not** flagged (its region didn't change).

**B2 — Promotion.** A `runtime_observation`-sourced property stays non-normative — no rule treats it as accepted; with two `corroborated_by` entries the eligibility rule logs; status changes only via the recorded MCP act.

**B3 — Proof signal.** A simulated kani pass through a ToolchainDef produces `signal_pass(subject, "proof")` and lifts the Band (unit test in `outcomes/`).

**B4 — Adversarial.** An unpromoted observation property exists and has a passing result; the evidence-gap rule (SPEC A §5.2) still warns — existence of a property and a result is not accepted intent.

## 7. Phasing

Blocked on SPEC A Phase 2 (region IDs, `changed_region`, `head_revision`).

- **B Phase 1:** the record + relations + demand-gated hydration + promotion rules + B1/B2/B4.
- **B Phase 2:** outcomes `proof` seam + staleness resolver + rule 5.2 wiring + B3.

## 8. Open questions

1. How do properties for non-function subjects (module invariants, data-structure well-formedness) express `subject` identity? Graph element IDs exist for modules; confirm coverage.
2. Should `set_property_status` require the `--because` reason at the tool boundary (lean yes; enforced in the handler, not the CLI)?
3. Do encoding artifact paths participate in graph relations (an `includes_file`-style edge) so stale encodings are detectable like stale rule bindings (`graph/bindings.rs`)?