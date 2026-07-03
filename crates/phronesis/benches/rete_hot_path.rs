//! Criterion benches for the RETE hot path.
//!
//! Two workloads:
//! - `assert_session`: build a fresh network, register a representative rule
//!   set, then assert ~100 facts and drain consequences. Models the hook
//!   scanning pattern (`crates/phronesis-mcp/src/hook.rs`).
//! - `assert_one`: time a single `assert_fact` against a network already
//!   populated with rules and prior facts. Isolates per-call cost where the
//!   beta-network propagation loop dominates.
//!
//! Goal is *relative* numbers across optimization steps, not absolute
//! micro-benchmarks. Run with `cargo bench -p phronesis --bench rete_hot_path`.

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use phronesis::{Action, Condition, Fact, ReteNetwork, Rule};
use tokio::runtime::Builder;

fn build_runtime() -> tokio::runtime::Runtime {
    Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
}

/// Build a representative rule set: a mix of single-, two-, and three-condition
/// rules. Models the active phronesis hook ruleset where most rules pattern-match
/// over `file_in_src` / `content_contains` style predicates and join on a file path.
fn build_rules() -> Vec<Rule> {
    let mut rules = Vec::new();

    // 10 single-condition rules — alpha-only path.
    for i in 0..10 {
        rules.push(Rule {
            id: format!("single-rule-{i}"),
            priority: 1,
            conditions: vec![Condition {
                predicate: format!("predicate_a_{}", i % 5),
                args: vec!["?x".to_string()],
                script: None,
            }],
            actions: vec![Action {
                action_type: "constraint_violation".to_string(),
                params: vec![format!("hit single {i}: x=?x")],
            }],
        });
    }

    // 10 two-condition rules — one beta join on `?file`.
    for i in 0..10 {
        rules.push(Rule {
            id: format!("two-rule-{i}"),
            priority: 1,
            conditions: vec![
                Condition {
                    predicate: format!("predicate_b_{}", i % 5),
                    args: vec!["?file".to_string()],
                    script: None,
                },
                Condition {
                    predicate: format!("predicate_c_{}", i % 5),
                    args: vec!["?file".to_string(), "?detail".to_string()],
                    script: None,
                },
            ],
            actions: vec![Action {
                action_type: "constraint_violation".to_string(),
                params: vec![format!("hit two {i}: file=?file detail=?detail")],
            }],
        });
    }

    // 10 three-condition rules — two beta joins on `?file`.
    for i in 0..10 {
        rules.push(Rule {
            id: format!("three-rule-{i}"),
            priority: 1,
            conditions: vec![
                Condition {
                    predicate: format!("predicate_d_{}", i % 5),
                    args: vec!["?file".to_string()],
                    script: None,
                },
                Condition {
                    predicate: format!("predicate_e_{}", i % 5),
                    args: vec!["?file".to_string(), "?kind".to_string()],
                    script: None,
                },
                Condition {
                    predicate: format!("predicate_f_{}", i % 5),
                    args: vec!["?file".to_string(), "?owner".to_string()],
                    script: None,
                },
            ],
            actions: vec![Action {
                action_type: "constraint_violation".to_string(),
                params: vec![format!("hit three {i}: file=?file kind=?kind owner=?owner")],
            }],
        });
    }

    rules
}

/// Build a scaled rule set of `n` rules, split evenly across the same three
/// condition-count families as `build_rules` and using the identical predicate
/// naming (`predicate_a_*` … `predicate_f_*`, indexed `i % 5`). Because the
/// predicates and arities match `build_facts`, every family genuinely fires —
/// the two- and three-condition families exercise the beta-join path rather
/// than sitting dead. Models a SpamAssassin-scale ruleset (~200 rules) where
/// the existing fixed 30-rule `build_rules` is too small to surface the
/// super-linear assert/fire cost that fan-out drives.
fn build_rules_scaled(n: usize) -> Vec<Rule> {
    let mut rules = Vec::with_capacity(n);
    let per_family = n / 3;

    for i in 0..per_family {
        rules.push(Rule {
            id: format!("scale-single-{i}"),
            priority: 1,
            conditions: vec![Condition {
                predicate: format!("predicate_a_{}", i % 5),
                args: vec!["?x".to_string()],
                script: None,
            }],
            actions: vec![Action {
                action_type: "constraint_violation".to_string(),
                params: vec![format!("scale single {i}: x=?x")],
            }],
        });
    }

    for i in 0..per_family {
        rules.push(Rule {
            id: format!("scale-two-{i}"),
            priority: 1,
            conditions: vec![
                Condition {
                    predicate: format!("predicate_b_{}", i % 5),
                    args: vec!["?file".to_string()],
                    script: None,
                },
                Condition {
                    predicate: format!("predicate_c_{}", i % 5),
                    args: vec!["?file".to_string(), "?detail".to_string()],
                    script: None,
                },
            ],
            actions: vec![Action {
                action_type: "constraint_violation".to_string(),
                params: vec![format!("scale two {i}: file=?file detail=?detail")],
            }],
        });
    }

    // Remainder folds into the three-condition family so `rules.len() == n`.
    for i in 0..(n - 2 * per_family) {
        rules.push(Rule {
            id: format!("scale-three-{i}"),
            priority: 1,
            conditions: vec![
                Condition {
                    predicate: format!("predicate_d_{}", i % 5),
                    args: vec!["?file".to_string()],
                    script: None,
                },
                Condition {
                    predicate: format!("predicate_e_{}", i % 5),
                    args: vec!["?file".to_string(), "?kind".to_string()],
                    script: None,
                },
                Condition {
                    predicate: format!("predicate_f_{}", i % 5),
                    args: vec!["?file".to_string(), "?owner".to_string()],
                    script: None,
                },
            ],
            actions: vec![Action {
                action_type: "constraint_violation".to_string(),
                params: vec![format!(
                    "scale three {i}: file=?file kind=?kind owner=?owner"
                )],
            }],
        });
    }

    rules
}

/// Build a representative fact set: predicates that match each rule family.
/// Args use longer strings to make `Vec<String>` clone cost realistic.
fn build_facts(n: usize) -> Vec<Fact> {
    let mut facts = Vec::with_capacity(n);
    let buckets = [
        "predicate_a_",
        "predicate_b_",
        "predicate_c_",
        "predicate_d_",
        "predicate_e_",
        "predicate_f_",
    ];
    for i in 0..n {
        let bucket = buckets[i % buckets.len()];
        let pred = format!("{bucket}{}", i % 5);
        // Mimic file paths + detail strings as args.
        let args = if pred.starts_with("predicate_a")
            || pred.starts_with("predicate_b")
            || pred.starts_with("predicate_d")
        {
            vec![format!("crates/some-crate/src/module_{}.rs", i % 7)]
        } else {
            vec![
                format!("crates/some-crate/src/module_{}.rs", i % 7),
                format!("detail-payload-string-number-{i}"),
            ]
        };
        facts.push(Fact {
            id: format!("fact-{i}"),
            predicate: pred,
            args,
            timestamp: 0,
        });
    }
    facts
}

async fn populate_rules(net: &ReteNetwork, rules: &[Rule]) {
    for r in rules {
        net.add_rule(r.clone()).await.expect("add_rule");
    }
}

async fn assert_all(net: &ReteNetwork, facts: &[Fact]) {
    for f in facts {
        net.assert_fact(f.clone()).await.expect("assert_fact");
    }
}

fn bench_assert_session(c: &mut Criterion) {
    let rt = build_runtime();
    let rules = build_rules();

    let mut group = c.benchmark_group("assert_session");
    for &n in &[25usize, 100, 250] {
        let facts = build_facts(n);
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &facts, |b, facts| {
            b.to_async(&rt).iter(|| async {
                let net = ReteNetwork::new();
                populate_rules(&net, &rules).await;
                assert_all(&net, facts).await;
                let consequences = net.fire_all_consequences().expect("fire");
                criterion::black_box(consequences);
            });
        });
    }
    group.finish();
}

fn bench_assert_one(c: &mut Criterion) {
    let rt = build_runtime();
    let rules = build_rules();

    let mut group = c.benchmark_group("assert_one");
    for &preload in &[0usize, 50, 200] {
        group.bench_with_input(
            BenchmarkId::from_parameter(preload),
            &preload,
            |b, &preload| {
                let preload_facts = build_facts(preload);
                let extra_fact = Fact {
                    id: "probe-fact".to_string(),
                    predicate: "predicate_b_0".to_string(),
                    args: vec!["crates/probe/src/lib.rs".to_string()],
                    timestamp: 0,
                };
                b.iter_batched(
                    || {
                        let net = ReteNetwork::new();
                        rt.block_on(async {
                            populate_rules(&net, &rules).await;
                            assert_all(&net, &preload_facts).await;
                        });
                        (net, extra_fact.clone())
                    },
                    |(net, fact)| {
                        rt.block_on(async {
                            net.assert_fact(fact).await.expect("assert_fact");
                        });
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }
    group.finish();
}

/// Time `add_rule` in isolation, segmented by condition count. Session-shaped
/// workloads create a fresh `ReteNetwork` per scan (see `phronesis-mcp`'s
/// `hook.rs`), so rule registration is part of the steady-state cost — this
/// bench tells us whether it's the dominant part.
fn bench_add_rule(c: &mut Criterion) {
    let rt = build_runtime();
    let rules = build_rules();

    // Pre-partitioned for clarity in the report.
    let singles: Vec<Rule> = rules
        .iter()
        .filter(|r| r.conditions.len() == 1)
        .cloned()
        .collect();
    let twos: Vec<Rule> = rules
        .iter()
        .filter(|r| r.conditions.len() == 2)
        .cloned()
        .collect();
    let threes: Vec<Rule> = rules
        .iter()
        .filter(|r| r.conditions.len() == 3)
        .cloned()
        .collect();

    let mut group = c.benchmark_group("add_rule");
    for (label, set) in [("1cond", &singles), ("2cond", &twos), ("3cond", &threes)] {
        group.throughput(Throughput::Elements(set.len() as u64));
        group.bench_function(label, |b| {
            b.iter_batched(
                ReteNetwork::new,
                |net| {
                    rt.block_on(async {
                        for r in set {
                            net.add_rule(r.clone()).await.expect("add_rule");
                        }
                    });
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }
    // And the full mixed set, which is what assert_session pays for once per iter.
    group.throughput(Throughput::Elements(rules.len() as u64));
    group.bench_function("full_30", |b| {
        b.iter_batched(
            ReteNetwork::new,
            |net| {
                rt.block_on(async {
                    populate_rules(&net, &rules).await;
                });
            },
            criterion::BatchSize::SmallInput,
        );
    });
    group.finish();
}

/// SpamAssassin-scale fan-out: ~200 rules, contrasting a *sparse* fact set
/// (few matching facts) against a *dense* one (every fact matches). The dense
/// case drives consequence fan-out into the thousands and surfaces the
/// super-linear assert/fire cost that the fixed 30-rule `assert_session` bench
/// is too small to show. Ported from the former `scale_test.rs` example, but
/// built on `build_rules_scaled`/`build_facts` so every rule family genuinely
/// fires (the original example's content and join families never matched).
fn bench_scale_fanout(c: &mut Criterion) {
    let rt = build_runtime();
    let rules = build_rules_scaled(200);

    let mut group = c.benchmark_group("scale_fanout");
    // (label, fact_count): sparse models realistic low hit-rate; dense is the
    // worst-case fan-out where the network does the most work.
    for &(label, n) in &[("sparse", 50usize), ("dense", 500usize)] {
        let facts = build_facts(n);
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(label), &facts, |b, facts| {
            b.to_async(&rt).iter(|| async {
                let net = ReteNetwork::new();
                populate_rules(&net, &rules).await;
                assert_all(&net, facts).await;
                let consequences = net.fire_all_consequences().expect("fire");
                criterion::black_box(consequences);
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_assert_session,
    bench_assert_one,
    bench_add_rule,
    bench_scale_fanout
);
criterion_main!(benches);
