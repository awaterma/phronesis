# SPEC: pack opt-in markers as facts (pack-level supersession)

**Status:** draft
**Authors:** Claude, Andrew Waterman
**Date:** 2026-06-20
**Target release:** phronesis-mcp 0.13.x (PATCH — small fact-extraction
              addition + a rule clause change in the `llm` pack)
**Affects:** `crates/phronesis-mcp/src/hook_facts.rs` (or a peer that
              extracts pack-marker facts), `crates/phronesis-mcp/src/init.rs::deflection_rules`
              (one `when` clause added), tests.

## Premise

When a project opts into the `confidence` pack, the per-commit
`nudge-verify-before-commit` warning (from the `llm` pack) duplicates the
band-gated commit check operationally — both fire on `bash_command_matches:
"git commit"`, both ask the model to verify before claiming done. With
both packs active the user sees two warning lines on every commit, one
nudging the same call-chain tracing the gate's `signal_pass` count
already enforces.

We don't want to delete the nudge from the `llm` pack — projects that
**don't** opt into confidence scoring still benefit from it. We want
**conditional firing**: when confidence is on, the nudge stays silent;
when confidence is off, it fires as before.

This SPEC introduces a minimal mechanism for pack-level supersession that
uses only existing engine surface.

## The mechanism: opt-in markers as zero-arg facts

`crates/phronesis-mcp/src/clock_facts.rs` already asserts zero-arg facts
like `business_hours_local` at every hook invocation, computed from
real-world state at the moment of firing. The shape is exactly what we
need: at hook fire time, the hook inspects on-disk state and asserts
boolean facts.

Mirror that pattern: assert a `confidence_enabled` fact at every hook
invocation **iff** `.phronesis/confidence.json` exists. Add it alongside
the existing clock facts (or in a peer module — see §Module choice).

The nudge rule then self-deactivates via the existing `__script__` DSL's
`facts_count(..., []) == 0` absence form:

```json
{
  "id": "nudge-verify-before-commit",
  "phase": "pre",
  "when": [
    { "bash_command_matches": "git commit" },
    { "__script__": "facts_count('confidence_enabled', []) == 0" }
  ],
  "then": {
    "warn": "About to commit. Trace the call chain end-to-end before reporting done. Half-fixes where one layer is wired but another is not are a recurring failure mode."
  }
}
```

- Confidence opted in → fact asserted → count is 1 → second clause false → rule does not fire.
- Confidence not opted in → fact absent → count is 0 → second clause true → rule fires as before.

The pure-script-rules engine fix from 0.13.0 isn't needed here — the rule
still has a leaf condition (`bash_command_matches`), so it reaches the
agenda via the normal alpha path. The `__script__` clause filters
activations as it does for other mixed-form rules.

## Why this is better than the alternatives

We considered three options before settling on this one:

| Option | Cost | UX |
|---|---|---|
| Init-time composition (filter nudge out at scaffold) | ~5 lines in init.rs | Static at scaffold time; broken when `--packs confidence` added later to an existing project |
| New `config_file_present` predicate in the engine | engine work | Cleanest UX but largest blast radius |
| **Opt-in markers as zero-arg facts** | ~5 lines fact extraction + 1 rule clause | Runtime conditional; reuses existing engine surface; generalizes to other pack pairs |

Option 1 fails on the live composability gap (adding `--packs confidence`
to an existing project doesn't reflow `rules.json`; the dedup never
happens). Option 2 is over-engineered for what zero-arg facts already
express. Option 3 — this SPEC — uses only existing surface and
generalizes: any future pack-pair supersession can use the same shape.

## Other markers worth asserting

Once the pattern lands, other opt-in files become marker facts the same
way:

| Fact | Asserted when |
|---|---|
| `confidence_enabled` | `.phronesis/confidence.json` exists |
| `journey_enabled` | `.phronesis/journey.json` exists |
| `wiki_present` | `.phronesis/wiki/decisions/` exists and is non-empty |
| `bugs_registry_present` | `.phronesis/bugs.json` exists |

v1 of this SPEC ships **only `confidence_enabled`** — the other markers
land when a concrete supersession use-case appears. The general shape is
the contribution; the specific marker set grows by demand.

## Module choice

Two natural homes for the marker-extraction fn:

1. **Add to `clock_facts.rs`.** Rename to `ambient_facts.rs` (or
   similar) since clock facts are one kind of ambient state and pack
   markers are another. Smallest change.
2. **New `pack_marker_facts.rs` peer module.** Cleaner separation;
   pack markers grow over time and may want their own tests.

**Recommendation: option 1 with a rename.** Clock facts are ambient by
the same pattern (computed at fire time from real-world state); pack
markers are a strict generalization. Rename, add the marker assertion
inside, done. Tests live alongside the existing clock-fact tests.

The `crate::outcomes::enabled` predicate already exists and returns
`true` iff `.phronesis/confidence.json` is present — that's the
function the extractor calls. No new file-existence check needed.

## Tests

1. **Fact assertion.** In whatever module owns the extractor, a unit
   test: `confidence_enabled` fact present iff `.phronesis/confidence.json`
   exists.
2. **Nudge silence when confidence is on.** Integration test mirroring
   the existing `confidence_gate_integration.rs` pattern: set up a
   project with both `llm` and `confidence` packs, run pre-check on a
   `git commit` payload, assert the action log does NOT contain a
   `nudge-verify-before-commit` consequence.
3. **Nudge fires when confidence is off.** Same setup minus the
   `confidence.json` file; assert the nudge fires.
4. **Backward-compat.** Existing `confidence_gate_integration.rs` tests
   pass unchanged (their seeders don't depend on the nudge firing or
   not firing).

## Rollout

1. Implement the extractor (~5 lines in `clock_facts.rs` or its
   renamed successor).
2. Update the `nudge-verify-before-commit` rule in
   `init.rs::deflection_rules` to include the absence clause.
3. **Migration story for existing projects:** the new rule shape lands
   in `init.rs`, but projects with an existing `rules.json` still carry
   the old shape. Two paths, matching the gate-merge-commits migration
   story:
   - `phr-mcp init --rules-only --force --packs llm,...` — rewrites
     `rules.json` with the updated rule (backs up to `.bak`).
   - Hand-edit the existing rule to add the second `when` clause.
4. Document in CHANGELOG and CLAUDE.md.
5. PATCH bump (e.g. 0.13.2 → 0.13.3).

## Out of scope

- **Pack composability for additions.** Adding `--packs <new>` to an
  existing project should reflow `rules.json` to include the new pack's
  rules. Tracked separately. The supersession mechanism in this SPEC
  works once the rule is in `rules.json`; how it gets there is a
  separate concern.
- **Marker facts for non-file state.** The pattern as proposed reads
  filesystem state. Process-environment markers (`PHRONESIS_*` env
  vars) would be a small extension; deferred until a use-case appears.
- **A pack-level "supersedes" relation in the rule schema.** A future
  schema could express `supersedes: [rule-id]` as a structural relation
  rather than a runtime fact dance. The fact approach is the smaller
  v1; the schema relation is a v2 question.

## Open questions

- **Naming.** `confidence_enabled` vs. `confidence_pack_active` vs.
  `confidence_opt_in`. Pick the shortest readable form and stay
  consistent across the marker family.
- **Should journey-fact rules also self-deactivate when journey is
  off?** The `build-staleness` rule references `journey_since_ge` —
  with no journey events, the fact set is empty and the rule
  trivially doesn't fire. So the supersession isn't strictly needed
  for journey rules. Leave as-is unless a concrete noise problem
  surfaces.
- **Audit interaction.** Audit-only rules don't reach the hook agenda,
  so they aren't affected. But if a project author writes a
  pre-check rule whose suppression they want at audit time too, the
  marker fact pattern doesn't help (audit uses a separate path).
  Flag for future thought; not a v1 blocker.

## Why this isn't a 0.14.0

Pre-1.0 semver: MINOR is "new feature surface" (subcommand, pack, hook
stage). This change is "the existing `llm` pack's `nudge-verify-before-commit`
rule now conditions on confidence opt-in." Rule-pack tweak shape.
PATCH.
