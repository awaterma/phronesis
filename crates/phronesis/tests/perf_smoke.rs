//! Performance smoke test — runs with `cargo test`, not `cargo bench`.
//!
//! Asserts the RETE hot path (build network + assert facts + fire
//! consequences) completes within a generous time budget. Each check
//! warms up once, then measures five fresh fixtures and checks the median.
//! This tolerates up to two scheduling spikes, but rejects a slow majority;
//! it is a coarse guard against sustained regressions, not a benchmark.
//! Persistent runner contention can still fail this wall-clock check.
//!
//! The criterion benchmarks in `benches/rete_hot_path.rs` provide
//! proper statistical measurement; this test provides the always-on
//! guardrail.

use phronesis::{Action, Condition, Fact, ReteNetwork, Rule};
use std::future::Future;
use std::time::{Duration, Instant};

const SAMPLE_COUNT: usize = 5;

async fn sample_timings<F, Fut>(mut run: F) -> [Duration; SAMPLE_COUNT]
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Duration>,
{
    // Discard cold-start effects, not errors: a panic still fails the test.
    run().await;
    let mut samples = [Duration::ZERO; SAMPLE_COUNT];
    for sample in &mut samples {
        *sample = run().await;
    }
    samples
}

#[tokio::test]
async fn timing_policy_excludes_warmup_and_keeps_every_measured_sample() {
    let mut timings = [100, 1, 2, 3, 4, 5].into_iter();
    let samples = sample_timings(|| {
        std::future::ready(Duration::from_millis(
            timings.next().expect("bounded sample count"),
        ))
    })
    .await;
    assert_eq!(samples, [1, 2, 3, 4, 5].map(Duration::from_millis));
    assert_eq!(timings.next(), None);
}

fn assert_samples_within_budget(label: &str, samples: [Duration; SAMPLE_COUNT], budget: Duration) {
    let mut sorted = samples;
    sorted.sort_unstable();
    let median = sorted[SAMPLE_COUNT / 2];
    assert!(
        median < budget,
        "{label} median {median:?} — expected <{budget:?}; samples (in order): {samples:?}. \
         Possible sustained performance regression or runner contention."
    );
}

#[test]
fn timing_policy_tolerates_isolated_spikes_in_any_position() {
    for spike in 0..SAMPLE_COUNT {
        let mut samples = [Duration::from_millis(1); SAMPLE_COUNT];
        samples[spike] = Duration::from_millis(100);
        samples[(spike + 1) % SAMPLE_COUNT] = Duration::from_millis(13);
        assert_samples_within_budget("injected", samples, Duration::from_millis(10));
    }
}

#[test]
fn timing_policy_rejects_a_slow_majority_despite_fast_outliers() {
    for fast in 0..SAMPLE_COUNT {
        let mut samples = [Duration::from_millis(13); SAMPLE_COUNT];
        samples[fast] = Duration::from_millis(1);
        samples[(fast + 1) % SAMPLE_COUNT] = Duration::from_millis(1);
        assert!(
            std::panic::catch_unwind(|| {
                assert_samples_within_budget("injected", samples, Duration::from_millis(10));
            })
            .is_err()
        );
    }
}

#[test]
fn timing_policy_keeps_the_strict_budget_boundary() {
    for budget in [Duration::from_millis(2), Duration::from_millis(10)] {
        assert_samples_within_budget(
            "injected",
            [budget - Duration::from_nanos(1); SAMPLE_COUNT],
            budget,
        );
        assert!(
            std::panic::catch_unwind(|| {
                assert_samples_within_budget("injected", [budget; SAMPLE_COUNT], budget);
            })
            .is_err()
        );
    }
}

fn build_rules() -> Vec<Rule> {
    let mut rules = Vec::new();
    for i in 0..10 {
        rules.push(Rule {
            id: format!("single-{i}"),
            priority: 1,
            conditions: vec![Condition {
                predicate: format!("pred_a_{}", i % 5),
                args: vec!["?x".to_string()],
                script: None,
            }],
            actions: vec![Action {
                action_type: "constraint_violation".to_string(),
                params: vec![format!("hit {i}")],
                ..Default::default()
            }],
        });
    }
    for i in 0..10 {
        rules.push(Rule {
            id: format!("two-{i}"),
            priority: 1,
            conditions: vec![
                Condition {
                    predicate: format!("pred_b_{}", i % 5),
                    args: vec!["?file".to_string()],
                    script: None,
                },
                Condition {
                    predicate: format!("pred_c_{}", i % 5),
                    args: vec!["?file".to_string(), "?detail".to_string()],
                    script: None,
                },
            ],
            actions: vec![Action {
                action_type: "constraint_violation".to_string(),
                params: vec![format!("hit two {i}")],
                ..Default::default()
            }],
        });
    }
    rules
}

fn build_facts(n: usize) -> Vec<Fact> {
    let preds = ["pred_a_", "pred_b_", "pred_c_"];
    (0..n)
        .map(|i| {
            let pred = format!("{}{}", preds[i % preds.len()], i % 5);
            let args = if pred.starts_with("pred_a") || pred.starts_with("pred_b") {
                vec![format!("crates/src/mod_{}.rs", i % 7)]
            } else {
                vec![
                    format!("crates/src/mod_{}.rs", i % 7),
                    format!("detail-{i}"),
                ]
            };
            Fact {
                id: format!("fact-{i}"),
                predicate: pred,
                args,
                timestamp: 0,
                source: None,
            }
        })
        .collect()
}

/// The hook hot path: build network, register 20 rules, assert 100
/// facts, fire consequences. Median must complete in under 10ms.
/// Each sample owns a fresh network; fixture generation remains outside
/// the timed region. Optimized Criterion timings are not debug-build baselines.
#[tokio::test]
async fn hot_path_completes_within_budget() {
    let samples = sample_timings(hot_path_sample).await;
    assert_samples_within_budget("hot path", samples, Duration::from_millis(10));
}

async fn hot_path_sample() -> Duration {
    let rules = build_rules();
    let facts = build_facts(100);

    let start = Instant::now();

    let net = ReteNetwork::new();
    for r in &rules {
        net.add_rule(r.clone()).await.expect("add_rule");
    }
    for f in &facts {
        net.assert_fact(f.clone()).await.expect("assert_fact");
    }
    let consequences = net.fire_all_consequences().expect("fire");
    drop(consequences);

    start.elapsed()
}

/// Single fact assertion into a populated network. Median must complete
/// in under 2ms. Rebuild the 200-fact preload before each timed assertion
/// so samples never accumulate probes or reuse mutated network state.
#[tokio::test]
async fn single_assert_within_budget() {
    let samples = sample_timings(single_assert_sample).await;
    assert_samples_within_budget("single assert", samples, Duration::from_millis(2));
}

async fn single_assert_sample() -> Duration {
    let rules = build_rules();
    let preload = build_facts(200);
    let probe = Fact {
        id: "probe".to_string(),
        predicate: "pred_b_0".to_string(),
        args: vec!["crates/probe/src/lib.rs".to_string()],
        timestamp: 0,
        source: None,
    };

    let net = ReteNetwork::new();
    for r in &rules {
        net.add_rule(r.clone()).await.expect("add_rule");
    }
    for f in &preload {
        net.assert_fact(f.clone()).await.expect("assert_fact");
    }

    let start = Instant::now();
    net.assert_fact(probe).await.expect("assert_fact");
    start.elapsed()
}
