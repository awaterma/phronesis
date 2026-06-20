# phronesis-mcp

MCP server and CLI (`phr-mcp`) wrapping the
[phronesis](https://crates.io/crates/phronesis) RETE rules engine for
durable enforcement of project conventions in LLM-assisted work.

Rules live on disk in `.phronesis/rules.json`, fire from outside the
context window at every tool call, and cannot be compressed away.

## Quick start

```sh
# Install the binary
cargo install phronesis-mcp

# Register as a global MCP server (Claude Code + Gemini CLI)
phr-mcp install

# Initialize in your project
cd /your/project
phr-mcp init --packs llm,rust,confidence,journey
```

## What it does

- **Pre/post hooks** fire rules against every file edit, blocking violations
  and warning on anti-patterns.
- **MCP tools** let the model query rules, fire the engine, audit the tree,
  and detect drift between prose guidance and enforced rules.
- **Starter packs** ship rules for Rust, Python, TypeScript, Swift, Rhai,
  and LLM-behavior (deflection, unverified claims).
- **Journey facts** — durable per-call journal + project-defined taggers
  in `.phronesis/journey.json` let rules match cross-call temporal
  patterns (`auth-churn-without-tests`, `build-staleness`, recent SQL)
  without any in-memory accumulation.
- **Confidence scoring** — per-toolchain adapter (cargo first) parses
  build / test / known-bug outcomes into grounded signals; gate rules
  block or warn a `git commit` by confidence band. Opt-in via
  `.phronesis/confidence.json`.
- **Wiki decisions** — ADR-style pages in `.phronesis/wiki/decisions/`
  travel with the repo and are scored against rules for coverage.

## Commands

```
phr-mcp serve             # MCP stdio server
phr-mcp pre-check         # PreToolUse hook
phr-mcp post-check        # PostToolUse hook
phr-mcp init              # One-command project setup
phr-mcp audit             # Whole-tree rule sweep
phr-mcp stats             # Per-rule activity summary
phr-mcp journey           # journey_* facts asserted right now
phr-mcp confidence        # Confidence band + grounded signals
phr-mcp wiki-drift        # Decision/rule coverage gaps
phr-mcp decision new <s>  # Scaffold an ADR page
```

## Documentation

- [The Explainer](https://awaterma.github.io/phronesis/explainer.html) — how the engine works
- [The Catalogue](https://awaterma.github.io/phronesis/catalogue.html) — starter rules reference
- [Command Reference](https://github.com/awaterma/phronesis/blob/main/crates/phronesis-mcp/CLAUDE.md) — full CLI surface

## License

MIT
