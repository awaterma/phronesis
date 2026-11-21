# Contributing

Development, testing, release, and documentation-maintenance policies.

- [Local developer state](local-setup.md) — provision reviewed presets, preserve customized files, and understand canonical/legacy ADR compatibility.
- [Releasing](RELEASING.md) — release-plz flow, conventional commits, versioning, and the manual steps around each release.
- [Website build and checks](website.md) — shared rendering, locked previews, URL compatibility, and manual deployment.
- [Documentation policy](documentation.md) — where documents live, lifecycle statuses, historical notices, and how to keep this hierarchy healthy.
- Agent workflow and code standards: [`AGENTS.md`](../../AGENTS.md) (build/test/lint commands, rule packs, review checklist).
- Patterns guide for Rust changes: [`crates/phronesis-mcp/docs/RUST-PATTERNS-GUIDE.md`](../../crates/phronesis-mcp/docs/RUST-PATTERNS-GUIDE.md).

Structural requirements (internal links, indexes, metadata) are enforced by the shared documentation checks run in CI; editorial judgment is guided by the `maintain-phronesis-docs` skill at [`.agents/skills/maintain-phronesis-docs/SKILL.md`](../../.agents/skills/maintain-phronesis-docs/SKILL.md).
