# Diff-scoped introduced-debt metric — pilot-lite results

**Motivation.** The pilot's whole-tree quality audit ties on 14 of 15 pairs
(466/466 etc.) because it counts the base repo's pre-existing violations;
the agent's contribution is invisible. This metric attributes violations to
the lines the patch actually adds: for each arm's clone, stage the identical
52-rule `llm,rust` pack, run `phr-mcp audit --json`, and count violations
whose (file, line) falls inside the patch's added-line hunks. Computed over
the existing pilot clones and patches — no agent re-runs. Raw data:
`bench/report/data/diff-scoped-debt.json`.

## Results (15 paired rust tasks, k=1, glm-5.3:cloud)

| pair control vs treatment | count |
|---|---|
| both introduced 0 violations | **13 of 15** |
| identical introduced debt (sharkdp__bat-1892: 20 vs 20 — converged solutions) | 1 |
| treatment introduced 2 trivial style-audit hits (ruff-15394, ruff-15543) | 1 |

| rule | control | treatment |
|---|---|---|
| enforce-no-unwrap-in-src | 13 | 13 |
| warn-pub-fn-missing-doc | 7 | 7 |
| audit-rust-let-mut-count-high | 0 | 1 |
| audit-rust-let-binding-count-high | 0 | 1 |
| **total** | **20** | **22** |

## Interpretation

On this sample, the model writes clean rust **either way**: 13 tasks carried
zero introduced violations in both arms, and the only deltas are two trivial
style-audit counts against treatment. The governance's 11 runtime warns never
became blocks (0 blocks) and never visibly changed delivered code. Combined
with the pilot's efficiency finding (+30–35% turns/wall/tokens under
governance), the honest read: **for this model, pack, and task mix, the
measured effect of governance is cost without measurable debt or resolution
benefit.** That is a legitimate negative result for a pilot — and a pointer:
a benefit signal needs either a pack whose block-level rules fire on the
paths these agents actually touch, a model with a higher baseline debt, or
tasks that provoke the rules (the 0-block count says the llm/rust packs'
blocking rules were never exercised).

## Follow-ups recorded

- The quality stage's primary output should be this diff-scoped number
  (whole-tree as secondary context).
- t10 bug: the quality stage's cleanup deletes the treatment clone's OWN
  `.phronesis/rules.json` (it should remove only what it staged) — discovered
  while reproducing this metric; the treatment clones' post-run state was
  mutated by the audit pass.
- 0 blocks across 15 treatment runs: the false-positive review has nothing
  to grade; the 11 warns are the raw material for a friction-quality pass.