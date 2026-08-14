//! Ad-hoc querying of the code graph.
//!
//! Until now the only way to ask the graph a question was to write a rule and
//! trip it at the hook, or run a whole-tree audit. Both answer "is this
//! forbidden?" Neither answers "what is true?" — so questions the graph can
//! trivially answer (which tests cover this function, what depends on this
//! module) required hand-parsing the JSONL.
//!
//! The query surface deliberately mirrors the fact shape — relation plus
//! positional arguments, `*` for a wildcard — rather than inventing a
//! question vocabulary. One concept to learn, and it composes with any
//! relation added later instead of needing a new verb per question.

use super::model::Edge;

/// A relation pattern. `None` in any position matches anything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pattern {
    pub relation: Option<String>,
    /// Positional argument constraints; shorter than an edge's args is fine —
    /// unconstrained trailing positions match anything.
    pub args: Vec<Option<String>>,
}

fn glob_matches(pattern: &str, value: &str) -> bool {
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let (mut p, mut v) = (0, 0);
    let (mut star, mut retry) = (None, 0);

    while v < value.len() {
        if p < pattern.len() && (pattern[p] == b'?' || pattern[p] == value[v]) {
            p += 1;
            v += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            p += 1;
            retry = v;
        } else if let Some(star_pos) = star {
            p = star_pos + 1;
            retry += 1;
            v = retry;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

impl Pattern {
    /// Parse CLI-style tokens: the first is the relation, the rest are
    /// arguments. `*` (or `?`) in any position means "anything".
    pub fn parse(tokens: &[String]) -> Self {
        let wild = |s: &String| -> Option<String> {
            if s == "*" || s == "?" {
                None
            } else {
                Some(s.clone())
            }
        };
        let mut it = tokens.iter();
        let relation = it.next().and_then(wild);
        Pattern {
            relation,
            args: it.map(wild).collect(),
        }
    }

    fn matches(&self, e: &Edge) -> bool {
        if let Some(r) = &self.relation
            && !glob_matches(r, &e.p)
        {
            return false;
        }
        for (i, want) in self.args.iter().enumerate() {
            let Some(want) = want else { continue };
            match e.a.get(i) {
                Some(got) if glob_matches(want, got) => {}
                _ => return false,
            }
        }
        true
    }
}

/// Edges matching `pattern`, in stored order, capped at `limit` (0 = all).
pub fn query<'a>(edges: &'a [Edge], pattern: &Pattern, limit: usize) -> Vec<&'a Edge> {
    let it = edges.iter().filter(|e| pattern.matches(e));
    if limit == 0 {
        it.collect()
    } else {
        it.take(limit).collect()
    }
}

/// Total matches, ignoring any limit. Reported alongside truncated results so
/// a capped list never reads as a complete answer.
pub fn count(edges: &[Edge], pattern: &Pattern) -> usize {
    edges.iter().filter(|e| pattern.matches(e)).count()
}

/// Relation names present in the graph with their edge counts, so a caller
/// can discover the vocabulary without consulting the spec.
pub fn relation_summary(edges: &[Edge]) -> Vec<(String, usize)> {
    let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for e in edges {
        *counts.entry(e.p.as_str()).or_insert(0) += 1;
    }
    let mut out: Vec<(String, usize)> = counts
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph() -> Vec<Edge> {
        vec![
            Edge::base("defines_fn", &["src/a.rs", "crate::a::f"], "src/a.rs"),
            Edge::base("defines_fn", &["src/b.rs", "crate::b::g"], "src/b.rs"),
            Edge::base("tested_by", &["f", "t::one"], "tests/t.rs"),
            Edge::base("tested_by", &["f", "t::two"], "tests/t.rs"),
            Edge::base("imports", &["crate::a", "crate::b"], "src/a.rs"),
            Edge::derived("no_direct_test", &["crate::b::g"]),
        ]
    }

    fn toks(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn a_relation_filter_selects_only_that_relation() {
        let g = graph();
        let p = Pattern::parse(&toks(&["tested_by"]));
        assert_eq!(query(&g, &p, 0).len(), 2);
    }

    #[test]
    fn a_positional_argument_narrows_the_match() {
        let g = graph();
        let p = Pattern::parse(&toks(&["defines_fn", "src/a.rs"]));
        let got = query(&g, &p, 0);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].a[1], "crate::a::f");
    }

    #[test]
    fn a_wildcard_matches_any_value_in_that_position() {
        let g = graph();
        // "which tests cover f?" — constrain arg 0, leave arg 1 open.
        let p = Pattern::parse(&toks(&["tested_by", "f", "*"]));
        assert_eq!(query(&g, &p, 0).len(), 2);
    }

    #[test]
    fn a_wildcard_relation_searches_every_relation() {
        let g = graph();
        // "what does the graph know about crate::b::g?"
        let p = Pattern::parse(&toks(&["*", "crate::b::g"]));
        assert_eq!(query(&g, &p, 0).len(), 1);
    }

    #[test]
    fn embedded_globs_match_relations_and_arguments() {
        let g = graph();
        let p = Pattern::parse(&toks(&["defines_*", "src/?.rs", "*::g"]));
        let got = query(&g, &p, 0);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].a[0], "src/b.rs");
        assert_eq!(count(&g, &p), 1);
    }

    #[test]
    fn ordinary_tokens_still_match_exactly() {
        let g = graph();
        let p = Pattern::parse(&toks(&["defines", "src/a.rs"]));
        assert!(query(&g, &p, 0).is_empty());
    }

    #[test]
    fn standalone_question_mark_remains_a_whole_position_wildcard() {
        let g = graph();
        let p = Pattern::parse(&toks(&["tested_by", "f", "?"]));
        assert_eq!(query(&g, &p, 1).len(), 1);
        assert_eq!(count(&g, &p), 2);
    }

    #[test]
    fn a_later_position_can_be_constrained_alone() {
        let g = graph();
        // "what imports crate::b?" — the reverse-dependency question.
        let p = Pattern::parse(&toks(&["imports", "*", "crate::b"]));
        let got = query(&g, &p, 0);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].a[0], "crate::a");
    }

    #[test]
    fn an_unmatched_value_yields_nothing() {
        let g = graph();
        let p = Pattern::parse(&toks(&["defines_fn", "src/nope.rs"]));
        assert!(query(&g, &p, 0).is_empty());
    }

    #[test]
    fn an_unknown_relation_yields_nothing_rather_than_erroring() {
        let g = graph();
        let p = Pattern::parse(&toks(&["not_a_relation"]));
        assert!(query(&g, &p, 0).is_empty());
    }

    #[test]
    fn derived_edges_are_queryable_like_any_other() {
        let g = graph();
        let p = Pattern::parse(&toks(&["no_direct_test"]));
        assert_eq!(query(&g, &p, 0).len(), 1);
    }

    #[test]
    fn the_limit_caps_returned_rows() {
        let g = graph();
        let p = Pattern::parse(&toks(&["tested_by"]));
        assert_eq!(query(&g, &p, 1).len(), 1);
    }

    #[test]
    fn the_count_ignores_the_limit_so_truncation_is_visible() {
        let g = graph();
        // A capped list that reported its own length as the total would read
        // as a complete answer.
        let p = Pattern::parse(&toks(&["tested_by"]));
        assert_eq!(query(&g, &p, 1).len(), 1);
        assert_eq!(count(&g, &p), 2);
    }

    #[test]
    fn an_empty_pattern_matches_everything() {
        let g = graph();
        assert_eq!(query(&g, &Pattern::default(), 0).len(), 6);
    }

    #[test]
    fn the_relation_summary_lists_the_vocabulary_by_frequency() {
        let g = graph();
        let s = relation_summary(&g);
        assert_eq!(s[0], ("defines_fn".to_string(), 2));
        assert!(s.iter().any(|(r, _)| r == "no_direct_test"));
    }
}
