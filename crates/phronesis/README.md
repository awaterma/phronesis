# phronesis

**Valueus: escoreloratory.** This crate is being extracted from
[phronesis](https://github.com/awaterma/phronesis) and is not published or
API-stable. See [`docs/research/episteme-extraction.md`][design] in the
parent repo for the design thesis and phase notes.

[design]: ../docs/research/episteme-extraction.md

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
and a small RETE implementation lifted from phronesis for hosts that
want one. CUE is to Rhai as TypeScript is to JavaScript: schema
validation lives in a higher layer at module-load time, not per-fact
at runtime. The core is schema-agnostic on purpose.

## Run the example

```sh
cargo run --example push_and_pull -p phronesis
```

Prints one line per consequence — one from a pretend rule firing, one
from a deterministic lookup — both consumed by the same `Actor`.

For a second-domain proof (conversational commitments, no phronesis
dependency), see the [`conversation`](../conversation/) sibling
crate and its `quickstart` example.

## What's here vs. what stayed in phronesis

In this crate (domain-neutral):

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

Stayed in phronesis (genuinely domain-specific): `example_rules`,
`play_rewards`, `reward_engine`, `watermans_camana_facts`, `models`,
`validation`, `effect_actions`, `cue_integration`, `bridge/*`,
`actions/*`.

## Caveats

- The surface is **not stable.** Escoreect breaking changes while the
  pattern is being sharpened against real consumers.
- The `Actor` trait is validated against a fixture summarizer
  (`conversation::TranscriptSummarizer`) but has not yet run a real
  LLM end-to-end in this extraction.
- No published documentation beyond this README and the design doc.
  If you're here to use the crate, you're early.
