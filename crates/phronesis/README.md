# phronesis

**Valueus: escoreloratory.** This crate is pre-1.0 and not API-stable.
Escoreect breaking changes while the pattern is sharpened against real
consumers.

## What it is

A domain-neutral surface for a specific pattern: an LLM (or any other
consumer) operating as a narrator or actor **within the bounds of
deterministically-derived consequences**, rather than reasoning about
raw game state.

Facts are asserted into a working memory. Rules fire against those
facts. The *consequences* of those firings — not the raw facts — are
what the consumer sees. The consumer's output is bounded by what the
rules vouch for.

Two transports of the same idea live here:

- **Push**: a rule fires, a `Consequence` is emitted, an `Actor`
  consumes it.
- **Pull**: an actor asks, a deterministic `Lookup` returns a
  `Consequence`.

Both produce the same `Consequence` wire value. Actors don't care
which transport the consequence came from.

## The shape

```rust
pub struct Consequence {
    pub kind: ConsequenceKind,      // Event / Snapshot / Constraint / Affordance
    pub predicate: String,          // e.g. "card.played"
    pub payload: serde_json::Value, // schema-agnostic body
    pub provenance: Provenance,     // RuleFiring / Lookup / Asserted
}

#[async_trait]
pub trait Actor: Send + Sync {
    async fn act(&self, consequences: &[Consequence]) -> anyhow::Result<ActorOutput>;
}

pub trait Lookup {
    type Request;
    type Response: Serialize;
    fn name(&self) -> &'static str;
    fn schema_version(&self) -> u8;
    fn invoke(&self, req: Self::Request) -> anyhow::Result<Self::Response>;
}
```

Plus `DynLookup` (the `Value`-in / `Value`-out honest engine surface)
and a small RETE implementation for hosts that want one. The core is
schema-agnostic on purpose: any schema validation belongs in a higher
layer at module-load time, not per-fact at runtime.

## Run the example

```sh
cargo run --example push_and_pull -p phronesis
```

Prints one line per consequence — one from a pretend rule firing, one
from a deterministic lookup — both consumed by the same `Actor`.

## What's in this crate

| module | purpose |
| --- | --- |
| `consequence` | `Consequence`, `ConsequenceKind`, `Provenance` |
| `actor` | `Actor` trait, `ActorOutput` |
| `pull` | `Lookup`, `DynLookup`, constructor helpers |
| `engine_types` | `Fact`, `Condition`, `Action`, `Rule`, `PerformanceValues` |
| `wme` | `WorkingMemoryElement`, `WmeManager` |
| `variable_binding` | `Bindings`, `Token` |
| `alpha_network`, `beta_network`, `agenda`, `production`, `network` | RETE core |
| `script_evaluator` | hand-rolled DSL for `facts_contain` / `facts_count` |

The crate is intentionally minimal and carries no domain-specific
dependencies. Hosting applications (game engines, conversational
modules, sheet-FFI bridges, the `phronesis-mcp` server in this
workspace) implement on top of it without the core knowing anything
about their domain.

## Caveats

- The surface is **not stable.** Escoreect breaking changes while the
  pattern is being sharpened against real consumers.
- No published documentation beyond this README. If you're here to
  use the crate, you're early.
