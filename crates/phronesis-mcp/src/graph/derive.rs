//! Derived facts: whole-graph computations the engine cannot express.
//!
//! The engine has no negation-as-failure at the pattern level and no forward
//! chaining, so "untested" (closed-world negation) and "in_cycle" (transitive
//! closure) are computed here instead (spec §4.5).
//!
//! Both are pure functions of the edge set — no source parsing, no I/O — which
//! is why they can run on *every* save without reparsing the repository.

use super::model::Edge;
use std::collections::{BTreeMap, BTreeSet};

/// Compute all derived edges over a complete base-edge set.
pub fn derive_all(base: &[Edge]) -> Vec<Edge> {
    let mut out = inventory(base);
    out.extend(untested(base));
    out.extend(in_cycle(base));
    out
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

/// Final segment of a qualified path (`crate::a::fire` -> `fire`).
///
/// The extractor cannot resolve a callee to its defining module without
/// whole-crate name resolution, so `tested_by` carries bare callee names while
/// `defines_fn` carries qualified ones. Matching on the final segment bridges
/// them.
fn short_name(qualified: &str) -> &str {
    qualified.rsplit("::").next().unwrap_or(qualified)
}

/// `untested(F)` for every `F` in `defines_fn` with no `tested_by` edge
/// naming it.
///
/// Coverage is matched by short name, which **over-approximates** it: two
/// functions sharing a name are both considered covered when either is
/// tested. That direction is chosen deliberately. A missed warning is
/// recoverable; a false "untested" verdict blocks legitimate work and is what
/// destroys trust in an enforcement layer (spec §4.4).
pub fn untested(base: &[Edge]) -> Vec<Edge> {
    let covered: BTreeSet<&str> = base_edges(base, "tested_by")
        .filter_map(|e| e.a.first())
        .map(|f| short_name(f))
        .collect();

    base_edges(base, "defines_fn")
        .filter_map(|e| e.a.get(1))
        .map(String::as_str)
        .filter(|f| !covered.contains(short_name(f)))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|f| Edge::derived("untested", &[f]))
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
        let out = untested(&[defines("a.rs", "crate::a")]);
        assert_eq!(args_of(&out, "untested").len(), 1);
        assert_eq!(out[0].a, vec!["crate::a"]);
    }

    #[test]
    fn function_with_a_test_edge_is_not_untested() {
        let base = vec![defines("a.rs", "crate::a"), tested("crate::a", "t::ta")];
        assert!(untested(&base).is_empty());
    }

    #[test]
    fn untested_edges_are_marked_derived() {
        let out = untested(&[defines("a.rs", "crate::a")]);
        assert!(out.iter().all(|e| e.d && e.src.is_empty()));
    }

    #[test]
    fn a_test_edge_covers_only_its_own_function() {
        let base = vec![
            defines("a.rs", "crate::a"),
            defines("a.rs", "crate::b"),
            tested("crate::a", "t::ta"),
        ];
        let out = untested(&base);
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
        assert!(untested(&base).is_empty());
    }

    #[test]
    fn a_test_calling_by_short_name_covers_the_qualified_function() {
        // The extractor cannot resolve a callee to its defining module, so
        // `tested_by` carries a bare name while `defines_fn` carries a
        // qualified one. Coverage matches on the final segment.
        let base = vec![defines("a.rs", "crate::a::fire"), tested("fire", "t::ta")];
        assert!(untested(&base).is_empty());
    }

    #[test]
    fn a_shared_short_name_is_treated_as_covered() {
        // Deliberate over-approximation of coverage: a missed warning is
        // recoverable, a false "untested" block is a trust-killer.
        let base = vec![
            defines("a.rs", "crate::a::fire"),
            defines("b.rs", "crate::b::fire"),
            tested("fire", "t::ta"),
        ];
        assert!(untested(&base).is_empty());
    }

    #[test]
    fn untested_edges_keep_the_qualified_name() {
        let out = untested(&[defines("a.rs", "crate::a::fire")]);
        assert_eq!(out[0].a, vec!["crate::a::fire"]);
    }

    #[test]
    fn duplicate_definitions_yield_one_untested_edge() {
        let base = vec![defines("a.rs", "crate::a"), defines("a.rs", "crate::a")];
        assert_eq!(untested(&base).len(), 1);
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
        assert_eq!(args_of(&out, "untested").len(), 1);
        assert_eq!(args_of(&out, "in_cycle").len(), 2);
    }

    #[test]
    fn derive_all_ignores_preexisting_derived_edges() {
        // Derived edges are regenerated wholesale; stale ones must not feed back.
        let base = vec![
            defines("a.rs", "crate::a"),
            tested("crate::a", "t::ta"),
            Edge::derived("untested", &["crate::a"]),
        ];
        assert!(args_of(&derive_all(&base), "untested").is_empty());
    }
}
