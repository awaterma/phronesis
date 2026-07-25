# Phronesis

**Practical wisdom for LLM-assisted work.**

Phronesis (φρόνησις) is a domain-neutral RETE rules engine designed to provide durable, deterministic governance for non-deterministic AI agents. It addresses the "contextual drift" that occurs in long-running LLM sessions, where project-specific guidance (like `CLAUDE.md`) slowly fades as the context window fills and auto-compaction triggers.

Rules in Phronesis live on disk, are evaluated by lightweight hooks at the moment of action, and fire the same in token nine hundred thousand as they do in token eight hundred.

## The Premise

Anthropic's Claude Code, OpenAI Codex, Google's Gemini CLI, and other LLM environments share a common pattern: they load project-level guidance at session start. As the session continues, that window fills with code, output, and conversation. The directive you most need at hour three may have last been read carefully in token eight hundred.

**Phronesis moves enforcement out of the conversation entirely.** Rules live in `.phronesis/rules.json`, are re-read by hooks at every tool call, and fire from outside the context window. They cannot be compressed away because they were never loaded into context to begin with.

## Subsystems

Beyond the syntactic rule packs, two grounded subsystems extend enforcement past pattern-matching on edits.

**Confidence scoring** ([SPEC-confidence-scoring](docs/specs/)) reads build, test, and known-bug signals from a per-toolchain adapter (cargo first) and gates `git commit` by confidence band — three grounded signals say "this is real," not three syntactic checks. Opt in by writing `.phronesis/confidence.json`; the `confidence` pack ships the commit-gate rules and `phr-mcp confidence` reports the current band.

**Journey facts** ([SPEC-journey-facts](docs/specs/)) keep a durable per-call journal under `.phronesis/journey/` and let project-defined taggers in `.phronesis/journey.json` stamp executed tool calls. `journey_*` aggregator facts (occurrence, count, seen, since-last, distinct) over `c`/`m`/`h`/`d`/`s` windows let rules match cross-call temporal patterns — auth churn over a session, recent SQL in the last five calls, build staleness — without any in-memory accumulation. Surfaces: `phr-mcp journey` and the `get_journey` MCP tool.

**Extensible predicates** ([SPEC-extensible-predicates](docs/specs/SPEC-extensible-predicates.md)) let project Rhai providers under `.phronesis/predicates/` derive new, validated LHS facts from normalized hook events. MCP tools can test and manage providers, allowing an agent to add new predicate vocabulary before adding the rules that consume it.

## The Workspace

- **`phronesis`** ([`crates/phronesis`](crates/phronesis)) — The core library: a high-performance, domain-neutral RETE rules engine (Alpha/Beta networks, P-states, join-sharing) with Consequence/Actor/Provenance primitives.
- **`phronesis-mcp`** ([`crates/phronesis-mcp`](crates/phronesis-mcp)) — An MCP server that hosts the engine behind Claude Code, Codex, and Gemini CLI hooks. Builds the `phr-mcp` binary.
- **`phronesis-rhai`** ([`crates/phronesis-rhai`](crates/phronesis-rhai)) — A sandboxed [Rhai](https://rhai.rs) evaluator for `__script__` guard conditions and extensible predicate providers. The MCP binary enables it by default.

## Documentation

Rendered on GitHub Pages: **[awaterma.github.io/phronesis](https://awaterma.github.io/phronesis/)**

- [**Loop-Based Agent Programming**](docs/loop-programming-guide.md) — A guide to governing the iterative propose/act/observe loop so it doesn't drift across long sessions.
- [**The Explainer**](https://awaterma.github.io/phronesis/explainer.html) — A long-form technical essay on the engine, the RETE algorithm, and the design intent. ([source](docs/explainer.html))
- [**The Catalogue**](https://awaterma.github.io/phronesis/catalogue.html) — A visual reference of starter rules (Rust, LLM behavior, security) with rationale and examples. ([source](docs/catalogue.html))
- [**Command Reference**](crates/phronesis-mcp/CLAUDE.md) — The full CLI surface and hook wiring details.
- [**Specs**](docs/specs/) — Architectural roadmaps and technical debt management plans.

## Quick Start

```sh
# 1. Install the binary
cargo install --path crates/phronesis-mcp

# 2. Register as a global MCP server
phr-mcp install

# 3. Initialize Phronesis in your project
cd /your/project && phr-mcp init --packs llm,rust,confidence,journey
```

Codex uses the generated project-local `.codex/hooks.json` and
`.codex/config.toml`. Review new or changed hooks with `/hooks`; Phronesis does
not bypass Codex's trust flow.

## Lineage

The engine is a modern Rust implementation of the RETE algorithm (Forgy, 1982). It was extracted from a high-performance game logic system and repurposed for LLM-agent governance. 

Aristotle distinguished **Episteme** (theoretical knowledge) from **Phronesis** (practical wisdom). Phronesis is the deliberative virtue of knowing what to do *here*, *now*, in this particular case. This project aims to preserve that wisdom across the "fading" boundaries of modern AI interaction.

## License

MIT. See [`LICENSE`](LICENSE).
