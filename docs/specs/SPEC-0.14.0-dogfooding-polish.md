# SPEC: 0.14.0 — dogfooding-driven polish

**Status:** draft
**Authors:** Claude, Andrew Waterman
**Date:** 2026-06-21
**Target release:** phronesis-mcp 0.14.0 (MINOR — net new feature
              surface across four small subsystems; phr library
              stays at 0.13.3 because the engine is unchanged)
**Compiles:**
- `SPEC-gate-merge-commits.md`
- `SPEC-pack-opt-in-facts.md`
- `SPEC-extract-rules-defaults.md`
- `SPEC-journey-filtered-since.md`

## Why these four ship together

The 0.12.0 confidence-scoring and 0.13.0 journey-facts releases were
designed by reasoning about how phronesis *should* work. The 0.13.x
patch line was driven by playtest — bugs we only saw after the binary
was installed and the project ate its own dog food for a session. Four
distinct friction points surfaced from that dogfooding:

| Friction observed | Origin spec |
|---|---|
| `git merge`, `rebase`, etc. produce commits but bypass the confidence gate | `SPEC-gate-merge-commits` |
| Every `git commit` warned twice — once from the confidence gate, once from the LLM-pack `nudge-verify-before-commit` | `SPEC-pack-opt-in-facts` |
| Gemini ran `extract_rules` against `RUST-PATTERNS-GUIDE.md` and added 27 block-action rules with `[pattern]` prefixes leaking into messages | `SPEC-extract-rules-defaults` |
| `build-staleness` fired after 8 Bash commands even though no code was written; the threshold counted raw tool calls, not writes | `SPEC-journey-filtered-since` |

Each friction is local; the fix for each is small (single-file, single
function, low test surface). Releasing them as four separate PATCH
bumps would be honest but tedious. Bundling them into 0.14.0 lets
users get the full quality-of-life lift in one rebuild + `cargo
install` cycle.

None of the four changes shape-breaks anything. Each existing rule, fact,
or tool keeps working; the new behavior is additive (`confidence_enabled`
fact, `journey_filtered_since_ge` aggregator, broadened pattern), or it's
a default-tightening that the salvage paths in each sub-spec handle for
existing projects (extracted rules salvage, gate pattern migration).

## What 0.14.0 ships

### From `SPEC-gate-merge-commits`

- **Broaden the confidence gate** in `init.rs::confidence_rules`:
  ```diff
  - { "bash_command_matches": "git commit" }
  + { "bash_command_matches": "git (commit|merge|rebase|cherry-pick|revert|pull)" }
  ```
- **Test fan-out** in `confidence_gate_integration.rs`: one test per
  command shape (commit / merge / rebase / cherry-pick / revert / pull),
  plus `gate_does_not_fire_on_unrelated_git_command`.
- **Migration story**: existing projects re-run
  `phr-mcp init --rules-only --force --packs confidence` or hand-edit
  the two rules. Documented in CHANGELOG.

### From `SPEC-pack-opt-in-facts`

- **Assert `confidence_enabled` fact** at every hook fire when
  `.phronesis/confidence.json` exists. Mirrors the
  `clock_facts.rs::business_hours_local` pattern — zero-arg fact,
  computed from real-world state at the moment of firing. Add to
  `clock_facts.rs` (and rename internally to clarify it's now general
  ambient facts, not just clock-derived).
- **Self-deactivate the nudge** in `init.rs::deflection_rules`:
  ```diff
    "when": [
  -   { "new_content_contains": "git commit -m" }
  +   { "new_content_contains": "git commit -m" },
  +   { "__script__": "facts_count('confidence_enabled', []) == 0" }
    ]
  ```
- **Generalize for the future**: document the marker-fact pattern in
  CLAUDE.md so future packs (`journey_enabled`, etc.) can use the same
  shape.

### From `SPEC-extract-rules-defaults`

Scoped down to the smallest useful slice for 0.14.0:

- **Default extractor action `warn`, not `block`** (`server.rs::extract_rules`).
- **Strip the bracketed prefix** (`[pattern]` / `[anti_pattern]` /
  `[context]` / `[problem]`) before serializing the message.
- **Deferred** to a follow-up PATCH: the per-pattern marker conditions
  (Problem 3b), the static skip-list for structurally-enforced patterns
  (Problem 4a), `phr-mcp migrate-extracted-rules` for existing projects.
  Reason: those are 2–3× the implementation surface and 0.14.0 already
  has plenty.

### From `SPEC-journey-filtered-since`

- **New aggregator** `journey_filtered_since_ge(target, counted, k)` in
  `journey/derive.rs`: scan + selector validation + emit.
- **Tests**: per-aggregator unit tests + extension of the determinism
  contract test in `tests/journey_derive.rs`.
- **Documentation**: SPEC-journey-facts.md "v1 aggregator family" table
  becomes "v1.1 aggregator family" with the new entry; CLAUDE.md
  updated.

## Cross-cutting work

- **Version bump.** `crates/phronesis-mcp/Cargo.toml` → `0.14.0`. `phr`
  stays at `0.13.3` (engine unchanged); `phr-mcp`'s `phr` dep stays
  pinned at `0.13.3`.
- **CHANGELOG entry** with one Added/Changed/Fixed group per sub-spec,
  consolidated migration notes at the end.
- **CLAUDE.md updates**: new aggregator reference, marker-fact pattern,
  broadened gate pattern (mention in the confidence pack description).

## Implementation order (smallest blast radius first)

1. **gate-merge-commits.** Two-line JSON change in `confidence_rules`,
   plus tests. Self-contained.
2. **pack-opt-in-facts.** Add the marker-fact extractor (~5 lines),
   add one `when` clause to `nudge-verify-before-commit`. Two
   isolated changes; no overlap with other specs.
3. **journey-filtered-since.** New aggregator, fully contained in
   `derive.rs`. Mirrors `emit_since_ge`; ~40 lines + tests.
4. **extract-rules-defaults (scoped).** Two changes in `extract_rules`:
   default action and prefix-strip. The deferred work doesn't ride
   this release.
5. **Release work.** Version bump, CHANGELOG, CLAUDE.md updates,
   reinstall, push.

This order minimizes the chance any one change blocks another. The
journey aggregator is the most architecturally novel and gets the
freshest context; the release work goes last so versions only bump
once everything compiles cleanly.

## Test surface

| Sub-spec | New tests | Modified tests |
|---|---|---|
| gate-merge-commits | 6 per-command shapes + 1 negative | 0 |
| pack-opt-in-facts | 3 (marker fact assertion, nudge silent when on, nudge fires when off) | 0 |
| extract-rules-defaults | 2 (default warn, prefix stripped) | n/a — the existing extract_rules tests verify shapes, which are mostly unchanged |
| journey-filtered-since | ~5 (aggregator emission, target-not-found, counted-not-found, target == counted edge case, determinism extension) | 1 (the existing determinism test) |

Total: ~17 new tests, ~1 modified. Coverage stays above the workspace
baseline (currently ~86% lines).

## What 0.14.0 does NOT ship

Honest about what's still on the radar:

- **`phr-mcp migrate-extracted-rules`** — salvage tool for projects
  that already invoked the old `extract_rules`. Punted to a follow-up
  PATCH because the in-tree salvage script in `SPEC-extract-rules-defaults`
  is "good enough" for the project that surfaced it.
- **Per-pattern marker conditions** (`SPEC-extract-rules-defaults`
  Problem 3b) and the **structural-rule skip-list** (Problem 4a).
  Bigger redesign of the extractor; needs its own focused pass.
- **Subject inheritance across merge commits** (the open question in
  `SPEC-gate-merge-commits`). Real design surface; deferred.
- **Repo-lifetime journey windows** (`r`). Still phase 2 of
  SPEC-journey-facts.
- **First-class `not` in the rule schema** (still emulated via
  `facts_count(..., []) == 0`).

## Rollout

1. Implement in the order above, each as its own commit on `main`.
2. Tests pass at each step (`cargo test --workspace`).
3. After all four implementations land, bump `phr-mcp` to `0.14.0`,
   update CHANGELOG, push.
4. `cargo install --path crates/phronesis-mcp --force` updates the
   user-level binary; `phr-mcp --version` reports 0.14.0.
5. Live-test the project's own `.phronesis/rules.json` after the
   release: confirm `nudge-verify-before-commit` goes silent (because
   `confidence.json` is present), `git merge` triggers the gate,
   `build-staleness` (if rewritten to the new aggregator) fires on
   actual write activity rather than Bash sessions.

## References

- `docs/specs/SPEC-gate-merge-commits.md`
- `docs/specs/SPEC-pack-opt-in-facts.md`
- `docs/specs/SPEC-extract-rules-defaults.md`
- `docs/specs/SPEC-journey-filtered-since.md`
- `crates/phronesis-mcp/src/init.rs::{confidence_rules, deflection_rules}`
- `crates/phronesis-mcp/src/clock_facts.rs`
- `crates/phronesis-mcp/src/server.rs::extract_rules`
- `crates/phronesis-mcp/src/journey/derive.rs`
