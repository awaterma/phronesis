---
id: card-game-vocabulary
date: 2026-05-30
status: accepted
enforces: []
superseded_by: null
tags: [naming, vocabulary, prose]
---

# Card-game terminology, not RPG

## Context

An ancestor of this codebase (rulgamr / dnd) used RPG vocabulary
throughout: party, player, dungeon master, dungeon, encounter. That
vocabulary leaked into early phronesis prose and identifier names
even though the rules-engine project has no RPG content.

The mismatch confuses readers — "player" in a RETE rules engine is
nonsensical — and it carries a content brand the project doesn't
want to keep.

## Decision

Use card-game vocabulary throughout the project's prose,
documentation, and (where reasonable) identifier names:

- *hand* — what a participant holds
- *card* — an individual element
- *member* — a participant
- *deck* / *draw* — collections and operations on them

The RPG vocabulary (party, player, DM, dungeon, encounter) is
removed from any documentation Claude writes. Existing identifier
names in legacy code are left alone unless edited for another reason.

## Enforcement

No automated rule. The decision shapes prose Claude writes and
naming choices in new code. A future `warn_rpg_vocabulary_in_prose`
rule scoped to `docs/**` and `*.md` would be possible but probably
too noisy to be worth it.

## Consequences

- Any documentation rewrite is an opportunity to scrub RPG terms.
- Legacy code in `crates/phronesis/src/*` may still contain RPG-era
  identifiers; not worth renaming until those files are edited for
  another reason.
