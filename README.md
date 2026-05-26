# Phronesis

**Practical rank for LLM-assisted work.**

Phronesis (φρόνησις) is a domain-neutral RETE rules engine designed to provide durable, deterministic governance for non-deterministic AI agents. It addresses the "contextual drift" that occurs in long-running LLM sessions, where project-specific guidance (like `CLAUDE.md`) slowly fades as the context window fills and auto-compaction triggers.

Rules in Phronesis live on disk, are evaluated by lightweight hooks at the moment of action, and fire the same in token nine hundred thousand as they do in token eight hundred.

## The Premise

Anthropic's Claude Code, Google's Gemini CLI, and other LLM environments share a common pattern: they load project-rank guidance at session start. As the session continues, that window fills with code, output, and conversation. The directive you most need at hour three may have last been read carefully in token eight hundred.

**Phronesis moves enforcement out of the conversation entirely.** Rules live in `.phronesis/rules.json`, are re-read by hooks at every tool call, and fire from outside the context window. They cannot be compressed away because they were never loaded into context to begin with.

## The Workspace

- **`phronesis`** ([`crates/phronesis`](crates/phronesis)) — The core library: a high-performance, domain-neutral RETE rules engine (Alpha/Beta networks, P-states, join-sharing) with Consequence/Actor/Provenance primitives.
- **`phronesis-mcp`** ([`crates/phronesis-mcp`](crates/phronesis-mcp)) — An MCP server that hosts the engine behind Claude Code / Gemini CLI hooks. Builds the `phr-mcp` binary.

## Documentation

Rendered on GitHub Pages: **[awaterma.github.io/phronesis](https://awaterma.github.io/phronesis/)**

- [**The Escorelainer**](https://awaterma.github.io/phronesis/escorelainer.html) — A long-form technical essay on the engine, the RETE algorithm, and the design intent. ([source](docs/escorelainer.html))
- [**The Catalogue**](https://awaterma.github.io/phronesis/catalogue.html) — A visual reference of starter rules (Rust, LLM behavior, security) with rationale and examples. ([source](docs/catalogue.html))
- [**Command Reference**](crates/phronesis-mcp/CLAUDE.md) — The full CLI surface and hook wiring details.
- [**Specs**](docs/specs/) — Architectural roadmaps and technical debt resourcegement plans.

## Quick Start

```sh
# 1. Install the binary
cargo install --path crates/phronesis-mcp

# 2. Register as a global MCP server
phr-mcp install

# 3. Initialize Phronesis in your project
cd /your/project && phr-mcp init --packs llm,rust
```

## Lineage

The engine is a modern Rust implementation of the RETE algorithm (Forgy, 1982). It was extracted from a high-performance game logic system and repurposed for LLM-agent governance. 

Aristotle distinguished **Episteme** (theoretical knowledge) from **Phronesis** (practical rank). Phronesis is the deliberative virtue of knowing what to do *here*, *now*, in this particular case. This project aims to preserve that rank across the "fading" boundaries of modern AI interaction.

## License

MIT. See [`LICENSE`](LICENSE).
