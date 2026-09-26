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

/// Per-file resolution breakdown: `(unresolved, ambiguous)` counts keyed by
/// the source-file provenance (`Edge::src`) of the dropped edge.
pub type PerFileResolution = BTreeMap<String, (usize, usize)>;

/// Replace extractor-local bare `tested_by` callees with canonical
/// `defines_fn` identities using only same-module or explicit-import evidence.
/// Unresolved and ambiguous calls are discarded rather than attributed to
/// every definition sharing a leaf name.
///
/// Returns the global `(unresolved, ambiguous)` totals alongside a per-file
/// breakdown keyed by the dropped edge's `src` provenance. The sum of all
/// per-file counts equals the global totals.
pub fn canonicalize_function_edges(base: &mut Vec<Edge>) -> (usize, usize, PerFileResolution) {
    let mut unresolved = 0usize;
    let mut ambiguous = 0usize;
    let mut per_file: PerFileResolution = BTreeMap::new();
    let definitions = base_edges(base, "defines_fn")
        .filter_map(|edge| edge.a.get(1).cloned())
        .collect::<BTreeSet<_>>();
    let methods = base_edges(base, "defines_method")
        .filter_map(|edge| edge.a.get(1).cloned())
        .collect::<BTreeSet<_>>();
    let production_files = base_edges(base, "file_type")
        .filter_map(|edge| {
            (edge.a.get(1).map(String::as_str) == Some("production"))
                .then(|| edge.a.first().cloned())
                .flatten()
        })
        .collect::<BTreeSet<_>>();
    let mut production_definitions = base_edges(base, "defines_fn")
        .filter(|edge| {
            edge.a
                .first()
                .is_some_and(|file| production_files.contains(file))
        })
        .filter_map(|edge| edge.a.get(1).cloned())
        .collect::<BTreeSet<_>>();
    if production_files.is_empty() {
        production_definitions = definitions.clone();
    }
    let production_methods = base_edges(base, "defines_method")
        .filter(|edge| {
            production_files.is_empty()
                || edge
                    .a
                    .first()
                    .is_some_and(|file| production_files.contains(file))
        })
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
    let production_by_leaf = production_definitions.iter().fold(
        BTreeMap::<String, BTreeSet<String>>::new(),
        |mut map, definition| {
            let leaf = definition.rsplit("::").next().unwrap_or(definition);
            map.entry(leaf.to_string())
                .or_default()
                .insert(definition.clone());
            map
        },
    );
    let method_by_leaf = methods.iter().fold(
        BTreeMap::<String, BTreeSet<String>>::new(),
        |mut map, definition| {
            let leaf = definition.rsplit("::").next().unwrap_or(definition);
            map.entry(leaf.to_string())
                .or_default()
                .insert(definition.clone());
            map
        },
    );
    let production_method_by_leaf = production_methods.iter().fold(
        BTreeMap::<String, BTreeSet<String>>::new(),
        |mut map, definition| {
            let leaf = definition.rsplit("::").next().unwrap_or(definition);
            map.entry(leaf.to_string())
                .or_default()
                .insert(definition.clone());
            map
        },
    );
    // `test_imports` (a `use` under `#[cfg(test)]`) counts for resolving what a
    // test can see, but never for `in_cycle`, which reads `imports` alone.
    let imports = base_edges(base, "imports")
        .chain(base_edges(base, "test_imports"))
        .fold(
            BTreeMap::<String, BTreeSet<String>>::new(),
            |mut map, edge| {
                if let (Some(from), Some(to)) = (edge.a.first(), edge.a.get(1)) {
                    map.entry(from.clone()).or_default().insert(to.clone());
                }
                map
            },
        );
    let reexports = base_edges(base, "reexports").fold(
        BTreeMap::<(String, String), BTreeSet<String>>::new(),
        |mut map, edge| {
            if let (Some(module), Some(target), Some(item)) =
                (edge.a.first(), edge.a.get(1), edge.a.get(2))
            {
                map.entry((module.clone(), item.clone()))
                    .or_default()
                    .insert(target.clone());
            }
            map
        },
    );

    let mut normalized = Vec::with_capacity(base.len());
    for mut edge in base.drain(..) {
        if !matches!(edge.p.as_str(), "tested_by" | "calls") || edge.a.len() != 2 {
            normalized.push(edge);
            continue;
        }
        let (callee_index, caller) = if edge.p == "tested_by" {
            (0, edge.a[1].as_str())
        } else if canonicalizes_calls(&edge.a[0]) {
            (1, edge.a[0].as_str())
        } else {
            normalized.push(edge);
            continue;
        };
        let raw_callee = &edge.a[callee_index];
        let method_hint = raw_callee.strip_prefix("@method:");
        let (receiver_type, callee) = method_hint.map_or((None, raw_callee.as_str()), |hint| {
            hint.rsplit_once(':')
                .map_or((None, hint), |(ty, method)| (Some(ty), method))
        });
        let method_call = method_hint.is_some();
        let (eligible, candidates_by_leaf) = if edge.p == "tested_by" && method_call {
            (&production_methods, &production_method_by_leaf)
        } else if method_call {
            (&methods, &method_by_leaf)
        } else if edge.p == "tested_by" {
            (&production_definitions, &production_by_leaf)
        } else {
            (&definitions, &by_leaf)
        };
        if eligible.contains(callee) {
            normalized.push(edge);
            continue;
        }
        let caller_module = caller.rsplit_once("::").map(|(module, _)| module);
        let Some(candidates) = candidates_by_leaf.get(callee) else {
            unresolved += 1;
            per_file.entry(edge.src.clone()).or_insert((0, 0)).0 += 1;
            continue;
        };
        let receiver = receiver_type.map(normalize_receiver);
        let qualified_receiver = receiver.is_some_and(|r| r.contains("::"));
        // `by_path`: a qualified receiver must suffix-match the candidate's
        // type path; otherwise only the type's last segment is compared.
        let filter_candidates = |by_path: bool| {
            candidates
                .iter()
                .filter(|candidate| {
                    let Some((module, _)) = candidate.rsplit_once("::") else {
                        return false;
                    };
                    let method_scope = method_call.then(|| module.rsplit_once("::")).flatten();
                    if receiver.is_some_and(|receiver| {
                        let candidate_type = strip_generic_args(module);
                        if by_path && receiver.contains("::") {
                            !receiver_matches_qualified(candidate_type, receiver)
                        } else {
                            last_path_segment(candidate_type) != last_path_segment(receiver)
                        }
                    }) {
                        return false;
                    }
                    let visible_imports = caller_module.into_iter().flat_map(|caller| {
                        imports.iter().filter_map(move |(module, targets)| {
                            (caller == module
                                || caller
                                    .strip_prefix(module)
                                    .is_some_and(|suffix| suffix.starts_with("::")))
                            .then_some(targets)
                        })
                    });
                    caller_module.is_some_and(|caller_module| {
                        caller_module == module
                            || caller_module
                                .strip_prefix(module)
                                .is_some_and(|suffix| suffix.starts_with("::"))
                    }) || visible_imports.into_iter().any(|targets| {
                        targets.contains(module)
                            || targets
                                .iter()
                                .any(|imported| unit_contains(imported, module))
                            || (method_call
                                && targets.iter().any(|imported| {
                                    method_scope.is_some_and(|(parent, ty)| {
                                        imported == parent
                                            || reexports
                                                .get(&(imported.clone(), ty.to_string()))
                                                .is_some_and(|modules| modules.contains(parent))
                                    })
                                }))
                            || targets.iter().any(|imported| {
                                reexports
                                    .get(&(imported.clone(), callee.to_string()))
                                    .is_some_and(|modules| modules.contains(module))
                            })
                    }) || receiver_type.is_some()
                })
                .collect::<Vec<_>>()
        };
        let mut resolved = filter_candidates(true);
        // A qualified path that names no known type (an alias such as
        // `use crate::python as py; py::Sensor`) falls back to its last
        // segment. It is then no longer a statement about which module the
        // type lives in, so the same-type preference below does not apply.
        if qualified_receiver && resolved.is_empty() {
            resolved = filter_candidates(false);
        }
        // An unqualified typed hint names a type by its last segment, which
        // several modules can share (one `Sensor` per language extractor).
        // When the caller is itself a method of exactly one of the matching
        // types, the hint means that type: Rust resolves an unqualified type
        // name to the enclosing module's own definition. Otherwise the call
        // stays ambiguous.
        let resolved = if resolved.len() > 1 && receiver.is_some() && !qualified_receiver {
            let caller_type = caller_module.map(strip_generic_args);
            let same_type = resolved
                .iter()
                .copied()
                .filter(|candidate| {
                    candidate
                        .rsplit_once("::")
                        .is_some_and(|(module, _)| Some(strip_generic_args(module)) == caller_type)
                })
                .collect::<Vec<_>>();
            if same_type.len() == 1 {
                same_type
            } else {
                resolved
            }
        } else {
            resolved
        };
        if resolved.len() == 1 {
            edge.a[callee_index] = (*resolved[0]).clone();
            normalized.push(edge);
        } else if resolved.is_empty() {
            unresolved += 1;
            per_file.entry(edge.src.clone()).or_insert((0, 0)).0 += 1;
        } else {
            ambiguous += 1;
            per_file.entry(edge.src.clone()).or_insert((0, 0)).1 += 1;
        }
    }
    *base = normalized;
    (unresolved, ambiguous, per_file)
}

/// Strip `<...>` generic arguments from a type name, returning the base name.
/// A receiver type as the resolver compares it: generic arguments dropped,
/// and the leading `crate::`/`self::`/`super::` segments removed, since
/// canonical identities name the unit and module path, never those keywords.
fn normalize_receiver(receiver: &str) -> &str {
    let mut receiver = strip_generic_args(receiver);
    while let Some(rest) = ["crate::", "self::", "super::"]
        .iter()
        .find_map(|prefix| receiver.strip_prefix(prefix))
    {
        receiver = rest;
    }
    receiver
}

fn last_path_segment(path: &str) -> &str {
    path.rsplit("::").next().unwrap_or(path)
}

fn strip_generic_args(ty: &str) -> &str {
    match ty.find('<') {
        Some(idx) => &ty[..idx],
        None => ty,
    }
}

/// Extract the last `::`-separated path segment, skipping `::` inside `<...>`
/// generic argument lists. For `Foo<std::vec::Vec<T>>` returns `Foo`.
fn bracket_aware_last_segment(path: &str) -> &str {
    let bytes = path.as_bytes();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'<' => depth += 1,
            b'>' => depth -= 1,
            b':' if depth == 0 => {
                start = i + 1;
            }
            _ => {}
        }
    }
    &path[start..]
}

/// Whether a qualified receiver hint (e.g. `a::Foo`) suffix-matches the
/// candidate's full type path (e.g. `rust:app::a::Foo`). The hint must be a
/// suffix of the candidate path aligned on `::` segment boundaries, and the
/// final segments must be equal. This prevents `a::Foo` from matching
/// `rust:app::x::Foo` (different module, same leaf).
fn receiver_matches_qualified(candidate_type: &str, hint: &str) -> bool {
    let candidate_last = bracket_aware_last_segment(candidate_type);
    let hint_last = bracket_aware_last_segment(hint);
    if candidate_last != hint_last {
        return false;
    }
    candidate_type.ends_with(hint)
        && (candidate_type.len() == hint.len()
            || candidate_type
                .as_bytes()
                .get(candidate_type.len() - hint.len() - 1)
                .is_some_and(|&c| c == b':'))
}

/// Whether a function identity belongs to a language whose extractor emits
/// bare-callee `calls` edges for resolution here. Rust resolves by module and
/// `use` evidence; Swift by the unit-wide `imports` every file states.
fn canonicalizes_calls(function: &str) -> bool {
    function.starts_with("rust:") || function.starts_with("swift:")
}

/// Whether `imported` names a whole unit (`swift:project`, `rust:app` — a
/// language tag and unit name with no module path) that `module` belongs to.
/// Importing a unit, rather than a module inside it, is how a language with
/// target-wide namespaces states that a file sees everything in its target.
fn unit_contains(imported: &str, module: &str) -> bool {
    !imported.contains("::")
        && module
            .strip_prefix(imported)
            .is_some_and(|rest| rest.starts_with("::"))
}

/// Compute all derived edges over a complete base-edge set.
pub fn derive_all(base: &[Edge]) -> Vec<Edge> {
    let mut out = inventory(base);
    out.extend(data_flows_to(base));
    out.extend(configuration_lifecycle_gaps(base));
    out.extend(rhai_reachability(base));
    out.extend(rhai_predicate_flow(base));
    out.extend(test_reachability(base));
    out.extend(no_direct_test(base));
    out.extend(in_cycle(base));
    out
}

/// Statically resolved transitive test reachability. `tested_by` remains the
/// direct call evidence; this relation follows only canonical function call
/// edges and therefore makes no claim about dynamic dispatch or execution.
pub fn test_reachability(base: &[Edge]) -> Vec<Edge> {
    let mut calls: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for edge in base_edges(base, "calls") {
        let (Some(caller), Some(callee)) = (edge.a.first(), edge.a.get(1)) else {
            continue;
        };
        if canonicalizes_calls(caller) && canonicalizes_calls(callee) {
            calls.entry(caller).or_default().insert(callee);
        }
    }

    let mut out = BTreeSet::new();
    for edge in base_edges(base, "tested_by") {
        let (Some(function), Some(test)) = (edge.a.first(), edge.a.get(1)) else {
            continue;
        };
        let mut pending = vec![function.as_str()];
        let mut seen = BTreeSet::new();
        while let Some(current) = pending.pop() {
            if !seen.insert(current) {
                continue;
            }
            out.insert((test.as_str(), current));
            pending.extend(calls.get(current).into_iter().flatten().copied());
        }
    }
    out.into_iter()
        .map(|(test, function)| Edge::derived("test_reaches", &[test, function]))
        .collect()
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
    for edge in base_edges(base, "defines_test") {
        if let (Some(file), Some(test)) = (edge.a.first(), edge.a.get(1)) {
            out.insert(("graph_function".into(), vec![test.clone()]));
            out.insert(("graph_test".into(), vec![test.clone()]));
            out.insert(("element_in_file".into(), vec![test.clone(), file.clone()]));
            for module in modules_by_file.get(file.as_str()).into_iter().flatten() {
                out.insert((
                    "element_in_module".into(),
                    vec![test.clone(), (*module).to_string()],
                ));
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
/// after its sorted members (`cycle:a+b+c`) makes the id a function of the
/// cycle's contents alone, and also makes it self-describing: an id named after
/// only the smallest member read as "module `a` is in cycle `a`", which looked
/// like a self-import when `a` was the module being reported.
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
        let members: BTreeSet<&str> = scc.iter().copied().collect();
        let cycle_id = format!(
            "cycle:{}",
            members.iter().copied().collect::<Vec<_>>().join("+")
        );
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
            Edge::base(
                "defines_test",
                &["tests/run.rs", "rust:app#test:run::works"],
                "tests/run.rs",
            ),
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
        let _ = canonicalize_function_edges(&mut base);
        assert_eq!(args_of(&base, "tested_by")[0][0], "rust:app::a::fire");
        assert!(no_direct_test(&base).is_empty());
    }

    #[test]
    fn a_whole_unit_import_makes_every_module_in_the_unit_visible() {
        // Swift files see their entire target without naming a module, so
        // the extractor imports the unit itself rather than a module in it.
        let mut base = vec![
            defines("App/A.swift", "swift:project::App::A::Overlay::tap"),
            imports("swift:project::AppTests::ATests", "swift:project"),
            tested("tap", "swift:project::AppTests::ATests::ATests::testTap"),
        ];
        let _ = canonicalize_function_edges(&mut base);
        assert_eq!(
            args_of(&base, "tested_by")[0][0],
            "swift:project::App::A::Overlay::tap"
        );
        assert!(no_direct_test(&base).is_empty());
    }

    #[test]
    fn an_ambiguous_bare_test_call_is_not_guessed() {
        let mut base = vec![
            defines("a.rs", "rust:app::a::fire"),
            defines("b.rs", "rust:app::b::fire"),
            tested("fire", "rust:app#test:integration::test_fire"),
        ];
        let _ = canonicalize_function_edges(&mut base);
        assert!(args_of(&base, "tested_by").is_empty());
        assert_eq!(no_direct_test(&base).len(), 2);
    }

    #[test]
    fn ordinary_calls_are_canonicalized_without_guessing_ambiguous_names() {
        let mut base = vec![
            defines("a.rs", "rust:app::a::entry"),
            defines("a.rs", "rust:app::a::helper"),
            defines("b.rs", "rust:app::b::helper"),
            Edge::base("calls", &["rust:app::a::entry", "helper"], "a.rs"),
        ];
        let _ = canonicalize_function_edges(&mut base);
        assert!(base.iter().any(|edge| {
            edge.p == "calls" && edge.a == ["rust:app::a::entry", "rust:app::a::helper"]
        }));
    }

    #[test]
    fn swift_calls_are_canonicalized_through_unit_wide_visibility() {
        let caller = "swift:project::App::A::Overlay::tap";
        let helper = "swift:project::App::B::helper";
        let mut base = vec![
            defines("App/A.swift", caller),
            defines("App/B.swift", helper),
            imports("swift:project::App::A", "swift:project"),
            Edge::base("calls", &[caller, "helper"], "App/A.swift"),
        ];
        let _ = canonicalize_function_edges(&mut base);
        assert!(
            base.iter()
                .any(|edge| edge.p == "calls" && edge.a == [caller, helper]),
            "{base:?}"
        );
    }

    #[test]
    fn imported_unique_inherent_method_is_attributed_to_its_test() {
        let method = "rust:app::state::GameState::apply_damage";
        let test = "rust:app::state::tests::damage_works";
        let mut base = vec![
            defines("src/state.rs", method),
            Edge::base("defines_method", &["src/state.rs", method], "src/state.rs"),
            Edge::base("file_type", &["src/state.rs", "production"], "src/state.rs"),
            Edge::base("file_type", &["tests/state.rs", "test"], "tests/state.rs"),
            imports("rust:app::state::tests", "rust:app::state"),
            tested("@method:apply_damage", test),
        ];
        let _ = canonicalize_function_edges(&mut base);
        assert!(
            base.iter()
                .any(|edge| { edge.p == "tested_by" && edge.a == [method, test] })
        );
    }

    #[test]
    fn ambiguous_inherent_method_names_are_not_guessed() {
        let mut base = vec![
            defines("src/a.rs", "rust:app::state::A::refresh"),
            defines("src/b.rs", "rust:app::state::B::refresh"),
            Edge::base(
                "defines_method",
                &["src/a.rs", "rust:app::state::A::refresh"],
                "src/a.rs",
            ),
            Edge::base(
                "defines_method",
                &["src/b.rs", "rust:app::state::B::refresh"],
                "src/b.rs",
            ),
            Edge::base("file_type", &["src/a.rs", "production"], "src/a.rs"),
            Edge::base("file_type", &["src/b.rs", "production"], "src/b.rs"),
            imports("rust:app::state::tests", "rust:app::state"),
            tested("@method:refresh", "rust:app::state::tests::works"),
        ];
        let _ = canonicalize_function_edges(&mut base);
        assert!(args_of(&base, "tested_by").is_empty());
    }

    #[test]
    fn test_reachability_includes_direct_and_transitive_resolved_calls() {
        let base = vec![
            tested("rust:app::entry", "rust:app#test:flow::works"),
            Edge::base(
                "calls",
                &["rust:app::entry", "rust:app::helper"],
                "src/lib.rs",
            ),
            Edge::base(
                "calls",
                &["rust:app::helper", "rust:app::leaf"],
                "src/lib.rs",
            ),
        ];
        let out = test_reachability(&base);
        assert_eq!(
            out.iter().map(|edge| edge.a.clone()).collect::<Vec<_>>(),
            vec![
                vec![
                    String::from("rust:app#test:flow::works"),
                    String::from("rust:app::entry")
                ],
                vec![
                    String::from("rust:app#test:flow::works"),
                    String::from("rust:app::helper")
                ],
                vec![
                    String::from("rust:app#test:flow::works"),
                    String::from("rust:app::leaf")
                ],
            ]
        );
    }

    #[test]
    fn test_reachability_terminates_on_call_cycles_and_ignores_bare_calls() {
        let base = vec![
            tested("rust:app::a", "rust:app#test:flow::works"),
            Edge::base("calls", &["rust:app::a", "rust:app::b"], "src/lib.rs"),
            Edge::base("calls", &["rust:app::b", "rust:app::a"], "src/lib.rs"),
            Edge::base("calls", &["rust:app::a", "ambiguous"], "src/lib.rs"),
        ];
        assert_eq!(test_reachability(&base).len(), 2);
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
        let out = in_cycle(&[imports("a", "a")]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].a[1], "cycle:a");
    }

    /// Regression: a two-module cycle used to be named after its smallest
    /// member only (`cycle:a`), so the `a` row of the audit read as
    /// "a is in cycle a" — indistinguishable from a self-import. The id now
    /// enumerates every member, sorted, regardless of edge order.
    #[test]
    fn cycle_id_names_every_member_not_just_the_smallest() {
        let base = vec![imports("wme", "binding"), imports("binding", "wme")];
        let out = in_cycle(&base);
        assert_eq!(out.len(), 2);
        for e in &out {
            assert_eq!(e.a[1], "cycle:binding+wme", "edge {:?}", e.a);
            assert_ne!(
                format!("cycle:{}", e.a[0]),
                e.a[1],
                "id must not equal a lone member"
            );
        }
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

    #[test]
    fn generic_receiver_type_matches_non_generic_method_definition() {
        // The extractor emits `@method:Vec<u32>:push` when a `let v: Vec<u32>`
        // variable calls `.push()`. The method is defined under
        // `rust:app::Vec::push` — the last module segment is `Vec` (no generics).
        // Generics normalization should strip `<u32>` from the receiver hint
        // so the comparison `Vec == Vec` succeeds.
        let method = "rust:app::Vec::push";
        let caller = "rust:app::container::fill";
        let mut base = vec![
            defines("src/vec.rs", method),
            Edge::base("defines_method", &["src/vec.rs", method], "src/vec.rs"),
            defines("src/container.rs", caller),
            Edge::base("file_type", &["src/vec.rs", "production"], "src/vec.rs"),
            Edge::base(
                "file_type",
                &["src/container.rs", "production"],
                "src/container.rs",
            ),
            Edge::base(
                "calls",
                &[caller, "@method:Vec<u32>:push"],
                "src/container.rs",
            ),
        ];
        let _ = canonicalize_function_edges(&mut base);
        assert!(
            base.iter()
                .any(|edge| { edge.p == "calls" && edge.a == [caller, method] }),
            "generic receiver should resolve to non-generic method def: {base:?}"
        );
    }

    #[test]
    fn self_receiver_hint_resolves_same_impl_among_same_leaf_methods() {
        // DoD case: two types in different modules define the same method
        // leaf (`dup`), both visible to the caller via imports. A call
        // emitted from inside `impl A` as `@method:A:dup` (the `self`
        // receiver hint from the extractor) must resolve to A's method —
        // receiver evidence decides, the leaf name alone stays ambiguous.
        let caller = "rust:app::a::entry";
        let a_method = "rust:app::a::A::dup";
        let mut base = vec![
            defines("src/a.rs", caller),
            defines("src/a.rs", a_method),
            Edge::base("defines_method", &["src/a.rs", a_method], "src/a.rs"),
            defines("src/b.rs", "rust:app::b::B::dup"),
            Edge::base(
                "defines_method",
                &["src/b.rs", "rust:app::b::B::dup"],
                "src/b.rs",
            ),
            Edge::base("file_type", &["src/a.rs", "production"], "src/a.rs"),
            Edge::base("file_type", &["src/b.rs", "production"], "src/b.rs"),
            imports("rust:app::a", "rust:app::b"),
            Edge::base("calls", &[caller, "@method:A:dup"], "src/a.rs"),
        ];
        canonicalize_function_edges(&mut base);
        assert!(
            base.iter()
                .any(|edge| edge.p == "calls" && edge.a == [caller, a_method]),
            "typed self hint should resolve to the same-impl candidate: {base:?}"
        );
    }

    #[test]
    fn unresolved_and_ambiguous_counts_are_reported() {
        // One call to `missing` (no definition at all → unresolved),
        // one call to `dup` where two definitions share the leaf name and
        // both are visible to the caller via imports (→ ambiguous).
        let mut base = vec![
            defines("src/a.rs", "rust:app::a::entry"),
            defines("src/b.rs", "rust:app::b::dup"),
            defines("src/c.rs", "rust:app::c::dup"),
            Edge::base("file_type", &["src/a.rs", "production"], "src/a.rs"),
            Edge::base("file_type", &["src/b.rs", "production"], "src/b.rs"),
            Edge::base("file_type", &["src/c.rs", "production"], "src/c.rs"),
            imports("rust:app::a", "rust:app::b"),
            imports("rust:app::a", "rust:app::c"),
            Edge::base("calls", &["rust:app::a::entry", "missing"], "src/a.rs"),
            Edge::base("calls", &["rust:app::a::entry", "dup"], "src/a.rs"),
        ];
        let (unresolved, ambiguous, _) = canonicalize_function_edges(&mut base);
        assert_eq!(unresolved, 1, "one unresolved call expected");
        assert_eq!(ambiguous, 1, "one ambiguous call expected");
    }

    #[test]
    fn per_file_resolution_attributes_to_edge_src() {
        // src/a.rs: one unresolved call (to `missing`) + one ambiguous call
        //           (to `dup` — two defs visible via imports).
        // src/b.rs: one unresolved call (to `ghost`).
        let mut base = vec![
            defines("src/a.rs", "rust:app::a::entry"),
            defines("src/b.rs", "rust:app::b::entry"),
            defines("src/c.rs", "rust:app::c::dup"),
            defines("src/d.rs", "rust:app::d::dup"),
            Edge::base("file_type", &["src/a.rs", "production"], "src/a.rs"),
            Edge::base("file_type", &["src/b.rs", "production"], "src/b.rs"),
            Edge::base("file_type", &["src/c.rs", "production"], "src/c.rs"),
            Edge::base("file_type", &["src/d.rs", "production"], "src/d.rs"),
            imports("rust:app::a", "rust:app::c"),
            imports("rust:app::a", "rust:app::d"),
            // src/a.rs: unresolved call to `missing`
            Edge::base("calls", &["rust:app::a::entry", "missing"], "src/a.rs"),
            // src/a.rs: ambiguous call to `dup` (two defs visible)
            Edge::base("calls", &["rust:app::a::entry", "dup"], "src/a.rs"),
            // src/b.rs: unresolved call to `ghost`
            Edge::base("calls", &["rust:app::b::entry", "ghost"], "src/b.rs"),
        ];
        let (unresolved, ambiguous, per_file) = canonicalize_function_edges(&mut base);
        assert_eq!(per_file.get("src/a.rs"), Some(&(1, 1)));
        assert_eq!(per_file.get("src/b.rs"), Some(&(1, 0)));
        // Sum over files equals global totals.
        let total_u: usize = per_file.values().map(|(u, _)| u).sum();
        let total_a: usize = per_file.values().map(|(_, a)| a).sum();
        assert_eq!(total_u, unresolved);
        assert_eq!(total_a, ambiguous);
    }

    // ─── adversarial self-receiver resolution (T4) ────────────────

    /// Helper: a `defines_fn` + `defines_method` + `file_type=production`
    /// triple for a method inside a module.
    fn method_def(file: &str, method: &str) -> Vec<Edge> {
        vec![
            defines(file, method),
            Edge::base("defines_method", &[file, method], file),
            Edge::base("file_type", &[file, "production"], file),
        ]
    }

    #[test]
    fn t4_a_same_impl_self_call_resolves_to_same_type_method() {
        // Case A: `self.b()` inside `impl Foo` emits `@method:Foo:b`.
        // Foo::a and Foo::b both exist; the receiver hint `Foo` must
        // resolve the call to `Foo::b` (not dropped, not misattributed).
        let caller = "rust:app::foo::Foo::a";
        let target = "rust:app::foo::Foo::b";
        let mut base = vec![];
        base.extend(method_def("src/foo.rs", caller));
        base.extend(method_def("src/foo.rs", target));
        base.push(Edge::base(
            "calls",
            &[caller, "@method:Foo:b"],
            "src/foo.rs",
        ));
        let (unresolved, ambiguous, _) = canonicalize_function_edges(&mut base);
        assert_eq!(unresolved, 0, "Case A: no unresolved expected");
        assert_eq!(ambiguous, 0, "Case A: no ambiguous expected");
        assert!(
            base.iter()
                .any(|e| e.p == "calls" && e.a == [caller, target]),
            "Case A: calls(Foo::a, Foo::b) must exist, got: {base:?}"
        );
    }

    #[test]
    fn t4_b_same_leaf_on_two_types_resolves_by_receiver_not_name() {
        // Case B: Foo and Bar both define `b`. A call `@method:Foo:b`
        // from Foo::a must resolve to Foo::b, NOT Bar::b. Both directions
        // asserted.
        let caller = "rust:app::foo::Foo::a";
        let foo_b = "rust:app::foo::Foo::b";
        let bar_b = "rust:app::bar::Bar::b";
        let mut base = vec![];
        base.extend(method_def("src/foo.rs", caller));
        base.extend(method_def("src/foo.rs", foo_b));
        base.extend(method_def("src/bar.rs", bar_b));
        // cross-module import so both candidates are visible
        base.push(imports("rust:app::foo", "rust:app::bar"));
        base.push(Edge::base(
            "calls",
            &[caller, "@method:Foo:b"],
            "src/foo.rs",
        ));
        let (unresolved, ambiguous, _) = canonicalize_function_edges(&mut base);
        assert_eq!(unresolved, 0, "Case B: no unresolved expected");
        assert_eq!(
            ambiguous, 0,
            "Case B: receiver evidence should disambiguate"
        );
        assert!(
            base.iter()
                .any(|e| e.p == "calls" && e.a == [caller, foo_b]),
            "Case B: Foo::a -> Foo::b must exist, got: {base:?}"
        );
        assert!(
            !base
                .iter()
                .any(|e| e.p == "calls" && e.a == [caller, bar_b]),
            "Case B: Foo::a -> Bar::b must NOT exist, got: {base:?}"
        );
    }

    #[test]
    fn same_named_types_in_two_modules_resolve_self_calls_to_the_callers_own_type() {
        // One `Sensor` per language extractor: a typed `self` hint matches
        // both by last segment, so the caller's own impl type decides.
        let rust_walk = "rust:app::rust::Sensor<'_>::walk";
        let rust_visit = "rust:app::rust::Sensor<'_>::visit";
        let py_walk = "rust:app::python::Sensor<'_>::walk";
        let py_visit = "rust:app::python::Sensor<'_>::visit";
        let mut base = vec![];
        base.extend(method_def("src/rust.rs", rust_walk));
        base.extend(method_def("src/rust.rs", rust_visit));
        base.extend(method_def("src/python.rs", py_walk));
        base.extend(method_def("src/python.rs", py_visit));
        base.push(Edge::base(
            "calls",
            &[rust_walk, "@method:Sensor:visit"],
            "src/rust.rs",
        ));
        base.push(Edge::base(
            "calls",
            &[py_walk, "@method:Sensor:visit"],
            "src/python.rs",
        ));
        let (unresolved, ambiguous, _) = canonicalize_function_edges(&mut base);
        assert_eq!((unresolved, ambiguous), (0, 0), "{base:?}");
        assert!(
            base.iter()
                .any(|e| e.p == "calls" && e.a == [rust_walk, rust_visit])
        );
        assert!(
            base.iter()
                .any(|e| e.p == "calls" && e.a == [py_walk, py_visit])
        );
        assert!(
            !base
                .iter()
                .any(|e| e.p == "calls" && e.a == [rust_walk, py_visit])
        );
    }

    #[test]
    fn same_named_types_stay_ambiguous_for_a_caller_outside_both() {
        let caller = "rust:app::driver::run";
        let mut base = vec![];
        base.extend(method_def("src/driver.rs", caller));
        base.extend(method_def("src/rust.rs", "rust:app::rust::Sensor::visit"));
        base.extend(method_def(
            "src/python.rs",
            "rust:app::python::Sensor::visit",
        ));
        base.push(Edge::base(
            "calls",
            &[caller, "@method:Sensor:visit"],
            "src/driver.rs",
        ));
        let (unresolved, ambiguous, _) = canonicalize_function_edges(&mut base);
        assert_eq!(
            (unresolved, ambiguous),
            (0, 1),
            "no guess between two Sensors"
        );
        assert!(!base.iter().any(|e| e.p == "calls" && e.a[0] == caller));
    }

    #[test]
    fn t4_c_generic_impl_self_call_resolves_after_normalization() {
        // Case C: `@method:Foo:b` (generics already stripped by extract)
        // must resolve to `rust:app::foo::Foo::b` whose module last
        // segment is `Foo`.
        let caller = "rust:app::foo::Foo::a";
        let target = "rust:app::foo::Foo::b";
        let mut base = vec![];
        base.extend(method_def("src/foo.rs", caller));
        base.extend(method_def("src/foo.rs", target));
        base.push(Edge::base(
            "calls",
            &[caller, "@method:Foo:b"],
            "src/foo.rs",
        ));
        let (unresolved, ambiguous, _) = canonicalize_function_edges(&mut base);
        assert_eq!(unresolved, 0, "Case C: no unresolved expected");
        assert_eq!(ambiguous, 0, "Case C: no ambiguous expected");
        assert!(
            base.iter()
                .any(|e| e.p == "calls" && e.a == [caller, target]),
            "Case C: generic impl self call must resolve, got: {base:?}"
        );
    }

    #[test]
    fn t4_d_self_assoc_fn_resolves_to_same_type() {
        // Case D: `Self::b()` emits `@method:Foo:b`. Must resolve to
        // Foo::b (here an associated function, but the graph does not
        // distinguish — receiver type filter matches `Foo`).
        let caller = "rust:app::foo::Foo::a";
        let target = "rust:app::foo::Foo::b";
        let mut base = vec![];
        base.extend(method_def("src/foo.rs", caller));
        base.extend(method_def("src/foo.rs", target));
        base.push(Edge::base(
            "calls",
            &[caller, "@method:Foo:b"],
            "src/foo.rs",
        ));
        let (unresolved, ambiguous, _) = canonicalize_function_edges(&mut base);
        assert_eq!(unresolved, 0, "Case D: no unresolved expected");
        assert_eq!(ambiguous, 0, "Case D: no ambiguous expected");
        assert!(
            base.iter()
                .any(|e| e.p == "calls" && e.a == [caller, target]),
            "Case D: Self::b() must resolve to Foo::b, got: {base:?}"
        );
    }

    #[test]
    fn t4_e_unknown_receiver_bare_hint_no_invented_edge() {
        // Case E: bare `@method:b` (no receiver type). Two types define
        // `b`, both visible. The call must NOT be guessed; it must be
        // counted as ambiguous and dropped (no invented edge).
        let caller = "rust:app::foo::Foo::a";
        let foo_b = "rust:app::foo::Foo::b";
        let bar_b = "rust:app::bar::Bar::b";
        let mut base = vec![];
        base.extend(method_def("src/foo.rs", caller));
        base.extend(method_def("src/foo.rs", foo_b));
        base.extend(method_def("src/bar.rs", bar_b));
        base.push(imports("rust:app::foo", "rust:app::bar"));
        base.push(Edge::base("calls", &[caller, "@method:b"], "src/foo.rs"));
        let (unresolved, ambiguous, _) = canonicalize_function_edges(&mut base);
        assert_eq!(
            unresolved, 0,
            "Case E: bare hint with candidates is not unresolved"
        );
        assert_eq!(
            ambiguous, 1,
            "Case E: bare hint with 2 visible candidates is ambiguous"
        );
        assert!(
            !base
                .iter()
                .any(|e| e.p == "calls" && (e.a == [caller, foo_b] || e.a == [caller, bar_b])),
            "Case E: no invented edge to Foo::b or Bar::b, got: {base:?}"
        );
    }

    #[test]
    fn t4_e_unknown_receiver_no_definition_is_unresolved() {
        // Case E (complement): bare `@method:b` with NO definition at all
        // → unresolved, not ambiguous.
        let caller = "rust:app::foo::Foo::a";
        let mut base = vec![];
        base.extend(method_def("src/foo.rs", caller));
        base.push(Edge::base("calls", &[caller, "@method:b"], "src/foo.rs"));
        let (unresolved, ambiguous, _) = canonicalize_function_edges(&mut base);
        assert_eq!(unresolved, 1, "Case E: no definition → unresolved");
        assert_eq!(ambiguous, 0, "Case E: no candidates → not ambiguous");
    }

    #[test]
    fn t4_f_collision_across_unrelated_types_no_resolution_by_name() {
        // Case F: `.contains` on an opaque receiver (bare hint). Bag and
        // StrWrap both define `contains`, both visible to the caller via
        // imports of their type modules. The call must NOT resolve to
        // either by name alone — it is ambiguous and dropped.
        let caller = "rust:app::bag::checker";
        let bag_contains = "rust:app::bag::Bag::contains";
        let str_contains = "rust:app::strwrap::StrWrap::contains";
        let mut base = vec![];
        base.extend(method_def("src/bag.rs", caller));
        base.extend(method_def("src/bag.rs", bag_contains));
        base.extend(method_def("src/strwrap.rs", str_contains));
        // Import the type's parent module so method_scope visibility
        // matches for both candidates.
        base.push(imports("rust:app::bag", "rust:app::strwrap"));
        // Bag::contains is in the caller's own module, but the method's
        // module path includes the type (`rust:app::bag::Bag`), which
        // does not match the caller's module (`rust:app::bag`). The
        // method_scope path matches `imported == parent` where parent is
        // `rust:app::bag`, so we need the caller's own module as an
        // import target too — or a self-import. Use a reexport-free
        // self-referencing import to make Bag visible.
        base.push(imports("rust:app::bag", "rust:app::bag"));
        base.push(Edge::base(
            "calls",
            &[caller, "@method:contains"],
            "src/bag.rs",
        ));
        let (unresolved, ambiguous, _) = canonicalize_function_edges(&mut base);
        assert_eq!(unresolved, 0, "Case F: candidates exist, not unresolved");
        assert_eq!(ambiguous, 1, "Case F: 2 visible candidates → ambiguous");
        assert!(
            !base.iter().any(|e| e.p == "calls"
                && (e.a == [caller, bag_contains] || e.a == [caller, str_contains])),
            "Case F: no resolution to unrelated type by name, got: {base:?}"
        );
    }

    // ─── T4 review findings F1–F4 (derive side) ───────────────────

    #[test]
    fn t4_f1_qualified_impl_path_resolves_to_qualified_candidate() {
        // F1: extract emits `@method:a::Foo:baz` for `impl<T> a::Foo<T>`.
        // The candidate method is `rust:app::a::Foo::baz`. Derive's
        // receiver filter compares `strip_generic_args(module_last)` to
        // `strip_generic_args(receiver)`. module_last is `Foo` (from
        // `rust:app::a::Foo`), receiver is `a::Foo` (from the hint).
        // `Foo != a::Foo` → the edge is dropped. This test falsifies the
        // fix: a qualified impl path hint does not match the candidate.
        let caller = "rust:app::a::Foo::bar";
        let target = "rust:app::a::Foo::baz";
        let mut base = vec![];
        base.extend(method_def("src/a.rs", caller));
        base.extend(method_def("src/a.rs", target));
        base.push(Edge::base(
            "calls",
            &[caller, "@method:a::Foo:baz"],
            "src/a.rs",
        ));
        let (unresolved, ambiguous, _) = canonicalize_function_edges(&mut base);
        // Expected if the fix is correct: resolves. If F1 is a real bug,
        // unresolved=1 and no edge. We assert the GROUND TRUTH: the call
        // SHOULD resolve (same impl, same type).
        let resolved = base
            .iter()
            .any(|e| e.p == "calls" && e.a == [caller, target]);
        if !resolved {
            // F1 defect exposed: the qualified receiver `a::Foo` does not
            // match the candidate module last segment `Foo`. Record it.
            // We do NOT weaken conservative behavior; we just report.
        }
        // Ground truth from fixture: bar and baz are on the same impl,
        // so this MUST resolve. If it doesn't, the fix has a defect.
        assert!(
            resolved,
            "F1: qualified impl path should resolve to same-type candidate \
             (unresolved={unresolved}, ambiguous={ambiguous}), base: {base:?}"
        );
    }

    #[test]
    fn t4_f2_bracket_aware_rsplit_for_generic_impl_with_path_in_args() {
        // F2: candidate module `rust:app::Foo<std::vec::Vec<T>>`.
        // `module.rsplit(\"::\").next()` would yield `Vec<T>>` (wrong).
        // The fix uses `strip_generic_args` on `module_last`, but
        // `module_last` itself is already wrong if rsplit is not
        // bracket-aware. Ground truth: `self.baz()` in
        // `impl Foo<std::vec::Vec<T>>` should resolve to `Foo::baz`.
        let caller = "rust:app::foo::Foo::bar";
        let _target = "rust:app::foo::Foo::baz";
        // Simulate a candidate whose module path contains generic args
        // with `::` inside the brackets. The defines_method identity is
        // the fully-qualified path including generics.
        let target_with_generics = "rust:app::foo::Foo<std::vec::Vec<T>>::baz";
        let mut base = vec![];
        base.extend(method_def("src/foo.rs", caller));
        base.push(defines("src/foo.rs", target_with_generics));
        base.push(Edge::base(
            "defines_method",
            &["src/foo.rs", target_with_generics],
            "src/foo.rs",
        ));
        base.push(Edge::base(
            "file_type",
            &["src/foo.rs", "production"],
            "src/foo.rs",
        ));
        base.push(Edge::base(
            "calls",
            &[caller, "@method:Foo:baz"],
            "src/foo.rs",
        ));
        let (unresolved, ambiguous, _) = canonicalize_function_edges(&mut base);
        // Ground truth: the leaf is `baz`, the receiver is `Foo`, the
        // candidate type (after stripping generics) is `Foo`. This should
        // resolve. If rsplit is bracket-unaware, module_last becomes
        // `Vec<T>>` and strip_generic_args yields `Vec`, which != `Foo`.
        let resolved = base
            .iter()
            .any(|e| e.p == "calls" && e.a[0] == caller && e.a[1].ends_with("::baz"));
        assert!(
            resolved,
            "F2: bracket-unaware rsplit must not block resolution of \
             Foo<std::vec::Vec<T>>::baz (unresolved={unresolved}, \
             ambiguous={ambiguous}), base: {base:?}"
        );
    }

    #[test]
    fn t4_f3_deref_autoderef_typed_hint_blocks_inner_resolution() {
        // F3: `impl Wrapper` with `Deref<Target=Inner>`. `self.foo()`
        // from `Wrapper::bar` emits `@method:Wrapper:foo`. The candidate
        // is `Inner::foo` (module last segment `Inner`). The receiver
        // filter rejects `Inner != Wrapper`. Ground truth from the
        // review finding: ideally `Wrapper::bar` should still reach
        // `Inner::foo` via autoderef. The typed hint now BLOCKS that
        // unique resolution. We assert the CONSERVATIVE ground truth:
        // the call does NOT resolve (the fix is conservative, not
        // magic), AND we record this as a known limitation (false
        // negative relative to Rust's real method resolution).
        let caller = "rust:app::wrap::Wrapper::bar";
        let inner_foo = "rust:app::wrap::Inner::foo";
        let mut base = vec![];
        base.extend(method_def("src/wrap.rs", caller));
        base.extend(method_def("src/wrap.rs", inner_foo));
        base.push(Edge::base(
            "calls",
            &[caller, "@method:Wrapper:foo"],
            "src/wrap.rs",
        ));
        let (unresolved, ambiguous, _) = canonicalize_function_edges(&mut base);
        // The typed hint `Wrapper` does not match candidate `Inner`.
        // Conservatively, the call is unresolved (no Wrapper::foo def).
        assert_eq!(
            unresolved, 1,
            "F3: Wrapper::foo has no definition → unresolved (conservative)"
        );
        assert_eq!(ambiguous, 0, "F3: not ambiguous");
        assert!(
            !base
                .iter()
                .any(|e| e.p == "calls" && e.a == [caller, inner_foo]),
            "F3: typed hint blocks Inner::foo resolution (conservative false negative), \
             got: {base:?}"
        );
        // Record: this is a known limitation. The fix is conservative and
        // does NOT perform autoderef. This is acceptable (no guessing),
        // but it IS a false negative vs Rust's real resolution.
    }

    #[test]
    fn t4_f4_assoc_fn_and_method_sharing_leaf() {
        // F4: `Self::dup()` emits `@method:Foo:dup`. The same type has
        // both `fn dup() -> Foo` (assoc fn) and `fn dup(&self)` (method).
        // Both are defines_fn/defines_method with the same identity
        // `rust:app::foo::Foo::dup` (the graph does not distinguish
        // assoc fn from method). The receiver filter matches `Foo` for
        // both. But since they share the SAME identity, there is only
        // ONE candidate — resolved.len() == 1, not 2. So the call
        // resolves. If the graph DID emit two distinct identities, it
        // would be ambiguous. We assert ground truth: one candidate,
        // resolves.
        let caller = "rust:app::foo::Foo::make";
        let dup = "rust:app::foo::Foo::dup";
        let mut base = vec![];
        base.extend(method_def("src/foo.rs", caller));
        base.extend(method_def("src/foo.rs", dup));
        base.push(Edge::base(
            "calls",
            &[caller, "@method:Foo:dup"],
            "src/foo.rs",
        ));
        let (unresolved, ambiguous, _) = canonicalize_function_edges(&mut base);
        // Ground truth: only one candidate identity exists, so this
        // resolves unambiguously. The review finding's concern about
        // double-counting does not materialize because the graph uses a
        // single identity per function name.
        assert_eq!(unresolved, 0, "F4: one candidate → not unresolved");
        assert_eq!(ambiguous, 0, "F4: one candidate → not ambiguous");
        assert!(
            base.iter().any(|e| e.p == "calls" && e.a == [caller, dup]),
            "F4: Self::dup() should resolve to Foo::dup, got: {base:?}"
        );
    }

    // ─── T4 transitive reachability (G, H) ────────────────────────

    #[test]
    fn t4_g_transitive_test_reachability_through_resolved_self_calls() {
        // Case G: test_t -> A::a -> A::b -> A::c. All calls are typed
        // self-receiver hints that resolve. test_reachability (via
        // derive_all) must cover a, b, AND c.
        let a = "rust:app::a::A::a";
        let b = "rust:app::a::A::b";
        let c = "rust:app::a::A::c";
        let test = "rust:app#test:a::test_t";
        let mut base = vec![];
        base.extend(method_def("src/a.rs", a));
        base.extend(method_def("src/a.rs", b));
        base.extend(method_def("src/a.rs", c));
        base.push(tested(a, test));
        base.push(Edge::base("calls", &[a, "@method:A:b"], "src/a.rs"));
        base.push(Edge::base("calls", &[b, "@method:A:c"], "src/a.rs"));
        // Canonicalize first (as the sync pipeline does), then derive.
        let (unresolved, ambiguous, _) = canonicalize_function_edges(&mut base);
        assert_eq!(unresolved, 0, "Case G: all calls should resolve");
        assert_eq!(ambiguous, 0, "Case G: no ambiguity");
        let derived = derive_all(&base);
        let reaches: BTreeSet<String> = derived
            .iter()
            .filter(|e| e.p == "test_reaches" && e.a[0] == test)
            .map(|e| e.a[1].clone())
            .collect();
        assert!(
            reaches.contains(a),
            "Case G: reachability must cover a, got: {reaches:?}"
        );
        assert!(
            reaches.contains(b),
            "Case G: reachability must cover b, got: {reaches:?}"
        );
        assert!(
            reaches.contains(c),
            "Case G: reachability must cover c (transitive), got: {reaches:?}"
        );
    }

    #[test]
    fn t4_h_negative_control_unresolvable_call_stops_reachability() {
        // Case H: same fixture but a->b is unresolvable (bare hint, two
        // candidates). Reachability must stop at a — b and c are NOT
        // reached. This demonstrates why the original bug (dropping all
        // self-receiver calls) produced false zeros.
        let a = "rust:app::a::A::a";
        let b1 = "rust:app::a::A::b";
        let b2 = "rust:app::b::B::b";
        let c = "rust:app::a::A::c";
        let test = "rust:app#test:a::test_t";
        let mut base = vec![];
        base.extend(method_def("src/a.rs", a));
        base.extend(method_def("src/a.rs", b1));
        base.extend(method_def("src/b.rs", b2));
        base.extend(method_def("src/a.rs", c));
        base.push(imports("rust:app::a", "rust:app::b"));
        base.push(tested(a, test));
        // a -> b is BARE (no receiver type) → ambiguous, dropped
        base.push(Edge::base("calls", &[a, "@method:b"], "src/a.rs"));
        // b -> c would be transitive, but since a->b is dropped, c is
        // unreachable. Add the edge anyway to prove the point.
        base.push(Edge::base("calls", &[b1, "@method:A:c"], "src/a.rs"));
        let (unresolved, ambiguous, _) = canonicalize_function_edges(&mut base);
        assert_eq!(ambiguous, 1, "Case H: a->b is ambiguous (2 candidates)");
        assert_eq!(
            unresolved, 0,
            "Case H: the typed b->c edge still resolves, so nothing is unresolved"
        );
        let derived = derive_all(&base);
        let reaches: BTreeSet<String> = derived
            .iter()
            .filter(|e| e.p == "test_reaches" && e.a[0] == test)
            .map(|e| e.a[1].clone())
            .collect();
        assert!(
            reaches.contains(a),
            "Case H: reachability must cover a, got: {reaches:?}"
        );
        assert!(
            !reaches.contains(b1),
            "Case H: reachability must NOT cover b (a->b unresolved), got: {reaches:?}"
        );
        assert!(
            !reaches.contains(c),
            "Case H: reachability must NOT cover c (transitive gap), got: {reaches:?}"
        );
    }

    // ─── T4 integration fixture (I) ───────────────────────────────

    #[test]
    fn t4_i_integration_downstream_consumer_reach_parity() {
        // Case I: replicate the downstream-consumer shape. An impl method
        // calls `self.get_player_combatants()` where the same leaf is
        // defined on 2+ types across modules. Plus the reach parity
        // invariant:
        //   test_reaches(get_player_combatants) >= test_reaches(next_alive_enemy_id)
        // using those exact function names. No hard-coded counts.
        let caller = "rust:game::combat::CombatantList::resolve_target";
        let gpc_combat = "rust:game::combat::CombatantList::get_player_combatants";
        let gpc_party = "rust:game::party::PartyRoster::get_player_combatants";
        let nae = "rust:game::combat::CombatantList::next_alive_enemy_id";
        let test = "rust:game#test:combat::test_resolve";

        let mut base = vec![];
        base.extend(method_def("src/combat.rs", caller));
        base.extend(method_def("src/combat.rs", gpc_combat));
        base.extend(method_def("src/party.rs", gpc_party));
        base.extend(method_def("src/combat.rs", nae));
        // cross-module import so both get_player_combatants candidates
        // are visible
        base.push(imports("rust:game::combat", "rust:game::party"));
        // The test directly exercises `resolve_target` and
        // `next_alive_enemy_id`.
        base.push(tested(caller, test));
        base.push(tested(nae, test));
        // resolve_target calls self.get_player_combatants() — typed hint
        // `CombatantList` must disambiguate to the combat module's def.
        base.push(Edge::base(
            "calls",
            &[caller, "@method:CombatantList:get_player_combatants"],
            "src/combat.rs",
        ));
        // get_player_combatants calls next_alive_enemy_id via self
        base.push(Edge::base(
            "calls",
            &[gpc_combat, "@method:CombatantList:next_alive_enemy_id"],
            "src/combat.rs",
        ));
        let (unresolved, ambiguous, _) = canonicalize_function_edges(&mut base);
        assert_eq!(unresolved, 0, "Case I: no unresolved expected");
        assert_eq!(ambiguous, 0, "Case I: typed hints disambiguate");
        // Verify the typed hint resolved to the combat module's def, not party's
        assert!(
            base.iter()
                .any(|e| e.p == "calls" && e.a == [caller, gpc_combat]),
            "Case I: resolve_target -> CombatantList::get_player_combatants must exist, \
             got: {base:?}"
        );
        assert!(
            !base
                .iter()
                .any(|e| e.p == "calls" && e.a == [caller, gpc_party]),
            "Case I: must NOT resolve to PartyRoster::get_player_combatants, got: {base:?}"
        );
        let derived = derive_all(&base);
        let reaches: BTreeSet<String> = derived
            .iter()
            .filter(|e| e.p == "test_reaches" && e.a[0] == test)
            .map(|e| e.a[1].clone())
            .collect();
        // Reach parity invariant:
        //   test_reaches(get_player_combatants) >= test_reaches(next_alive_enemy_id)
        // Both are directly tested, and gpc is transitively reached from
        // resolve_target. The invariant holds as a set-containment check.
        let gpc_reached = reaches
            .iter()
            .any(|f| f.ends_with("::get_player_combatants") && f.starts_with("rust:game::combat"));
        let nae_reached = reaches
            .iter()
            .any(|f| f.ends_with("::next_alive_enemy_id") && f.starts_with("rust:game::combat"));
        assert!(
            gpc_reached,
            "Case I: test_reaches(get_player_combatants) must be true, got: {reaches:?}"
        );
        assert!(
            nae_reached,
            "Case I: test_reaches(next_alive_enemy_id) must be true, got: {reaches:?}"
        );
        // Parity: if next_alive_enemy_id is reached, get_player_combatants
        // must also be reached (it is a transitive predecessor in the
        // call chain). The invariant: gpc_reached >= nae_reached.
        assert!(
            gpc_reached || !nae_reached,
            "Case I: reach parity violated — next_alive_enemy_id reached \
             but get_player_combatants not, got: {reaches:?}"
        );
        // Exact-name assertions using the fixture function names.
        assert!(
            reaches.contains(gpc_combat),
            "Case I: test_reaches must contain the combat get_player_combatants, \
             got: {reaches:?}"
        );
        assert!(
            reaches.contains(nae),
            "Case I: test_reaches must contain next_alive_enemy_id, got: {reaches:?}"
        );
    }
}
