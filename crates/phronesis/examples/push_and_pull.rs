//! The episteme pattern in ~60 lines, without any domain layer.
//!
//! Run with:
//!
//!     cargo run --example push_and_pull -p phronesis
//!
//! Shows both transports:
//!
//! * **Push mode.** We construct a `Consequence` directly from a
//!   pretend rule firing (the rules engine is elsewhere — this example
//!   just shows what a firing looks like as a wire value). An actor
//!   consumes it and produces output.
//!
//! * **Pull mode.** We implement a tiny `Lookup` (a deterministic
//!   "what's 2+2?" tool), invoke it via `lookup_as_consequence`, and
//!   hand the resulting `Consequence` to the same actor.
//!
//! The point: the actor doesn't know or care whether the consequence
//! came from a rule firing or a tool call. It only sees
//! `&[Consequence]`. That uniformity is the load-bearing property.

use async_trait::async_trait;
use phronesis::{Actor, ActorOutput, Consequence, ConsequenceKind, Lookup, lookup_as_consequence};

/// The simplest possible Actor. Renders each consequence's predicate
/// and provenance source on one line. Deterministic, no LLM.
struct PredicateReporter;

#[async_trait]
impl Actor for PredicateReporter {
    async fn act(&self, consequences: &[Consequence]) -> anyhow::Result<ActorOutput> {
        let mut lines = Vec::new();
        for c in consequences {
            let source = match &c.provenance {
                phronesis::Provenance::RuleFiring { rule_id, .. } => {
                    format!("rule_firing({rule_id})")
                }
                phronesis::Provenance::Lookup { tool, .. } => format!("lookup({tool})"),
                phronesis::Provenance::RuleDrivenLookup { rule_id, tool, .. } => {
                    format!("rule_driven_lookup({rule_id} → {tool})")
                }
                phronesis::Provenance::Asserted { by } => format!("asserted({by})"),
            };
            lines.push(format!("  [{:?}] {} ← {}", c.kind, c.predicate, source));
        }
        Ok(ActorOutput::Text(lines.join("\n")))
    }
}

/// A trivial Lookup — the "hello game" of pull mode.
struct AdderTool;

impl Lookup for AdderTool {
    type Request = (i64, i64);
    type Response = serde_json::Value;

    fn name(&self) -> &'static str {
        "adder"
    }

    fn schema_version(&self) -> u8 {
        1
    }

    fn invoke(&self, req: Self::Request) -> anyhow::Result<Self::Response> {
        let (a, b) = req;
        Ok(serde_json::json!({
            "a": a,
            "b": b,
            "sum": a + b,
        }))
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    // Push: manufacture a Consequence as if a rule just fired.
    let push = Consequence::from_rule_firing(
        phronesis::RuleFiringContext::new(
            "score.points_changed",
            "hand.card_played",
            vec!["fact-42".into()],
            ConsequenceKind::Event,
        ),
        &serde_json::json!({ "actor": "alice", "score_delta": -3 }),
    )?;

    // Pull: invoke a deterministic Lookup and wrap the result.
    let pulled = lookup_as_consequence(&AdderTool, (2, 2))?;

    // One Actor, one uniform input type — same slice.
    let actor = PredicateReporter;
    let output = actor.act(&[push, pulled]).await?;

    println!("=== Actor output ===");
    match output {
        ActorOutput::Text(s) => println!("{s}"),
        other => println!("{other:?}"),
    }

    Ok(())
}
