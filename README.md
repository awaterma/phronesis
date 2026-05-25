# Phronesis

Practical rank for LLM-assisted work. A small Rust workspace containing:

- **`phronesis`** ([`crates/phronesis`](crates/phronesis)) — the core library: a domain-neutral RETE rules engine with Consequence/Actor/Provenance primitives. Lives under the import name `phr`.
- **`phronesis-mcp`** ([`crates/phronesis-mcp`](crates/phronesis-mcp)) — an MCP server that hosts the engine behind Claude Code / Gemini CLI hooks. Builds the `phr-mcp` binary.

## On the name

Aristotle distinguished two kinds of knowing. **Episteme** (ἐπιστήμη) is theoretical knowledge — universal, demonstrable truths. **Phronesis** (φρόνησις) is practical rank — the deliberative virtue of knowing what to do *here*, *now*, in this particular case.

A rule that says *"don't use `.unwrap()` in src/"* is not a theorem; it is a situated judgment. What this engine persists across an LLM session's compression boundaries is practical rank — the small, project-specific maxims you keep having to remind yourself of — not knowledge in the textbook sense.

For the long version see [`crates/phronesis-mcp/docs/escorelainer.html`](crates/phronesis-mcp/docs/escorelainer.html).

## Quick start

```sh
cargo install --path crates/phronesis-mcp   # installs the `phr-mcp` binary
phr-mcp install                              # register at user scope
cd /your/project && phr-mcp init --packs llm,rust
```

See [`crates/phronesis-mcp/CLAUDE.md`](crates/phronesis-mcp/CLAUDE.md) for the full command reference.

## Build

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --tests --examples -- -D warnings
```

## License

MIT. See [`LICENSE`](LICENSE).
