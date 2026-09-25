# SPEC: Property-driven verification artifact generation (safety-gated)

**Status:** Approved for implementation — revised per `REVIEW-c-spec-design.md` (cross-model design review: deepseek-v4-pro + glm-5.3). Preconditions met: `SPEC-coverage-evidence.md` (A) and `SPEC-property-ontology.md` (B) are landed.
**Author:** awaterma (agent-drafted, adapted from `REQUIREMENTS-phronesis-coverage-verification-rust-sketch.md`); revised per the pre-implementation cross-model review
**Created:** 2026-09-23; revised 2026-09-24
**Scope:** `crates/phronesis-mcp` host only
**Derives from:** `REQUIREMENTS-phronesis-coverage-verification-rust-sketch.md` §15–§16, §19, §22–§24
**Companions:** `SPEC-coverage-evidence.md` (A), `SPEC-property-ontology.md` (B)
**Reuses:** `SPEC-confidence-scoring.md` (ToolchainDefs, signals), `SPEC-journey-facts.md` (journal)

## Summary

One semantic property should derive both a proof harness and a conventional test (sketch §15). The spec's primary content is the **safety contract**, because executing generated code is arbitrary code execution. This revision incorporates the pre-implementation cross-model review: human-principal trust anchors, an explicit first-proof obligation, a decided render seam, a verifier sandbox, and the C5–C10 acceptance criteria.

### The sketch's "Phronesis-Rye" is `phronesis-rhai`

Templates are `.rhai` scripts (`kani-postcondition.rhai`, `verus-postcondition.rhai`, …): each receives the property record and relevant facts in scope and emits the rendered artifact body. Division of labor:

- **Rhai renders.** Through a **dedicated render entry point** (decided here, resolving the spec's former open question 2 — do NOT reuse provider evaluation with a synthetic event: the artifact body must never enter the fact stream, where any rule could match it or another template re-embed it). The entry: read-only access to the property record and a **frozen, sorted input set**; returns a string; `emit_fact` is absent from render scope; `eval` and dynamic script evaluation are disabled (already the raw-engine default — enforced by a scope-freeze test).
- **Rust acts.** The host validates the emitted body (S5), writes it into `verification/unreviewed/` (S3, S6), invokes the verifier through the post-check seam (S4), and records the structured result.

Known limits: emitted bodies are capped (see S5 for the measured cap); a template needing more becomes an explicit host-side Rust template, recorded as an exception in review.

### Artifact kind: standalone Verus-native (decided)

The proven 10-VC Verus harness on this machine is **standalone `verus!`-native code** (spec fns + `requires`/`ensures` over dedicated harness functions), not in-tree annotated production code. Phase 1's artifact kind is therefore a **standalone Verus-native module** — no in-tree spec-insertion kind is required yet. A standalone harness proving a *plain-Rust* function would need `assume`d specs — a faked proof — so C1's subject is a Verus-native function. Kani remains a later adapter (its plain-Rust-external-harness model differs; the skip-if-absent CI pattern applies to both).

## Problem

A property plus a changed region yields an obligation: the claim must be re-established. Today the only way to satisfy it is hand-written harnesses. The omitted risk: an agent-driven pipeline that generates-then-executes without a review boundary is a machine that turns prompt-adjacent data into arbitrary execution — and the trust anchors (opt-in marker, allowlist, approval records, templates) are themselves files an agent can actuate. The safety contract below therefore treats **trust anchors as human-principal acts**, not merely "audited" ones: an audit trail that records the agent approving the agent is not a review gate.

## Pipeline

```text
accepted property (record read host-side at generation time — facts trigger, they never authorize)
        ↓  obligation = (no verification_result at HEAD) OR (stale: dependent-region content hash changed)
           [journaled as durable facts, recomputable from the journal — never fire-ephemeral RETE state]
           [coalesced per fire; render idempotent by content hash; executions deduped by (artifact hash, tree revision)]
Rhai template script (version-controlled, human-principal-owned repo content)
        ↓  emits the rendered artifact body via the dedicated render entry
host validates (S5: field-class contract + rendered-body re-parse) and writes verification/unreviewed/<artifact>.rs
        ↓  review gate (S3: human-principal approval, content-hash-bound)
execution via registered ToolchainDefs — composed as argv, never interpolated shell strings (S4, sandboxed per S9)
        ↓  structured result
verification_result fact + result_revision + journey tag + provenance (SPEC B §3); band lift per S8
```

## Safety requirements

**S1 — Opt-in only, and the opt-in is a trust anchor.** Nothing is generated or executed unless the project opts in (marker-file pattern). The opt-in marker, the allowlist, approval records, and `verification/templates/` are **trust anchors**: the host refuses agent-seam writes to all four (journaled refusal); if the host cannot enforce write refusal, the spec's review-gate claim downgrades to "advisory" and this spec says so — it does not ship silently. Default installs generate nothing.

**S2 — Generation precondition: accepted status read from the record.** Generation requires `property_status ∈ {accepted, verified}` **read host-side from `properties.json` at generation time** — facts trigger the obligation, they never authorize it (a fact stream claiming `accepted` while the record says `candidate` is refused and journaled). Any edit to a property record bumps its revision; artifact provenance binds (template hash, property id, property revision), which invalidates allowlisted artifacts after property edits. Never `candidate`, never `observed`.

**S3 — Trust anchors and the review gate.** Generated artifacts are untrusted until reviewed: they land in `verification/unreviewed/` and are **never executed in the same fire that created them** — the prohibition holds even when the bytes match an allowlisted hash, because that file was written *this* fire. The **content-hash allowlist** is sound for re-renders under three conditions: (i) allowlist mutations are **human-principal acts** — additions rejected if attributable to the hooked session; (ii) each entry records the provenance tuple (artifact hash → template hash, property id + revision, approver principal, date); (iii) execution re-hashes the on-disk file at execution time and refuses on mismatch — approval binds to bytes, not paths. The first render of any (template, property) pair always requires human review; only re-renders skip. Approval registry is **hash-keyed** (an approved artifact may move to `verification/reviewed/`; execution keys on hash, never directory location — "unreviewed/" must not become a lie).

**S4 — Execution only via the post-check seam, composed as argv.** Verifiers run through registered ToolchainDefs on `Bash`-class tools, post-check, never pre-check, always journaled with revision and exit code. Verifier invocations are composed as **argv, never interpolated shell strings** — the artifact path is host-injected, never user-supplied. The S4 seam does not itself confine the verifier process — that is S9.

**S9 — Verifier process sandbox.** The verifier executes generated code: run it in a container/VM with no network, read-only filesystem except the artifact, no secret access, and CPU/time limits. If the host cannot sandbox, execution stays refused and the spec's claims are downgraded accordingly.

**S5 — Templates are trusted; property data and rendered output are not.** Template scripts are version-controlled, **human-principal-owned** repo content, loaded under the same constraints as predicate providers. Two validation layers: (1) the **field-class contract** at property ingest *and* render — identifier-shaped fields (id, subject) validated against an identifier charset; free-text fields embedded only through host-side escaped-literal encoding (escape *before* the value enters Rhai scope); property ids containing quotes/delimiters rejected at ingest; (2) the rendered body is re-parsed and every interpolated value asserted to appear only in its sanctioned syntactic position; dangerous constructs (`include!`/`include_str!`/`include_bytes!`, `#[path]`, `extern crate`, `unsafe`, `std::process`/`fs`/`net`) are rejected. Validation failure journals the refusal and writes nothing. No string reaches a written artifact unvalidated.

**S6 — Containment.** Artifact paths canonicalize inside `verification/` (host-generated slug + short content-hash filenames — two properties must never clobber one file; differing content at the same name creates a new file requiring fresh review); sizes capped; no template output escapes `verification/`. Verifier invocations are composed as argv, never interpolated shell strings.

**S7 — Full audit trail.** Every generation and execution appends: `log.jsonl` entry, journey tag, and the resulting facts with `Fact.source` and revision — sufficient to answer "who decided this artifact could run, and on what evidence?" Artifact bodies journal as hash + path, never inline.

**S8 — Failures never silently count — and silence is a state.** Result statuses are `passed` | `failed` | `inconclusive` | `timeout` | `unknown`. Failed/inconclusive/timeout never upgrade confidence. **A run whose output parses to zero outcomes emits `verification_result` with status `inconclusive` plus the raw output tail in the journal and never lifts the proof signal** — verifier version drift changing output format must degrade to loud silence, not a green light.

## Known-bug precedence

An open known-bug entry referencing a property blocks band lift from `signal_pass(…, "proof")`, flags the `verification_result` as contradicted, and journals both facts. A passing proof must not lift confidence over a documented contradicting bug.

## Execution discipline

Proofs are minutes-long; the post-check seam never blocks the current call. Executions dedup by **(artifact content hash, tree revision)** — at most one execution per revision; `result_revision` and the dirty flag are recorded at *run start*. Queue executions to a once-per-revision drain rather than per-fire.

## Non-goals

1. No verifier bundling — the toolchain provides Verus/Kani; we recognize and parse their output.
2. Verus-first (standalone Verus-native harness); Kani as a later adapter. No Verus harness that leans on `assume` to go green.
3. No LLM free-form codegen. Templates only — deterministic, reviewable, diffable.
4. No automatic execution of unreviewed artifacts, ever, including "just this once" exceptions — and the same-fire prohibition holds even when content matches an allowlisted hash.

## Acceptance criteria

**C1 — Real proof, no simulation (verus-native).** A Verus-native `safe_divide` postcondition property renders a compiling Verus harness that proves it with real `cargo-verus`/`verus` (opt-in integration test; skipped when the toolchain is absent, never faked, never `assume`d green).
**C2 — S2 refusal.** A `candidate` property's generation request is refused and journaled; the refusal reads the record, not the fact stream (a fact stream claiming `accepted` while the record says `candidate` is refused — C8).
**C3 — Same-fire prohibition.** An artifact created in this fire is not executed in that fire — even when it matches an allowlisted hash.
**C4 — Provenance.** Execution appends `verification_result` with source, revision, and evidence kind; a failed proof does not lift the confidence Band.
**C5 — Injection containment.** A property record carrying hostile Rust payloads renders inert (escaped) or generation refuses — journaled either way; nothing is written unvalidated.
**C6 — Mutation detection.** Introduce the exact bug the property forbids into the subject function, re-run the pipeline: the proof must fail. (The only test that catches claim-binding drift.)
**C7 — Trust-anchor tamper.** Agent-seam writes to the allowlist/opt-in/templates are refused; an artifact whose hash was self-added is refused execution.
**C8 — Status source-of-truth.** A fact stream claiming `accepted` while the record says `candidate` → refusal, journaled.
**C9 — First-proof obligation.** Accepted property, changed dependent region, no prior result → generation fires. (The obligation's OR, not the staleness AND.)
**C10 — Empty-parse.** Garbage verifier output → `inconclusive` fact exists, no band lift.

## Open questions — resolved

1. **Review-gate UX:** content-hash allowlist, with the three S3 conditions (human-only mutations; provenance tuples; disk re-hash at execution). First render of any (template, property) pair requires human review; re-renders of identical bytes skip — the same-fire prohibition still holds for the file written this fire.
2. **Render-seam shape:** the **dedicated render entry point**, decided now. Scope: read-only property record + frozen sorted dependency facts; one typed return (the body) with exactly one consumer (validator → write); `emit_fact` absent; `eval` disabled; scope-freeze test (zero host functions in render scope) and a determinism test (same inputs → byte-identical output, twice).
3. **Obligation surfacing:** an `Affordance` at hook time presenting the review-relevant fields (property id + revision, template id + hash, artifact path, content hash, diff vs last approved artifact) and a capsule for durable context. Accepting the Affordance is not the approval — approval remains the human-principal act (S3).

## Post-implementation calibration

The emitted-body cap is set by measurement against the proven 10-VC harness (209 lines); the review proposes 64 KiB + a 512-line absolute cap — commit the number only after measuring. The host-stamped provenance header (`GENERATED — DO NOT EDIT`, property id + revision, template hash) is budgeted outside the render cap. The host-side Rust template path is first-class (for Verus inductive proofs it is the common path — a frequent exception must be a design smell the spec acknowledges, which this paragraph does).

## Open questions (new, from review)

1. Do encoding artifact paths participate in graph relations (`includes_file`-style edges) so stale encodings are detectable like stale rule bindings?
2. Is the container sandbox (S9) available on macOS hosts without Docker? If not, S9 degrades to: verifier runs with a scrubbed env, no network access via sandbox-exec profile, and the spec says so.