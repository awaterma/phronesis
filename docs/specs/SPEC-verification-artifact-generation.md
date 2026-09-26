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

### Language neutrality (the architectural frame)

The pipeline is **(language, verifier)-parameterized** and language-neutral by construction — the same discipline as the rules engine (language-neutral; per-language predicates) and the tree-sitter extractors (per-language parsers behind one interface). Nothing in S1–S9 is Rust-specific:

- **Property records** are language-neutral (SPEC B); `depends_on` regions carry language-prefixed graph identity.
- **Encodings** are declared per pair on the property record: `{ language, verifier, artifact }` — a property may carry encodings in several (language, verifier) pairs.
- **Templates** are registered per `(language, verifier, kind)` — `verus-postcondition.rhai`, `kani-postcondition.rhai`, later `dafny-*`/`lean-*`; the template registry maps the encoding triple, never the pipeline.
- **ToolchainDefs** are already declarative and language-neutral (`matches`, `per_test`, `outcome_kind: "proof"`); S9's sandbox applies to whatever process the def runs.

The safety contract is the invariant across instantiations; only encodings, templates, and ToolchainDefs vary per language.

### Phase-1 artifact kind: standalone Verus-native (the first instantiation)

The proven 10-VC Verus harness on this machine is **standalone `verus!`-native code** (spec fns + `requires`/`ensures` over dedicated harness functions), not in-tree annotated production code. Phase 1 instantiates the generic pipeline with the (rust, verus) encoding — a **standalone Verus-native module**. A standalone harness proving a *plain-Rust* function would need `assume`d specs — a faked proof — so C1's subject is a Verus-native function. Kani remains the next (rust, kani) adapter (its plain-Rust-external-harness model differs; the skip-if-absent CI pattern applies to both). Other (language, verifier) pairs follow the same registration path: an encoding on the property record, a template, a ToolchainDef.

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

**S3 — Trust anchors and the review gate.** Generated artifacts are untrusted until reviewed: they land in `verification/unreviewed/` and are **never executed in the same fire that created them** — the prohibition holds even when the bytes match an allowlisted hash, because that file was written *this* fire. The **content-hash allowlist** is sound for re-renders under three conditions: (i) allowlist mutations are **human-principal acts** — additions rejected if attributable to the hooked session; (ii) each entry records the provenance tuple (artifact hash → template hash, property id + revision, approver principal, date); (iii) execution re-hashes the on-disk file at execution time and refuses on mismatch — approval binds to bytes, not paths. The executor computes the SHA-256 itself (64 lowercase hex) and never trusts a caller-supplied hash: it refuses when the on-disk digest differs from the hash the caller meant to run, or when that digest is not allowlisted, and the verifier then runs against a copy of exactly the hashed bytes in a fresh per-run directory, so the file cannot change between the check and the run. The allowlist loads fail-closed: any entry `record` would refuse (empty or non-SHA-256 artifact hash, empty template hash, property id, principal, or date) makes the whole file an error naming that entry, never a silently skipped or matching row. The first render of any (template, property) pair always requires human review; only re-renders skip. Approval registry is **hash-keyed** (an approved artifact may move to `verification/reviewed/`; execution keys on hash, never directory location — "unreviewed/" must not become a lie).

**S4 — Execution only via the post-check seam, composed as argv.** Verifiers run through registered ToolchainDefs on `Bash`-class tools, post-check, never pre-check, always journaled with revision and exit code. Verifier invocations are composed as **argv, never interpolated shell strings** — the artifact path is host-injected, never user-supplied. The S4 seam does not itself confine the verifier process — that is S9.

**S9 — Verifier process sandbox (tiered: devcontainer → sandbox-exec → raw → fail-closed).** The verifier executes generated code with full user privileges — the process must be confined. Confinement is **devcontainer-based** for portability: the (language, verifier) instantiation ships a `devcontainer.json` declaring the verifier's environment, and the **host composes the container forcing the confinement flags itself** — no network, read-only mounts except the artifact, no secret mounts, CPU/time limits — regardless of what the file says, so a weakening edit to the devcontainer config cannot unsandbox execution. The file contributes exactly one thing, its `image`, which must be pinned by digest (`name@sha256:<64 hex>`); a missing file, field, or pin refuses the tier with an error naming the file. The host runs that image with `--pull=never` (a registry fetch never chooses the verifier), `--network=none`, `--read-only`, a `/tmp` tmpfs, all capabilities dropped, `no-new-privileges`, and memory/CPU/pid limits; the only mount is the per-run directory, read-only at `/verification` (a run directory containing a comma is refused — `--mount` has no escaping for it), and the artifact path is rewritten to its path under `/verification`. Verifier tokens follow the image after a `--` terminator, so none of them can become the image. (The devcontainer file is human-principal-owned repo content — a trust anchor under S3 — but the host never trusts it for confinement.)

**The `raw` tier is a downgraded trust tier, not a peer.** The provenance argument: `cargo build`/`cargo test` already compile and execute unsandboxed code on every run (build scripts, proc macros) — but that code is human-written and reviewed in PRs, the accepted baseline. Machine-rendered artifacts are compiled from property records whose fields can carry prompt-adjacent data — exactly the "prompt-adjacent data → arbitrary execution" machine this spec exists to prevent. Raw removes confinement, never review. Four disciplines:

1. **Raw is opt-in per project** — a config the human sets (S1 marker discipline), never a default. The ladder: devcontainer → sandbox-exec → **raw (explicitly configured)** → fail-closed refusal.
2. **Raw still requires allowlisted bytes** — the S3 human-approved-hash gate applies before execution regardless of tier.
3. **The tier is recorded in the audit trail** (S7) — a proof that ran raw is visible evidence, so a project drifting to raw-everywhere is visible rather than silent.
4. **Tier selection stays host-enforced** — the runner picks the strongest available tier and falls back to raw only when the config explicitly allows it.

**S5 — Templates are trusted; property data and rendered output are not.** Template scripts are version-controlled, **human-principal-owned** repo content, loaded under the same constraints as predicate providers. Two validation layers: (1) the **field-class contract** at property ingest *and* render — identifier-shaped fields (id, subject) validated against an identifier charset; free-text fields embedded only through host-side escaped-literal encoding (escape *before* the value enters Rhai scope); property ids containing quotes/delimiters rejected at ingest; (2) the rendered body is re-parsed and every interpolated value asserted to appear only in its sanctioned syntactic position; dangerous constructs are rejected from a **per-language deny-list** registered with the template (rust: `include!`/`include_str!`/`include_bytes!`, `#[path]`, `extern crate`, `unsafe`, `std::process`/`fs`/`net`; python: `eval`/`exec`/`os.system`/`subprocess`; etc.) — the deny-lists are part of the (language, verifier) instantiation, not the pipeline. Validation failure journals the refusal and writes nothing. No string reaches a written artifact unvalidated.

**S6 — Containment.** Artifact paths canonicalize inside `verification/` (host-generated slug + short content-hash filenames — two properties must never clobber one file; differing content at the same name creates a new file requiring fresh review); sizes capped; no template output escapes `verification/`. Verifier invocations are composed as argv, never interpolated shell strings.

**S7 — Full audit trail.** Every generation and execution appends: `log.jsonl` entry, journey tag, and the resulting facts with `Fact.source` and revision — sufficient to answer "who decided this artifact could run, and on what evidence?" Artifact bodies journal as hash + path, never inline.

**S8 — Failures never silently count — and silence is a state.** Result statuses are `passed` | `failed` | `inconclusive` | `timeout` | `unknown`. Failed/inconclusive/timeout never upgrade confidence. **A run whose output parses to zero outcomes emits `verification_result` with status `inconclusive` plus the raw output tail in the journal and never lifts the proof signal** — verifier version drift changing output format must degrade to loud silence, not a green light.

## Known-bug precedence

An open known-bug entry referencing a property blocks band lift from `signal_pass(…, "proof")`, flags the `verification_result` as contradicted, and journals both facts. A passing proof must not lift confidence over a documented contradicting bug.

## Execution discipline

Proofs are minutes-long; the post-check seam never blocks the current call. Executions dedup by **(artifact content hash, tree revision)** — at most one execution per revision; `result_revision` and the dirty flag are recorded at *run start*. Queue executions to a once-per-revision drain rather than per-fire.

## Non-goals

1. No verifier bundling — the toolchain provides Verus/Kani; we recognize and parse their output.
2. One (language, verifier) instantiation first — (rust, verus), the proven toolchain on this machine; Kani as the next; other languages (python/dafny, lean, …) follow the identical registration path (encoding + template + ToolchainDef) after SPEC A Phase 3 per-language adapters exist. No harness of any language that leans on `assume` to go green.
3. No LLM free-form codegen. Templates only — deterministic, reviewable, diffable.
4. No automatic execution of unreviewed artifacts, ever, including "just this once" exceptions — and the same-fire prohibition holds even when content matches an allowlisted hash.

## Acceptance criteria

**C1 — Real proof, no simulation (first instantiation: verus-native).** A Verus-native `safe_divide` postcondition property renders a compiling Verus harness that proves it with real `cargo-verus`/`verus` (opt-in integration test; skipped when the toolchain is absent, never faked, never `assume`d green). The test doubles as the template for later (language, verifier) pairs — a python/dafny instantiation repeats this criterion with its own def, unchanged in shape.
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

## Resolved decisions (review follow-up)

1. **Encoding artifact paths participate in graph relations.** Each property encoding carrying an `artifact` path emits an `includes_file`-style graph edge at rebuild (the `graph/bindings.rs` stale-rule-binding precedent): deleted artifact, content drift, or path escape surfaces through the graph's existing drift machinery and feeds `stale_evidence` the same way. The edge is demand-gated like other graph relations — a rule must mention it before it asserts.

2. **S9 confinement tiers (macOS reality, resolved).** The threat S9 answers: the verifier executes generated code with full user privileges, so something must confine that process. Which mechanism is available is host-dependent, so S9 resolves confinement by **tier at execution time, recording the tier in the audit trail (S7)**:
   - **Tier 1 — devcontainer** (portable across docker/podman and across Linux/macOS hosts with a runtime; the same `devcontainer.json` carries the environment to CI): host-enforced no-network, read-only mounts except the artifact, resource limits.
   - **Tier 2 — sandbox-exec** (macOS native Seatbelt): no network, and every file write denied except under the per-run directory (which holds the hashed artifact copy in `artifact/` and the `TMPDIR` the host sets in `tmp/`), plus `/dev/null`. Writes by proxy count as writes: mach-lookup is denied, so no daemon (cfprefsd behind `defaults write`, the pasteboard behind `pbcopy`) can write on the verifier's behalf, and signals are allowed only to the verifier itself, so it cannot kill or stop the user's other processes. The profile is `(allow default)(deny network*)(deny mach-lookup)(deny signal)(allow signal (target self))(deny file-write*)(allow file-write* (subpath (param "RUN_DIR")))…`; the canonical run directory is passed with `sandbox-exec -D RUN_DIR=…`, never spliced into the profile text (Seatbelt matches resolved absolute paths, so a relative `subpath` confines nothing). Measured: the Verus 10-VC harness proves under this profile (mach-lookup and foreign signals denied) and writes nothing. Deprecated-but-functional Apple API, zero dependencies.
   - **Tier 3 — raw** (explicitly configured, human-set): no confinement — full S3 review gate still required, tier recorded so raw-everywhere drift is visible.
   - **Tier 4 — fail-closed**: no confinement available and raw not configured → execution refused, journaled as refused-sandbox, and the S9 claim is downgraded in that host's audit trail rather than silently.