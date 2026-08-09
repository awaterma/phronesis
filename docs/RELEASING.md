# Releasing

Releases are automated by [release-plz](https://release-plz.dev)
(`.github/workflows/release-plz.yml` + `release-plz.toml`). This is the
operator guide.

## How the flow works

1. **Conventional commits land on `main`** (squash merges: the PR title
   becomes the commit message release-plz parses, so PR titles must be
   conventional-commit shaped — `feat:`, `fix:`, `chore:`, ...).
2. **release-plz opens/updates a Release PR** proposing the next version
   (pre-1.0 house rule: any user-visible feature bumps MINOR —
   `features_always_increment_minor = true`). All three crates share one
   version (`version_group = "workspace"`).
3. **A human reviews the Release PR** — this is the approval gate:
   - Hand-write the `CHANGELOG.md` entry (release-plz does not touch the
     changelog; `changelog_update = false`).
   - If pack rules changed, run `phr-mcp catalogue` and commit the
     regenerated `docs/catalogue.html` before merging.
4. **Merge the Release PR with a MERGE COMMIT, not squash.** With the
   default merge strategy release-plz checks out the PR's final commit
   before releasing; a squash merge creates a new commit it can't find,
   which triggers a mid-release failure (`failed to create ref ... the
   commit likely hasn't been pushed`) that can leave the version group
   partially published (see Troubleshooting). Ordinary PRs keep using
   squash — this exception is for Release PRs only.
   NOTE (2026-07-19): a merge commit does NOT reliably prevent this —
   v0.21.0 was merge-committed and still failed mid-release the same
   way (release-plz tried to tag a sha that exists nowhere on GitHub).
   UPDATE (2026-08-03): the mid-release failure itself is not fixed, but
   it should no longer *conceal* a partial publish — `git_tag_name` is
   now per-package, so a later crate is no longer skipped on the
   strength of an earlier crate's tag. Verify against crates.io anyway
   (step 6); that is the check that does not depend on this reasoning
   being right.
5. **Merging the Release PR** triggers the release job
   (`release_always = true`; ordinary pushes to `main` are harmless because
   an unchanged workspace version is already present in the registry). CI then:
   - publishes `phronesis`, `phronesis-rhai`, `phronesis-mcp` to
     crates.io in dependency order via trusted publishing (OIDC — no
     `CARGO_REGISTRY_TOKEN`),
   - tags `<package>-vX.Y.Z` (one tag per crate; `vX.Y.Z` up to and
     including v0.24.0),
   - creates the GitHub release,
   - builds and attaches `phr-mcp` archives for Linux x86-64, macOS Apple
     Silicon, and Windows x86-64 after release-plz completes. The
     reusable binary workflow verifies that the expected `phronesis-mcp` tag
     points at the release commit, then waits for its GitHub release before
     uploading, so ordinary pushes are no-ops and versions cannot diverge.
6. **Verify against crates.io**, not the job status: check all three
   crates show the new version. A green release job does NOT guarantee
   a complete publish (see "Partial publish" below).
7. **Locally**, after each release: `cargo install --path
   crates/phronesis-mcp` so the binary your hooks invoke matches.

## One-time setup

Done by a human, once:

1. **crates.io trusted publishing** — for EACH of `phronesis`,
   `phronesis-rhai`, `phronesis-mcp`: crate page → Settings → Trusted
   Publishing → Add GitHub:
   - owner: `awaterma`
   - repo: `phronesis`
   - workflow filename: `release-plz.yml`
   - environment: leave EMPTY
2. **GitHub PAT** — create a fine-grained PAT scoped to
   `awaterma/phronesis` with **Contents: read/write** and **Pull
   requests: read/write**; save it as repo secret `RELEASE_PLZ_TOKEN`.
   (Without it, the Release PR opened with the default `GITHUB_TOKEN`
   would not trigger CI checks.)
3. **Workflow permissions** — GitHub repo Settings → Actions → General →
   Workflow permissions: enable "Allow GitHub Actions to create and
   approve pull requests".
4. **Anchor tag** — tag the current release on `main` before first use
   (tagging lapsed after `v0.18.0`; this keeps changelog/commit ranges
   clean):

   ```bash
   git tag v0.20.0 fb604a9 && git push origin v0.20.0
   ```

## Troubleshooting

- **Phantom release PR** (a Release PR appears with no real changes):
  two known causes here.
  - *Lockfile drift* (verified in this repo): `Cargo.lock` is not
    committed, but published binary crates embed one. When any
    transitive dependency publishes a new version, packaging
    `phronesis-mcp` locally produces a `Cargo.lock` that differs from
    the one inside the published tarball, so release-plz sees the crate
    as "changed" and proposes a patch bump for the whole version group.
    Fix: commit `Cargo.lock` (recommended for a repo that ships a
    binary), or accept the occasional dependency-only patch release.
  - release-plz [issue #1181](https://github.com/release-plz/release-plz/issues/1181)
    — workspace-dependency loop can make it think internal crates
    changed. Close the PR; if it recurs, check path-dependency version
    pins.
- **403 on publish**: the crates.io trusted-publishing registration does
  not match the workflow — verify owner `awaterma`, repo `phronesis`,
  and workflow filename exactly `release-plz.yml` (a renamed workflow
  file breaks the OIDC claim match), environment empty.
- **Partial publish** (some crates published, then a failure — seen on
  v0.20.1 after a squash merge AND on v0.21.0 after a merge commit, so
  treat it as expected on any group release): a plain re-run
  does NOT fix this. If the failed run already created the `vX.Y.Z`
  tag, the re-run logs `Already published — Tag vX.Y.Z already exists`
  for every crate and exits green, because with
  `version_group = "workspace"` the shared tag is taken as proof the
  whole group is published — the registry is never consulted.

  As of 2026-08-03 `git_tag_name` is per-package, which should stop the
  masking at its source: each crate's tag now speaks only for that
  crate. Treat the recovery below as the fallback if a partial publish
  recurs, not as the expected routine it used to be.

  Recovery (v0.24.0 and earlier, or any future recurrence):

  1. Delete the tag for the crate that did not publish —
     `git push origin :refs/tags/<package>-vX.Y.Z`, or
     `:refs/tags/vX.Y.Z` for v0.24.0 and earlier.
  2. Re-run the release job **on the run whose commit is the release
     commit**, not the newest run — the tag is recreated at whatever
     sha that run checked out. Without the tag, release-plz checks
     crates.io per crate, skips the ones already published, publishes
     the remainder via OIDC, and recreates the tag.
  3. Confirm all three crates show the new version on crates.io. The
     log wording distinguishes the two paths: `already published` is a
     registry check; `Already published - Tag vX.Y.Z already exists` is
     the tag shortcut that hides the bug.

  Worked example — v0.24.0, recovered 2026-08-03. `phronesis-mcp` was
  the missing crate; deleting `v0.24.0` and re-running the original
  release job (not the newest) published it and recreated the tag at
  the correct commit. A local `cargo publish` is NOT a substitute:
  crates.io rejects it with `403 ... can only be published using
  Trusted Publishing`, so recovery must go through CI.

  Full post-mortem:
  `.phronesis/wiki/decisions/2026-07-18-release-tag-masks-partial-publish.md`.
