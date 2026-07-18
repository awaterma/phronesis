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
5. **Merging the Release PR** triggers the release job
   (`release_always = false`, so ordinary pushes to `main` never
   publish). CI then:
   - publishes `phronesis`, `phronesis-rhai`, `phronesis-mcp` to
     crates.io in dependency order via trusted publishing (OIDC — no
     `CARGO_REGISTRY_TOKEN`),
   - tags `vX.Y.Z`,
   - creates the GitHub release.
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
  v0.20.1, caused by squash-merging the Release PR): a plain re-run
  does NOT fix this. If the failed run already created the `vX.Y.Z`
  tag, the re-run logs `Already published — Tag vX.Y.Z already exists`
  for every crate and exits green, because with
  `version_group = "workspace"` the shared tag is taken as proof the
  whole group is published — the registry is never consulted. Recovery:

  1. Delete the version tag: `git push origin :refs/tags/vX.Y.Z`
  2. Re-run the release job. Without the tag, release-plz checks
     crates.io per crate, skips the ones already published, publishes
     the remainder via OIDC, and recreates the tag on main.
  3. Confirm all three crates show the new version on crates.io.

  Full post-mortem:
  `.phronesis/wiki/decisions/2026-07-18-release-tag-masks-partial-publish.md`.
