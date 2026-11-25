# Tutorials

First successful setup and end-to-end examples: follow these in order and you end with a working, governed session.

1. **Your first governed session** — [First governed session](first-governed-session.md): use TDD to finish a small Rust task, observe a real policy refusal, recover with a compliant implementation, and run the unchanged tests green.
2. **Install and govern your first project** — the [README quick start](../../README.md#quick-start): install `phr-mcp`, run `phr-mcp init --packs llm,rust`, restart your agent, and confirm hooks fire.
3. **See what the hooks catch** — [Phronesis for Coding Agents](../guides/FOR_AGENTS.md) walks through connecting Claude Code, Codex, or Gemini CLI and testing Phronesis' claims on your own repository in read-only mode.
4. **Run a long governed session** — [Phronesis for Loop-Based Agent Programming](../guides/loop-programming-guide.md) is the end-to-end example of keeping a propose/act/observe loop on the rails.

**Installation check:** [Verify your installation](verify-installation.md) retains the language-independent marker exercise, detailed doctor findings and host setup troubleshooting.

Related entry points: the [website tutorial](https://awaterma.github.io/phronesis/guide.html) and the [explainer](https://awaterma.github.io/phronesis/explainer.html) for conceptual grounding. The [disposable practice example](../../examples/first-governed-session/) backs both tutorials with fixtures and a Python standard-library checker (`check_tutorial.py`). The TDD check also requires Cargo and compiles a crate with no dependencies.

A deeper first-setup walkthrough beyond the quick start and the first governed session is not yet written; if a setup question recurs, add one here (see [contributing/documentation.md](../contributing/documentation.md)).
