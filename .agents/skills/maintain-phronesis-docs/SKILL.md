---
name: maintain-phronesis-docs
description: Use when maintaining Phronesis documentation, changing documented public behavior, preparing releases, updating specs or ADRs, or checking documentation drift. Guides routing of content into the canonical docs/ hierarchy, lifecycle-status honesty, historical preservation, and the shared documentation checks. Supports explicit invocation with $maintain-phronesis-docs.
---

# Maintain Phronesis docs

Editorial judgment for this repository's documentation. CI enforces structural
requirements; this skill decides *what* to write and *where*. The canonical
policy is [`docs/contributing/documentation.md`](../../../docs/contributing/documentation.md)
— read it; do not restate it here.

## Activation

Invoke when maintaining documentation, changing documented public behavior,
preparing releases, updating specs or ADRs, or checking documentation drift.
Normal automatic discovery stays enabled; explicit invocation works with
`$maintain-phronesis-docs`.

## Workflow

1. **Scope.** Inspect the relevant changes (diff, release notes, new code) and
   list the canonical documents they affect. Inventory tracked documents with
   `git ls-files` only; never read, move, or publish untracked private drafts.
2. **Route.** Assign content to one of the eight purpose directories
   (`tutorials`, `guides`, `reference`, `architecture`, `decisions`, `specs`,
   `contributing`, `archive` — see `docs/README.md`). Update the existing
   authoritative page instead of creating a near-duplicate. Crate READMEs,
   `CLAUDE.md`, fixtures, and executable examples stay beside their code.
3. **Reconcile.** Check claims against code and tests (module and test paths,
   named in the spec status header). Distinguish shipped, partially
   implemented, proposed (`needs evidence`), and historical behavior. First
   release claims come only from Git tags (`git tag --contains` on the
   introducing commit); a `CHANGELOG.md` topic match is not implementation
   evidence. Never invent a lifecycle status; keep `docs/specs/README.md` in
   agreement. Original draft labels are historical records, not current status.
4. **Wire navigation.** When a document moves or changes status, update
   indexes, cross-links, lifecycle status, and `docs/migration-map.json`
   (`moves[]` with `from`, `to`, `note`) in the same change. Never reuse an
   old path for new content. Paths pinned by shipped code strings move only
   with a forwarding stub retained at the original path; retiring a stub
   requires the code change that owns the new path.
5. **Preserve ADR identity.** Decisions keep stable IDs, dates, and status
   vocabulary (`docs/decisions/`). When touching decision discovery or ADR
   locations, keep both-location compatibility (canonical `docs/decisions/`
   wins; legacy `.phronesis/wiki/decisions/` still read) and enforcement links
   intact.
6. **Check and report.** Run the repository's shared documentation check
   command (the same one CI runs). Report what passed, what remains uncertain,
   and any deferred documentation work. Never authorize commits, deployment,
   or merges from this skill.

## Boundaries

- Link to the contributor documentation policy rather than duplicating it.
- Use the same validation commands as CI; never maintain a second checker.
- Never publish private drafts, copy runtime state (`.phronesis/` local data,
  logs) into Git, or rewrite historical records. Archiving means moving intact
  with a historical notice, plus a replacement link when one exists.
- Keep changes proportional to the request; do not reorganize the repository
  during routine edits. Escalate structural reorganizations as a proposal.

## Templates

- [`templates/spec-status-header.md`](templates/spec-status-header.md) — the
  lifecycle-status blockquote for spec pages; copy, fill the evidence, keep
  the vocabulary.

## Validation

Structural checks (links, anchors, metadata, navigation, migration-map
consistency) come from the shared check command owned by the website/CI lane;
run it locally before handing off. Skill-level behavior is exercised through
representative fixtures: documenting a public API change, reconciling a
partially implemented spec, moving an ADR, archiving a review, and meeting an
untracked private draft — check resulting behavior, not exact wording.
