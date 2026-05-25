//! Rule-driven tool invocation: push and pull composed at one point.
//!
//! Run with:
//!
//!     cargo run --example rule_driven_lookup -p phronesis
//!
//! A rule fires because an "opponent_appeared" fact was asserted. Its
//! action names a registered pull tool (`lookup_opponent`). The
//! compose helper routes the invocation through the tool and emits a
//! Consequence whose Provenance records *both* the rule and the tool.
//!
//! This is the legitimate "more RETE-centric" direction: not fusing
//! pull into RETE's evaluator, but letting rules declaratively invoke
//! tools. The composition stays clean:
//!
//!   rule triggers  →  action names tool  →  tool invoked  →
//!                  Consequence carries both provenance layers
//!
//! An actor handed the resulting Consequence can trace "why do I
//! know the goblin has 7 VALUE?" through two layers: the rule that
//! fired and the tool that supplied the values. That dual provenance
//! is what the evaluation harness (§8 of the design doc) needs to
//! verify narration against ground truth.

use phronesis::{
    invoke_rule_driven_lookups, Action, Condition, DynLookup, Fact, LookupRegistry, Provenance,
    ReteNetwork, Rule,
};

/// A toy pull tool: given a opponent id, return its value block.
/// In phronesis this would wrap `tools::lookup_opponent`.
struct LookupCard;

impl DynLookup for LookupCard {
    fn name(&self) -> &'static str {
        "lookup_opponent"
    }
    fn schema_version(&self) -> u8 {
        1
    }
    fn invoke_dyn(&self, req: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let id = req
            .as_array()
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let (value, ac, desc) = match id {
            "goblin" => (7, 15, "A scrappy, sneering goblin"),
            "orc" => (15, 13, "A hulking orc with a jagged cleaver"),
            _ => (1, 10, "An unknown creature"),
        };
        Ok(serde_json::json!({
            "id": id,
            "value": value,
            "ac": ac,
            "description": desc,
        }))
    }
}

/// The rule: when an opponent appears, invoke lookup_opponent with the
/// opponent id from the fact's bindings.
fn opponent_appeared_rule() -> Rule {
    Rule {
        id: "opponent_appeared_rule".to_string(),
        priority: 10,
        conditions: vec![Condition {
            predicate: "opponent_appeared".to_string(),
            args: vec!["?opponent_id".to_string()],
            script: None,
        }],
        actions: vec![Action {
            // action_type = tool name → the compose layer routes this.
            action_type: "lookup_opponent".to_string(),
            params: vec!["?opponent_id".to_string()],
        }],
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    // Build a registry of tools the rules can invoke.
    let mut registry = LookupRegistry::new();
    registry.register(LookupCard);
    println!("Registered tools: {:?}", registry.tool_names());

    // Build a RETE network with the opponent-appeared rule.
    let network = ReteNetwork::new();
    network
        .add_rule(opponent_appeared_rule())
        .await
        .map_err(|e| anyhow::anyhow!(e))?;

    // Assert that a goblin just appeared.
    let fact = Fact {
        id: "fact-1".to_string(),
        predicate: "opponent_appeared".to_string(),
        args: vec!["goblin".to_string()],
        timestamp: 0,
    };
    println!("\nAsserting fact: opponent_appeared(goblin)");
    network
        .assert_fact(fact)
        .await
        .map_err(|e| anyhow::anyhow!(e))?;

    // Fire rules. Returns the raw Actions from rule firings.
    network
        .update_agenda()
        .await
        .map_err(|e| anyhow::anyhow!(e))?;
    let actions = network
        .execute_all_agenda_items()
        .map_err(|e: String| anyhow::anyhow!(e))?;
    println!("Rules fired, {} action(s) produced", actions.len());

    // Route the actions through the compose layer. The lookup_opponent
    // action gets resolved to a Consequence; others would pass through
    // in `remaining`.
    let (consequences, remaining) = invoke_rule_driven_lookups(
        "opponent_appeared_rule",
        &["fact-1".to_string()],
        actions,
        &registry,
    );

    println!("\n=== Consequences ({}) ===", consequences.len());
    for c in &consequences {
        println!("predicate: {}", c.predicate);
        println!("payload:   {}", serde_json::to_string_pretty(&c.payload)?);
        match &c.provenance {
            Provenance::RuleDrivenLookup {
                rule_id,
                bound_facts,
                tool,
                schema_version,
                ..
            } => {
                println!(
                    "provenance: RuleDrivenLookup {{ rule={}, facts={:?}, tool={}, v={} }}",
                    rule_id, bound_facts, tool, schema_version
                );
            }
            other => println!("provenance: {other:?}"),
        }
    }

    println!("\n=== Remaining actions ({}) ===", remaining.len());
    for a in &remaining {
        println!("  {} {:?}", a.action_type, a.params);
    }

    Ok(())
}
