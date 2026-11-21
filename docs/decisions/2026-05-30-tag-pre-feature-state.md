---
id: tag-pre-feature-state
date: 2026-05-30
status: accepted
enforces: []
superseded_by: null
tags: [process, git, release]
---

# Tag the pre-feature release before starting a feature branch

## Context

When a feature branch grows long (multi-session, multi-commit), it
gets hard to refer to "the state before this feature started." Going
by SHA is awkward, and `main~N` shifts as `main` accepts other work.

The problem surfaced sharply during the v0.9.0 wiki-drift branch:
the implementation took multiple sessions, and being able to compare
against an explicit pre-feature anchor would have been useful for
audits, for rollback rehearsals, and for the eventual changelog
entry.

## Decision

Before starting any feature-branch implementation:

1. Make sure `main` is at the current release version
2. Tag it: `git tag v<current>` (e.g. `git tag v0.8.1`)
3. Push the tag if pushing the release was already authorized
4. Then create the feature branch and start work

The tag is a named anchor that doesn't move. Any subsequent
comparison ("what changed since the feature started?") is
`git diff v0.8.1..HEAD`, stable across sessions and across
context-window compactions.

## Enforcement

Procedural. A future `warn_no_release_tag_on_main_before_feature`
hook could check that `git tag --points-at main` includes a
semver-shaped tag before allowing a `git checkout -b feat/*` to
proceed, but that's speculative.

## Consequences

- Encourages crisp version-bump discipline at release time (the tag
  has to actually exist).
- Pairs well with [[commit-timing-rule]] for long branches that
  cross weekday boundaries: the tag survives session boundaries
  cleanly.
- Costs one extra step at branch creation, which is cheap.
