# `.phronesis/wiki/decisions/`

ADR-style decision pages. Each file is one decision (e.g. `2026-05-29-card-game-terminology.md`). The first block is YAML frontmatter (`id`, `date`, `status`, optional `enforces`, `superseded_by`, `tags`). The body uses Context / Decision / Enforcement / Consequences sections.

Run `phr-mcp wiki-drift` to see which decisions lack rule coverage.
Create new pages with `phr-mcp decision new <slug>`.

This directory is tracked in git (un-ignored from the broader `.phronesis/` ignore) because decisions are project knowledge. The rest of `.phronesis/` (rules.json, log.jsonl, etc.) stays gitignored.
