//! Release-mode scaling probe for batched versus unbatched fact workloads.
//!
//! Usage: `cargo run --release -p phronesis --example profile_fact_batch -- [N] [RULES] [REPEATS]`
//! Timing is observational; no wall-clock threshold is asserted by tests.

use phronesis::engine_types::{Action, Condition, Fact, Rule};
use phronesis::network::ReteNetwork;
use std::hint::black_box;
use std::time::Instant;

fn fact(id: String, predicate: &str, value: usize) -> Fact {
    Fact {
        id,
        predicate: predicate.into(),
        args: vec![value.to_string()],
        timestamp: 0,
        source: None,
    }
}

fn rule(id: usize) -> Rule {
    Rule {
        id: format!("r{id}"),
        priority: 0,
        conditions: vec![Condition {
            predicate: "probe".into(),
            args: vec!["?x".into()],
            script: None,
        }],
        actions: vec![Action {
            action_type: "log".into(),
            params: vec![format!("r{id}")],
            ..Default::default()
        }],
    }
}

#[derive(Clone, Copy, Debug)]
enum Mode {
    /// Interleaved assert+retract of N distinct facts, one at a time.
    Churn,
    /// Preload N facts (untimed), then time retracting all N.
    PopulatedRetract,
    /// Assert N facts and stop; nothing is retracted.
    AssertOnly,
}

async fn run(n: usize, rules: usize, batched: bool, mode: Mode) -> (std::time::Duration, u64) {
    let net = ReteNetwork::new();
    for i in 0..rules {
        net.add_rule(rule(i)).await.expect("add rule");
    }

    if matches!(mode, Mode::PopulatedRetract) {
        // Preload outside the timed phase: only retraction cost is measured.
        for i in 0..n {
            net.assert_fact(fact(format!("f{i}"), "probe", i))
                .await
                .expect("assert");
        }
        assert_eq!(
            net.agenda_snapshot().expect("agenda").len(),
            n * rules, // every fact's single-condition predicate matches every rule
            "preload must have produced one activation per fact per rule before timing"
        );
    }

    let start = Instant::now();
    let passes_before = net.agenda_pass_count();
    if batched {
        let batch = net.begin_batch().expect("begin batch");
        match mode {
            Mode::Churn => {
                for i in 0..n {
                    net.assert_fact(fact(format!("f{i}"), "probe", i))
                        .await
                        .expect("assert");
                    net.retract_fact(&format!("f{i}")).await.expect("retract");
                }
            }
            Mode::PopulatedRetract => {
                for i in 0..n {
                    net.retract_fact(&format!("f{i}")).await.expect("retract");
                }
            }
            Mode::AssertOnly => {
                for i in 0..n {
                    net.assert_fact(fact(format!("f{i}"), "probe", i))
                        .await
                        .expect("assert");
                }
            }
        }
        batch.close().expect("close batch");
    } else {
        match mode {
            Mode::Churn => {
                for i in 0..n {
                    net.assert_fact(fact(format!("f{i}"), "probe", i))
                        .await
                        .expect("assert");
                    net.retract_fact(&format!("f{i}")).await.expect("retract");
                }
            }
            Mode::PopulatedRetract => {
                for i in 0..n {
                    net.retract_fact(&format!("f{i}")).await.expect("retract");
                }
            }
            Mode::AssertOnly => {
                for i in 0..n {
                    net.assert_fact(fact(format!("f{i}"), "probe", i))
                        .await
                        .expect("assert");
                }
            }
        }
    }

    let elapsed = start.elapsed();
    let passes = net.agenda_pass_count() - passes_before;
    let snapshot = net.agenda_snapshot().expect("agenda");
    match mode {
        Mode::Churn | Mode::PopulatedRetract => {
            assert!(
                snapshot.is_empty(),
                "every asserted fact was retracted; the agenda must end empty, mode={mode:?} batched={batched}"
            );
        }
        Mode::AssertOnly => {
            assert_eq!(
                snapshot.len(),
                n * rules,
                "every fact's single condition must match every rule, mode={mode:?} batched={batched}"
            );
        }
    }
    black_box(&snapshot);
    (elapsed, passes)
}

fn median(mut samples: Vec<std::time::Duration>) -> std::time::Duration {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

async fn report(n: usize, rules: usize, repeats: usize, mode: Mode) {
    let mut ordinary = Vec::with_capacity(repeats);
    let mut batched = Vec::with_capacity(repeats);
    for _ in 0..repeats {
        ordinary.push(run(n, rules, false, mode).await);
        batched.push(run(n, rules, true, mode).await);
    }
    let ordinary_passes: Vec<_> = ordinary.iter().map(|(_, passes)| *passes).collect();
    let batched_passes: Vec<_> = batched.iter().map(|(_, passes)| *passes).collect();
    println!(
        "mode={mode:?} facts={n} rules={rules} repeats={repeats} ordinary_passes={ordinary_passes:?} batched_passes={batched_passes:?} ordinary_median={:?} batched_median={:?}",
        median(ordinary.iter().map(|(time, _)| *time).collect()),
        median(batched.iter().map(|(time, _)| *time).collect())
    );
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut args = std::env::args().skip(1);
    // Every fact's single `probe(?x)` condition matches every rule (unbound
    // variable), so live activations scale as N*rules; keep both bounded by
    // default (200*20 = 4,000 activations) rather than reproducing a large
    // N*rules crossproduct. Override via CLI args for a bigger probe.
    let n = args.next().and_then(|v| v.parse().ok()).unwrap_or(200);
    let rules = args.next().and_then(|v| v.parse().ok()).unwrap_or(20);
    let repeats = args.next().and_then(|v| v.parse().ok()).unwrap_or(3).max(1);
    report(n, rules, repeats, Mode::Churn).await;
    report(n, rules, repeats, Mode::PopulatedRetract).await;
    report(n, rules, repeats, Mode::AssertOnly).await;
    println!(
        "pass counts are full agenda scans; timings include fact propagation; Churn/PopulatedRetract also include alpha/beta/refraction/pending-agenda scan cost per retraction, which does not become linear under batching — only the full production scan is elided"
    );
}
