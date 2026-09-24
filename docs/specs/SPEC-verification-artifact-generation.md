# SPEC: Property-driven verification artifact generation (safety-gated)

**Status:** Deferred — do not implement before `SPEC-coverage-evidence.md` (A) and `SPEC-property-ontology.md` (B) land. Concept approved for design; schedule is not.
**Author:** awaterma (agent-drafted, adapted from `REQUIREMENTS-phronesis-coverage-verification-rust-sketch.md`)
**Created:** 2026-09-23
**Scope:** `crates/phronesis-mcp` host only
**Derives from:** `REQUIREMENTS-phronesis-coverage-verification-rust-sketch.md` §15–§16, §19, §22–§24
**Companions:** `SPEC-coverage-evidence.md` (A), `SPEC-property-ontology.md` (B)
**Reuses:** `SPEC-confidence-scoring.md` (ToolchainDefs, signals), `SPEC-journey-facts.md` (journal)

## Summary

Sketch §15 is the valuable idea: one semantic property should derive both a proof harness and a conventional test. This spec's primary content is **not** the rendering mechanism — it is the **safety contract**, because executing generated code is arbitrary code execution and the sketch has no model for that.

### The sketch's "Phronesis-Rye" is `phronesis-rhai`

The sketch's §16 template layer ("Phronesis-Rye") is `phronesis-rhai` — naming corrected. Its templates are the `.rhai` scripts the sketch already names (`kani-postcondition.rhai`, `verus-postcondition.rhai`, …): each template receives the property record and computes the verifier artifact body.

The sandbox boundary shapes — and this spec keeps — the division of labor:

- **Rhai renders.** A template script runs in the existing sandbox (`Engine::new_raw()`: no file I/O, no module imports, no closures; op and size limits), receives the property record and relevant facts in scope, and emits the rendered artifact body. One small extension to the existing entry points: guards already provide `facts` + `bindings` in scope and providers already provide `emit_fact`; a render invocation combines the two. No new engine machinery.
- **Rust acts.** The host validates the emitted body (S5), writes it into `verification/unreviewed/` (S3, S6 — the sandbox cannot write files, deliberately), invokes the verifier through the post-check seam (S4), and records the structured result.

Known limits, stated rather than hidden: emitted strings are capped (~4 KiB, `phronesis-rhai` limits) — fine for harness-sized artifacts. A template that needs more becomes an explicit host-side Rust template, recorded as an exception in review, never a silent fallback.

## Problem

A property (SPEC B) plus a changed region (SPEC A) yields an obligation: the claim must be re-established. Today the only way to satisfy it is hand-written harnesses. Sketch §15 correctly observes the harness is *derivable* from the normalized property. What the sketch omits: generated artifacts are code, verifiers execute code, and an agent-driven pipeline that generates-then-executes without a review boundary is a machine that turns prompt-adjacent data into arbitrary execution.

## Pipeline

```text
accepted property (SPEC B)
        ↓  obligation: changed_region ⋈ property_depends_on + stale_evidence
Rhai template script (language × verifier × kind → e.g. kani-postcondition.rhai; version-controlled repo content)
        ↓  emits the rendered artifact body
host validates (S5) and writes verification/unreviewed/<artifact>.rs
        ↓  review gate (S3)
execution via registered ToolchainDef (post-check Bash seam only — S4)
        ↓  structured result
verification_result fact + result_revision + journey tag + provenance (SPEC B §3)
```

## Safety requirements

**S1 — Opt-in only.** Nothing is generated or executed unless the project opts in (marker-file pattern, as `confidence.json` gates the outcomes machinery). Default installs generate nothing.

**S2 — Generation precondition: accepted intent.** Generation requires `property_status ∈ {accepted, verified}`. Never `candidate`, never `observed`. Sketch §17's promotion discipline, applied to execution: unreviewed observations do not get compiled into obligations.

**S3 — Generated artifacts are untrusted until reviewed.** They land in a clearly-marked `verification/unreviewed/` directory and are **never executed in the same hook fire that created them**. Execution requires either an explicit recorded approval (`set_property_status`-style audited act) or a policy allowlist keyed on content hash (a re-render identical to a previously approved artifact may skip re-review — see open questions).

**S4 — Execution only via the existing post-check seam.** Verifiers run through registered ToolchainDefs on `Bash`-class tools, post-check, never pre-check (verifiers are slow; nothing blocks on them), always journaled with revision and exit code.

**S5 — Templates are trusted; property data and rendered output are not.** Template scripts are version-controlled repo content, loaded under the same constraints as predicate providers (path canonicalization inside the project root, size caps). Property fields flowing into them are untrusted input, and the emitted artifact body is untrusted *output*: both must survive the `security.rs` validators before the host writes anything. No string reaches a written artifact unvalidated.

**S6 — Containment.** Artifact paths canonicalize inside the project root (`security.rs`); sizes capped; no template output escapes `verification/`.

**S7 — Full audit trail.** Every generation and execution appends: `log.jsonl` entry, journey tag, and the resulting facts with `Fact.source` and revision — sufficient to answer "who decided this artifact could run, and on what evidence?"

**S8 — Failures never silently count.** `failed` / `inconclusive` / `timeout` results never upgrade confidence; the three-state discipline from `outcomes/` is mandatory.

## Non-goals

1. No verifier bundling — the user's toolchain provides Kani/Verus; we only recognize and parse their output.
2. Rust + Kani first; Verus second; other languages only after SPEC A Phase 3 adapters exist.
3. No LLM free-form codegen. Templates only — deterministic, reviewable, diffable.
4. No automatic execution of unreviewed artifacts, ever, including "just this once" policy exceptions.

## Acceptance criteria (evaluated when unblocked)

**C1 — Real proof, no simulation.** `safe_divide.zero_returns_error` renders a compiling Kani harness that proves the property. The end-to-end test runs real `cargo kani` (opt-in integration test; skipped when kani is absent, never faked).

**C2 — S2 refusal.** A `candidate` property's generation request is refused, and the refusal is journaled.

**C3 — Same-fire prohibition.** An artifact created in this hook fire is not executed in that fire (test).

**C4 — Provenance.** Execution appends `verification_result` with source, revision, and evidence kind; a failed proof does not lift the confidence Band.

## Open questions

1. Review-gate UX: human review per artifact vs. content-hash allowlist for identical re-renders (lean: allowlist, since templates + property hash determine the artifact deterministically).
2. Render-seam shape: extend `phronesis-rhai` with a render entry point (property record + facts in scope, `emit_fact` out) vs. reuse provider evaluation with a synthetic event — decided at implementation time; the sandbox limits bind either way.
3. Where obligations surface to the agent: an `Affordance` consequence ("reprove ?p via ?v") vs. a capsule. Lean Affordance at hook time, capsule for durable context.