//! The smallest end-to-end Phronesis RETE example.
//!
//! Run with:
//!
//!     cargo run --example minimal --package phronesis

use anyhow::{Context, ensure};
use phronesis::{Action, Condition, Fact, ReteNetwork, Rule};

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let network = ReteNetwork::new();

    // Variables begin with `?`. When the rule matches, Phronesis substitutes
    // the value from the fact into the action.
    network
        .add_rule(Rule {
            id: "welcome-new-user".to_string(),
            priority: 10,
            conditions: vec![Condition {
                predicate: "user_joined".to_string(),
                args: vec!["?name".to_string()],
                script: None,
            }],
            actions: vec![Action {
                action_type: "send_welcome".to_string(),
                params: vec!["?name".to_string()],
            }],
        })
        .await
        .context("failed to add the welcome rule")?;

    network
        .assert_fact(Fact {
            id: "fact-1".to_string(),
            predicate: "user_joined".to_string(),
            args: vec!["Ada".to_string()],
            timestamp: 0,
        })
        .await
        .context("failed to assert the user_joined fact")?;

    network
        .update_agenda()
        .await
        .context("failed to update the agenda")?;
    let actions = network
        .execute_all_agenda_items()
        .context("failed to execute the agenda")?;

    ensure!(actions.len() == 1, "expected one action, got {actions:?}");
    let action = &actions[0];
    ensure!(
        action.action_type == "send_welcome" && action.params == ["Ada"],
        "unexpected action: {action:?}"
    );

    println!("rule fired: {}({})", action.action_type, action.params[0]);
    Ok(())
}
