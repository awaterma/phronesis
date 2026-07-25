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

# Register globally for Claude Code + Gemini CLI.
# Codex receives project-scoped MCP registration during init.
phr-mcp install

# Initialize in your project
cd /your/project
phr-mcp init --packs llm,rust,confidence,journey
```

## What it does

- **Pre/post hooks** fire rules against every file edit, blocking violations
  and warning on anti-patterns.
- **MCP tools** let the model query rules, fire the engine, audit the tree,
  detect drift between prose guidance and enforced rules, and test/manage
  sandboxed project predicate providers.
- **Extensible predicates** — Rhai providers derive project vocabulary from
  normalized events. Codex `apply_patch` supplies a batch `files` change set
  before the existing per-file `file_path` evaluation.
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
phr-mcp codex-hook        # Codex lifecycle adapter (JSON stdin/stdout)
phr-mcp init              # One-command project setup
phr-mcp audit             # Whole-tree rule sweep
phr-mcp stats             # Per-rule activity summary
phr-mcp journey           # journey_* facts asserted right now
phr-mcp confidence        # Confidence band + grounded signals
phr-mcp wiki-drift        # Decision/rule coverage gaps
phr-mcp decision new <s>  # Scaffold an ADR page
```

## Codex setup

`phr-mcp init` merges project hooks into `.codex/hooks.json` and registers the
stdio MCP server in `.codex/config.toml`, preserving unrelated settings. It
wires `Bash` and `apply_patch` pre/post events plus session, prompt, compaction,
and subagent context events. Re-running is idempotent; `--dry-run` writes
nothing, and `--hooks-only` refreshes integration files without touching rules.

Codex deliberately does not trust new or changed project hooks automatically.
Open `/hooks`, inspect the exact commands, and trust them explicitly. Phronesis
hooks are deterministic guardrails, not a complete security boundary: some
specialized or hosted tool paths may not traverse local lifecycle hooks.

Codex observes unified shell execution as `Bash`, file edits as `apply_patch`,
and can expose other local or MCP tools. Phronesis v0.22 governs only `Bash` and
`apply_patch`; unsupported tools are safe no-ops.

## Documentation

- [The Explainer](https://awaterma.github.io/phronesis/explainer.html) — how the engine works
- [The Catalogue](https://awaterma.github.io/phronesis/catalogue.html) — starter rules reference
- [Command Reference](https://github.com/awaterma/phronesis/blob/main/crates/phronesis-mcp/CLAUDE.md) — full CLI surface

## License

MIT
