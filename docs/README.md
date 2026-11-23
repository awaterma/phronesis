# Phronesis documentation

One canonical hierarchy. Every active topic lives in exactly one place; start here.

| Section | What it holds | Index |
|---|---|---|
| [Tutorials](tutorials/) | First successful setup and end-to-end examples | [tutorials/README.md](tutorials/README.md) |
| [Guides](guides/) | Task-oriented instructions | [guides/README.md](guides/README.md) |
| [Reference](reference/) | CLI, MCP, configuration, predicates, language support | [reference/README.md](reference/README.md) |
| [Architecture](architecture/) | Explanations of implemented behavior | [architecture/README.md](architecture/README.md) |
| [Decisions](decisions/) | ADRs with stable IDs and enforcement links | [decisions/README.md](decisions/README.md) |
| [Specs](specs/) | Proposed or evolving designs with explicit status | [specs/README.md](specs/README.md) |
| [Contributing](contributing/) | Development, testing, releases, documentation maintenance | [contributing/README.md](contributing/README.md) |
| [Archive](archive/) | Historical plans, reviews, reports, superseded designs | [archive/README.md](archive/README.md) |

Organized by what you are doing:

- **Using Phronesis** on your own repository — start with the [first governed session tutorial](tutorials/first-governed-session.md), then the [README quick start](../README.md#quick-start), the [tutorials](tutorials/README.md), and [Phronesis for Coding Agents](guides/FOR_AGENTS.md).
- **Embedding** the engine — [phronesis-governance](../crates/phronesis-governance/README.md) and the [workspace overview](../README.md#the-workspace).
- **Extending** — [language packs](reference/LANGUAGE-PACKS.md), [extensible predicates](specs/SPEC-extensible-predicates.md), [participatory governance](architecture/participatory-governance.md).
- **Contributing** — [`AGENTS.md`](../AGENTS.md) for agent guidance, [contributing/](contributing/README.md) for release and documentation policies.

Point-in-time records (plans, reviews, validation reports) are preserved intact under [archive/](archive/README.md) with historical notices; they are not maintained. Documentation changes follow the policy in [contributing/documentation.md](contributing/documentation.md); agents maintaining docs should invoke the `$maintain-phronesis-docs` skill at [`.agents/skills/maintain-phronesis-docs/SKILL.md`](../.agents/skills/maintain-phronesis-docs/SKILL.md).

The rendered website (homepage, explainer, catalogue) lives beside this hierarchy in `docs/*.html` and is owned by the website toolchain; paths that moved during the 2026-11-20 reorganization are recorded in [`migration-map.json`](migration-map.json).
