# Opus Handoff: Token-Aware Durable Context

**Date:** 2026-07-31  
**Branch:** `feat/token-aware-durable-context`  
**State:** Mid-implementation, uncommitted  
**Source specification:** [`../specs/2026-07-31-token-aware-durable-context-design.md`](../specs/2026-07-31-token-aware-durable-context-design.md)

## Objective

Implement the token-aware durable-context design for `phronesis-mcp`: preserve a small durable kernel, select evidence-backed contextual nudges through ordinary RETE rules, pack context deterministically within byte and estimated-token budgets, expose diagnostics and measurements, and retain byte-for-byte legacy behavior unless a project opts in through `.phronesis/context.json`.

The specification was aggressively reviewed with `glm-5.2:cloud`. Its final assessment was:

> PLAN-READY — NO CRITICAL OR HIGH BLOCKERS

Do not treat the present code as complete merely because it compiles. The core is in place, but several correctness, observability, initialization, and acceptance-measurement requirements remain.

## Repository Safety

The working tree was already dirty and no implementation commit exists yet.

- The untracked design specification belongs on this feature branch.
- `docs/phronesis-evaluation-2026-07-31.html` is unrelated existing work. Do not modify, delete, stage, or commit it.
- Preserve all user changes. Do not reset or check out files destructively.

Current status:

```text
## feat/token-aware-durable-context
 M crates/phronesis-mcp/src/codex_hook.rs
 M crates/phronesis-mcp/src/context.rs
 M crates/phronesis-mcp/src/init.rs
 M crates/phronesis-mcp/src/main.rs
?? crates/phronesis-mcp/src/context/
?? docs/phronesis-evaluation-2026-07-31.html
?? docs/superpowers/specs/2026-07-31-token-aware-durable-context-design.md
```

## Implemented So Far

### Configuration and packing

New modules are declared by `context.rs`:

- `context/config.rs`: strict versioned configuration, defaults, validation, safe path resolution, and missing/malformed/invalid distinctions.
- `context/packing.rs`: stateless deterministic packing, byte and estimated-token budgets, per-kind ceilings, nudge ordering, omission reason types, and a bounded omission footer.
- Token estimation currently uses `ceil(bytes / 3)`, as specified.

Legacy `run_interaction_context` and `run_session_context` remain intact. New asynchronous configured paths opt into packing only when `.phronesis/context.json` exists. Missing configuration delegates to the exact legacy functions.

### Context capsules and RETE selection

`context/capsule.rs` implements:

- Strict capsule frontmatter parsing with recursive duplicate-key rejection.
- Exact v1 predicate allowlist:
  - `context_confidence_band`
  - `journey_filtered_since_ge`
  - `journey_seen`
  - `journey_since_ge`
- Positive-only `all`/`any` condition trees, depth and DNF expansion caps.
- File, aggregate, and author-declared body-size limits.
- Safe path resolution and symlink-escape protection.
- Deterministic file loading and duplicate-ID rejection.
- Compilation to ordinary `phr::Rule` values using the string action type `context_nudge`.
- Demand hydration of journey facts only when selected predicates require them.
- Confidence-band fact projection and consequence-to-nudge matching.

### Metrics and CLI

`context/metrics.rs` writes `kind: "context"` observations into the existing action log and aggregates payload size, token estimates, latency, selections, omissions, and raw truncations.

`main.rs` adds:

```text
phr-mcp context inspect --event interaction|session [--path ...] [--json]
phr-mcp context predicates [--json]
phr-mcp context stats [--since ...] [--path ...] [--json]
```

The normal session and interaction commands now use the configured asynchronous renderers.

### Host wiring and initialization

- Codex context construction is asynchronous and uses the configured renderers for Session, UserPrompt, PostCompact, and SubagentStart events.
- `Pack::Context` was added to initialization.
- The context pack can scaffold a compact durable kernel, default `.phronesis/context.json`, and a capsule README while preserving existing files.

## Verification Already Performed

These passed with the present implementation:

```text
cargo check -p phronesis-mcp
cargo test -p phronesis-mcp --lib context::
  30 passed, 0 failed
cargo test -p phronesis-mcp --lib
  1069 passed, 0 failed
```

The full workspace suite, integration tests, formatting, and Clippy have not been run after these changes.

## Highest-Priority Remaining Work

### P0 — Make packing and metrics semantically exact

1. Refactor the configured render path so it returns a structured render/inspection result before envelope wrapping. The same result should drive live output, inspection, and metrics.
2. Detect and record any last-resort raw truncation performed by `wrap_additional_context`. Metrics are currently written before wrapping and always report `raw_truncation: false`.
3. Tighten `displaced_by_nudge`: it is currently inferred whenever an activity item overflows after at least one nudge is selected, rather than proven to have fit without that nudge.
4. Prove reserve/ceiling behavior with focused and property-style tests, including footer admission and very small budgets.
5. Make activity ordering faithfully represent current block, current warning, then older decisions newest-first. Current lexical stable-ID ordering is only an approximation.
6. Guard confidence projection with `outcomes::enabled(root)` so confidence facts/state appear only when configured and an open subject exists.

### P0 — Make `context inspect` a real dry-run diagnostic

At present, `inspect` invokes the live configured renderer and therefore writes a context metric, contaminating the data it is supposed to inspect. It also reports little more than the rendered body and capsule load diagnostics.

Implement an inspection model that:

- Does not append to `log.jsonl`.
- Reports missing, malformed, invalid, or defaulted configuration explicitly.
- Lists candidate items, selected items, byte/token costs, ceilings, and item-level omission reason codes.
- Includes capsule load and runtime fact-hydration/matching diagnostics.
- Exposes whether the envelope’s last-resort guard would truncate.
- Makes human and JSON output projections of the same data.

### P1 — Complete capsule correctness and diagnostics

Add tests for:

- Nested duplicate JSON keys.
- Symlink escape attempts.
- Body larger than author `max_bytes`.
- Duplicate capsule IDs skipping every copy.
- Template/variable-like body rejection.
- Maximum condition depth and DNF alternative count.
- Invalid journey selectors/windows and their inspect-time diagnostics.
- Deterministic ordering, including problematic filenames where practical.

Avoid silently relying on `dnf(...).unwrap_or_default()` after validation; make the validated invariant or error path explicit. Runtime journey derivation failures currently collapse matching to an empty result and are not surfaced by inspection.

### P1 — Finish initialization behavior

1. Inspect `.gitignore` generation. Existing `.phronesis/*` patterns may ignore the new project-owned context config and capsules. Add the necessary carve-outs and tests.
2. Expand the generated capsule README to document the actual v1 schema, predicate allowlist, examples, static-body restriction, and limits.
3. Add init integration tests for:
   - Context pack selection.
   - Dry runs.
   - Existing-file preservation.
   - Gitignore behavior.
   - Repeated/idempotent initialization.

### P1 — Complete integration coverage

Add CLI and host-level tests for:

- Exact legacy parity when configuration is absent.
- Malformed configuration using bounded defaults with a visible diagnostic.
- Interaction and session budget compliance.
- Async Codex hook behavior after the refactor.
- Claude/Gemini command paths.
- `context predicates` and both `context stats` formats.
- Invalid `--since` producing a diagnostic rather than silently becoming all-time.
- Metrics aggregation, empty logs, cutoff filtering, and p95 calculations.
- Existing stats readers safely ignoring `kind: "context"` records.

Consider whether SubagentStart should emit the same session metric event name; it is outside the core durability contract and is currently indistinguishable in observations.

### P2 — State completeness and documentation

- Add the graph-freshness state/diagnostic called for by the specification if the repository exposes a reliable source.
- Document the new commands and context pack in README/CLAUDE/AGENTS references where appropriate.
- Add a changelog entry and decide the pre-1.0 version bump. This is a user-visible feature and normally warrants a minor bump under the repository policy.
- Include the source design specification in the feature commit.

## Acceptance and Rollout Gate

Do not switch this on by default merely after tests pass. The specification requires opt-in first, then evidence.

Use representative repositories/events to compare configured output with legacy output and record:

- Payload bytes and estimated tokens.
- Selected item mix.
- Omissions by reason.
- Raw truncation count.
- Rendering latency.
- Whether essential kernel/rule/blocking information survived.

Apply the rollout thresholds from the source specification, including the intended payload reduction target and zero unexpected raw truncation. A temporary fixture/project copy is preferable to modifying this repository’s live `.phronesis` state during measurement.

## Recommended Continuation Sequence

1. Read the full source specification before editing; it is the contract.
2. Inspect the complete diff and new modules.
3. Introduce a structured, side-effect-free render result shared by live rendering and inspect.
4. Fix packing attribution, activity ordering, confidence gating, and raw-truncation metrics.
5. Complete capsule diagnostics and adversarial tests.
6. Finish init/gitignore/README behavior and integration tests.
7. Run formatting and progressively broader tests.
8. Run the measurement gate and document results.
9. Update user-facing docs/version/changelog.
10. Stage only relevant files, explicitly excluding the unrelated evaluation HTML.

## Commands for Resuming

```bash
git status --short --branch
git diff -- crates/phronesis-mcp/src/context.rs \
  crates/phronesis-mcp/src/context \
  crates/phronesis-mcp/src/codex_hook.rs \
  crates/phronesis-mcp/src/init.rs \
  crates/phronesis-mcp/src/main.rs

cargo fmt --all -- --check
cargo test -p phronesis-mcp --lib context::
cargo test -p phronesis-mcp --tests
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

Run the final workspace commands directly rather than through output-filtering pipes so failures and exit codes remain trustworthy.

## Repository Conventions to Preserve

- No `.unwrap()` in production paths.
- Use safe path resolution for project-controlled paths.
- Preserve exact legacy behavior outside explicit opt-in.
- Keep selection deterministic and stateless; the action log is observational, not hidden selection state.
- Use the ordinary RETE engine and arbitrary string `Action` contract for capsule nudges.
- Treat byte caps as hard limits and token estimates as a conservative planning signal.
- Use `apply_patch` for source edits and preserve unrelated dirty-worktree content.

