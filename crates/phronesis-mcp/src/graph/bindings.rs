//! Durable rule-to-code bindings used to detect referent-gone staleness.
//!
//! A literal becomes binding evidence only after one of its identifier runs
//! resolves to a local `defines_fn` edge. This transition-based model avoids
//! treating foreign names such as `unwrap` as stale merely because the graph
//! has never defined them.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::model::Edge;
use crate::rules_file::{SourceRule, WhenClause};

pub const BINDINGS_REL_PATH: &str = ".phronesis/bindings.json";
pub const BINDINGS_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingState {
    Bound,
    Moved,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub rule: String,
    pub rule_hash: String,
    pub symbol: String,
    pub bound_to: Vec<String>,
    pub surviving: Vec<String>,
    pub relocated: Vec<String>,
    pub bound_at: i64,
    pub stale_at: Option<i64>,
    pub state: BindingState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindingSet {
    pub version: u32,
    pub generation: u64,
    pub bindings: Vec<Binding>,
}

impl Default for BindingSet {
    fn default() -> Self {
        Self {
            version: BINDINGS_VERSION,
            generation: 0,
            bindings: Vec::new(),
        }
    }
}

pub fn bindings_path(root: &Path) -> PathBuf {
    root.join(BINDINGS_REL_PATH)
}

/// Extract conservative, unqualified function-call candidates from
/// `new_content_contains` literals.
///
/// A bare word is not binding evidence: prose such as "should work now" and
/// syntax such as `deny(warnings)` routinely collide with unrelated local
/// function names. Qualified calls (`console.log(`, `.unwrap()`,
/// `env::set_var(`) are also excluded because the graph records definition
/// leaves but cannot prove that receiver or namespace resolves locally.
pub fn extract(rule: &SourceRule) -> Vec<String> {
    if rule.binds == Some(false) {
        return Vec::new();
    }
    let mut out = BTreeSet::new();
    extract_clauses(&rule.when, &mut out);
    out.into_iter().collect()
}

fn extract_clauses(clauses: &[WhenClause], out: &mut BTreeSet<String>) {
    for clause in clauses {
        match clause {
            WhenClause::Or(alts) => extract_clauses(alts, out),
            WhenClause::Leaf(condition) if condition.predicate == "new_content_contains" => {
                for literal in &condition.args {
                    extract_call_candidates(literal, out);
                }
            }
            WhenClause::Leaf(_) => {}
        }
    }
}

fn extract_call_candidates(literal: &str, out: &mut BTreeSet<String>) {
    let bytes = literal.as_bytes();
    let mut start = 0;
    while start < bytes.len() {
        if bytes[start] != b'_' && !bytes[start].is_ascii_alphabetic() {
            start += 1;
            continue;
        }
        let mut end = start + 1;
        while end < bytes.len() && (bytes[end] == b'_' || bytes[end].is_ascii_alphanumeric()) {
            end += 1;
        }

        let qualified = start > 0 && matches!(bytes[start - 1], b'.' | b':' | b'[');
        if !qualified && call_follows(bytes, end) {
            out.insert(literal[start..end].to_string());
        }
        start = end;
    }
}

/// Whether an opening parenthesis follows `from`, past whitespace and an
/// optional regex escape (regex-authored rules may escape the parenthesis).
fn call_follows(bytes: &[u8], from: usize) -> bool {
    let mut next = from;
    while next < bytes.len() && bytes[next].is_ascii_whitespace() {
        next += 1;
    }
    if next < bytes.len() && bytes[next] == b'\\' {
        next += 1;
    }
    next < bytes.len() && bytes[next] == b'('
}

/// Stable hash of the canonical serialized rule form.
pub fn rule_hash(rule: &SourceRule) -> String {
    let canonical = serde_json::to_string(rule).unwrap_or_default();
    format!("{:016x}", super::sync::hash_content(&canonical))
}

fn definitions(edges: &[Edge]) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for edge in edges.iter().filter(|edge| edge.p == "defines_fn") {
        let Some(path) = edge.a.get(1) else { continue };
        let leaf = path.rsplit("::").next().unwrap_or(path);
        out.entry(leaf.to_string())
            .or_default()
            .insert(path.clone());
    }
    out
}

/// Identity of one reconciliation pass: the graph generation it binds
/// against and the wall-clock time it records.
#[derive(Debug, Clone, Copy)]
pub struct ReconcileStamp {
    pub generation: u64,
    pub now: i64,
}

/// Reconcile prior binding history with the current rules and graph.
///
/// Thin shim over [`reconcile_with`] kept for existing callers.
pub fn reconcile(
    persisted: &BindingSet,
    rules: &[SourceRule],
    edges: &[Edge],
    generation: u64,
    now: i64,
) -> BindingSet {
    reconcile_with(persisted, rules, edges, ReconcileStamp { generation, now })
}

/// Reconcile prior binding history with the current rules and graph.
pub fn reconcile_with(
    persisted: &BindingSet,
    rules: &[SourceRule],
    edges: &[Edge],
    stamp: ReconcileStamp,
) -> BindingSet {
    let defs = definitions(edges);
    let rule_data: BTreeMap<_, _> = rules
        .iter()
        .map(|rule| {
            let candidates: BTreeSet<_> = extract(rule).into_iter().collect();
            (rule.id.as_str(), (rule_hash(rule), candidates))
        })
        .collect();
    let mut bindings = Vec::new();
    let mut existing_keys = BTreeSet::new();

    for prior in &persisted.bindings {
        let Some((hash, candidates)) = rule_data.get(prior.rule.as_str()) else {
            continue;
        };
        if hash != &prior.rule_hash || !candidates.contains(&prior.symbol) {
            continue;
        }
        existing_keys.insert((prior.rule.clone(), prior.symbol.clone()));
        bindings.push(carry_forward(prior, &defs, stamp.now));
    }

    for (rule, hash) in rules.iter().map(|rule| (rule, rule_hash(rule))) {
        let key = RuleKey {
            id: &rule.id,
            hash: &hash,
        };
        for symbol in extract(rule) {
            if existing_keys.contains(&(rule.id.clone(), symbol.clone())) {
                continue;
            }
            let Some(paths) = defs.get(&symbol).filter(|paths| !paths.is_empty()) else {
                continue;
            };
            bindings.push(fresh_binding(key, symbol, paths, stamp.now));
        }
    }
    bindings.sort_by(|a, b| (&a.rule, &a.symbol).cmp(&(&b.rule, &b.symbol)));
    BindingSet {
        version: BINDINGS_VERSION,
        generation: stamp.generation,
        bindings,
    }
}

/// Re-evaluate a prior binding against the current definitions.
fn carry_forward(prior: &Binding, defs: &BTreeMap<String, BTreeSet<String>>, now: i64) -> Binding {
    let current = defs.get(&prior.symbol).cloned().unwrap_or_default();
    let original: BTreeSet<_> = prior.bound_to.iter().cloned().collect();
    let surviving: Vec<_> = original.intersection(&current).cloned().collect();
    let relocated: Vec<_> = current.difference(&original).cloned().collect();
    let state = if !surviving.is_empty() {
        BindingState::Bound
    } else if !relocated.is_empty() {
        BindingState::Moved
    } else {
        BindingState::Stale
    };
    let stale_at = match state {
        BindingState::Stale => prior.stale_at.or(Some(now)),
        BindingState::Bound | BindingState::Moved => None,
    };
    Binding {
        surviving,
        relocated,
        state,
        stale_at,
        ..prior.clone()
    }
}

/// A rule's id and content hash, as recorded on its bindings.
#[derive(Clone, Copy)]
struct RuleKey<'a> {
    id: &'a str,
    hash: &'a str,
}

/// A binding for a symbol the graph defines and no prior binding covers.
fn fresh_binding(rule: RuleKey<'_>, symbol: String, paths: &BTreeSet<String>, now: i64) -> Binding {
    let bound_to: Vec<_> = paths.iter().cloned().collect();
    Binding {
        rule: rule.id.to_string(),
        rule_hash: rule.hash.to_string(),
        symbol,
        surviving: bound_to.clone(),
        bound_to,
        relocated: Vec::new(),
        bound_at: now,
        stale_at: None,
        state: BindingState::Bound,
    }
}

pub fn load(path: &Path) -> std::io::Result<Option<BindingSet>> {
    match std::fs::read_to_string(path) {
        Ok(body) => match serde_json::from_str::<BindingSet>(&body) {
            Ok(set) if set.version == BINDINGS_VERSION => Ok(Some(set)),
            Ok(_) | Err(_) => Ok(None),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Graph-sync loader: quarantine malformed or unknown-version state before
/// rebuilding it. Hook-time callers use [`load`] and remain read-only.
pub fn load_recovering(path: &Path) -> std::io::Result<Option<BindingSet>> {
    let body = match std::fs::read_to_string(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        result => result?,
    };
    match serde_json::from_str::<BindingSet>(&body) {
        Ok(set) if set.version == BINDINGS_VERSION => Ok(Some(set)),
        Ok(_) | Err(_) => {
            let backup = path.with_extension("json.bak");
            if backup.exists() {
                std::fs::remove_file(&backup)?;
            }
            std::fs::rename(path, backup)?;
            Ok(None)
        }
    }
}

pub fn store_atomic(path: &Path, set: &BindingSet) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    {
        let mut file = std::fs::File::create(&tmp)?;
        serde_json::to_writer_pretty(&mut file, set)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
    }
    std::fs::rename(tmp, path)
}

/// Stale rule IDs safe to demote for this exact graph generation.
///
/// Guards are intentionally ordered: a stale/outdated graph or a generation
/// mismatch returns no demotions, preserving full enforcement. The bindings
/// file is consulted first because it is one cheap read that almost every
/// project fails — freshness re-hashes the whole indexed tree, a cost the
/// binding-less majority must not pay on every hook.
///
/// `graph_verified_fresh` lets a caller that already hashed the tree this
/// invocation (hydration) vouch for freshness instead of paying the pass
/// twice. Pass `false` when no such verification happened.
pub fn stale_rules(root: &Path, graph_verified_fresh: bool) -> BTreeMap<phr::RuleId, Vec<String>> {
    let set = match load(&bindings_path(root)) {
        Ok(Some(set)) if !set.bindings.is_empty() => set,
        _ => return BTreeMap::new(),
    };
    let index = match super::sync::load_index(&super::sync::index_path(root)) {
        Ok(index) => index,
        Err(_) => return BTreeMap::new(),
    };
    if set.generation != index.generation {
        return BTreeMap::new();
    }
    if !graph_verified_fresh
        && super::sync::check_freshness(root, &index) != super::sync::Freshness::Fresh
    {
        return BTreeMap::new();
    }
    let mut grouped: BTreeMap<&str, Vec<&Binding>> = BTreeMap::new();
    for binding in &set.bindings {
        grouped.entry(&binding.rule).or_default().push(binding);
    }
    grouped
        .into_iter()
        .filter(|(_, bindings)| {
            !bindings.is_empty()
                && bindings
                    .iter()
                    .all(|binding| binding.state == BindingState::Stale)
        })
        .map(|(rule, bindings)| {
            (
                phr::RuleId::from(rule),
                bindings
                    .into_iter()
                    .map(|binding| binding.symbol.clone())
                    .collect(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn rule(id: &str, literal: &str) -> SourceRule {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "phase": "pre",
            "when": [{"new_content_contains": literal}],
            "then": {"block": "blocked"}
        }))
        .expect("rule")
    }

    fn defined(path: &str) -> Edge {
        Edge::base("defines_fn", &["src/lib.rs", path], "src/lib.rs")
    }

    #[test]
    fn extraction_keeps_only_unqualified_call_shapes() {
        assert_eq!(
            extract(&rule("r", ".foo().await Result<_, String>")),
            Vec::<String>::new()
        );
        assert_eq!(extract(&rule("r", "legacy_call()")), ["legacy_call"]);
        assert_eq!(extract(&rule("r", r"legacy_call\(")), ["legacy_call"]);
    }

    #[test]
    fn prose_and_unrelated_syntax_do_not_bind() {
        for literal in [
            "not from our changes",
            "should work now",
            "git commit -m",
            "#![deny(warnings)]",
            "console.log(",
            ".unwrap()",
            "env::set_var(",
        ] {
            assert!(extract(&rule("r", literal)).is_empty(), "{literal}");
        }
    }

    #[test]
    fn bash_regex_contributes_no_candidates() {
        let source: SourceRule = serde_json::from_value(serde_json::json!({
            "id": "r", "when": [{"bash_command_matches": "cargo.*foo"}],
            "then": {"block": "blocked"}
        }))
        .expect("rule");
        assert!(extract(&source).is_empty());
    }

    #[test]
    fn unresolved_foreign_symbol_is_never_bound() {
        let set = reconcile(
            &BindingSet::default(),
            &[rule("r", ".unwrap()")],
            &[],
            1,
            10,
        );
        assert!(set.bindings.is_empty());
    }

    #[test]
    fn vanished_definition_becomes_stale_and_keeps_original_path() {
        let rules = [rule("r", "foo(")];
        let first = reconcile(
            &BindingSet::default(),
            &rules,
            &[defined("crate::A::foo")],
            1,
            10,
        );
        let stale = reconcile(&first, &rules, &[], 2, 20);
        assert_eq!(stale.bindings[0].state, BindingState::Stale);
        assert_eq!(stale.bindings[0].bound_to, ["crate::A::foo"]);
        assert_eq!(stale.bindings[0].stale_at, Some(20));
    }

    #[test]
    fn same_leaf_elsewhere_is_moved_not_stale_and_never_rewrites_origin() {
        let rules = [rule("r", "foo(")];
        let first = reconcile(
            &BindingSet::default(),
            &rules,
            &[defined("crate::A::foo")],
            1,
            10,
        );
        let moved = reconcile(&first, &rules, &[defined("crate::B::foo")], 2, 20);
        assert_eq!(moved.bindings[0].state, BindingState::Moved);
        assert_eq!(moved.bindings[0].bound_to, ["crate::A::foo"]);
        assert_eq!(moved.bindings[0].relocated, ["crate::B::foo"]);
    }

    #[test]
    fn original_path_recovers_a_stale_binding() {
        let rules = [rule("r", "foo(")];
        let first = reconcile(
            &BindingSet::default(),
            &rules,
            &[defined("crate::A::foo")],
            1,
            10,
        );
        let stale = reconcile(&first, &rules, &[], 2, 20);
        let recovered = reconcile(&stale, &rules, &[defined("crate::A::foo")], 3, 30);
        assert_eq!(recovered.bindings[0].state, BindingState::Bound);
        assert_eq!(recovered.bindings[0].stale_at, None);
        assert_eq!(recovered.bindings[0].bound_at, 10);
    }

    #[test]
    fn pre_reconcile_stamp_v1_fixture_remains_idempotent_and_marks_stale_once() {
        let dir = TempDir::new().expect("tempdir");
        let path = bindings_path(dir.path());
        std::fs::create_dir_all(path.parent().expect("bindings parent")).expect("mkdir");
        std::fs::write(
            &path,
            include_str!("../../tests/fixtures/graph/bindings-v1.json"),
        )
        .expect("legacy fixture");

        let persisted = load_recovering(&path)
            .expect("load legacy bindings")
            .expect("supported v1 fixture");
        let rules = [rule("legacy-call", "legacy_call(")];
        assert_eq!(persisted.bindings[0].rule_hash, rule_hash(&rules[0]));

        let unchanged = reconcile(
            &persisted,
            &rules,
            &[defined("crate::old::legacy_call")],
            7,
            200,
        );
        assert_eq!(
            unchanged, persisted,
            "same graph must be an idempotent load/reconcile"
        );

        let stale = reconcile(&unchanged, &rules, &[], 8, 300);
        assert_eq!(stale.bindings[0].state, BindingState::Stale);
        assert_eq!(stale.bindings[0].stale_at, Some(300));
        assert_eq!(stale.bindings[0].bound_to, ["crate::old::legacy_call"]);

        let still_stale = reconcile(&stale, &rules, &[], 9, 400);
        assert_eq!(still_stale.bindings[0].stale_at, Some(300));
    }

    #[test]
    fn changed_rule_content_discards_old_binding() {
        let first_rule = [rule("r", "foo(")];
        let first = reconcile(
            &BindingSet::default(),
            &first_rule,
            &[defined("crate::A::foo")],
            1,
            10,
        );
        let changed = [rule("r", "bar(")];
        let next = reconcile(&first, &changed, &[], 2, 20);
        assert!(next.bindings.is_empty());
    }

    #[test]
    fn binds_false_suppresses_candidate_extraction() {
        let mut source = rule("r", "foo(");
        source.binds = Some(false);
        assert!(extract(&source).is_empty());
    }

    fn fresh_project(generation: u64, set: &BindingSet) -> TempDir {
        let dir = TempDir::new().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");
        let body = "fn foo() {}";
        std::fs::write(dir.path().join("src/lib.rs"), body).expect("source");
        super::super::sync::save_index(
            &super::super::sync::index_path(dir.path()),
            &super::super::sync::Index {
                format: super::super::sync::GRAPH_FORMAT,
                generation,
                entries: BTreeMap::from([(
                    "src/lib.rs".to_string(),
                    super::super::sync::hash_content(body),
                )]),
            },
        )
        .expect("index");
        store_atomic(&bindings_path(dir.path()), set).expect("bindings");
        dir
    }

    fn binding(rule: &str, symbol: &str, state: BindingState) -> Binding {
        Binding {
            rule: rule.to_string(),
            rule_hash: "hash".to_string(),
            symbol: symbol.to_string(),
            bound_to: vec![format!("crate::{symbol}")],
            surviving: Vec::new(),
            relocated: Vec::new(),
            bound_at: 10,
            stale_at: (state == BindingState::Stale).then_some(20),
            state,
        }
    }

    #[test]
    fn stale_rules_requires_every_binding_for_a_rule_to_be_stale() {
        let set = BindingSet {
            version: BINDINGS_VERSION,
            generation: 4,
            bindings: vec![
                binding("all-stale", "foo", BindingState::Stale),
                binding("mixed", "foo", BindingState::Stale),
                binding("mixed", "bar", BindingState::Bound),
            ],
        };
        let dir = fresh_project(4, &set);
        let ids = stale_rules(dir.path(), false);
        assert!(ids.contains_key(&phr::RuleId::from("all-stale")));
        assert!(!ids.contains_key(&phr::RuleId::from("mixed")));
    }

    #[test]
    fn generation_mismatch_disables_all_demotions() {
        let set = BindingSet {
            version: BINDINGS_VERSION,
            generation: 3,
            bindings: vec![binding("r", "foo", BindingState::Stale)],
        };
        let dir = fresh_project(4, &set);
        assert!(stale_rules(dir.path(), false).is_empty());
    }

    #[test]
    fn stale_graph_disables_all_binding_demotions() {
        let set = BindingSet {
            version: BINDINGS_VERSION,
            generation: 4,
            bindings: vec![binding("r", "foo", BindingState::Stale)],
        };
        let dir = fresh_project(4, &set);
        std::fs::write(dir.path().join("src/lib.rs"), "fn changed() {}").expect("drift");
        assert!(stale_rules(dir.path(), false).is_empty());
    }
}
