# Reference

Factual descriptions of the shipped surface: commands, tools, configuration, predicates, and language support.

- [Language packs](LANGUAGE-PACKS.md) — install, validate, author, and inspect language packs; declared assets, Rhai sandbox, digests, and evidence limits.
- [Command reference (`phr-mcp`)](../../crates/phronesis-mcp/CLAUDE.md) — the full CLI surface, MCP tools, and hook wiring details.
- [Predicates](../../AGENTS.md#available-predicates) — the built-in predicate table (lives in `AGENTS.md` so it stays next to the rule-schema contract it serves).
- [Engine surface](../../crates/phronesis/README.md) — the core RETE library: push/pull transport, consequences, provenance.
- [Metrics](../../crates/phronesis-metrics/README.md) — the Prometheus `/metrics` endpoint and `phr-mcp metrics` exporter.
- [Go runtime coverage](../guides/GO-COVERAGE.md) — per-test Go coverage semantics and the coverage-select contract (how-to in Guides).
