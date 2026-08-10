# phronesis

A domain-neutral RETE rules engine for durable, context-window-independent
enforcement of project conventions in LLM-assisted work.

Rules live on disk. They fire deterministically against asserted facts.
The **consequences** of those firings — not raw state — are what an LLM sees.

## The pattern

Two transports of the same idea:

- **Push** — rule fires -> `Consequence` -> `Actor` consumes it.
- **Pull** — actor asks -> deterministic `Lookup` returns a `Consequence`.

The crate defines the types and the engine; integration with any particular
host (an MCP server, a game engine, a conversational module) lives outside
this crate.

## Quick example

Run the smallest complete RETE cycle:

```sh
cargo run --example minimal --package phronesis
```

Expected output:

```text
rule fired: send_welcome(Ada)
```

See [Your first Phronesis rule](../../docs/tutorial.md) for the five-minute
walkthrough. The pull-mode API is shown below:

```rust
use phronesis::{Consequence, ConsequenceKind, Lookup, lookup_as_consequence};

// Pull mode: invoke a deterministic Lookup and wrap the result.
struct AdderTool;

impl Lookup for AdderTool {
    type Request = (i64, i64);
    type Response = serde_json::Value;

    fn name(&self) -> &'static str { "adder" }
    fn schema_version(&self) -> u8 { 1 }

    fn invoke(&self, req: Self::Request) -> anyhow::Result<Self::Response> {
        let (a, b) = req;
        Ok(serde_json::json!({ "sum": a + b }))
    }
}

let consequence = lookup_as_consequence(&AdderTool, (2, 2)).unwrap();
assert_eq!(consequence.kind, ConsequenceKind::Observation);
```

See `examples/push_and_pull.rs` for the full push + pull pattern.

## Companion crate

[`phronesis-mcp`](https://crates.io/crates/phronesis-mcp) wraps this
engine behind an MCP server and CLI (`phr-mcp`) for use with Claude Code
and Gemini CLI.

## Documentation

- [API docs (docs.rs)](https://docs.rs/phronesis)
- [Your first Phronesis rule](../../docs/tutorial.md) — a five-minute runnable tutorial
- [The Explainer](https://awaterma.github.io/phronesis/explainer.html) — long-form essay on the engine and RETE algorithm
- [The Catalogue](https://awaterma.github.io/phronesis/catalogue.html) — visual reference of starter rules

## License

MIT
