//! Criterion bench comparing `ReteNetwork::execute_agenda_items_for_fact`
//! (backed by `Agenda::pop_matching`: native heap pop for a matching root,
//! otherwise a scan-and-rebuild) against an ordinary full drain
//! (`execute_all_agenda_items`, which
//! pops the binary heap in its native O(log n) per item).
//! These are end-to-end drain timings: both APIs also search the production
//! states for each activation. Heap-selection complexity alone therefore does
//! not describe the complexity of the whole benchmark.
//!
//! `pop_matching` exists because an embedding host sometimes needs to fire
//! only the activations touched by one just-asserted fact, leaving every
//! other pending activation's sequence untouched — something an ordinary
//! drain cannot do. The question this bench answers is how expensive that
//! selectivity is at realistic agenda sizes and match fractions, so any
//! future change to the selection strategy has a measured baseline instead
//! of a speculative one.
//!
//! Requires the `embedding-host` feature (`execute_agenda_items_for_fact` is
//! gated on it): `cargo bench -p phronesis --bench selective_drain --features embedding-host`.
//!
//! # Comparison limits
//!
//! `selective` and `full_drain` do not do the same amount of work except at
//! `match_fraction == 1.0`: `selective` only judges/executes the `matching`
//! activations and leaves the rest on the agenda, while `full_drain` always
//! judges/executes every one of `total`. At low match fractions `selective`
//! is cheaper largely *because* it does less work, not only because its
//! per-item cost differs — that is the intended, documented behavior of a
//! fact-scoped drain, not an apples-to-apples throughput comparison. The two
//! return different types too (`Vec<ExecutedActivation>` vs `Vec<Action>`,
//! the latter without the original `AgendaItem`), so result-size-driven
//! allocation cost also differs slightly and is not isolated here. Read this
//! bench as "what does calling each API actually cost a host", not as a
//! micro-benchmark of `Agenda::pop_matching` in isolation (it is
//! `pub(crate)`, so an external bench cannot call it directly).

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use phronesis::{Condition, Fact, ReteNetwork, Rule};
use tokio::runtime::Builder;

fn build_runtime() -> tokio::runtime::Runtime {
    Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
}

/// Build an agenda of `total` pending activations, `matching` of which all
/// reference the single fact id `"target"` (several single-condition rules
/// sharing predicate `"shared"`, all triggered by one assertion); the rest
/// are unrelated one-rule-per-fact activations that never reference
/// `"target"`. `guarded` adds an always-true `__script__` condition to every
/// rule, so each selection/drain also pays guard-judging cost.
async fn populate(net: &ReteNetwork, total: usize, matching: usize, guarded: bool) {
    let guard = |conds: &mut Vec<Condition>| {
        if guarded {
            conds.push(Condition {
                predicate: "__script__".to_string(),
                args: vec![],
                script: Some("facts_contain('ready', [])".to_string()),
            });
        }
    };

    for i in 0..matching {
        let mut conditions = vec![Condition {
            predicate: "shared".to_string(),
            args: vec!["?x".to_string()],
            script: None,
        }];
        guard(&mut conditions);
        net.add_rule(Rule {
            id: format!("shared-{i}"),
            priority: 1,
            conditions,
            actions: vec![],
        })
        .await
        .expect("add_rule");
    }

    for i in 0..(total - matching) {
        let mut conditions = vec![Condition {
            predicate: format!("unrelated-{i}"),
            args: vec!["?x".to_string()],
            script: None,
        }];
        guard(&mut conditions);
        net.add_rule(Rule {
            id: format!("unrelated-{i}"),
            priority: 1,
            conditions,
            actions: vec![],
        })
        .await
        .expect("add_rule");
        net.assert_fact(Fact {
            id: format!("filler-{i}"),
            predicate: format!("unrelated-{i}"),
            args: vec!["value".to_string()],
            timestamp: 0,
            source: None,
        })
        .await
        .expect("assert_fact");
    }

    if guarded {
        net.assert_fact(Fact {
            id: "ready".to_string(),
            predicate: "ready".to_string(),
            args: vec![],
            timestamp: 0,
            source: None,
        })
        .await
        .expect("assert_fact");
    }

    if matching > 0 {
        net.assert_fact(Fact {
            id: "target".to_string(),
            predicate: "shared".to_string(),
            args: vec!["value".to_string()],
            timestamp: 0,
            source: None,
        })
        .await
        .expect("assert_fact");
    }
}

fn bench_selective_vs_full_drain(c: &mut Criterion) {
    let rt = build_runtime();

    let mut group = c.benchmark_group("selective_vs_full_drain");
    for &total in &[100usize, 1_000, 10_000] {
        for &match_fraction in &[1.0f64, 0.5, 0.1, 0.01] {
            let matching = ((total as f64) * match_fraction).round() as usize;
            if matching == 0 || matching > total {
                continue;
            }
            for guarded in [false, true] {
                let label = format!(
                    "{}total_{}pct_{}",
                    total,
                    (match_fraction * 100.0) as u32,
                    if guarded { "guarded" } else { "unguarded" }
                );

                group.bench_with_input(
                    BenchmarkId::new("selective", &label),
                    &(total, matching, guarded),
                    |b, &(total, matching, guarded)| {
                        b.iter_batched(
                            || {
                                let net = ReteNetwork::new();
                                rt.block_on(populate(&net, total, matching, guarded));
                                // Validated here, outside the timed routine: the
                                // agenda the routine is about to drain really has
                                // the size/match-fraction this benchmark claims.
                                let snapshot = net.agenda_snapshot().expect("agenda_snapshot");
                                assert_eq!(
                                    snapshot.len(),
                                    total,
                                    "setup produced the wrong agenda size"
                                );
                                let matching_count = snapshot
                                    .iter()
                                    .filter(|item| item.wme_list.iter().any(|w| w.id == "target"))
                                    .count();
                                assert_eq!(
                                    matching_count, matching,
                                    "setup produced the wrong target-matching count"
                                );
                                net
                            },
                            |net| {
                                // Return `net` (and the result) rather than
                                // dropping it here: `iter_batched` only times this
                                // closure's own execution, so a `net` dropped
                                // inside it (tearing down every Arc<Mutex<..>> the
                                // network owns) would count as part of the
                                // measured selective-drain cost. Returning it
                                // defers that drop to after timing stops.
                                let executed = net
                                    .execute_agenda_items_for_fact("target")
                                    .expect("execute_agenda_items_for_fact");
                                (net, executed)
                            },
                            criterion::BatchSize::SmallInput,
                        );
                    },
                );

                group.bench_with_input(
                    BenchmarkId::new("full_drain", &label),
                    &(total, matching, guarded),
                    |b, &(total, matching, guarded)| {
                        b.iter_batched(
                            || {
                                let net = ReteNetwork::new();
                                rt.block_on(populate(&net, total, matching, guarded));
                                let snapshot = net.agenda_snapshot().expect("agenda_snapshot");
                                assert_eq!(
                                    snapshot.len(),
                                    total,
                                    "setup produced the wrong agenda size"
                                );
                                let matching_count = snapshot
                                    .iter()
                                    .filter(|item| item.wme_list.iter().any(|w| w.id == "target"))
                                    .count();
                                assert_eq!(
                                    matching_count, matching,
                                    "setup produced the wrong target-matching count"
                                );
                                net
                            },
                            |net| {
                                // See the `selective` closure above: `net` is
                                // returned, not dropped, so teardown lands outside
                                // the timed region.
                                let actions = net.execute_all_agenda_items().expect("execute_all");
                                (net, actions)
                            },
                            criterion::BatchSize::SmallInput,
                        );
                    },
                );
            }
        }
    }
    group.finish();
}

criterion_group!(benches, bench_selective_vs_full_drain);
criterion_main!(benches);
