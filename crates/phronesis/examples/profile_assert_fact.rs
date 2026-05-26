//! Per-section profiler for `ReteNetwork::assert_fact`.
//!
//! Replicates the body of `assert_fact` (network.rs:65) using the engine's
//! public `Arc<Mutex<...>>` field handles, with `Instant` timers between
//! sections. Models the `assert_one/200` bench: 30 rules, 200 preloaded facts,
//! then time per-section across many probe asserts to amortize jitter.
//!
//! Run: `cargo run --example profile_assert_fact --release`.
//!
//! Skips two things present in the real `assert_fact`:
//! - `evaluate_script_conditions` — none of the bench rules use scripts, so
//!   the call is a fast no-op in production anyway.
//! - `fired_activations` dedup — uses a local HashSet to preserve identical
//!   work even though it's not the same instance.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use phronesis::production;
use phronesis::{Action, Condition, Fact, ReteNetwork, Rule, WorkingMemoryElement};
use tokio::runtime::Builder;

fn build_rules() -> Vec<Rule> {
    let mut rules = Vec::new();
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

#[derive(Default, Clone, Copy)]
struct Sections {
    wme_insert: Duration,
    alpha_process: Duration,
    beta_propagate: Duration,
    activation_loop: Duration,
    single_cond_path: Duration,
    total: Duration,
}

impl std::ops::AddAssign for Sections {
    fn add_assign(&mut self, rhs: Self) {
        self.wme_insert += rhs.wme_insert;
        self.alpha_process += rhs.alpha_process;
        self.beta_propagate += rhs.beta_propagate;
        self.activation_loop += rhs.activation_loop;
        self.single_cond_path += rhs.single_cond_path;
        self.total += rhs.total;
    }
}

/// Replicates `ReteNetwork::assert_fact` step-by-step with timers.
/// Returns per-section nanoseconds.
fn assert_fact_timed(net: &ReteNetwork, fact: Fact, fired: &mut HashSet<String>) -> Sections {
    let mut s = Sections::default();
    let total_start = Instant::now();

    let wme = WorkingMemoryElement::new(fact);
    let wme_id = wme.id.clone();

    // 1. WME insert
    let t = Instant::now();
    {
        let mut wm = net.wme_manager.lock().unwrap();
        wm.assert(wme).unwrap();
    }
    s.wme_insert = t.elapsed();

    // 2. Alpha processing
    let t = Instant::now();
    let alpha_match_results = {
        let mut alpha = net.alpha_network.lock().unwrap();
        let wm = net.wme_manager.lock().unwrap();
        alpha.process_wme(wm.get(&wme_id).unwrap())
    };
    s.alpha_process = t.elapsed();

    // 3. Beta propagation
    let t = Instant::now();
    let p_state_activations = {
        let mut beta = net.beta_network.lock().unwrap();
        let mut acts = Vec::new();
        for (state_id, token) in alpha_match_results {
            acts.extend(beta.process_token_from_source(&state_id, token));
        }
        acts
    };
    s.beta_propagate = t.elapsed();

    // 4. Activation → agenda loop (multi-condition)
    let t = Instant::now();
    if !p_state_activations.is_empty() {
        let prod = net.production_network.lock().unwrap();
        let mut agenda = net.agenda.lock().unwrap();
        for activation in p_state_activations {
            let wme_ids: Vec<String> = activation.token.wmes.iter().map(|w| w.id.clone()).collect();
            let key = format!("{}:{}", activation.rule_id, wme_ids.join(","));
            if fired.contains(&key) {
                continue;
            }
            let rule = prod
                .find_by_rule_id(activation.rule_id.as_str())
                .map(|pn| pn.rule.clone());
            if let Some(rule) = rule {
                let wme_list = {
                    let wm = net.wme_manager.lock().unwrap();
                    activation
                        .token
                        .wmes
                        .iter()
                        .filter_map(|w| wm.get(&w.id).cloned())
                        .collect::<Vec<_>>()
                };
                agenda.add_item(
                    rule,
                    wme_list,
                    activation.token.bindings,
                    activation.salience,
                );
                fired.insert(key);
            }
        }
    }
    s.activation_loop = t.elapsed();

    // 5. Single-condition rule scan via predicate-keyed index (matches the
    // current `update_agenda_for_wme_single_condition` implementation).
    let t = Instant::now();
    {
        let wm_wme = {
            let wm = net.wme_manager.lock().unwrap();
            wm.get(&wme_id).cloned()
        };
        if let Some(wme) = wm_wme {
            let candidates: Vec<production::SingleCondRuleEntry> = {
                let prod = net.production_network.lock().unwrap();
                prod.single_cond_index
                    .get(&wme.fact.predicate)
                    .cloned()
                    .unwrap_or_default()
            };
            for entry in candidates {
                if !entry.condition.matches(&wme.fact) {
                    continue;
                }
                let key = format!("{}:{}", entry.rule.id, wme.id);
                if fired.contains(&key) {
                    continue;
                }
                let mut bindings = phronesis::Bindings::new();
                for (ca, fa) in entry.condition.args.iter().zip(wme.fact.args.iter()) {
                    if ca.starts_with('?') {
                        bindings.add_binding(ca, fa).ok();
                    }
                }
                let mut agenda = net.agenda.lock().unwrap();
                agenda.add_item(entry.rule, vec![wme.clone()], bindings, entry.salience);
                fired.insert(key);
            }
        }
    }
    s.single_cond_path = t.elapsed();

    s.total = total_start.elapsed();
    s
}

fn pp(ns: Duration) -> String {
    let n = ns.as_nanos();
    if n < 1_000 {
        format!("{n}ns")
    } else if n < 1_000_000 {
        format!("{:.2}µs", n as f64 / 1_000.0)
    } else {
        format!("{:.2}ms", n as f64 / 1_000_000.0)
    }
}

fn pct(part: Duration, whole: Duration) -> f64 {
    if whole.as_nanos() == 0 {
        0.0
    } else {
        100.0 * (part.as_nanos() as f64) / (whole.as_nanos() as f64)
    }
}

fn main() {
    let rt = Builder::new_current_thread().enable_all().build().unwrap();
    let rules = build_rules();

    for &preload in &[0usize, 50, 200, 500, 1000] {
        let preload_facts = build_facts(preload);
        // Tighter iteration counts at higher preload since each setup costs more.
        let probe_count = if preload >= 500 { 100usize } else { 500usize };

        let mut acc = Sections::default();
        for run in 0..(probe_count) {
            let net = ReteNetwork::new();
            rt.block_on(async {
                for r in &rules {
                    net.add_rule(r.clone()).await.unwrap();
                }
                for f in &preload_facts {
                    net.assert_fact(f.clone()).await.unwrap();
                }
            });
            let mut fired = HashSet::new();
            // Probe with a deterministic fact that varies by run to avoid
            // identical activation_key dedup; this matches the bench shape.
            let probe = Fact {
                id: format!("probe-{run}"),
                predicate: "predicate_b_0".to_string(),
                args: vec!["crates/probe/src/lib.rs".to_string()],
                timestamp: 0,
            };
            let s = assert_fact_timed(&net, probe, &mut fired);
            acc += s;
        }

        // Sanity-check: also call the real assert_fact directly and time it,
        // to confirm whether the criterion bench's larger numbers were the
        // routine or harness overhead.
        let mut direct_total = Duration::ZERO;
        let direct_runs = if preload >= 500 { 50usize } else { 200usize };
        for run in 0..direct_runs {
            let net = ReteNetwork::new();
            rt.block_on(async {
                for r in &rules {
                    net.add_rule(r.clone()).await.unwrap();
                }
                for f in &preload_facts {
                    net.assert_fact(f.clone()).await.unwrap();
                }
            });
            let probe = Fact {
                id: format!("direct-probe-{run}"),
                predicate: "predicate_b_0".to_string(),
                args: vec!["crates/probe/src/lib.rs".to_string()],
                timestamp: 0,
            };
            let t = Instant::now();
            rt.block_on(async {
                net.assert_fact(probe).await.unwrap();
            });
            direct_total += t.elapsed();
        }
        let direct_avg = direct_total / direct_runs as u32;

        // Average
        let n = probe_count as u32;
        let avg = Sections {
            wme_insert: acc.wme_insert / n,
            alpha_process: acc.alpha_process / n,
            beta_propagate: acc.beta_propagate / n,
            activation_loop: acc.activation_loop / n,
            single_cond_path: acc.single_cond_path / n,
            total: acc.total / n,
        };
        let unaccounted = avg.total.saturating_sub(
            avg.wme_insert
                + avg.alpha_process
                + avg.beta_propagate
                + avg.activation_loop
                + avg.single_cond_path,
        );

        println!("=== preload={preload}, n={probe_count} ===");
        println!(
            "  wme_insert      {:>10}  ({:>5.1}%)",
            pp(avg.wme_insert),
            pct(avg.wme_insert, avg.total)
        );
        println!(
            "  alpha_process   {:>10}  ({:>5.1}%)",
            pp(avg.alpha_process),
            pct(avg.alpha_process, avg.total)
        );
        println!(
            "  beta_propagate  {:>10}  ({:>5.1}%)",
            pp(avg.beta_propagate),
            pct(avg.beta_propagate, avg.total)
        );
        println!(
            "  activation_loop {:>10}  ({:>5.1}%)",
            pp(avg.activation_loop),
            pct(avg.activation_loop, avg.total)
        );
        println!(
            "  single_cond     {:>10}  ({:>5.1}%)",
            pp(avg.single_cond_path),
            pct(avg.single_cond_path, avg.total)
        );
        println!(
            "  unaccounted     {:>10}  ({:>5.1}%)",
            pp(unaccounted),
            pct(unaccounted, avg.total)
        );
        println!("  TOTAL           {:>10}", pp(avg.total));
        println!(
            "  direct_call     {:>10}  (real net.assert_fact, no instrumentation)",
            pp(direct_avg)
        );
        println!();
    }
}
