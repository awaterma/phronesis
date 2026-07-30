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

**2026-07-30 — a third mechanism, and the fix.** v0.22.1 repeated the
partial publish (`phronesis-mcp` again). Recovery required a hand-authored
Release PR, because `release-plz-pr` refuses to compute a next version
while a tagged version is missing from the registry. That PR merged the
bump to `main` as a merge commit — and the release job skipped it:
`skipping release: current commit is not from a release PR`, exiting
green having published nothing. `release_always = false` means "only from
a Release PR **release-plz itself authored**", not "only when a Release PR
merges".

That is three distinct mechanisms across four releases, all sharing one
property: **the job reported success and nothing checked the registry.**
The prescription below ("verify releases against crates.io, not job
status") was correct and was not followed, because it depended on a human
remembering it. It is now enforced by `scripts/verify-crates-io.py`, run
from the release workflow after the release job, failing the build when
the workspace version is not on crates.io. `release_always` is now `true`,
which decouples publishing from PR authorship.

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
  *2026-07-30: automated as `scripts/verify-crates-io.py`, wired into the
  release workflow. This is now a build failure rather than a habit.*
- **`release_always = true`**: publishing depends on the workspace version
  being absent from crates.io, not on who authored the Release PR. A
  hand-authored Release PR is sometimes the only way forward, and it must
  still release.

## Enforcement

- Registry verification is enforced by `scripts/verify-crates-io.py` in the
  `verify-crates-io` job of `.github/workflows/release-plz.yml`. It runs on
  every push to `main`: an ordinary push has the workspace version already on
  crates.io and passes, while a release that silently published nothing
  fails the build.
- Merge-method choice remains a process step no rule can express.
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
