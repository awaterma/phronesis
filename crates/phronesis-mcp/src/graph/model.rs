//! Structural graph edges: the on-disk unit of `.phronesis/graph.jsonl`.
//!
//! Each edge is one relation instance. The relation is the *fact predicate*
//! (`defines_fn`, `calls_api`, …) rather than a generic `triple` wrapper, so
//! the engine's `predicate_index` keeps alpha memories small and per-relation.
//! See `docs/specs/SPEC-triple-store-rete.md` §1.

use phr::Fact;
use serde::{Deserialize, Serialize};

/// One directed structural relation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    /// Relation name; becomes `Fact::predicate`.
    pub p: String,
    /// Relation arguments; becomes `Fact::args`.
    pub a: Vec<String>,
    /// Provenance: the source file whose extraction produced this edge.
    /// Compaction is keyed on this, never on the edge's subject — most
    /// subjects are functions, not files (§3.1).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub src: String,
    /// Derived edges are a function of the whole graph, not of any one file,
    /// so compaction regenerates them wholesale rather than attributing them
    /// to a provenance (§4.5).
    #[serde(default, skip_serializing_if = "is_false")]
    pub d: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Edge {
    /// A base edge attributed to the file it was parsed from.
    pub fn base(p: &str, args: &[&str], src: &str) -> Self {
        Self {
            p: p.to_string(),
            a: args.iter().map(|s| s.to_string()).collect(),
            src: src.to_string(),
            d: false,
        }
    }

    /// A derived edge, computed over the full edge set.
    pub fn derived(p: &str, args: &[&str]) -> Self {
        Self {
            p: p.to_string(),
            a: args.iter().map(|s| s.to_string()).collect(),
            src: String::new(),
            d: true,
        }
    }

    /// Stable identity for the engine, so re-asserting the same structural
    /// edge replaces rather than duplicates it.
    pub fn fact_id(&self) -> String {
        format!("graph:{}:{}", self.p, self.a.join("\u{1f}"))
    }

    /// Hydrate into an engine fact while retaining the bounded graph producer.
    pub fn to_fact(&self) -> Fact {
        Fact {
            id: self.fact_id(),
            predicate: self.p.clone(),
            args: self.a.clone(),
            timestamp: 0,
            source: Some(if self.src.is_empty() {
                "graph:structural".to_string()
            } else {
                format!("graph:{}", self.src)
            }),
        }
    }
}

/// Parse a `graph.jsonl` body. Malformed lines are skipped and counted rather
/// than aborting the load: a single bad line must not take the whole
/// enforcement layer offline.
pub fn parse_jsonl(body: &str) -> (Vec<Edge>, usize) {
    let mut edges = Vec::new();
    let mut skipped = 0;
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<Edge>(line) {
            Ok(e) => edges.push(e),
            Err(_) => skipped += 1,
        }
    }
    (edges, skipped)
}

/// Render edges as newline-delimited JSON, one edge per line.
pub fn to_jsonl(edges: &[Edge]) -> String {
    let mut out = String::new();
    for e in edges {
        if let Ok(line) = serde_json::to_string(e) {
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_edge_round_trips_through_jsonl() {
        let edges = vec![Edge::base(
            "defines_fn",
            &["src/network.rs", "crate::network::fire"],
            "src/network.rs",
        )];
        let (parsed, skipped) = parse_jsonl(&to_jsonl(&edges));
        assert_eq!(skipped, 0);
        assert_eq!(parsed, edges);
    }

    #[test]
    fn derived_edge_round_trips_and_keeps_derived_flag() {
        let edges = vec![Edge::derived("no_direct_test", &["crate::network::fire"])];
        let (parsed, _) = parse_jsonl(&to_jsonl(&edges));
        assert_eq!(parsed, edges);
        assert!(parsed[0].d);
        assert!(parsed[0].src.is_empty());
    }

    #[test]
    fn malformed_lines_are_skipped_and_counted() {
        let body = "{\"p\":\"imports\",\"a\":[\"a\",\"b\"],\"src\":\"a\"}\nnot json\n{\n";
        let (parsed, skipped) = parse_jsonl(body);
        assert_eq!(parsed.len(), 1);
        assert_eq!(skipped, 2);
    }

    #[test]
    fn edge_becomes_fact_with_relation_as_predicate() {
        let fact = Edge::base("calls_api", &["crate::f", "std::unwrap"], "src/f.rs").to_fact();
        assert_eq!(fact.predicate, "calls_api");
        assert_eq!(fact.args, vec!["crate::f", "std::unwrap"]);
    }

    #[test]
    fn graph_source_is_metadata_not_a_relation_argument() {
        let fact = Edge::base("defines_fn", &["src/f.rs", "crate::f"], "src/f.rs").to_fact();
        assert!(!fact.args.contains(&"src/f.rs".to_string()) || fact.args.len() == 2);
        assert_eq!(fact.args.len(), 2, "src must not be appended to args");
        assert_eq!(fact.source.as_deref(), Some("graph:src/f.rs"));
    }

    #[test]
    fn same_relation_yields_same_fact_id() {
        let a = Edge::base("imports", &["x", "y"], "x.rs");
        let b = Edge::base("imports", &["x", "y"], "other.rs");
        assert_eq!(a.fact_id(), b.fact_id());
    }

    #[test]
    fn different_relations_yield_different_fact_ids() {
        let a = Edge::base("imports", &["x", "y"], "x.rs");
        let b = Edge::base("imports", &["x", "z"], "x.rs");
        assert_ne!(a.fact_id(), b.fact_id());
    }
}
