---
title: Private ADR repositories
section: guides
---

# Private ADR repositories

Keep personal design decisions in a separate local checkout while working on a
shared code repository. Phronesis can create and discover ADRs there without
copying them into the shared project. Select a project-specific directory in
that checkout so unrelated projects do not share an ADR namespace.

## Configure the destination

Create the destination directory yourself, then run this from the code project:

```sh
phr-mcp decision configure --directory /absolute/path/to/private-specs/example-app/decisions
phr-mcp decision new storage-choice
phr-mcp decision list --json
phr-mcp wiki-drift --json
```

The destination must already exist, be absolute, and resolve outside the code
project. Phronesis stores the mapping in `.phronesis/decision-store.json`; it
refuses configuration unless that file is untracked and Git ignores it. If the
project does not already ignore `.phronesis/`, add
`/.phronesis/decision-store.json` to its **local Git exclude file** (locate it
with `git rev-parse --git-path info/exclude`). This does not require changing a
team-shared `.gitignore`. Do not put the mapping in tracked presets.

Repeated configuration of the same destination is idempotent. To change it,
explicitly edit the ignored local file, whose only field is `directory`. Remove
that local mapping to restore project defaults; this never removes ADRs from
the external checkout. Invalid configuration or an unavailable destination is
an error: Phronesis does not silently write a private decision into shared docs.
The mapping is local to this worktree and is not inherited by fresh clones.

## Select a different directory for one command

`--decisions-dir` overrides the mapping for creation and listing; `--wiki-dir`
does the same for `wiki-drift`. Relative `--decisions-dir` paths resolve from
the project root (`--path`). The existing `--wiki-dir` option retains its
current-working-directory semantics, even when a different project root is
passed positionally to `wiki-drift`.

```sh
phr-mcp decision new shared-choice --decisions-dir docs/decisions
phr-mcp decision list --decisions-dir docs/decisions --json
phr-mcp wiki-drift --wiki-dir docs/decisions --json
```

An explicit override scans exactly that directory. Without a mapping or
an override, creation and discovery retain the canonical/legacy behavior:
creation prefers an existing `docs/decisions/`, otherwise
`.phronesis/wiki/decisions/`; discovery reads both, with canonical precedence.
ADRs keep their existing schema and stable IDs. Duplicate IDs within a selected
directory are errors, and creation refuses to overwrite an existing file.

## Privacy and publication boundaries

The local mapping applies only to the explicit `decision` and `wiki-drift` CLI
commands. Automatic graph extraction, hook provenance, consolidated `drift`,
and MCP drift continue to use project decisions. Private decisions therefore
do not become team policy or enter automatic context through this mapping.
To inspect a private directory through another interface, its existing explicit
directory override must be selected deliberately. Listing and drift output can
contain private IDs, paths, and text: keep that output out of shared logs,
issues, PR descriptions, generated documentation, and unauthorized model calls.

These commands do not stage, commit, push, fetch, change remotes, or publish a
site. They save files locally. Push only when explicitly requested for the
external repository, after checking its actual remote and branch and reviewing
the intended files. Authorization to push the code repository does not authorize
pushing the private spec repository. Do not mirror private decisions, add
public links to them, or create submodules/symlinks in the shared project.

Configuration validates local Git exclusion; it is not an access-control system
for other processes or people with filesystem access. Keep normal filesystem
and repository access controls on the private checkout. No private repository
is required in CI: tests use isolated temporary repositories.
