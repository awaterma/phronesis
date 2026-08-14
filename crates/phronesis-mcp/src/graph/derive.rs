//! Derived facts: whole-graph computations the engine cannot express.
//!
//! The engine has no negation-as-failure at the pattern level and no forward
//! chaining, so "no_direct_test" (closed-world negation) and "in_cycle" (transitive
//! closure) are computed here instead (spec §4.5).
//!
//! Both are pure functions of the edge set — no source parsing, no I/O — which
//! is why they can run on *every* save without reparsing the repository.

use super::model::Edge;
use std::collections::{BTreeMap, BTreeSet};

/// Replace extractor-local bare `tested_by` callees with canonical
/// `defines_fn` identities using only same-module or explicit-import evidence.
/// Unresolved and ambiguous calls are discarded rather than attributed to
/// every definition sharing a leaf name.
pub fn canonicalize_tested_by(base: &mut Vec<Edge>) {
    let definitions = base_edges(base, "defines_fn")
        .filter_map(|edge| edge.a.get(1).cloned())
        .collect::<BTreeSet<_>>();
    let by_leaf = definitions.iter().fold(
        BTreeMap::<String, BTreeSet<String>>::new(),
        |mut map, definition| {
            let leaf = definition.rsplit("::").next().unwrap_or(definition);
            map.entry(leaf.to_string())
                .or_default()
                .insert(definition.clone());
            map
        },
    );
    let imports = base_edges(base, "imports").fold(
        BTreeMap::<String, BTreeSet<String>>::new(),
        |mut map, edge| {
            if let (Some(from), Some(to)) = (edge.a.first(), edge.a.get(1)) {
                map.entry(from.clone()).or_default().insert(to.clone());
            }
            map
        },
    );

    let mut normalized = Vec::with_capacity(base.len());
    for mut edge in base.drain(..) {
        if edge.p != "tested_by" || edge.a.len() != 2 {
            normalized.push(edge);
            continue;
        }
        let callee = &edge.a[0];
        if definitions.contains(callee) {
            normalized.push(edge);
            continue;
        }
        let test_module = edge.a[1].rsplit_once("::").map(|(module, _)| module);
        let Some(candidates) = by_leaf.get(callee) else {
            continue;
        };
        let resolved = candidates
            .iter()
            .filter(|candidate| {
                let Some((module, _)) = candidate.rsplit_once("::") else {
                    return false;
                };
                test_module.is_some_and(|test_module| {
                    test_module == module
                        || test_module
                            .strip_prefix(module)
                            .is_some_and(|suffix| suffix.starts_with("::"))
                }) || test_module
                    .and_then(|test_module| imports.get(test_module))
                    .is_some_and(|targets| targets.contains(module))
            })
            .collect::<Vec<_>>();
        if resolved.len() == 1 {
            edge.a[0] = (*resolved[0]).clone();
            normalized.push(edge);
        }
    }
    *base = normalized;
}

/// Compute all derived edges over a complete base-edge set.
pub fn derive_all(base: &[Edge]) -> Vec<Edge> {
    let mut out = inventory(base);
    out.extend(data_flows_to(base));
    out.extend(configuration_lifecycle_gaps(base));
    out.extend(rhai_reachability(base));
    out.extend(rhai_predicate_flow(base));
    out.extend(no_direct_test(base));
    out.extend(in_cycle(base));
    out
}

pub fn rhai_predicate_flow(base: &[Edge]) -> Vec<Edge> {
    let emitted: BTreeMap<&str, BTreeSet<&str>> = base_edges(base, "rhai_emits_predicate")
        .filter_map(|edge| Some((edge.a.get(1)?.as_str(), edge.a.first()?.as_str())))
        .fold(BTreeMap::new(), |mut map, (predicate, script)| {
            map.entry(predicate).or_default().insert(script);
            map
        });
    let mut out = BTreeSet::new();
    for edge in base_edges(base, "rule_uses_predicate") {
        let (Some(rule), Some(predicate)) = (edge.a.first(), edge.a.get(1)) else {
            continue;
        };
        if let Some(scripts) = emitted.get(predicate.as_str()) {
            for script in scripts {
                out.insert((script.to_string(), predicate.clone(), rule.clone()));
            }
        }
    }
    out.into_iter()
        .map(|(script, predicate, rule)| {
            Edge::derived("rhai_implements_predicate", &[&script, &predicate, &rule])
        })
        .collect()
}

pub fn rhai_reachability(base: &[Edge]) -> Vec<Edge> {
    let mut registered_names = BTreeSet::new();
    for edge in base_edges(base, "exposes") {
        if let Some(callable) = edge.a.get(1)
            && callable.starts_with("rhai:callable::")
        {
            registered_names.insert(callable.as_str());
        }
    }
    let mut definitions: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for edge in base_edges(base, "defines_fn") {
        if let Some(function) = edge.a.get(1) {
            let name = function.rsplit("::").next().unwrap_or(function);
            definitions.entry(name).or_default().insert(function);
        }
    }
    let backing_names: BTreeMap<&str, BTreeSet<&str>> = base_edges(base, "rhai_callable_backing")
        .filter_map(|edge| Some((edge.a.first()?.as_str(), edge.a.get(1)?.as_str())))
        .fold(BTreeMap::new(), |mut map, (callable, backing)| {
            map.entry(callable).or_default().insert(backing);
            map
        });
    let mut out = BTreeSet::new();
    for edge in base_edges(base, "calls") {
        let (Some(script), Some(callable)) = (edge.a.first(), edge.a.get(1)) else {
            continue;
        };
        if !callable.starts_with("rhai:callable::") || !registered_names.contains(callable.as_str())
        {
            continue;
        }
        let exposed_name = callable
            .strip_prefix("rhai:callable::")
            .expect("checked Rhai callable prefix");
        let candidates = match backing_names.get(callable.as_str()) {
            Some(names) if names.len() == 1 => *names.first().expect("one backing name"),
            Some(_) => {
                out.insert((
                    "rhai_binding_diagnostic",
                    vec![script.as_str(), callable.as_str(), "ambiguous_backing"],
                ));
                continue;
            }
            None => exposed_name,
        };
        match definitions.get(candidates) {
            Some(functions) if functions.len() == 1 => {
                let function = *functions.first().expect("one registration");
                out.insert(("resolves_to", vec![callable.as_str(), function]));
                out.insert(("runtime_reachable", vec![function, script.as_str()]));
            }
            Some(_) => {
                out.insert((
                    "rhai_binding_diagnostic",
                    vec![script.as_str(), callable.as_str(), "ambiguous"],
                ));
            }
            None => {
                // Registration proves that the script call is bound, but a
                // closure or differently named host implementation prevents
                // a sound Rust-function identity.
            }
        }
    }
    out.into_iter()
        .map(|(predicate, args)| Edge::derived(predicate, &args))
        .collect()
}

/// Closed-world lifecycle evidence for project-specific policy. These facts
/// are derived centrally because RETE conditions intentionally have no
/// negation-as-failure. Starter packs do not warn on them: deployment outputs
/// and hand-authored configuration make the policy project-dependent.
pub fn configuration_lifecycle_gaps(base: &[Edge]) -> Vec<Edge> {
    let generated = base_edges(base, "generates")
        .filter_map(|edge| edge.a.get(1).map(String::as_str))
        .collect::<BTreeSet<_>>();
    let consumed = base_edges(base, "consumes_data")
        .filter_map(|edge| edge.a.get(1).map(String::as_str))
        .collect::<BTreeSet<_>>();
    let mut out = Vec::new();
    for artifact in generated.difference(&consumed) {
        out.push(Edge::derived("generated_without_consumer", &[artifact]));
    }
    for artifact in consumed.difference(&generated) {
        out.push(Edge::derived("consumed_without_producer", &[artifact]));
    }
    out
}

/// `data_flows_to(artifact_module, consumer)` is the navigational inverse of
/// `deserializes(consumer, artifact_module)`. Keeping both
/// preserves the precise consumer claim while making producer -> artifact ->
/// consumer flow follow one direction in graph renderers and queries.
pub fn data_flows_to(base: &[Edge]) -> Vec<Edge> {
    base.iter()
        .filter(|edge| !edge.d && matches!(edge.p.as_str(), "consumes_data" | "deserializes"))
        .filter_map(|edge| Some((edge.a.first()?, edge.a.get(1)?)))
        .map(|(consumer, artifact)| (artifact.as_str(), consumer.as_str()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|(artifact, consumer)| Edge::derived("data_flows_to", &[artifact, consumer]))
        .collect()
}

/// Materialize the positive unary inventory promised by graph format 5 and
/// containment for callable elements.  These facts are mechanical projections
/// of the older binary relations, so deriving them centrally keeps every
/// language extractor on the same contract.
pub fn inventory(base: &[Edge]) -> Vec<Edge> {
    let mut out: BTreeSet<(String, Vec<String>)> = BTreeSet::new();
    let mut modules_by_file: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();

    for edge in base_edges(base, "file_type") {
        if let Some(file) = edge.a.first() {
            out.insert(("graph_file".into(), vec![file.clone()]));
        }
    }
    for edge in base_edges(base, "declares_module") {
        if let (Some(file), Some(module)) = (edge.a.first(), edge.a.get(1)) {
            out.insert(("graph_file".into(), vec![file.clone()]));
            out.insert(("graph_module".into(), vec![module.clone()]));
            modules_by_file
                .entry(file.as_str())
                .or_default()
                .insert(module.as_str());
        }
    }
    for edge in base_edges(base, "defines_fn") {
        if let (Some(file), Some(function)) = (edge.a.first(), edge.a.get(1)) {
            out.insert(("graph_function".into(), vec![function.clone()]));
            out.insert((
                "element_in_file".into(),
                vec![function.clone(), file.clone()],
            ));
            for module in modules_by_file.get(file.as_str()).into_iter().flatten() {
                out.insert((
                    "element_in_module".into(),
                    vec![function.clone(), (*module).to_string()],
                ));
            }
        }
    }
    for edge in base_edges(base, "tested_by") {
        if let Some(test) = edge.a.get(1) {
            out.insert(("graph_function".into(), vec![test.clone()]));
            out.insert(("graph_test".into(), vec![test.clone()]));
            if !edge.src.is_empty() {
                out.insert((
                    "element_in_file".into(),
                    vec![test.clone(), edge.src.clone()],
                ));
                for module in modules_by_file.get(edge.src.as_str()).into_iter().flatten() {
                    out.insert((
                        "element_in_module".into(),
                        vec![test.clone(), (*module).to_string()],
                    ));
                }
            }
        }
    }

    out.into_iter()
        .map(|(predicate, args)| {
            Edge::derived(
                &predicate,
                &args.iter().map(String::as_str).collect::<Vec<_>>(),
            )
        })
        .collect()
}

/// Iterate base (non-derived) edges of one relation. Pre-existing derived
/// edges are ignored so that a stale `untested` cannot feed back into the
/// next derivation and pin itself in place.
fn base_edges<'a>(base: &'a [Edge], p: &'a str) -> impl Iterator<Item = &'a Edge> {
    base.iter().filter(move |e| !e.d && e.p == p)
}

/// `no_direct_test(F)` for every `F` in `defines_fn` with no `tested_by`
/// edge naming it.
pub fn no_direct_test(base: &[Edge]) -> Vec<Edge> {
    let covered: BTreeSet<&str> = base_edges(base, "tested_by")
        .filter_map(|e| e.a.first())
        .map(String::as_str)
        .collect();
    let production_files = base_edges(base, "file_type")
        .filter_map(|edge| {
            (edge.a.get(1).map(String::as_str) == Some("production"))
                .then(|| edge.a.first().map(String::as_str))
                .flatten()
        })
        .collect::<BTreeSet<_>>();

    base_edges(base, "defines_fn")
        .filter(|edge| {
            production_files.is_empty()
                || edge
                    .a
                    .first()
                    .is_some_and(|file| production_files.contains(file.as_str()))
        })
        .filter_map(|e| e.a.get(1))
        .map(String::as_str)
        .filter(|f| !covered.contains(f))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|f| Edge::derived("no_direct_test", &[f]))
        .collect()
}

/// `in_cycle(M, C)` for every module in a non-trivial SCC of the `imports`
/// graph, via Tarjan. `C` is a stable cycle identifier.
///
/// Stability matters: the cycle id lands in user-visible rule output, so it
/// must not churn as unrelated edges are added or reordered. Naming the cycle
/// after its lexicographically smallest member makes the id a function of the
/// cycle's contents alone.
pub fn in_cycle(base: &[Edge]) -> Vec<Edge> {
    // Adjacency over sorted collections, so traversal order — and therefore
    // the SCC grouping — does not depend on edge insertion order.
    let mut adj: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for e in base_edges(base, "imports") {
        let (Some(from), Some(to)) = (e.a.first(), e.a.get(1)) else {
            continue;
        };
        adj.entry(from.as_str()).or_default().insert(to.as_str());
        adj.entry(to.as_str()).or_default();
    }

    let mut out = Vec::new();
    for scc in tarjan_sccs(&adj) {
        let Some(&first) = scc.first() else { continue };
        // A lone node is a cycle only if it imports itself.
        let is_cycle = scc.len() > 1 || adj.get(first).is_some_and(|succ| succ.contains(&first));
        if !is_cycle {
            continue;
        }
        let cycle_id = format!("cycle:{first}");
        for m in scc {
            out.push(Edge::derived("in_cycle", &[m, &cycle_id]));
        }
    }
    out
}

/// Per-node Tarjan bookkeeping.
#[derive(Clone, Copy)]
struct Meta {
    index: usize,
    lowlink: usize,
    on_stack: bool,
}

/// Iterative Tarjan SCC. Iterative rather than recursive because import graphs
/// in generated or vendored trees can be deep enough to blow the stack, and a
/// hook that panics takes enforcement offline.
///
/// Each returned SCC is sorted, and the SCC list is sorted, so the output is a
/// deterministic function of the graph alone.
fn tarjan_sccs<'a>(adj: &BTreeMap<&'a str, BTreeSet<&'a str>>) -> Vec<Vec<&'a str>> {
    let mut meta: BTreeMap<&str, Meta> = BTreeMap::new();
    let mut stack: Vec<&str> = Vec::new();
    let mut next_index = 0usize;
    let mut sccs: Vec<Vec<&str>> = Vec::new();

    /// Lower `node`'s lowlink, ignoring nodes we have not visited. Every
    /// caller has already inserted `node`, so absence is a no-op rather than
    /// an error worth propagating out of a hook.
    fn relax(meta: &mut BTreeMap<&str, Meta>, node: &str, candidate: usize) {
        if let Some(m) = meta.get_mut(node) {
            m.lowlink = m.lowlink.min(candidate);
        }
    }

    for &root in adj.keys() {
        if meta.contains_key(root) {
            continue;
        }
        meta.insert(
            root,
            Meta {
                index: next_index,
                lowlink: next_index,
                on_stack: true,
            },
        );
        next_index += 1;
        stack.push(root);
        // (node, index of the next successor to visit)
        let mut work: Vec<(&str, usize)> = vec![(root, 0)];

        while let Some(&mut (node, ref mut succ_idx)) = work.last_mut() {
            let next = adj.get(node).and_then(|s| s.iter().nth(*succ_idx)).copied();
            let Some(w) = next else {
                // Node exhausted: close it out.
                work.pop();
                let Some(&nm) = meta.get(node) else { continue };
                if nm.lowlink == nm.index {
                    let mut scc = Vec::new();
                    while let Some(popped) = stack.pop() {
                        if let Some(m) = meta.get_mut(popped) {
                            m.on_stack = false;
                        }
                        scc.push(popped);
                        if popped == node {
                            break;
                        }
                    }
                    scc.sort_unstable();
                    sccs.push(scc);
                }
                if let Some(&(parent, _)) = work.last() {
                    relax(&mut meta, parent, nm.lowlink);
                }
                continue;
            };

            *succ_idx += 1;
            match meta.get(w).copied() {
                None => {
                    meta.insert(
                        w,
                        Meta {
                            index: next_index,
                            lowlink: next_index,
                            on_stack: true,
                        },
                    );
                    next_index += 1;
                    stack.push(w);
                    work.push((w, 0));
                }
                Some(m) if m.on_stack => relax(&mut meta, node, m.index),
                Some(_) => {}
            }
        }
    }

    sccs.sort_unstable();
    sccs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defines(file: &str, func: &str) -> Edge {
        Edge::base("defines_fn", &[file, func], file)
    }
    fn tested(func: &str, test: &str) -> Edge {
        Edge::base("tested_by", &[func, test], "tests/t.rs")
    }
    fn imports(from: &str, to: &str) -> Edge {
        Edge::base("imports", &[from, to], from)
    }

    #[test]
    fn unique_rhai_registration_makes_the_rust_proxy_runtime_reachable() {
        let base = vec![
            defines("src/bridge.rs", "rust:game::state_attempt_stunning_strike"),
            Edge::base(
                "exposes",
                &["rust:game::bridge", "rhai:callable::stunning_strike"],
                "src/bridge.rs",
            ),
            Edge::base(
                "rhai_callable_backing",
                &[
                    "rhai:callable::stunning_strike",
                    "state_attempt_stunning_strike",
                ],
                "src/bridge.rs",
            ),
            Edge::base(
                "calls",
                &["rhai:game::combat", "rhai:callable::stunning_strike"],
                "scripts/combat.rhai",
            ),
        ];
        let out = rhai_reachability(&base);
        assert!(out.iter().any(|edge| {
            edge.p == "runtime_reachable"
                && edge.a
                    == [
                        "rust:game::state_attempt_stunning_strike",
                        "rhai:game::combat",
                    ]
        }));
        assert!(out.iter().any(|edge| edge.p == "resolves_to"));
    }

    #[test]
    fn ambiguous_rhai_registration_is_diagnostic_not_reachability() {
        let base = vec![
            defines("src/a.rs", "rust:game::a::proxy"),
            defines("src/b.rs", "rust:game::b::proxy"),
            Edge::base(
                "exposes",
                &["rust:game::bridge", "rhai:callable::proxy"],
                "src/a.rs",
            ),
            Edge::base("calls", &["rhai:script", "rhai:callable::proxy"], "x.rhai"),
        ];
        let out = rhai_reachability(&base);
        assert!(!out.iter().any(|edge| edge.p == "runtime_reachable"));
        assert!(
            out.iter()
                .any(|edge| { edge.p == "rhai_binding_diagnostic" && edge.a[2] == "ambiguous" })
        );
    }

    #[test]
    fn rhai_provider_predicate_connects_to_the_rule_that_consumes_it() {
        let base = vec![
            Edge::base(
                "rhai_emits_predicate",
                &["rhai:project::change_set", "production_without_test"],
                ".phronesis/predicates/change_set.rhai",
            ),
            Edge::base(
                "rule_uses_predicate",
                &["require-tests", "production_without_test"],
                ".phronesis/rules.json",
            ),
        ];
        assert_eq!(
            rhai_predicate_flow(&base)[0].a,
            [
                "rhai:project::change_set",
                "production_without_test",
                "require-tests"
            ]
        );
    }

    #[test]
    fn inventory_projects_modules_functions_tests_and_containment() {
        let base = vec![
            Edge::base("file_type", &["src/lib.rs", "production"], "src/lib.rs"),
            Edge::base("declares_module", &["src/lib.rs", "rust:app"], "src/lib.rs"),
            defines("src/lib.rs", "rust:app::run"),
            Edge::base("file_type", &["tests/run.rs", "test"], "tests/run.rs"),
            Edge::base(
                "declares_module",
                &["tests/run.rs", "rust:app#test:run"],
                "tests/run.rs",
            ),
            tested("rust:app::run", "rust:app#test:run::works"),
        ];

        let out = inventory(&base);
        assert!(
            out.iter()
                .any(|e| e.p == "graph_file" && e.a == ["src/lib.rs"])
        );
        assert!(
            out.iter()
                .any(|e| e.p == "graph_module" && e.a == ["rust:app"])
        );
        assert!(
            out.iter()
                .any(|e| e.p == "graph_function" && e.a == ["rust:app::run"])
        );
        assert!(
            out.iter()
                .any(|e| e.p == "graph_test" && e.a == ["rust:app#test:run::works"])
        );
        assert!(
            out.iter()
                .any(|e| { e.p == "element_in_module" && e.a == ["rust:app::run", "rust:app"] })
        );
    }

    fn args_of<'a>(edges: &'a [Edge], p: &str) -> Vec<&'a Vec<String>> {
        edges.iter().filter(|e| e.p == p).map(|e| &e.a).collect()
    }

    // ─── untested ───────────────────────────────────────────────────

    #[test]
    fn function_with_no_test_edge_is_untested() {
        let out = no_direct_test(&[defines("a.rs", "crate::a")]);
        assert_eq!(args_of(&out, "no_direct_test").len(), 1);
        assert_eq!(out[0].a, vec!["crate::a"]);
    }

    #[test]
    fn helpers_defined_in_test_files_are_not_production_test_gaps() {
        let base = vec![
            defines("src/lib.rs", "rust:app::run"),
            defines("tests/common.rs", "rust:app#test:common::fixture"),
            Edge::base("file_type", &["src/lib.rs", "production"], "src/lib.rs"),
            Edge::base("file_type", &["tests/common.rs", "test"], "tests/common.rs"),
        ];
        let out = no_direct_test(&base);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].a, ["rust:app::run"]);
    }

    #[test]
    fn function_with_a_test_edge_is_not_untested() {
        let base = vec![defines("a.rs", "crate::a"), tested("crate::a", "t::ta")];
        assert!(no_direct_test(&base).is_empty());
    }

    #[test]
    fn untested_edges_are_marked_derived() {
        let out = no_direct_test(&[defines("a.rs", "crate::a")]);
        assert!(out.iter().all(|e| e.d && e.src.is_empty()));
    }

    #[test]
    fn a_test_edge_covers_only_its_own_function() {
        let base = vec![
            defines("a.rs", "crate::a"),
            defines("a.rs", "crate::b"),
            tested("crate::a", "t::ta"),
        ];
        let out = no_direct_test(&base);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].a, vec!["crate::b"]);
    }

    #[test]
    fn a_test_edge_from_another_file_still_covers_the_function() {
        // Coverage is whole-repo: this is why derivation cannot be per-file.
        let base = vec![
            defines("a.rs", "crate::a"),
            tested("crate::a", "t::elsewhere"),
        ];
        assert!(no_direct_test(&base).is_empty());
    }

    #[test]
    fn an_imported_test_call_is_canonicalized_before_coverage_derivation() {
        let mut base = vec![
            defines("a.rs", "rust:app::a::fire"),
            imports("rust:app#test:integration", "rust:app::a"),
            tested("fire", "rust:app#test:integration::test_fire"),
        ];
        canonicalize_tested_by(&mut base);
        assert_eq!(args_of(&base, "tested_by")[0][0], "rust:app::a::fire");
        assert!(no_direct_test(&base).is_empty());
    }

    #[test]
    fn an_ambiguous_bare_test_call_is_not_guessed() {
        let mut base = vec![
            defines("a.rs", "rust:app::a::fire"),
            defines("b.rs", "rust:app::b::fire"),
            tested("fire", "rust:app#test:integration::test_fire"),
        ];
        canonicalize_tested_by(&mut base);
        assert!(args_of(&base, "tested_by").is_empty());
        assert_eq!(no_direct_test(&base).len(), 2);
    }

    #[test]
    fn untested_edges_keep_the_qualified_name() {
        let out = no_direct_test(&[defines("a.rs", "crate::a::fire")]);
        assert_eq!(out[0].a, vec!["crate::a::fire"]);
    }

    #[test]
    fn duplicate_definitions_yield_one_untested_edge() {
        let base = vec![defines("a.rs", "crate::a"), defines("a.rs", "crate::a")];
        assert_eq!(no_direct_test(&base).len(), 1);
    }

    // ─── in_cycle ───────────────────────────────────────────────────

    #[test]
    fn acyclic_imports_produce_no_cycle_edges() {
        let base = vec![imports("a", "b"), imports("b", "c")];
        assert!(in_cycle(&base).is_empty());
    }

    #[test]
    fn a_two_module_cycle_marks_both_modules() {
        let base = vec![imports("a", "b"), imports("b", "a")];
        let out = in_cycle(&base);
        let mut modules: Vec<&str> = out.iter().map(|e| e.a[0].as_str()).collect();
        modules.sort();
        assert_eq!(modules, vec!["a", "b"]);
    }

    #[test]
    fn a_three_module_cycle_marks_all_three() {
        let base = vec![imports("a", "b"), imports("b", "c"), imports("c", "a")];
        assert_eq!(in_cycle(&base).len(), 3);
    }

    #[test]
    fn modules_in_one_cycle_share_a_cycle_id() {
        let base = vec![imports("a", "b"), imports("b", "a")];
        let out = in_cycle(&base);
        assert_eq!(out[0].a[1], out[1].a[1]);
    }

    #[test]
    fn separate_cycles_get_distinct_ids() {
        let base = vec![
            imports("a", "b"),
            imports("b", "a"),
            imports("x", "y"),
            imports("y", "x"),
        ];
        let out = in_cycle(&base);
        let id_of = |m: &str| {
            out.iter()
                .find(|e| e.a[0] == m)
                .map(|e| e.a[1].clone())
                .unwrap()
        };
        assert_ne!(id_of("a"), id_of("x"));
    }

    #[test]
    fn a_self_import_is_a_cycle() {
        assert_eq!(in_cycle(&[imports("a", "a")]).len(), 1);
    }

    #[test]
    fn cycle_ids_are_stable_across_edge_ordering() {
        let forward = vec![imports("a", "b"), imports("b", "a")];
        let reversed = vec![imports("b", "a"), imports("a", "b")];
        let norm = |base: &[Edge]| {
            let mut v: Vec<(String, String)> = in_cycle(base)
                .iter()
                .map(|e| (e.a[0].clone(), e.a[1].clone()))
                .collect();
            v.sort();
            v
        };
        assert_eq!(norm(&forward), norm(&reversed));
    }

    #[test]
    fn cycle_edges_are_marked_derived() {
        let out = in_cycle(&[imports("a", "b"), imports("b", "a")]);
        assert!(out.iter().all(|e| e.d && e.src.is_empty()));
    }

    // ─── derive_all ─────────────────────────────────────────────────

    #[test]
    fn derive_all_emits_both_relations() {
        let base = vec![
            defines("a.rs", "crate::a"),
            imports("a", "b"),
            imports("b", "a"),
        ];
        let out = derive_all(&base);
        assert_eq!(args_of(&out, "no_direct_test").len(), 1);
        assert_eq!(args_of(&out, "in_cycle").len(), 2);
    }

    #[test]
    fn deserialization_derives_forward_config_to_rust_flow() {
        let base = vec![Edge::base(
            "deserializes",
            &[
                "rust:app::config::Manifest",
                "yaml:project::config::manifest",
            ],
            ".phronesis/graph.toml",
        )];
        let derived = derive_all(&base);
        let flows = args_of(&derived, "data_flows_to");
        assert_eq!(flows.len(), 1);
        assert_eq!(
            flows[0],
            &[
                "yaml:project::config::manifest".to_string(),
                "rust:app::config::Manifest".to_string()
            ]
        );
    }

    #[test]
    fn generic_consumption_derives_forward_config_flow() {
        let base = vec![Edge::base(
            "consumes_data",
            &["python:app::config::load", "json:project::config::manifest"],
            ".phronesis/graph.toml",
        )];
        let derived = derive_all(&base);
        assert_eq!(
            args_of(&derived, "data_flows_to")[0],
            &[
                "json:project::config::manifest".to_string(),
                "python:app::config::load".to_string()
            ]
        );
    }

    #[test]
    fn configuration_lifecycle_gaps_are_derived_without_policy() {
        let base = vec![
            Edge::base(
                "generates",
                &["cue:app::export", "yaml:app::unused"],
                "graph.toml",
            ),
            Edge::base(
                "consumes_data",
                &["rust:app::load", "json:app::hand_authored"],
                "graph.toml",
            ),
        ];
        let derived = derive_all(&base);
        assert_eq!(
            args_of(&derived, "generated_without_consumer")[0],
            &["yaml:app::unused".to_string()]
        );
        assert_eq!(
            args_of(&derived, "consumed_without_producer")[0],
            &["json:app::hand_authored".to_string()]
        );
    }

    #[test]
    fn derive_all_ignores_preexisting_derived_edges() {
        // Derived edges are regenerated wholesale; stale ones must not feed back.
        let base = vec![
            defines("a.rs", "crate::a"),
            tested("crate::a", "t::ta"),
            Edge::derived("no_direct_test", &["crate::a"]),
        ];
        assert!(args_of(&derive_all(&base), "no_direct_test").is_empty());
    }
}
