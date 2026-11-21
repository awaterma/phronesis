# Spec lifecycle-status header template

Copy this blockquote directly under a spec's `# Title` line. Allowed statuses
match `docs/contributing/documentation.md`: `shipped`, `partially
implemented`, `needs evidence` (proposed), and companion records of a shipped
parent. Evidence means code and test inspection, named with clickable
relative paths; first-release claims come only from Git tags (`git tag
--contains` on the introducing commit) — a `CHANGELOG.md` topic match is not
implementation evidence. Never invent a status; when verification is
incomplete, use `partially implemented` or `needs evidence` and say exactly
what is known. The machine-readable `status:` YAML front matter uses the
website vocabulary (`shipped` / `partial` / `proposed`) and must agree with
the blockquote. An original draft label in the body (for example "Draft —
not approved") is a historical record; add a historical note rather than
changing it.

Shipped:

> **Lifecycle status: shipped** — reconciled against code and tests on
> <date>. Implementation: [`<module>.rs`](../../crates/<crate>/src/<module>.rs);
> tests: [`<test>.rs`](../../crates/<crate>/tests/<test>.rs). First release
> containing the primary module: `vX.Y.Z` (tag-derived from the
> module-introducing commit).

Partially implemented:

> **Lifecycle status: partially implemented** — reconciled against code on
> <date>. Shipped part: <what exists, with evidence path(s)>; gaps: <what was
> not found>. (Original draft label, if any, noted as historical.)

Needs evidence (proposed):

> **Lifecycle status: needs evidence (proposed)** — no matching implementation
> was identified in <searched paths> during the <date> reconciliation pass.
> The body's original label is retained below as history.

Companion record:

> **Lifecycle status: shipped (companion record)** — <appendix/inventory/
> decision record> for [`SPEC-<topic>.md`](SPEC-<topic>.md); the parent's
> status was re-verified against code on <date> (see that header).

When the status changes, update in the same change: this header, the YAML
`status:`, and the row in `docs/specs/README.md`.
