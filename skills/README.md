# Skills

Agent skills that use Phronesis from the consumer side — procedures an agent
follows in a project where Phronesis is installed, built on the MCP tools and the
`phr-mcp` CLI.

These are distinct from rule packs. A pack enforces; a skill teaches an agent how
to *use* what the packs, graph, and journals already know.

| Skill | Use when |
|---|---|
| [`exploring-a-repository`](exploring-a-repository/SKILL.md) | `Starting a feature, bugfix, or refactor in an unfamiliar Phronesis project |

## Installing a skill

`SKILL.md` frontmatter follows the [Agent Skills
specification](https://agentskills.io/specification), which Claude Code loads
from a skills directory. Copy or symlink the skill directory into either:

```sh
# available in every project
ln -s "$PWD/skills/exploring-a-repository" ~/.claude/skills/exploring-a-repository

# or scoped to one project
ln -s /path/to/phronesis/skills/exploring-a-repository .claude/skills/exploring-a-repository
```

On hosts without skill loading (Codex, Gemini CLI), read `SKILL.md` as a runbook;
each step gives its `phr-mcp` CLI equivalent.

## Writing another one

Keep skills host-neutral and project-neutral: they may assume a `.phronesis/`
directory exists, but nothing about a particular codebase. Give both the MCP tool
and the CLI command for every step, and state the limits of any evidence a step
produces — graph, drift, and audit output are triage input, not proof.
