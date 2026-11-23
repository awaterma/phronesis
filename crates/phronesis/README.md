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
- [The Explainer](https://awaterma.github.io/phronesis/explainer.html) — long-form essay on the engine and RETE algorithm
- [The Catalogue](https://awaterma.github.io/phronesis/catalogue.html) — visual reference of starter rules

## License

MIT

## Driving an embedding host

Enable `embedding-host` to use
`ReteNetwork::execute_agenda_items_for_fact(&str)`. It returns
`Vec<ExecutedActivation>` with the actual `AgendaItem` and its substituted
`Action` descriptions, selecting matches in salience/FIFO order even behind
unrelated higher-priority work. Other pending items retain their sequence and
refraction state. The core does not apply host side effects; the embedding
host handles the returned actions.

Guard-false matches are discarded and unlatched as in ordinary firing;
guard errors retain the existing fail-closed action behavior. On an execution
error, already accumulated results are lost and the failing item is consumed,
but items not yet selected remain pending. This is not a transaction.

`begin_ordered_batch()` returns the same `FactBatch` guard as `begin_batch()`
but preserves the ordered activation trace of the equivalent unbatched
assert/retract sequence, including equal-salience ties. It runs ordinary
incremental updates and does no closing scan, so it **does not provide the
fast batch's coalescing speedup**. Same-mode guards may nest and close in any
order; mixed-mode nesting is rejected before changing batch state. Serialize
mutation, firing and batch boundaries when a consistent boundary is needed.

The original `begin_batch()` remains the fast option: new matches become
visible at the final closing scan and equal-salience order may differ. A
fact-selective drain during a fast batch only sees already-pending items.
See the [embedding follow-ups](../../docs/specs/SPEC-embedding-host-follow-ups.md)
for separately proposed atomic deltas, fault injection and observers.
