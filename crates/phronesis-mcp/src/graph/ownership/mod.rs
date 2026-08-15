//! Evidence-backed Rust ownership relations.
//!
//! Implements `docs/specs/SPEC-rust-ownership-evidence.md` as amended by
//! `docs/specs/SPEC-rust-ownership-evidence-DECISIONS.md`. The enrichment is
//! opt-in (`[ownership.rust]` in `.phronesis/graph.toml`) and produces
//! *evidence*, never verdicts: a site says an operation was observed, and a
//! separate `ownership_evidence` edge says at what strength it was observed.
//!
//! Two invariants govern every producer in this module:
//!
//! 1. **Everything is a base edge** (decision D12). The spec calls four of
//!    these relations "derived", but `store::compact` silently discards fresh
//!    edges with `d: true`, and `derive::derive_all` works from the edge set
//!    alone — it has no syntax tree, so it cannot recompute an expression
//!    chain, a root place, or a narrowest enclosing block. Every ownership
//!    edge is therefore `Edge::base(relation, args, <repo-relative file>)`
//!    with a non-empty `src`. That also makes provenance-keyed compaction
//!    remove stale sites for free, and keeps fact provenance at
//!    `graph:<file>` rather than `graph:structural`.
//! 2. **The AST path never emits a compiler-only relation.** In particular
//!    `lock_scope_may_cross_await`, `ownership_transfer`, `borrow_live_across`,
//!    and `ownership_conflict_diagnostic` have no constant here, because no
//!    Phase One code path may produce them.

pub mod config;
pub mod extract;
pub mod provider;
pub mod query;

/// Declares an ownership observation site. `[site]`.
pub const OWNERSHIP_SITE: &str = "ownership_site";
/// Associates a site with one canonical function. `[site, function]`.
pub const OWNERSHIP_SITE_IN_FUNCTION: &str = "ownership_site_in_function";
/// Exact source span of the anchoring expression.
/// `[site, file, start_byte, end_byte]` — offsets are decimal strings.
pub const OWNERSHIP_SITE_SPAN: &str = "ownership_site_span";
/// An observed ownership-producing call. `[site, operation, operand]`.
pub const CLONE_SITE: &str = "clone_site";
/// An observed iterator filter operation. `[site, operand]`.
pub const FILTER_SITE: &str = "filter_site";
/// An await expression. `[site]`.
pub const AWAIT_SITE: &str = "await_site";
/// A bounded mutation operation. `[site, operation, place]`.
pub const MUTATION_SITE: &str = "mutation_site";
/// A synchronous lock/read/write acquisition. `[site, operation, guard]`.
pub const SYNC_LOCK_SITE: &str = "sync_lock_site";
/// Evidence available for a site, relationship, or function.
/// `[subject, level, provider]`.
pub const OWNERSHIP_EVIDENCE: &str = "ownership_evidence";
/// Explicit provider result, including unavailable or bounded analysis.
/// `[subject, capability, status, reason]`.
pub const OWNERSHIP_ANALYSIS_STATUS: &str = "ownership_analysis_status";
/// A compiler-aware provider resolved the relevant type. `[site, type]`.
pub const RESOLVED_TYPE: &str = "resolved_type";

/// A clone-producing operation shares a receiver chain with a filter.
/// `[function, filter_site, clone_site]`. Emitted as a **base** edge (D12).
pub const FILTER_BEFORE_CLONE: &str = "filter_before_clone";
/// A clone site lexically precedes an await in the same function.
/// `[function, clone_site, await_site]`. Emitted as a **base** edge (D12).
pub const CLONE_BEFORE_AWAIT: &str = "clone_before_await";
/// A bounded read site lexically precedes a mutation of the same root place.
/// `[function, read_site, mutation_site]`. Emitted as a **base** edge (D12).
pub const READ_BEFORE_MUTATION: &str = "read_before_mutation";
/// The guard's scope ends before an await: a bound guard's lexical block
/// closes first, the guard is explicitly dropped first, or an unbound
/// temporary's enclosing statement ends first (D6). Absence of this edge is
/// not evidence of a hazard — it may only mean no boundary was establishable.
/// `[function, lock_site, await_site]`. Emitted as a **base** edge (D12).
pub const LOCK_SCOPE_ENDS_BEFORE_AWAIT: &str = "lock_scope_ends_before_await";

/// Every relation Phase One may emit.
///
/// Registration is mechanical and each omission fails silently: a name absent
/// from `hydrate::GRAPH_RELATIONS` is persisted and queryable but never
/// hydrates into RETE, and a name absent from `audit::QUERY_ONLY_RELATIONS`
/// can become an audit headline. The unit tests below pin both.
pub const OWNERSHIP_RELATIONS: &[&str] = &[
    OWNERSHIP_SITE,
    OWNERSHIP_SITE_IN_FUNCTION,
    OWNERSHIP_SITE_SPAN,
    CLONE_SITE,
    FILTER_SITE,
    AWAIT_SITE,
    MUTATION_SITE,
    SYNC_LOCK_SITE,
    OWNERSHIP_EVIDENCE,
    OWNERSHIP_ANALYSIS_STATUS,
    RESOLVED_TYPE,
    FILTER_BEFORE_CLONE,
    CLONE_BEFORE_AWAIT,
    READ_BEFORE_MUTATION,
    LOCK_SCOPE_ENDS_BEFORE_AWAIT,
];

/// Closed set of `ownership_evidence` levels (spec §6.1).
///
/// New providers are configuration additions, never new levels: a level is a
/// claim about how strong the observation is, and the whole point of the
/// feature is that AST evidence is never silently upgraded.
pub const EVIDENCE_LEVELS: &[&str] = &["ast", "type_resolved", "mir", "diagnostic", "runtime"];

/// Closed set of `ownership_analysis_status` capabilities (spec §6.1).
pub const ANALYSIS_CAPABILITIES: &[&str] = &["ast_extraction", "type_inference", "mir_lowering"];

/// Closed set of `ownership_analysis_status` statuses.
///
/// `stale` is decision D9's amendment: §9 requires incremental edits to mark
/// compiler evidence stale and Addendum A.4 requires that state to be visible
/// in CLI and MCP output, but §6.1's four-value enum cannot express it.
pub const ANALYSIS_STATUSES: &[&str] = &["available", "partial", "unavailable", "failed", "stale"];

/// Provider name for the bounded tree-sitter extractor.
pub const PROVIDER_TREE_SITTER_RUST: &str = "tree_sitter_rust";
/// Provider name for the compiler-aware provider.
pub const PROVIDER_RUST_ANALYZER: &str = "rust_analyzer";

#[cfg(test)]
mod tests {
    use super::*;

    // Pins the on-disk wire names. A rename is a graph-format change, and a
    // silent one would leave every persisted edge unmatched by every query.
    #[test]
    fn relation_constants_keep_their_exact_wire_names() {
        assert_eq!(OWNERSHIP_SITE, "ownership_site", "site relation name");
        assert_eq!(
            OWNERSHIP_SITE_IN_FUNCTION, "ownership_site_in_function",
            "site-to-function relation name"
        );
        assert_eq!(
            OWNERSHIP_SITE_SPAN, "ownership_site_span",
            "span relation name"
        );
        assert_eq!(CLONE_SITE, "clone_site", "clone relation name");
        assert_eq!(FILTER_SITE, "filter_site", "filter relation name");
        assert_eq!(AWAIT_SITE, "await_site", "await relation name");
        assert_eq!(MUTATION_SITE, "mutation_site", "mutation relation name");
        assert_eq!(SYNC_LOCK_SITE, "sync_lock_site", "lock relation name");
        assert_eq!(
            OWNERSHIP_EVIDENCE, "ownership_evidence",
            "evidence relation name"
        );
        assert_eq!(
            OWNERSHIP_ANALYSIS_STATUS, "ownership_analysis_status",
            "analysis-status relation name"
        );
        assert_eq!(
            RESOLVED_TYPE, "resolved_type",
            "resolved-type relation name"
        );
        assert_eq!(
            FILTER_BEFORE_CLONE, "filter_before_clone",
            "filter-before-clone relation name"
        );
        assert_eq!(
            CLONE_BEFORE_AWAIT, "clone_before_await",
            "clone-before-await relation name"
        );
        assert_eq!(
            READ_BEFORE_MUTATION, "read_before_mutation",
            "read-before-mutation relation name"
        );
        assert_eq!(
            LOCK_SCOPE_ENDS_BEFORE_AWAIT, "lock_scope_ends_before_await",
            "lock-scope relation name"
        );
        assert_eq!(
            PROVIDER_TREE_SITTER_RUST, "tree_sitter_rust",
            "AST provider name"
        );
        assert_eq!(
            PROVIDER_RUST_ANALYZER, "rust_analyzer",
            "compiler provider name"
        );
    }

    // Pins the closed value sets, including D9's `stale` amendment. Dropping
    // `stale` would leave an incremental edit no way to say that compiler
    // evidence belongs to an older generation, and Addendum A.4 requires that
    // state to be renderable.
    #[test]
    fn closed_value_sets_include_the_amended_stale_status() {
        assert_eq!(
            EVIDENCE_LEVELS,
            &["ast", "type_resolved", "mir", "diagnostic", "runtime"],
            "evidence levels are a closed set"
        );
        assert_eq!(
            ANALYSIS_CAPABILITIES,
            &["ast_extraction", "type_inference", "mir_lowering"],
            "analysis capabilities are a closed set"
        );
        assert_eq!(
            ANALYSIS_STATUSES,
            &["available", "partial", "unavailable", "failed", "stale"],
            "analysis statuses include D9's stale amendment"
        );
    }

    // Pins D18: a relation missing from GRAPH_RELATIONS is persisted and
    // queryable but never hydrates into RETE, which silently breaks Goal 7.
    #[test]
    fn every_ownership_relation_is_registered_for_hydration() {
        for relation in OWNERSHIP_RELATIONS {
            assert!(
                crate::graph::hydrate::GRAPH_RELATIONS.contains(relation),
                "{relation} must be in hydrate::GRAPH_RELATIONS or it never reaches RETE"
            );
        }
    }

    // Pins spec §11: Phase One relations are query-only, and membership in
    // QUERY_ONLY_RELATIONS is the only thing that keeps them out of audit.
    #[test]
    fn every_ownership_relation_is_query_only() {
        for relation in OWNERSHIP_RELATIONS {
            assert!(
                crate::graph::audit::QUERY_ONLY_RELATIONS.contains(relation),
                "{relation} must be query-only until precision is measured (§11)"
            );
        }
    }

    // Pins §6.2 and D6: no compiler-only relation may be nameable from this
    // module, because the AST path must have no code path that emits one.
    #[test]
    fn compiler_only_relations_have_no_constant_in_the_ast_module() {
        for forbidden in [
            "lock_scope_may_cross_await",
            "ownership_transfer",
            "borrow_live_across",
            "ownership_conflict_diagnostic",
            "clone_cost_evidence",
        ] {
            assert!(
                !OWNERSHIP_RELATIONS.contains(&forbidden),
                "{forbidden} is not emittable from AST evidence"
            );
        }
    }

    // Pins that the registration list itself has no duplicates or accidental
    // omissions — it is what the two registration tests above iterate.
    #[test]
    fn the_relation_list_is_the_complete_deduplicated_phase_one_set() {
        let unique: std::collections::BTreeSet<&&str> = OWNERSHIP_RELATIONS.iter().collect();
        assert_eq!(
            unique.len(),
            OWNERSHIP_RELATIONS.len(),
            "OWNERSHIP_RELATIONS must not repeat a name"
        );
        assert_eq!(
            OWNERSHIP_RELATIONS.len(),
            15,
            "Phase One emits eleven base and four base-encoded ordering relations"
        );
    }
}
