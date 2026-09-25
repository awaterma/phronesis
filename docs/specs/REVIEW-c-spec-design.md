# Design review: SPEC-verification-artifact-generation.md (pre-implementation)

**Cross-model adversarial review, run through the swarm's batch-call fan-out** — three independent
families: DeepSeek-V4-Pro (auction winner, $0.047), GLM-5.3 ($0.095), Kimi-K3 (empty result; see note).
Merged verbatim findings, deduplicated. Consensus blockers — findings both models reached
independently — are the implementation gates.

## Consensus blockers (both reviewers, independently)

1. **Property-field injection into compiled code** — property fields flow through Rhai into a `.rs`
   file compiled with full crate privileges. `"); std::process::Command…` passes path/size checks.
   Both reviewers: identifier-shaped fields validated at ingest; free-text embedded only via
   host-side escaped-literal encoding; re-parse the rendered body and assert every interpolated
   value sits only in its sanctioned syntactic position. (DeepSeek #5, GLM #2)

2. **Trust anchors are agent-actuable** — the opt-in marker, allowlist, approval acts, and
   templates are all files the agent can write through the same hooked seam. "Audited is not
   reviewed": edit template → render → self-allowlist → execute satisfies every S-requirement
   as written. Fix: enumerate trust anchors and require human-principal acts; host refuses
   agent-seam writes to those paths (journaled refusal); allowlist entries bind approver
   principal + never an attribution from the hooked session. (GLM #1; DeepSeek #2/#3 are the
   narrower instances.)

3. **First-proof obligation gap** — `stale_evidence` presupposes an existing result, so the
   obligation join never fires for an accepted-but-never-proved property: the FIRST proof is
   unreachable, exactly the C1 scenario. Fix: obligation = (no result at head) OR (stale by
   region content hash, not revision inequality — reverts stop re-firing); obligations journal
   as durable facts, not fire-ephemeral RETE state. (GLM #3; DeepSeek #7 is the spurious/
   suppressed counterpart.)

4. **Execution sandbox for the verifier process** (S9, proposed) — even validated safe Rust
   does I/O if the verifier runs it. Run verifiers containerized: no network, read-only FS
   except the artifact, no secrets, resource limits. (DeepSeek #4.)

5. **C1 is unsatisfiable as written on this machine — and possibly by construction.** Kani is
   not installed; and a standalone Verus harness proving plain-Rust `safe_divide` would need
   `assume`d specs — assuming the postcondition to prove it violates C1's own no-simulation
   clause. Flip to Verus-first for pipeline validation, rewrite C1 (verus-native subject or
   install Kani), and classify the proven 10-VC harness first: in-tree annotated code implies a
   second artifact kind (in-tree spec insertion) that S6 doesn't cover. (GLM #7, DeepSeek #9.)

## Other findings (merged, both reviews)

- **S5 validator is nonexistent** — define the field-class contract; deny dangerous macros in
  rendered bodies (`include!`, `unsafe`, `std::process`/`fs`/`net`); structured output over raw
  strings. (DeepSeek #5, GLM #2)
- **S2 reads the record, not the fact stream** — facts trigger, they never authorize; property
  edits bump revision and invalidate allowlisted artifacts. (GLM #4)
- **Render seam decided now, not at implementation time**: dedicated render entry — no
  `emit_fact`, read-only facts, frozen sorted input set, `eval` disabled, scope-freeze test +
  byte-determinism test. (GLM #5, DeepSeek #8)
- **Allowlist: yes, with conditions** — human-only mutations; provenance tuples
  (artifact hash, template hash, property id+revision, approver, date); re-hash on disk at
  execution; first render of a (template, property) pair always needs human review. (GLM #6,
  DeepSeek #2/#14)
- **~4KiB cap**: measure against the proven harness; GLM proposes 64 KiB + line cap; DeepSeek
  proposes ≥64KiB or per-template config. The proven 10-VC harness is the calibration point.
  (DeepSeek #6, GLM #12)
- **Post-check execution thrash**: minutes-long proofs re-fire per edit — dedup by
  (artifact hash, tree revision), record dirty flag at run start. (GLM #8, DeepSeek #7c)
- **S8's missing fourth state**: parser matched nothing → `inconclusive` + raw output tail in
  the journal, never a band lift. (GLM #9)
- **Property-id forgery across per_test mapping**: host-generated slug + hash test names,
  anchored matching, delimiter rejection at ingest. (GLM #10, DeepSeek #16)
- **Missing acceptance criteria (merged)**: C5 injection containment; C6 mutation detection
  (introduce the forbidden bug — the proof must fail); C7 trust-anchor tamper refusal;
  C8 status source-of-truth (record vs fact stream); C9 first-proof obligation; C10
  empty-parse → inconclusive. (DeepSeek #10, GLM #11)
- **Known-bug precedence** (#15), **argv composition not shell strings** (#16), **journal economy —
  hash+path not inline bodies** (#17), **audit-trail integrity** (DeepSeek #12).

## Review provenance

- together-deepseek-v4-pro: auction winner ($5 bid, $0.047 actual) — 16 findings
- together-glm-5.3: independent second reviewer ($0.095 actual) — 17 findings, the deeper
  trust-anchor analysis
- Kimi-K3's call returned an empty result at 0.25 charged (second occurrence; reported as provider.rs bug) — its findings are absent from this merge.

## Implementation gates this review sets

1. Rewrite the spec: human-principal trust anchors, explicit obligation OR, the render entry
   decision, Verus-first with the C1 soundness rewrite, S9 sandbox, the merged C5-C10 criteria.
2. First implementation step: classify the proven Verus harness (in-tree vs standalone) — it
   decides whether the spec needs a second artifact kind.
