//! Rule-driven tool invocation: push and pull composed at one point.
//!
//! Run with:
//!
//!     cargo run --example rule_driven_lookup -p phronesis
//!
//! A rule fires because a `"card_drawn"` fact was asserted. Its action
//! names a registered pull tool (`lookup_card`). The compose helper
//! routes the invocation through the tool and emits a Consequence
//! whose Provenance records *both* the rule and the tool.
//!
//! This is the legitimate "more RETE-centric" direction: not fusing
//! pull into RETE's evaluator, but letting rules declaratively invoke
//! tools. The composition stays clean:
//!
//!   rule triggers  →  action names tool  →  tool invoked  →
//!                  Consequence carries both provenance layers
//!
//! An actor handed the resulting Consequence can trace "why is the
//! ace of spades ranked 7 and valued at 15?" through two layers: the
//! rule that fired and the tool that supplied the record. That dual
//! provenance is what an evaluation harness needs to verify an
//! actor's output against ground truth.

use phronesis::{
    Action, Condition, DynLookup, Fact, LookupRegistry, Provenance, ReteNetwork, Rule,
    invoke_rule_driven_lookups,
};

/// A toy pull tool: given a card id, return its record.
/// In a real host this would wrap whatever the host's tool
/// dispatcher exposes (e.g. `tools::lookup_card`).
struct LookupCard;

impl DynLookup for LookupCard {
    fn name(&self) -> &'static str {
        "lookup_card"
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
        let (rank, value, desc) = match id {
            "ace_spades" => (7, 15, "Ace of Spades"),
            "king_hearts" => (15, 13, "King of Hearts"),
            _ => (1, 10, "An unknown card"),
        };
        Ok(serde_json::json!({
            "id": id,
            "rank": rank,
            "value": value,
            "description": desc,
        }))
    }
}

/// The rule: when a card is drawn, invoke lookup_card with the
/// card id from the fact's bindings.
fn card_drawn_rule() -> Rule {
    Rule {
        id: "card_drawn_rule".to_string(),
        priority: 10,
        conditions: vec![Condition {
            predicate: "card_drawn".to_string(),
            args: vec!["?card_id".to_string()],
            script: None,
        }],
        actions: vec![Action {
            // action_type = tool name → the compose layer routes this.
            action_type: "lookup_card".to_string(),
            params: vec!["?card_id".to_string()],
            ..Default::default()
        }],
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    // Build a registry of tools the rules can invoke.
    let mut registry = LookupRegistry::new();
    registry.register(LookupCard);
    println!("Registered tools: {:?}", registry.tool_names());

    // Build a RETE network with the card-drawn rule.
    let network = ReteNetwork::new();
    network
        .add_rule(card_drawn_rule())
        .await
        .map_err(|e| anyhow::anyhow!(e))?;

    // Assert that a card was just drawn.
    let fact = Fact {
        id: "fact-1".to_string(),
        predicate: "card_drawn".to_string(),
        args: vec!["ace_spades".to_string()],
        timestamp: 0,
        source: None,
    };
    println!("\nAsserting fact: card_drawn(ace_spades)");
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
        .map_err(anyhow::Error::from)?;
    println!("Rules fired, {} action(s) produced", actions.len());

    // Route the actions through the compose layer. The lookup_card
    // action gets resolved to a Consequence; others would pass through
    // in `remaining`.
    let (consequences, remaining) = invoke_rule_driven_lookups(
        "card_drawn_rule",
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
