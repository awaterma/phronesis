//! #159 regression: immediate agenda activations for multi-condition rules.
//!
//! The join chain built for conditions [C1, C2, C3, ...] wires beta output
//! into each next join's left input and each alpha condition into the right
//! input (C1 into the first join's left). When the fact that completes the
//! rule arrives at a join other than the terminal one, the joined token must
//! continue propagating and reach the p-state, creating the activation
//! immediately — before any fire or rescan. These tests pin that behavior for
//! every arrival permutation of 2, 3, and 4 condition facts, plus retraction,
//! replacement, binding consistency, multiple matches, shared joins, and a
//! beta state feeding a join's right input (low-level, direct PStateActivation
//! examination).

use phronesis::beta_network::{BetaNetwork, PStateActivation};
use phronesis::consequence::Provenance;
use phronesis::engine_types::{Action, Condition, Fact, Rule};
use phronesis::network::ReteNetwork;
use phronesis::variable_binding::{Bindings, Token};
use phronesis::wme::WorkingMemoryElement;

fn var_condition(predicate: &str) -> Condition {
    Condition {
        predicate: predicate.into(),
        args: vec!["?x".into()],
        script: None,
    }
}

fn rule(id: &str, priority: i32, predicates: &[&str]) -> Rule {
    Rule {
        id: id.into(),
        priority,
        conditions: predicates.iter().map(|p| var_condition(p)).collect(),
        actions: vec![Action {
            action_type: "log".into(),
            params: vec![format!("{id} fired with ?x")],
            ..Default::default()
        }],
    }
}

fn fact(id: &str, predicate: &str, value: &str) -> Fact {
    Fact {
        id: id.into(),
        predicate: predicate.into(),
        args: vec![value.into()],
        timestamp: 0,
        source: None,
    }
}

fn two_var_condition(predicate: &str) -> Condition {
    Condition {
        predicate: predicate.into(),
        args: vec!["?x".into(), "?r".into()],
        script: None,
    }
}

fn fact2(id: &str, predicate: &str, x: &str, r: &str) -> Fact {
    Fact {
        id: id.into(),
        predicate: predicate.into(),
        args: vec![x.into(), r.into()],
        timestamp: 0,
        source: None,
    }
}

/// All permutations of `0..n` (arrival orders of the condition facts).
fn permutations(n: usize) -> Vec<Vec<usize>> {
    fn permute(prefix: Vec<usize>, rest: &[usize], out: &mut Vec<Vec<usize>>) {
        if rest.is_empty() {
            out.push(prefix);
            return;
        }
        for i in 0..rest.len() {
            let mut next_rest = rest.to_vec();
            let picked = next_rest.remove(i);
            let mut next_prefix = prefix.clone();
            next_prefix.push(picked);
            permute(next_prefix, &next_rest, out);
        }
    }
    let mut out = Vec::new();
    permute(Vec::new(), &(0..n).collect::<Vec<_>>(), &mut out);
    out
}

fn count_for_rule(agenda: &[phronesis::agenda::AgendaItem], rule_id: &str) -> usize {
    agenda.iter().filter(|item| item.rule.id == rule_id).count()
}

fn fired_rule_ids(consequences: &[phronesis::consequence::Consequence]) -> Vec<&str> {
    consequences
        .iter()
        .filter_map(|c| match &c.provenance {
            Provenance::RuleFiring { rule_id, .. } => Some(rule_id.as_str()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn three_condition_rule_activates_immediately_on_every_arrival_order() {
    let predicates = ["p", "q", "r"];
    for order in permutations(3) {
        let net = ReteNetwork::new();
        net.add_rule(rule("r3", 5, &predicates)).await.unwrap();
        let facts = [
            fact("fp", "p", "a"),
            fact("fq", "q", "a"),
            fact("fr", "r", "a"),
        ];
        for &i in &order {
            net.assert_fact(facts[i].clone()).await.unwrap();
        }

        let agenda = net.agenda_snapshot().unwrap();
        assert_eq!(
            count_for_rule(&agenda, "r3"),
            1,
            "immediate agenda must hold the activation for arrival order {order:?}"
        );

        let consequences = net.fire_all_consequences().unwrap();
        assert_eq!(
            consequences.len(),
            1,
            "exactly one firing for arrival order {order:?}: {consequences:?}"
        );
        assert_eq!(fired_rule_ids(&consequences), vec!["r3"]);
    }
}

#[tokio::test]
async fn four_condition_rule_activates_immediately_on_every_arrival_order() {
    let predicates = ["p", "q", "r", "s"];
    for order in permutations(4) {
        let net = ReteNetwork::new();
        net.add_rule(rule("r4", 5, &predicates)).await.unwrap();
        let facts = [
            fact("fp", "p", "a"),
            fact("fq", "q", "a"),
            fact("fr", "r", "a"),
            fact("fs", "s", "a"),
        ];
        for &i in &order {
            net.assert_fact(facts[i].clone()).await.unwrap();
        }

        let agenda = net.agenda_snapshot().unwrap();
        assert_eq!(
            count_for_rule(&agenda, "r4"),
            1,
            "immediate agenda must hold the activation for arrival order {order:?}"
        );

        let consequences = net.fire_all_consequences().unwrap();
        assert_eq!(
            consequences.len(),
            1,
            "exactly one firing for arrival order {order:?}: {consequences:?}"
        );
    }
}

#[tokio::test]
async fn two_condition_control_activates_immediately_on_every_arrival_order() {
    for order in permutations(2) {
        let net = ReteNetwork::new();
        net.add_rule(rule("r2", 5, &["p", "q"])).await.unwrap();
        let facts = [fact("fp", "p", "a"), fact("fq", "q", "a")];
        for &i in &order {
            net.assert_fact(facts[i].clone()).await.unwrap();
        }

        let agenda = net.agenda_snapshot().unwrap();
        assert_eq!(
            count_for_rule(&agenda, "r2"),
            1,
            "immediate agenda must hold the activation for arrival order {order:?}"
        );
        assert_eq!(net.fire_all_consequences().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn single_condition_control_activates_immediately() {
    let net = ReteNetwork::new();
    net.add_rule(rule("r1", 5, &["p"])).await.unwrap();
    net.assert_fact(fact("fp", "p", "a")).await.unwrap();

    let agenda = net.agenda_snapshot().unwrap();
    assert_eq!(count_for_rule(&agenda, "r1"), 1);
    assert_eq!(net.fire_all_consequences().unwrap().len(), 1);
}

#[tokio::test]
async fn inconsistent_bindings_never_activate_and_later_match_fires_once() {
    let net = ReteNetwork::new();
    net.add_rule(rule("r3", 5, &["p", "q", "r"])).await.unwrap();

    net.assert_fact(fact("fp", "p", "a")).await.unwrap();
    net.assert_fact(fact("fq", "q", "b")).await.unwrap();
    net.assert_fact(fact("fr", "r", "a")).await.unwrap();

    let agenda = net.agenda_snapshot().unwrap();
    assert_eq!(
        count_for_rule(&agenda, "r3"),
        0,
        "?x=a and ?x=b conflict; no activation"
    );
    assert!(net.fire_all_consequences().unwrap().is_empty());

    // q's matching fact arrives last — a non-last condition completing the
    // rule — and must activate immediately.
    net.assert_fact(fact("fq2", "q", "a")).await.unwrap();
    let agenda = net.agenda_snapshot().unwrap();
    assert_eq!(count_for_rule(&agenda, "r3"), 1);
    let consequences = net.fire_all_consequences().unwrap();
    assert_eq!(consequences.len(), 1);
    assert_eq!(fired_rule_ids(&consequences), vec!["r3"]);
}

#[tokio::test]
async fn multiple_matches_activate_once_each() {
    let net = ReteNetwork::new();
    net.add_rule(rule("r3", 5, &["p", "q", "r"])).await.unwrap();

    // Interleave values so each completion happens through a different join.
    net.assert_fact(fact("fp1", "p", "a")).await.unwrap();
    net.assert_fact(fact("fq2", "q", "b")).await.unwrap();
    net.assert_fact(fact("fp2", "p", "b")).await.unwrap();
    net.assert_fact(fact("fq1", "q", "a")).await.unwrap();
    net.assert_fact(fact("fr1", "r", "a")).await.unwrap();
    net.assert_fact(fact("fr2", "r", "b")).await.unwrap();

    let agenda = net.agenda_snapshot().unwrap();
    assert_eq!(
        count_for_rule(&agenda, "r3"),
        2,
        "one activation for ?x=a and one for ?x=b: {agenda:?}"
    );
    let consequences = net.fire_all_consequences().unwrap();
    assert_eq!(consequences.len(), 2);
}

#[tokio::test]
async fn retract_purges_pending_activation_and_reassert_restores_exactly_one() {
    let net = ReteNetwork::new();
    net.add_rule(rule("r3", 5, &["p", "q", "r"])).await.unwrap();

    net.assert_fact(fact("fp", "p", "a")).await.unwrap();
    net.assert_fact(fact("fr", "r", "a")).await.unwrap();
    net.assert_fact(fact("fq", "q", "a")).await.unwrap();
    assert_eq!(count_for_rule(&net.agenda_snapshot().unwrap(), "r3"), 1);

    net.retract_fact("fq").await.unwrap();
    assert_eq!(
        count_for_rule(&net.agenda_snapshot().unwrap(), "r3"),
        0,
        "stale activation must not survive retraction"
    );
    assert!(
        net.fire_all_consequences().unwrap().is_empty(),
        "retracted fact must not fire"
    );

    net.assert_fact(fact("fq3", "q", "a")).await.unwrap();
    assert_eq!(
        count_for_rule(&net.agenda_snapshot().unwrap(), "r3"),
        1,
        "re-assert must restore exactly one activation"
    );
    assert_eq!(net.fire_all_consequences().unwrap().len(), 1);
}

#[tokio::test]
async fn replacement_fact_swaps_binding_without_stale_firing() {
    let net = ReteNetwork::new();
    net.add_rule(rule("r3", 5, &["p", "q", "r"])).await.unwrap();

    net.assert_fact(fact("fp", "p", "a")).await.unwrap();
    net.assert_fact(fact("fq", "q", "a")).await.unwrap();
    net.assert_fact(fact("fr", "r", "a")).await.unwrap();
    assert_eq!(count_for_rule(&net.agenda_snapshot().unwrap(), "r3"), 1);

    // Replace p's fact with an incompatible value: no activation may fire.
    net.retract_fact("fp").await.unwrap();
    net.assert_fact(fact("fp2", "p", "b")).await.unwrap();
    assert_eq!(count_for_rule(&net.agenda_snapshot().unwrap(), "r3"), 0);
    assert!(net.fire_all_consequences().unwrap().is_empty());

    // Bring q and r to the new value: exactly one activation for ?x=b.
    net.retract_fact("fq").await.unwrap();
    net.assert_fact(fact("fq2", "q", "b")).await.unwrap();
    net.retract_fact("fr").await.unwrap();
    net.assert_fact(fact("fr2", "r", "b")).await.unwrap();
    assert_eq!(count_for_rule(&net.agenda_snapshot().unwrap(), "r3"), 1);
    let consequences = net.fire_all_consequences().unwrap();
    assert_eq!(consequences.len(), 1);
    assert_eq!(consequences[0].payload["message"], "r3 fired with b");
}

#[tokio::test]
async fn shared_join_completes_both_rules_when_non_last_condition_arrives_last() {
    let net = ReteNetwork::new();
    net.add_rule(rule("ra", 5, &["p", "q", "ra3"]))
        .await
        .unwrap();
    net.add_rule(rule("rb", 5, &["p", "q", "rb3"]))
        .await
        .unwrap();

    // q's fact arrives last; it completes the shared p×q join feeding both
    // rules, so both must activate immediately.
    net.assert_fact(fact("fp", "p", "a")).await.unwrap();
    net.assert_fact(fact("frb", "rb3", "a")).await.unwrap();
    net.assert_fact(fact("fra", "ra3", "a")).await.unwrap();
    net.assert_fact(fact("fq", "q", "a")).await.unwrap();

    let agenda = net.agenda_snapshot().unwrap();
    assert_eq!(count_for_rule(&agenda, "ra"), 1, "agenda: {agenda:?}");
    assert_eq!(count_for_rule(&agenda, "rb"), 1, "agenda: {agenda:?}");

    let consequences = net.fire_all_consequences().unwrap();
    let mut fired = fired_rule_ids(&consequences);
    fired.sort_unstable();
    assert_eq!(fired, vec!["ra", "rb"]);
}

fn wme(id: &str, predicate: &str, value: &str) -> WorkingMemoryElement {
    WorkingMemoryElement {
        id: id.into(),
        fact: fact(id, predicate, value),
        timestamp: 0,
    }
}

fn bound_token(id: &str, predicate: &str, value: &str) -> Token {
    let mut bindings = Bindings::new();
    bindings.add_binding("?x", value).unwrap();
    Token::new_with_bindings(vec![wme(id, predicate, value)], bindings)
}

/// A token binding `?x` to `value` plus extra per-side variables, so joins
/// must compose distinct bindings rather than one shared variable.
fn bound_token_vars(id: &str, predicate: &str, value: &str, extra: &[(&str, &str)]) -> Token {
    let mut bindings = Bindings::new();
    bindings.add_binding("?x", value).unwrap();
    for (var, val) in extra {
        bindings.add_binding(var, val).unwrap();
    }
    Token::new_with_bindings(vec![wme(id, predicate, value)], bindings)
}

fn assert_bindings(bindings: &Bindings, expected: &[(&str, &str)]) {
    for (var, value) in expected {
        assert_eq!(
            bindings.get_binding(var),
            Some(&value.to_string()),
            "binding {var}"
        );
    }
    assert_eq!(
        bindings.bindings.len(),
        expected.len(),
        "exact binding set: {:?}",
        bindings.bindings
    );
}

fn join_chain(beta: &mut BetaNetwork, sources: &[&str], terminal_rule: &str) -> String {
    let mut current = sources[0].to_string();
    for (index, next) in sources[1..].iter().enumerate() {
        let join =
            beta.get_or_create_join(current.clone(), (*next).to_string(), format!("join{index}"));
        current = join;
    }
    beta.mark_as_p_state(&current, terminal_rule, 7);
    current
}

#[test]
fn three_state_chain_activates_in_every_arrival_order() {
    let sources = ["alpha:p", "alpha:q", "alpha:r"];
    let tokens = [
        bound_token_vars("alpha:p", "p", "a", &[]),
        bound_token_vars("alpha:q", "q", "a", &[("?qy", "1")]),
        bound_token_vars("alpha:r", "r", "a", &[("?ry", "2")]),
    ];
    for order in permutations(3) {
        let mut beta = BetaNetwork::new();
        join_chain(&mut beta, &sources, "rule3");

        let mut saw_activation = false;
        for &i in &order {
            let acts: Vec<PStateActivation> =
                beta.process_token_from_source(sources[i], tokens[i].clone());
            if i == order[order.len() - 1] {
                assert_eq!(
                    acts.len(),
                    1,
                    "the last arrival must activate immediately, order {order:?}"
                );
                assert_eq!(acts[0].rule_id, "rule3");
                assert_eq!(acts[0].salience, 7);
                let ids: Vec<&str> = acts[0].token.wmes.iter().map(|w| w.id.as_str()).collect();
                assert_eq!(
                    ids,
                    vec!["alpha:p", "alpha:q", "alpha:r"],
                    "token composition, order {order:?}"
                );
                assert_bindings(
                    &acts[0].token.bindings,
                    &[("?x", "a"), ("?qy", "1"), ("?ry", "2")],
                );
                saw_activation = true;
            } else {
                assert!(acts.is_empty(), "premature activation, order {order:?}");
            }
        }
        assert!(saw_activation);
    }
}

#[test]
fn beta_output_feeding_right_input_activates_in_every_arrival_order() {
    // J2 = alpha:c × J1, i.e. the beta chain output feeds a join's RIGHT
    // input. The token produced at J1 must enter J2 through the declared
    // right side, not the left.
    let tokens = [
        bound_token_vars("alpha:p", "p", "a", &[]),
        bound_token_vars("alpha:q", "q", "a", &[("?qy", "1")]),
        bound_token_vars("alpha:r", "r", "a", &[("?ry", "2")]),
    ];
    for order in permutations(3) {
        let mut beta = BetaNetwork::new();
        let j1 = beta.add_join("alpha:p".into(), "alpha:q".into(), "j0".into());
        let j2 = beta.add_join("alpha:r".into(), j1.clone(), "j1".into());
        beta.mark_as_p_state(&j2, "rule_right", 3);

        let sources = ["alpha:p", "alpha:q", "alpha:r"];
        let mut saw_activation = false;
        for &i in &order {
            let acts = beta.process_token_from_source(sources[i], tokens[i].clone());
            if i == order[order.len() - 1] {
                assert_eq!(
                    acts.len(),
                    1,
                    "the last arrival must activate immediately, order {order:?}"
                );
                assert_eq!(acts[0].rule_id, "rule_right");
                assert_eq!(acts[0].salience, 3);
                let ids: Vec<&str> = acts[0].token.wmes.iter().map(|w| w.id.as_str()).collect();
                // J2's left input is alpha:r and its right input is J1's
                // output (which carries alpha:p then alpha:q), so the joined
                // token lists the WMEs in exactly that side order.
                assert_eq!(
                    ids,
                    vec!["alpha:r", "alpha:p", "alpha:q"],
                    "token composition, order {order:?}"
                );
                assert_bindings(
                    &acts[0].token.bindings,
                    &[("?x", "a"), ("?qy", "1"), ("?ry", "2")],
                );
                saw_activation = true;
            } else {
                assert!(acts.is_empty(), "premature activation, order {order:?}");
            }
        }
        assert!(saw_activation);
    }
}

#[test]
fn shared_beta_join_fanout_activates_every_consumer() {
    let mut beta = BetaNetwork::new();
    let j1 = beta.add_join("alpha:p".into(), "alpha:q".into(), "j0".into());
    let ja = beta.add_join(j1.clone(), "alpha:ra".into(), "ja".into());
    let jb = beta.add_join(j1.clone(), "alpha:rb".into(), "jb".into());
    beta.mark_as_p_state(&ja, "rule_a", 1);
    beta.mark_as_p_state(&jb, "rule_b", 1);

    // Seed the terminal joins' right inputs; no rule is complete yet. Each
    // terminal condition binds its own variable on top of the shared ?x so
    // consumer-side bindings are distinguishable.
    beta.process_token_from_source(
        "alpha:ra",
        bound_token_vars("alpha:ra", "ra", "a", &[("?ra", "1")]),
    );
    beta.process_token_from_source(
        "alpha:rb",
        bound_token_vars("alpha:rb", "rb", "a", &[("?rb", "2")]),
    );
    beta.process_token_from_source("alpha:p", bound_token_vars("alpha:p", "p", "a", &[]));

    // Completing the shared p×q join fans out to both consumers, once each,
    // with each consumer's own WMEs and bindings.
    let acts =
        beta.process_token_from_source("alpha:q", bound_token_vars("alpha:q", "q", "a", &[]));
    assert_eq!(acts.len(), 2);
    assert_fanout_activation(
        &acts,
        "rule_a",
        &["alpha:p", "alpha:q", "alpha:ra"],
        &[("?x", "a"), ("?ra", "1")],
    );
    assert_fanout_activation(
        &acts,
        "rule_b",
        &["alpha:p", "alpha:q", "alpha:rb"],
        &[("?x", "a"), ("?rb", "2")],
    );

    // A second p fact (same alpha state, new WME) joins the still-resident
    // q fact: fanout again, once each.
    let acts =
        beta.process_token_from_source("alpha:p", bound_token_vars("alpha:p2", "p", "a", &[]));
    assert_eq!(acts.len(), 2);
    assert_fanout_activation(
        &acts,
        "rule_a",
        &["alpha:p2", "alpha:q", "alpha:ra"],
        &[("?x", "a"), ("?ra", "1")],
    );
    assert_fanout_activation(
        &acts,
        "rule_b",
        &["alpha:p2", "alpha:q", "alpha:rb"],
        &[("?x", "a"), ("?rb", "2")],
    );

    // A second q fact joins both resident p facts: two joined tokens reach
    // each consumer, so each rule activates exactly twice more.
    let acts =
        beta.process_token_from_source("alpha:q", bound_token_vars("alpha:q2", "q", "a", &[]));
    let rule_a: Vec<&PStateActivation> = acts.iter().filter(|a| a.rule_id == "rule_a").collect();
    let rule_b: Vec<&PStateActivation> = acts.iter().filter(|a| a.rule_id == "rule_b").collect();
    assert_eq!(rule_a.len(), 2);
    assert_eq!(rule_b.len(), 2);
    let mut rule_a_ids: Vec<Vec<&str>> = rule_a
        .iter()
        .map(|a| a.token.wmes.iter().map(|w| w.id.as_str()).collect())
        .collect();
    rule_a_ids.sort_unstable();
    assert_eq!(
        rule_a_ids,
        vec![
            vec!["alpha:p", "alpha:q2", "alpha:ra"],
            vec!["alpha:p2", "alpha:q2", "alpha:ra"],
        ]
    );
    let mut rule_b_ids: Vec<Vec<&str>> = rule_b
        .iter()
        .map(|a| a.token.wmes.iter().map(|w| w.id.as_str()).collect())
        .collect();
    rule_b_ids.sort_unstable();
    assert_eq!(
        rule_b_ids,
        vec![
            vec!["alpha:p", "alpha:q2", "alpha:rb"],
            vec!["alpha:p2", "alpha:q2", "alpha:rb"],
        ]
    );

    // j1's memory holds exactly the four joined p×q tokens; the consumers
    // hold one complete token per activation produced so far.
    assert_eq!(beta.states[&j1].beta_memory.len(), 4);
    assert_eq!(beta.states[&ja].beta_memory.len(), 4);
    assert_eq!(beta.states[&jb].beta_memory.len(), 4);
}

fn assert_fanout_activation(
    acts: &[PStateActivation],
    rule_id: &str,
    wme_ids: &[&str],
    bindings: &[(&str, &str)],
) {
    let matches: Vec<&PStateActivation> = acts.iter().filter(|a| a.rule_id == rule_id).collect();
    assert_eq!(matches.len(), 1, "exactly one activation for {rule_id}");
    let ids: Vec<&str> = matches[0]
        .token
        .wmes
        .iter()
        .map(|w| w.id.as_str())
        .collect();
    assert_eq!(ids, wme_ids, "token composition for {rule_id}");
    assert_bindings(&matches[0].token.bindings, bindings);
}

#[test]
fn inconsistent_bindings_produce_no_beta_activation() {
    let mut beta = BetaNetwork::new();
    join_chain(&mut beta, &["alpha:p", "alpha:q", "alpha:r"], "rule3");

    beta.process_token_from_source("alpha:p", bound_token("alpha:p", "p", "a"));
    beta.process_token_from_source("alpha:q", bound_token("alpha:q", "q", "b"));
    let acts = beta.process_token_from_source("alpha:r", bound_token("alpha:r", "r", "a"));
    assert!(acts.is_empty(), "conflicting ?x bindings must not join");

    let acts = beta.process_token_from_source("alpha:q", bound_token("alpha:q2", "q", "a"));
    assert_eq!(acts.len(), 1);
    assert_eq!(acts[0].rule_id, "rule3");
}

#[test]
fn retracted_wme_clears_beta_memories_and_repropagation_refires_once() {
    let mut beta = BetaNetwork::new();
    join_chain(&mut beta, &["alpha:p", "alpha:q", "alpha:r"], "rule3");

    beta.process_token_from_source("alpha:p", bound_token_vars("alpha:p", "p", "v", &[]));
    beta.process_token_from_source(
        "alpha:q",
        bound_token_vars("alpha:q", "q", "v", &[("?qy", "1")]),
    );
    let acts = beta.process_token_from_source(
        "alpha:r",
        bound_token_vars("alpha:r", "r", "v", &[("?ry", "2")]),
    );
    assert_eq!(acts.len(), 1);
    assert_bindings(
        &acts[0].token.bindings,
        &[("?x", "v"), ("?qy", "1"), ("?ry", "2")],
    );

    beta.remove_wme_from_network("alpha:r");
    let acts = beta.process_token_from_source(
        "alpha:r",
        bound_token_vars("alpha:r2", "r", "v", &[("?ry", "2")]),
    );
    assert_eq!(
        acts.len(),
        1,
        "retraction must clear stale memories so repropagation activates exactly once"
    );
    let ids: Vec<&str> = acts[0].token.wmes.iter().map(|w| w.id.as_str()).collect();
    assert_eq!(ids, vec!["alpha:p", "alpha:q", "alpha:r2"]);
    assert_bindings(
        &acts[0].token.bindings,
        &[("?x", "v"), ("?qy", "1"), ("?ry", "2")],
    );
}

#[tokio::test]
async fn identical_condition_rules_sharing_terminal_activate_immediately_with_own_salience() {
    // Two rules whose complete condition sequences are identical share the
    // same terminal join. Both must land in the agenda immediately with
    // their own salience, for every rule insertion order and every arrival
    // order of the condition facts.
    let rules = [("ra", 10i32), ("rb", 3i32)];
    for add_order in [[0usize, 1], [1, 0]] {
        for order in [[0usize, 1, 2], [2, 1, 0], [1, 2, 0], [2, 0, 1]] {
            let net = ReteNetwork::new();
            for &r in &add_order {
                let (id, priority) = rules[r];
                net.add_rule(rule(id, priority, &["p", "q", "r"]))
                    .await
                    .unwrap();
            }
            let facts = [
                fact("fp", "p", "a"),
                fact("fq", "q", "a"),
                fact("fr", "r", "a"),
            ];
            for &i in &order {
                net.assert_fact(facts[i].clone()).await.unwrap();
            }

            let agenda = net.agenda_snapshot().unwrap();
            let context = format!("add order {add_order:?}, arrival {order:?}, agenda {agenda:?}");
            assert_eq!(
                count_for_rule(&agenda, "ra"),
                1,
                "ra must activate immediately: {context}"
            );
            assert_eq!(
                count_for_rule(&agenda, "rb"),
                1,
                "rb must activate immediately: {context}"
            );
            for item in &agenda {
                let expected_salience = if item.rule.id == "ra" { 10 } else { 3 };
                assert_eq!(
                    item.salience, expected_salience,
                    "each rule keeps its own salience: {context}"
                );
                let ids: Vec<&str> = item.wme_list.iter().map(|w| w.id.as_str()).collect();
                assert_eq!(ids, vec!["fp", "fq", "fr"], "agenda item wmes: {context}");
            }

            let consequences = net.fire_all_consequences().unwrap();
            let mut fired = fired_rule_ids(&consequences);
            fired.sort_unstable();
            assert_eq!(fired, vec!["ra", "rb"], "exact firing counts: {context}");
        }
    }
}

#[tokio::test]
async fn shared_terminal_retract_reassert_restores_both_rules_before_firing() {
    let net = ReteNetwork::new();
    net.add_rule(rule("ra", 10, &["p", "q", "r"]))
        .await
        .unwrap();
    net.add_rule(rule("rb", 3, &["p", "q", "r"])).await.unwrap();

    net.assert_fact(fact("fp", "p", "a")).await.unwrap();
    net.assert_fact(fact("fr", "r", "a")).await.unwrap();
    net.assert_fact(fact("fq", "q", "a")).await.unwrap();
    let agenda = net.agenda_snapshot().unwrap();
    assert_eq!(count_for_rule(&agenda, "ra"), 1);
    assert_eq!(count_for_rule(&agenda, "rb"), 1);

    net.retract_fact("fq").await.unwrap();
    let agenda = net.agenda_snapshot().unwrap();
    assert_eq!(
        count_for_rule(&agenda, "ra"),
        0,
        "stale ra activation must not survive retraction"
    );
    assert_eq!(
        count_for_rule(&agenda, "rb"),
        0,
        "stale rb activation must not survive retraction"
    );
    assert!(
        net.fire_all_consequences().unwrap().is_empty(),
        "retracted fact must not fire either rule"
    );

    net.assert_fact(fact("fq2", "q", "a")).await.unwrap();
    let agenda = net.agenda_snapshot().unwrap();
    assert_eq!(
        count_for_rule(&agenda, "ra"),
        1,
        "re-assert must restore ra exactly once"
    );
    assert_eq!(
        count_for_rule(&agenda, "rb"),
        1,
        "re-assert must restore rb exactly once"
    );

    let consequences = net.fire_all_consequences().unwrap();
    let mut fired = fired_rule_ids(&consequences);
    fired.sort_unstable();
    assert_eq!(fired, vec!["ra", "rb"]);
}

#[tokio::test]
async fn shared_terminal_retract_reassert_refires_both_after_firing() {
    let net = ReteNetwork::new();
    net.add_rule(rule("ra", 10, &["p", "q", "r"]))
        .await
        .unwrap();
    net.add_rule(rule("rb", 3, &["p", "q", "r"])).await.unwrap();

    net.assert_fact(fact("fp", "p", "a")).await.unwrap();
    net.assert_fact(fact("fq", "q", "a")).await.unwrap();
    net.assert_fact(fact("fr", "r", "a")).await.unwrap();

    let consequences = net.fire_all_consequences().unwrap();
    let mut fired = fired_rule_ids(&consequences);
    fired.sort_unstable();
    assert_eq!(fired, vec!["ra", "rb"]);
    assert!(
        net.fire_all_consequences().unwrap().is_empty(),
        "dedup must not re-fire without a change"
    );

    net.retract_fact("fq").await.unwrap();
    net.assert_fact(fact("fq2", "q", "a")).await.unwrap();
    let agenda = net.agenda_snapshot().unwrap();
    assert_eq!(count_for_rule(&agenda, "ra"), 1);
    assert_eq!(count_for_rule(&agenda, "rb"), 1);

    let consequences = net.fire_all_consequences().unwrap();
    let mut fired = fired_rule_ids(&consequences);
    fired.sort_unstable();
    assert_eq!(
        fired,
        vec!["ra", "rb"],
        "both rules must re-fire after retract/reassert"
    );
    assert!(net.fire_all_consequences().unwrap().is_empty());
}

#[tokio::test]
async fn shared_terminal_retraction_keeps_unrelated_match_alive() {
    let net = ReteNetwork::new();
    net.add_rule(rule("ra", 10, &["p", "q", "r"]))
        .await
        .unwrap();
    net.add_rule(rule("rb", 3, &["p", "q", "r"])).await.unwrap();

    net.assert_fact(fact("fp1", "p", "a")).await.unwrap();
    net.assert_fact(fact("fq1", "q", "a")).await.unwrap();
    net.assert_fact(fact("fr1", "r", "a")).await.unwrap();
    net.assert_fact(fact("fp2", "p", "b")).await.unwrap();
    net.assert_fact(fact("fq2", "q", "b")).await.unwrap();
    net.assert_fact(fact("fr2", "r", "b")).await.unwrap();

    let agenda = net.agenda_snapshot().unwrap();
    assert_eq!(count_for_rule(&agenda, "ra"), 2);
    assert_eq!(count_for_rule(&agenda, "rb"), 2);

    net.retract_fact("fq1").await.unwrap();
    let agenda = net.agenda_snapshot().unwrap();
    assert_eq!(
        count_for_rule(&agenda, "ra"),
        1,
        "only the ?x=b match survives"
    );
    assert_eq!(
        count_for_rule(&agenda, "rb"),
        1,
        "only the ?x=b match survives"
    );
    for item in &agenda {
        let ids: Vec<&str> = item.wme_list.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, vec!["fp2", "fq2", "fr2"]);
        assert_eq!(item.bindings.get_binding("?x"), Some(&"b".to_string()));
    }

    let consequences = net.fire_all_consequences().unwrap();
    assert_eq!(consequences.len(), 2);
    let mut messages: Vec<&str> = consequences
        .iter()
        .filter_map(|c| c.payload["message"].as_str())
        .collect();
    messages.sort_unstable();
    assert_eq!(messages, vec!["ra fired with b", "rb fired with b"]);
}

#[tokio::test]
async fn prefix_rule_and_longer_rule_share_intermediate_join() {
    // A [p,q] rule's terminal is the same join a [p,q,r] rule uses as an
    // intermediate. Both rules must activate with their own salience for
    // every rule insertion order and every arrival order.
    for long_added_first in [false, true] {
        for order in [[0usize, 1, 2], [2, 1, 0], [1, 2, 0]] {
            let net = ReteNetwork::new();
            if long_added_first {
                net.add_rule(rule("long", 3, &["p", "q", "r"]))
                    .await
                    .unwrap();
                net.add_rule(rule("short", 10, &["p", "q"])).await.unwrap();
            } else {
                net.add_rule(rule("short", 10, &["p", "q"])).await.unwrap();
                net.add_rule(rule("long", 3, &["p", "q", "r"]))
                    .await
                    .unwrap();
            }

            let facts = [
                fact("fp", "p", "a"),
                fact("fq", "q", "a"),
                fact("fr", "r", "a"),
            ];
            for &i in &order {
                net.assert_fact(facts[i].clone()).await.unwrap();
            }

            let agenda = net.agenda_snapshot().unwrap();
            let context = format!(
                "long added first {long_added_first}, arrival {order:?}, agenda {agenda:?}"
            );
            assert_eq!(
                count_for_rule(&agenda, "short"),
                1,
                "prefix rule must activate immediately: {context}"
            );
            assert_eq!(
                count_for_rule(&agenda, "long"),
                1,
                "longer rule must activate immediately: {context}"
            );
            for item in &agenda {
                let expected_salience = if item.rule.id == "short" { 10 } else { 3 };
                assert_eq!(
                    item.salience, expected_salience,
                    "salience per rule: {context}"
                );
            }

            let consequences = net.fire_all_consequences().unwrap();
            let mut fired = fired_rule_ids(&consequences);
            fired.sort_unstable();
            assert_eq!(
                fired,
                vec!["long", "short"],
                "exact firing counts: {context}"
            );
        }
    }
}

#[tokio::test]
async fn original_issue_two_variable_rule_recompletes_after_retract_and_swap() {
    // Issue #159 shape: A(?x), B(?x,?r), C(?x,?r). After C(r1) is retracted,
    // C(r2) alone must not match the resident B(r1) partial, and B(r2)
    // arriving last must complete the rule immediately with ?x=a, ?r=r2.
    let net = ReteNetwork::new();
    net.add_rule(Rule {
        id: "issue159".into(),
        priority: 5,
        conditions: vec![
            var_condition("A"),
            two_var_condition("B"),
            two_var_condition("C"),
        ],
        actions: vec![Action {
            action_type: "log".into(),
            params: vec!["issue159 fired x=?x r=?r".into()],
            ..Default::default()
        }],
    })
    .await
    .unwrap();

    net.assert_fact(fact("fa", "A", "a")).await.unwrap();
    net.assert_fact(fact2("fb1", "B", "a", "r1")).await.unwrap();
    net.assert_fact(fact2("fc1", "C", "a", "r1")).await.unwrap();

    let agenda = net.agenda_snapshot().unwrap();
    assert_eq!(count_for_rule(&agenda, "issue159"), 1);
    assert_eq!(agenda[0].bindings.get_binding("?x"), Some(&"a".to_string()));
    assert_eq!(
        agenda[0].bindings.get_binding("?r"),
        Some(&"r1".to_string())
    );
    let ids: Vec<&str> = agenda[0].wme_list.iter().map(|w| w.id.as_str()).collect();
    assert_eq!(ids, vec!["fa", "fb1", "fc1"]);
    assert_eq!(net.fire_all_consequences().unwrap().len(), 1);

    net.retract_fact("fc1").await.unwrap();
    let agenda = net.agenda_snapshot().unwrap();
    assert_eq!(
        count_for_rule(&agenda, "issue159"),
        0,
        "retracting C(r1) must remove the match"
    );
    assert!(net.fire_all_consequences().unwrap().is_empty());

    net.assert_fact(fact2("fc2", "C", "a", "r2")).await.unwrap();
    let agenda = net.agenda_snapshot().unwrap();
    assert_eq!(
        count_for_rule(&agenda, "issue159"),
        0,
        "?r=r2 must conflict with the resident B(r1) partial match"
    );

    net.assert_fact(fact2("fb2", "B", "a", "r2")).await.unwrap();
    let agenda = net.agenda_snapshot().unwrap();
    assert_eq!(
        count_for_rule(&agenda, "issue159"),
        1,
        "B(r2) arriving last must complete the rule immediately"
    );
    assert_eq!(agenda[0].bindings.get_binding("?x"), Some(&"a".to_string()));
    assert_eq!(
        agenda[0].bindings.get_binding("?r"),
        Some(&"r2".to_string())
    );
    let ids: Vec<&str> = agenda[0].wme_list.iter().map(|w| w.id.as_str()).collect();
    assert_eq!(ids, vec!["fa", "fb2", "fc2"]);

    let consequences = net.fire_all_consequences().unwrap();
    assert_eq!(consequences.len(), 1);
    assert_eq!(
        consequences[0].payload["message"],
        "issue159 fired x=a r=r2"
    );
}

#[test]
fn mark_as_p_state_on_a_shared_state_keeps_last_mark_wins() {
    // Compatibility pin: BetaState.p_state is a singular public field and
    // mark_as_p_state overwrites it — the LAST mark for a state wins and
    // only that rule activates from the shared state. Per-rule terminals
    // come from the rule-wiring layer (isolated terminal joins per rule),
    // not from making this low-level setter additive.
    let mut beta = BetaNetwork::new();
    let mut current = "alpha:p".to_string();
    for (index, next) in ["alpha:q", "alpha:r"].iter().enumerate() {
        current = beta.get_or_create_join(current, (*next).to_string(), format!("join{index}"));
    }
    beta.mark_as_p_state(&current, "rule_a", 9);
    beta.mark_as_p_state(&current, "rule_b", 2);

    beta.process_token_from_source("alpha:p", bound_token_vars("alpha:p", "p", "a", &[]));
    beta.process_token_from_source(
        "alpha:q",
        bound_token_vars("alpha:q", "q", "a", &[("?qy", "1")]),
    );
    let acts = beta.process_token_from_source(
        "alpha:r",
        bound_token_vars("alpha:r", "r", "a", &[("?ry", "2")]),
    );

    assert_eq!(
        acts.len(),
        1,
        "a shared state activates only its last-marked rule"
    );
    assert_eq!(acts[0].rule_id, "rule_b");
    assert_eq!(acts[0].salience, 2);
    let ids: Vec<&str> = acts[0].token.wmes.iter().map(|w| w.id.as_str()).collect();
    assert_eq!(ids, vec!["alpha:p", "alpha:q", "alpha:r"]);
    assert_bindings(
        &acts[0].token.bindings,
        &[("?x", "a"), ("?qy", "1"), ("?ry", "2")],
    );
    let p_state = beta.states[&current].p_state.as_ref().expect("p_state set");
    assert_eq!(
        p_state.rule_id, "rule_b",
        "singular p_state keeps the last mark"
    );
    assert_eq!(p_state.salience, 2);
}

#[test]
fn shared_join_retraction_removes_dependent_matches_and_keeps_independent_one() {
    let mut beta = BetaNetwork::new();
    let j1 = beta.add_join("alpha:p".into(), "alpha:q".into(), "j0".into());
    let ja = beta.add_join(j1.clone(), "alpha:ra".into(), "ja".into());
    let jb = beta.add_join(j1.clone(), "alpha:rb".into(), "jb".into());
    beta.mark_as_p_state(&ja, "rule_a", 1);
    beta.mark_as_p_state(&jb, "rule_b", 1);

    // Two independent matches over the shared join: ?x=a and ?x=b. Each
    // alpha state is one source; the second match rides the same sources
    // with distinct WME IDs.
    beta.process_token_from_source(
        "alpha:ra",
        bound_token_vars("alpha:ra", "ra", "a", &[("?ra", "1")]),
    );
    beta.process_token_from_source(
        "alpha:rb",
        bound_token_vars("alpha:rb", "rb", "a", &[("?rb", "2")]),
    );
    beta.process_token_from_source(
        "alpha:ra",
        bound_token_vars("alpha:ra2", "ra", "b", &[("?ra", "1")]),
    );
    beta.process_token_from_source(
        "alpha:rb",
        bound_token_vars("alpha:rb2", "rb", "b", &[("?rb", "2")]),
    );
    beta.process_token_from_source("alpha:p", bound_token_vars("alpha:p", "p", "a", &[]));
    let acts =
        beta.process_token_from_source("alpha:q", bound_token_vars("alpha:q", "q", "a", &[]));
    assert_eq!(acts.len(), 2);
    beta.process_token_from_source("alpha:p", bound_token_vars("alpha:p2", "p", "b", &[]));
    let acts =
        beta.process_token_from_source("alpha:q", bound_token_vars("alpha:q2", "q", "b", &[]));
    assert_eq!(acts.len(), 2);

    assert_eq!(beta.states[&ja].beta_memory.len(), 2);
    assert_eq!(beta.states[&jb].beta_memory.len(), 2);

    beta.remove_wme_from_network("alpha:q");
    let ja_ids: Vec<Vec<&str>> = beta.states[&ja]
        .beta_memory
        .iter()
        .map(|t| t.wmes.iter().map(|w| w.id.as_str()).collect())
        .collect();
    assert_eq!(
        ja_ids,
        vec![vec!["alpha:p2", "alpha:q2", "alpha:ra2"]],
        "the dependent ?x=a match must disappear from rule_a's terminal"
    );
    let jb_ids: Vec<Vec<&str>> = beta.states[&jb]
        .beta_memory
        .iter()
        .map(|t| t.wmes.iter().map(|w| w.id.as_str()).collect())
        .collect();
    assert_eq!(
        jb_ids,
        vec![vec!["alpha:p2", "alpha:q2", "alpha:rb2"]],
        "the dependent ?x=a match must disappear from rule_b's terminal"
    );
    assert_eq!(beta.states[&j1].beta_memory.len(), 1);

    let acts =
        beta.process_token_from_source("alpha:q", bound_token_vars("alpha:q3", "q", "a", &[]));
    assert_eq!(
        acts.len(),
        2,
        "re-asserting the retracted support must refire each consumer exactly once"
    );
    assert_fanout_activation(
        &acts,
        "rule_a",
        &["alpha:p", "alpha:q3", "alpha:ra"],
        &[("?x", "a"), ("?ra", "1")],
    );
    assert_fanout_activation(
        &acts,
        "rule_b",
        &["alpha:p", "alpha:q3", "alpha:rb"],
        &[("?x", "a"), ("?rb", "2")],
    );
}

#[tokio::test]
async fn rules_added_after_preloaded_facts_activate_on_next_completing_fact() {
    // Facts asserted before any rule exists never enter the beta network
    // (alpha states are created at add_rule time), so they must not
    // retroactively activate anything. Once the rules are wired, the next
    // completing facts must activate BOTH identical-sequence rules
    // immediately — the isolated per-rule terminals receive live traffic.
    let net = ReteNetwork::new();
    net.assert_fact(fact("fp", "p", "a")).await.unwrap();
    net.assert_fact(fact("fq", "q", "a")).await.unwrap();
    net.assert_fact(fact("fr", "r", "a")).await.unwrap();
    net.assert_fact(fact("fz", "z", "a")).await.unwrap();

    net.add_rule(rule("ra", 10, &["p", "q", "r"]))
        .await
        .unwrap();
    net.add_rule(rule("rb", 3, &["p", "q", "r"])).await.unwrap();

    let agenda = net.agenda_snapshot().unwrap();
    assert_eq!(
        count_for_rule(&agenda, "ra"),
        0,
        "preloaded facts must not retroactively activate a rule (agenda {agenda:?})"
    );
    assert_eq!(count_for_rule(&agenda, "rb"), 0);

    net.retract_fact("fz").await.unwrap();
    let agenda = net.agenda_snapshot().unwrap();
    assert_eq!(count_for_rule(&agenda, "ra"), 0);
    assert_eq!(count_for_rule(&agenda, "rb"), 0);

    net.assert_fact(fact("fp2", "p", "a")).await.unwrap();
    net.assert_fact(fact("fq2", "q", "a")).await.unwrap();
    net.assert_fact(fact("fr2", "r", "a")).await.unwrap();
    let agenda = net.agenda_snapshot().unwrap();
    assert_eq!(
        count_for_rule(&agenda, "ra"),
        1,
        "the next completing facts must activate ra: {agenda:?}"
    );
    assert_eq!(
        count_for_rule(&agenda, "rb"),
        1,
        "the next completing facts must activate rb: {agenda:?}"
    );
}

#[tokio::test]
async fn rule_added_after_complete_match_surfaces_match_through_rescan() {
    // ra matches first; rb (identical conditions) is inserted afterwards and
    // receives an isolated terminal seeded with the already-matched token,
    // so an unrelated retraction's rescan surfaces rb's activation exactly
    // as the previously shared terminal did.
    let net = ReteNetwork::new();
    net.assert_fact(fact("fz", "z", "a")).await.unwrap();
    net.add_rule(rule("ra", 10, &["p", "q", "r"]))
        .await
        .unwrap();
    net.assert_fact(fact("fp", "p", "a")).await.unwrap();
    net.assert_fact(fact("fq", "q", "a")).await.unwrap();
    net.assert_fact(fact("fr", "r", "a")).await.unwrap();
    assert_eq!(count_for_rule(&net.agenda_snapshot().unwrap(), "ra"), 1);

    net.add_rule(rule("rb", 3, &["p", "q", "r"])).await.unwrap();
    assert_eq!(
        count_for_rule(&net.agenda_snapshot().unwrap(), "rb"),
        0,
        "insertion alone must not duplicate activations"
    );

    net.retract_fact("fz").await.unwrap();
    let agenda = net.agenda_snapshot().unwrap();
    assert_eq!(
        count_for_rule(&agenda, "rb"),
        1,
        "rescan must surface rb's seeded pre-existing match: {agenda:?}"
    );
    assert_eq!(
        count_for_rule(&agenda, "ra"),
        1,
        "ra keeps its original activation (refraction, no duplicate)"
    );

    let consequences = net.fire_all_consequences().unwrap();
    let mut fired = fired_rule_ids(&consequences);
    fired.sort_unstable();
    assert_eq!(fired, vec!["ra", "rb"]);
}

#[tokio::test]
async fn longer_rule_added_after_prefix_match_activates_when_final_condition_arrives() {
    // The [p,q] prefix already matched before the [p,q,r] rule was inserted;
    // the isolated terminal must inherit the prefix's produced tokens so the
    // final condition completes the longer rule immediately.
    let net = ReteNetwork::new();
    net.add_rule(rule("short", 10, &["p", "q"])).await.unwrap();
    net.assert_fact(fact("fp", "p", "a")).await.unwrap();
    net.assert_fact(fact("fq", "q", "a")).await.unwrap();
    assert_eq!(count_for_rule(&net.agenda_snapshot().unwrap(), "short"), 1);

    net.add_rule(rule("long", 3, &["p", "q", "r"]))
        .await
        .unwrap();
    net.assert_fact(fact("fr", "r", "a")).await.unwrap();

    let agenda = net.agenda_snapshot().unwrap();
    assert_eq!(
        count_for_rule(&agenda, "long"),
        1,
        "the longer rule must complete over the pre-existing prefix match: {agenda:?}"
    );
    assert_eq!(
        count_for_rule(&agenda, "short"),
        1,
        "the prefix rule keeps exactly its original activation"
    );

    let consequences = net.fire_all_consequences().unwrap();
    let mut fired = fired_rule_ids(&consequences);
    fired.sort_unstable();
    assert_eq!(fired, vec!["long", "short"]);
}
