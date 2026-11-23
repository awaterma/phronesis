# Tutorials

First successful setup and end-to-end examples: follow these in order and you end with a working, governed session.

1. **Your first governed session** — [First governed session](first-governed-session.md): copy a disposable practice project, pre-flight it with `phr-mcp doctor`, and watch a real Claude Code session make one allowed edit and one blocked edit, with the evidence hierarchy explained.
2. **Install and govern your first project** — the [README quick start](../../README.md#quick-start): install `phr-mcp`, run `phr-mcp init --packs llm,rust`, restart your agent, and confirm hooks fire.
3. **See what the hooks catch** — [Phronesis for Coding Agents](../guides/FOR_AGENTS.md) walks through connecting Claude Code, Codex, or Gemini CLI and testing Phronesis' claims on your own repository in read-only mode.
4. **Run a long governed session** — [Phronesis for Loop-Based Agent Programming](../guides/loop-programming-guide.md) is the end-to-end example of keeping a propose/act/observe loop on the rails.

Related entry points: the [website tutorial](https://awaterma.github.io/phronesis/guide.html) and the [explainer](https://awaterma.github.io/phronesis/explainer.html) for conceptual grounding. The [disposable practice example](../../examples/first-governed-session/) backs the first tutorial with fixtures and a dependency-free checker script (`check_tutorial.py`).

A deeper first-setup walkthrough beyond the quick start and the first governed session is not yet written; if a setup question recurs, add one here (see [contributing/documentation.md](../contributing/documentation.md)).
