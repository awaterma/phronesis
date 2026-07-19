---
id: release-tag-masks-partial-publish
date: 2026-07-18
status: accepted
enforces: []
superseded_by: null
tags: [release, ci]
---

# release-tag-masks-partial-publish

## Context

The first automated release (v0.20.1, 2026-07-18) partially failed and
the documented recovery ("re-run the job") silently did nothing. Two
distinct mechanisms interacted:

1. **Squash-merge race.** The Release PR was squash-merged, so GitHub
   minted a new commit on main and the PR's own head commit was never
   pushed anywhere reachable. `release-plz release` computed the release
   from the PR head sha and failed mid-run:
   `failed to create ref refs/tags/v0.20.1 with sha <pr-head> — the
   commit likely hasn't been pushed to the remote repository yet`.
   By that point it had already published `phronesis` and
   `phronesis-rhai` — and (on a partial retry path) the `v0.20.1` tag
   was created at the correct main commit. `phronesis-mcp` was never
   published.

**2026-07-19 recurrence — merge-commit strategy did NOT prevent it.**
The v0.21.0 Release PR (#15) was merged with a true merge commit as
this decision prescribes (`66e8274`, second parent = the PR head
`bc52100`), yet the release run failed identically: after publishing
`phronesis` and `phronesis-rhai`, it tried to create
`refs/tags/v0.21.0` at sha `537eb90` — a commit that exists nowhere
on GitHub (422 "No commit found"), i.e. a commit release-plz
fabricated locally in the runner — while the tag already existed at
the correct release commit `bc52100`. `phronesis-mcp` was again left
unpublished. The mechanism-1 diagnosis above (squash-merge race) is
therefore incomplete: the unreachable-sha failure is internal to
release-plz's release computation and occurs regardless of merge
strategy. The tag-delete + re-run recovery below worked a second
time, verified against crates.io 2026-07-19.

2. **Tag masks the registry check.** On re-run, release-plz logged
   `phronesis-mcp 0.20.1: Already published — Tag v0.20.1 already
   exists` for every crate. With `version_group = "workspace"` all
   three crates share one tag, so the *existence of the tag* was taken
   as proof of publication for the whole group. The re-run exited
   green while the registry stayed inconsistent (two crates at 0.20.1,
   one at 0.20.0). A green release job therefore does NOT imply all
   crates are published.

## Decision

- **Merge Release PRs with a merge commit, not squash.** release-plz's
  own docs: with the default merge strategy it checks out the PR's
  final commit and avoids the race entirely; with squash-merge the
  checkout is skipped and it releases whatever main happens to be.
  Ordinary PRs keep using squash (their titles feed conventional-commit
  parsing); the `chore: release vX.Y.Z` title of a Release PR merge
  commit parses the same either way.
  *2026-07-19: shown insufficient — v0.21.0 was merge-committed and
  still failed the same way (see Context). Keep the merge-commit rule
  (it removes one known race) but treat the recovery procedure and
  registry verification below as the operative safeguards, expected to
  be needed on every group release until the root cause is fixed
  upstream or per-crate tags are adopted.*
- **Recovery for a partially published version group**: delete the
  version tag (`git push origin :refs/tags/vX.Y.Z`), then re-run the
  release job. Without the tag, release-plz falls back to per-crate
  registry checks, skips crates already on crates.io, publishes the
  remainder via OIDC, and recreates the tag. Verified 2026-07-18.
- **Verify releases against crates.io, not job status**: after any
  release run, check all three crates' latest version on crates.io.

## Enforcement

- No automated rule — merge-method choice and registry verification are
  process steps GitHub/CI can't express as phronesis predicates.
  Documented in `docs/RELEASING.md` (flow step and Troubleshooting).

## Consequences

- Release PR merges look different from feature PR merges in history
  (merge commit vs squash). Acceptable: the release commit itself is
  authored by release-plz with a conventional title.
- If a squash-merge happens anyway, the failure is recoverable with the
  tag-delete + re-run procedure; worst case is a temporarily
  inconsistent registry, never a wrong publish.
- Independent per-crate versioning (dropping `version_group =
  "workspace"`, giving each crate its own `vX.Y.Z-<crate>` style tag)
  would also dissolve the masking problem, at the cost of losing the
  single workspace version. Deferred — revisit if group releases keep
  causing friction. *2026-07-19: friction has now recurred on both
  automated releases (v0.20.1 and v0.21.0); revisiting this, or filing
  the unreachable-sha bug upstream against release-plz, is warranted.*
