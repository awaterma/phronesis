//! End-to-end profiler for `ReteNetwork::assert_fact`.
//!
//! This example deliberately uses only the stable public engine API. For
//! deeper section-level profiling, use a sampling profiler against
//! `assert_fact`; subsystem mutexes are implementation details.

use std::time::{Duration, Instant};

use phronesis::{Action, Condition, Fact, ReteNetwork, Rule};
use tokio::runtime::Builder;

fn build_rules() -> Vec<Rule> {
    (0..30)
        .map(|index| Rule {
            id: format!("rule-{index}"),
            priority: 1,
            conditions: vec![Condition {
                predicate: format!("predicate_{}", index % 5),
                args: vec!["?value".to_string()],
                script: None,
            }],
            actions: vec![Action {
                action_type: "constraint_violation".to_string(),
                params: vec!["matched ?value".to_string()],
            }],
        })
        .collect()
}

fn fact(id: impl Into<String>, index: usize) -> Fact {
    Fact {
        id: id.into(),
        predicate: format!("predicate_{}", index % 5),
        args: vec![format!("value-{index}")],
        timestamp: 0,
        source: None,
    }
}

fn display(duration: Duration) -> String {
    if duration.as_nanos() < 1_000 {
        format!("{}ns", duration.as_nanos())
    } else if duration.as_micros() < 1_000 {
        format!("{:.2}µs", duration.as_nanos() as f64 / 1_000.0)
    } else {
        format!("{:.2}ms", duration.as_nanos() as f64 / 1_000_000.0)
    }
}

fn main() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let rules = build_rules();

    for preload in [0usize, 50, 200, 500, 1_000] {
        let runs = if preload >= 500 { 100 } else { 500 };
        let mut elapsed = Duration::ZERO;
        for run in 0..runs {
            let network = ReteNetwork::new();
            runtime.block_on(async {
                for rule in &rules {
                    network.add_rule(rule.clone()).await.expect("add rule");
                }
                for index in 0..preload {
                    network
                        .assert_fact(fact(format!("preload-{index}"), index))
                        .await
                        .expect("preload fact");
                }
                let start = Instant::now();
                network
                    .assert_fact(fact(format!("probe-{run}"), run))
                    .await
                    .expect("probe fact");
                elapsed += start.elapsed();
            });
        }
        println!(
            "preload={preload:>4}, runs={runs:>3}, assert_fact avg={}",
            display(elapsed / runs as u32)
        );
    }
}
