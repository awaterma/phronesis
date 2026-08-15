//! Ownership enrichment end to end through a real `graph rebuild`
//! (SPEC-rust-ownership-evidence §13.2, decisions D9/D12/D16).
//!
//! Every other ownership test seeds a graph by hand. These do not: they write
//! Rust sources and a `.phronesis/graph.toml` into a tempdir, call the real
//! `sync::rebuild` / `sync::on_save`, and read the persisted
//! `.phronesis/graph.jsonl` back off disk. That is the only way to catch the
//! failure this suite existed to miss — for an entire round the extractor, the
//! config loader, and the provider all worked and passed their unit tests while
//! nothing called any of them, so enabling `[ownership.rust]` produced exactly
//! zero edges.
//!
//! Three properties are pinned here and nowhere else:
//!
//! 1. **Opt-in is byte-exact.** With ownership off, the persisted graph is
//!    byte-identical to the same project with no config file at all. That is
//!    §13.2's first integration test and the entire basis of the opt-in claim.
//! 2. **Enabled means persisted.** Edges survive `store::compact`, the
//!    derivation pass, and a `graph.jsonl` round trip with a non-empty `src`.
//! 3. **Incremental saves self-heal.** A single-file save replaces that file's
//!    ownership sites rather than accumulating them (D12), and marks the
//!    file's compiler evidence stale rather than carrying a previous
//!    rebuild's conclusions forward (D9).

use std::path::Path;

use phronesis_mcp::graph::model::Edge;
use phronesis_mcp::graph::ownership as own;
use phronesis_mcp::graph::ownership::provider::{AnalysisTrigger, RustAnalyzerProvider};
use phronesis_mcp::graph::store;
use phronesis_mcp::graph::sync;
use tempfile::TempDir;

/// A Rust source with one site of every kind the AST provider can observe.
const CHAIN_SRC: &str = r#"
pub struct Party { pub members: Vec<String> }

pub fn filtered_clone(xs: &[String]) -> Vec<String> {
    xs.iter().filter(|x| !x.is_empty()).cloned().collect::<Vec<_>>()
}

pub async fn snapshot_then_await(party: &Party) -> usize {
    let snapshot = party.members.clone();
    tokio::task::yield_now().await;
    snapshot.len()
}
"#;

fn project(files: &[(&str, &str)]) -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    for (rel, body) in files {
        let path = dir.path().join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(path, body).expect("write source");
    }
    dir
}

fn enable_ownership(root: &Path, body: &str) {
    let path = root.join(".phronesis/graph.toml");
    std::fs::create_dir_all(path.parent().expect("config parent")).expect("mkdir .phronesis");
    std::fs::write(path, body).expect("write graph.toml");
}

fn graph(root: &Path) -> Vec<Edge> {
    store::load(&store::graph_path(root)).expect("load persisted graph")
}

fn ownership_edges(root: &Path) -> Vec<Edge> {
    graph(root)
        .into_iter()
        .filter(|edge| own::OWNERSHIP_RELATIONS.contains(&edge.p.as_str()))
        .collect()
}

fn args_of<'a>(edges: &'a [Edge], relation: &str) -> Vec<&'a Vec<String>> {
    edges
        .iter()
        .filter(|edge| edge.p == relation)
        .map(|edge| &edge.a)
        .collect()
}

// The opt-in claim in one assertion. §4.2 makes ownership an enrichment rather
// than an expansion of the Rust pack precisely because sites are intra-function
// detail that multiplies edge volume, so a project that has not asked for it
// must get a graph indistinguishable from today's — not merely one without
// ownership relations, but the same bytes.
#[test]
fn a_rebuild_with_ownership_disabled_writes_a_byte_identical_graph() {
    let without = project(&[("src/lib.rs", CHAIN_SRC)]);
    sync::rebuild(without.path()).expect("rebuild without any config");
    let baseline =
        std::fs::read_to_string(store::graph_path(without.path())).expect("read baseline graph");

    let off = project(&[("src/lib.rs", CHAIN_SRC)]);
    enable_ownership(
        off.path(),
        "[ownership.rust]\nenabled = false\nprovider = \"rust-analyzer\"\n",
    );
    sync::rebuild(off.path()).expect("rebuild with ownership disabled");
    let disabled =
        std::fs::read_to_string(store::graph_path(off.path())).expect("read disabled graph");

    assert_eq!(
        disabled, baseline,
        "an explicitly disabled [ownership.rust] section must not change one byte of the graph"
    );
    assert!(
        ownership_edges(off.path()).is_empty(),
        "disabled ownership must emit no ownership relation at all (§13.2)"
    );
}

// The headline: enabling the section makes a real rebuild produce and persist
// evidence. Before this wiring existed, `rebuild` called the extractor with a
// hardcoded disabled config, so this assertion failed on every relation.
#[test]
fn enabling_ownership_persists_ast_evidence_through_a_real_rebuild() {
    let dir = project(&[("src/lib.rs", CHAIN_SRC)]);
    enable_ownership(
        dir.path(),
        "[ownership.rust]\nenabled = true\nprovider = \"ast\"\ninclude = [\"src/**/*.rs\"]\n",
    );
    sync::rebuild(dir.path()).expect("rebuild with ownership enabled");

    let edges = ownership_edges(dir.path());
    assert!(
        !edges.is_empty(),
        "an enabled [ownership.rust] section must produce ownership edges through rebuild"
    );

    for relation in [
        own::OWNERSHIP_SITE,
        own::OWNERSHIP_SITE_IN_FUNCTION,
        own::OWNERSHIP_SITE_SPAN,
        own::CLONE_SITE,
        own::FILTER_SITE,
        own::AWAIT_SITE,
        own::OWNERSHIP_EVIDENCE,
        own::OWNERSHIP_ANALYSIS_STATUS,
        own::FILTER_BEFORE_CLONE,
        own::CLONE_BEFORE_AWAIT,
    ] {
        assert!(
            edges.iter().any(|edge| edge.p == relation),
            "{relation} must be persisted for this fixture; got {:?}",
            edges.iter().map(|e| e.p.as_str()).collect::<Vec<_>>()
        );
    }

    // D12: `src` is the only compaction key, so an ownership edge without one
    // is unreachable by file replacement and becomes permanently stale.
    for edge in &edges {
        assert_eq!(
            edge.src, "src/lib.rs",
            "every ownership edge carries its source file as provenance (D12): {edge:?}"
        );
        assert!(
            !edge.d,
            "ownership edges are base edges; store::compact silently discards fresh derived edges (D12): {edge:?}"
        );
    }

    // Addendum A.1: every site must reach a function the graph actually
    // defines, or the explanation query has a dangling hop.
    let defined: Vec<String> = graph(dir.path())
        .iter()
        .filter(|edge| edge.p == "defines_fn")
        .filter_map(|edge| edge.a.get(1).cloned())
        .collect();
    for args in args_of(&edges, own::OWNERSHIP_SITE_IN_FUNCTION) {
        let function = args
            .get(1)
            .expect("site-in-function has a function argument");
        assert!(
            defined.contains(function),
            "site function {function} must be a real defines_fn identity (§15, D21); defined: {defined:?}"
        );
    }

    // The evidence level is what stops an AST observation reading as a
    // compiler claim, so it must survive persistence intact.
    for args in args_of(&edges, own::OWNERSHIP_EVIDENCE) {
        assert_eq!(
            args.get(1).map(String::as_str),
            Some("ast"),
            "the tree-sitter provider may only claim `ast` evidence"
        );
        assert_eq!(
            args.get(2).map(String::as_str),
            Some(own::PROVIDER_TREE_SITTER_RUST),
            "the provider name is part of the claim"
        );
    }
}

// §13.1 and D19: the worst failure this feature can have is AST evidence
// silently promoted to a compiler claim. A rebuild over real sources is the
// place a copy-paste would show up.
#[test]
fn a_rebuild_never_persists_a_compiler_only_relation_from_ast_evidence() {
    let dir = project(&[("src/lib.rs", CHAIN_SRC)]);
    enable_ownership(
        dir.path(),
        "[ownership.rust]\nenabled = true\nprovider = \"ast\"\n",
    );
    sync::rebuild(dir.path()).expect("rebuild");

    let names: Vec<String> = graph(dir.path()).into_iter().map(|edge| edge.p).collect();
    for forbidden in [
        "lock_scope_may_cross_await",
        "ownership_transfer",
        "borrow_live_across",
        "ownership_conflict_diagnostic",
        "clone_cost_evidence",
        own::RESOLVED_TYPE,
    ] {
        assert!(
            !names.iter().any(|name| name == forbidden),
            "{forbidden} is not derivable from AST evidence and must never be persisted"
        );
    }
}

// D16: include/exclude filter `sync::tracked_files`. They must never trigger a
// walk of their own — an independent walk indexes files the freshness check can
// never match, which is drift that nothing heals.
#[test]
fn include_and_exclude_filter_the_tracked_walk_rather_than_widening_it() {
    let dir = project(&[
        ("src/lib.rs", CHAIN_SRC),
        ("src/vendor/generated.rs", CHAIN_SRC),
    ]);
    enable_ownership(
        dir.path(),
        "[ownership.rust]\nenabled = true\ninclude = [\"src/**/*.rs\"]\nexclude = [\"src/vendor/**\"]\n",
    );
    sync::rebuild(dir.path()).expect("rebuild");

    let sources: Vec<String> = ownership_edges(dir.path())
        .into_iter()
        .map(|edge| edge.src)
        .collect();
    assert!(
        sources.iter().any(|src| src == "src/lib.rs"),
        "an included file must produce ownership edges"
    );
    assert!(
        !sources.iter().any(|src| src.starts_with("src/vendor/")),
        "an excluded file must produce none: {sources:?}"
    );
}

// D12 and §15: file replacement removes stale ownership sites for free,
// *because* `src` is the compaction key. Accumulating sites across saves would
// leave site ids pointing at byte offsets the file no longer has.
#[test]
fn an_incremental_save_replaces_that_files_ownership_sites_and_leaves_no_stale_ones() {
    let dir = project(&[("src/lib.rs", CHAIN_SRC)]);
    enable_ownership(
        dir.path(),
        "[ownership.rust]\nenabled = true\nprovider = \"ast\"\n",
    );
    sync::rebuild(dir.path()).expect("rebuild");
    let before = args_of(&ownership_edges(dir.path()), own::OWNERSHIP_SITE).len();
    assert!(before > 0, "the rebuild must have indexed sites to replace");

    let edited = "pub fn only_one(v: &String) -> String { v.clone() }\n";
    std::fs::write(dir.path().join("src/lib.rs"), edited).expect("edit source");
    sync::on_save(dir.path(), "src/lib.rs", edited).expect("incremental save");

    let edges = ownership_edges(dir.path());
    let sites = args_of(&edges, own::OWNERSHIP_SITE);
    assert_eq!(
        sites.len(),
        1,
        "the edited file's sites are replaced, never accumulated: {sites:?}"
    );
    let clones = args_of(&edges, own::CLONE_SITE);
    assert_eq!(
        clones.len(),
        1,
        "exactly the one clone the new content contains: {clones:?}"
    );
    assert!(
        !edges.iter().any(|edge| edge.p == own::FILTER_SITE
            || edge.p == own::AWAIT_SITE
            || edge.p == own::FILTER_BEFORE_CLONE),
        "sites from the replaced content must be gone: {edges:?}"
    );
}

// D9. Compiler results are full-rebuild-only (§9), so an incremental edit
// leaves the previous rebuild's type/MIR conclusions describing bytes they
// never saw. Nothing constructed this edge before; without it there is no way
// to say "stale" at all, and the old conclusions simply read as current.
#[test]
fn an_incremental_save_marks_the_files_compiler_evidence_stale() {
    let dir = project(&[("src/lib.rs", CHAIN_SRC)]);
    enable_ownership(
        dir.path(),
        "[ownership.rust]\nenabled = true\nprovider = \"rust-analyzer\"\n",
    );
    sync::rebuild(dir.path()).expect("rebuild");

    let edited = "pub fn only_one(v: &String) -> String { v.clone() }\n";
    std::fs::write(dir.path().join("src/lib.rs"), edited).expect("edit source");
    sync::on_save(dir.path(), "src/lib.rs", edited).expect("incremental save");

    let statuses: Vec<Vec<String>> =
        args_of(&ownership_edges(dir.path()), own::OWNERSHIP_ANALYSIS_STATUS)
            .into_iter()
            .cloned()
            .collect();
    for capability in ["type_inference", "mir_lowering"] {
        assert!(
            statuses.iter().any(|args| args
                == &vec![
                    "src/lib.rs".to_string(),
                    capability.to_string(),
                    "stale".to_string(),
                    "incremental_edit".to_string(),
                ]),
            "D9 requires ownership_analysis_status(<file>, {capability}, stale, incremental_edit): {statuses:?}"
        );
    }
    assert!(
        !statuses.iter().any(
            |args| args.first().map(String::as_str) == Some("src/lib.rs")
                && args.get(1).map(String::as_str) == Some("type_inference")
                && args.get(2).map(String::as_str) != Some("stale")
        ),
        "the stale status replaces the prior compiler status for this file rather than joining it: {statuses:?}"
    );
}

// §8.2, structurally. The provider may run during an explicit rebuild and
// nowhere else; the incremental save above must not have invoked it. If it
// had, the file would carry `unavailable`/`tool_missing` observations from a
// hook-time run instead of D9's `stale`.
#[test]
fn the_incremental_save_path_cannot_invoke_the_compiler_provider() {
    let config = phronesis_mcp::graph::ownership::config::parse(
        "[ownership.rust]\nenabled = true\nprovider = \"rust-analyzer\"\n",
    )
    .expect("parse config");
    for trigger in [
        AnalysisTrigger::PreCheck,
        AnalysisTrigger::PostCheck,
        AnalysisTrigger::Hydration,
        AnalysisTrigger::IncrementalUpdate,
    ] {
        assert!(
            RustAnalyzerProvider::for_rebuild(&config, trigger).is_none(),
            "{trigger:?} must have no provider value to call (§8.2)"
        );
    }

    let dir = project(&[("src/lib.rs", CHAIN_SRC)]);
    enable_ownership(
        dir.path(),
        "[ownership.rust]\nenabled = true\nprovider = \"rust-analyzer\"\n",
    );
    // No rebuild first: this graph has only ever seen the save path, so any
    // compiler observation in it could only have come from a hook-time run.
    sync::on_save(dir.path(), "src/lib.rs", CHAIN_SRC).expect("incremental save");

    let statuses: Vec<Vec<String>> =
        args_of(&ownership_edges(dir.path()), own::OWNERSHIP_ANALYSIS_STATUS)
            .into_iter()
            .cloned()
            .collect();
    assert!(
        !statuses.iter().any(|args| {
            matches!(
                args.get(3).map(String::as_str),
                Some(
                    "tool_missing"
                        | "no_structured_interface"
                        | "project_load_failed"
                        | "provider_error"
                )
            )
        }),
        "no provider reason may appear on a graph built entirely by the save path: {statuses:?}"
    );
}

// §8.2's diagnostics requirement. Build scripts and procedural macros are
// disabled by default, and the run must say so rather than let the caller
// assume the analysis was macro-complete. This test must pass on a machine
// with rust-analyzer installed and on one without, so it asserts the
// limitation, never the availability.
#[test]
fn a_rebuild_with_the_compiler_provider_reports_its_disabled_analysis() {
    let dir = project(&[("src/lib.rs", CHAIN_SRC)]);
    enable_ownership(
        dir.path(),
        "[ownership.rust]\nenabled = true\nprovider = \"rust-analyzer\"\n",
    );
    let outcome = sync::rebuild(dir.path()).expect("rebuild with the compiler provider");
    assert!(
        outcome
            .diagnostics
            .contains(&"rust_analyzer:build_scripts_disabled".to_string()),
        "disabled build scripts must be recorded in rebuild diagnostics: {:?}",
        outcome.diagnostics
    );
    assert!(
        outcome
            .diagnostics
            .contains(&"rust_analyzer:proc_macros_disabled".to_string()),
        "disabled proc macros must be recorded in rebuild diagnostics: {:?}",
        outcome.diagnostics
    );

    // Availability-only (D10): whatever this machine has installed, the
    // provider contributes status edges and nothing that could be read as a
    // resolved type or a MIR relationship.
    let statuses: Vec<Vec<String>> =
        args_of(&ownership_edges(dir.path()), own::OWNERSHIP_ANALYSIS_STATUS)
            .into_iter()
            .cloned()
            .collect();
    assert!(
        statuses
            .iter()
            .any(|args| args.get(1).map(String::as_str) == Some("mir_lowering")),
        "the provider must report on MIR lowering explicitly; silence would read as a clean result (Goal 3): {statuses:?}"
    );
    assert!(
        !graph(dir.path())
            .iter()
            .any(|edge| edge.p == own::RESOLVED_TYPE),
        "the Phase One provider is availability-only and never resolves a type (D10)"
    );
}

// A rebuild with `provider = "ast"` asks for no compiler enrichment at all, so
// it must not synthesize compiler statuses either. Reporting `unavailable` for
// a capability nobody requested would fill the query surface with noise that
// looks like a degraded analysis.
#[test]
fn the_ast_provider_alone_adds_no_compiler_capability_status() {
    let dir = project(&[("src/lib.rs", CHAIN_SRC)]);
    enable_ownership(
        dir.path(),
        "[ownership.rust]\nenabled = true\nprovider = \"ast\"\n",
    );
    let outcome = sync::rebuild(dir.path()).expect("rebuild");
    assert!(
        outcome.diagnostics.is_empty(),
        "no provider ran, so there is no provider limitation to report: {:?}",
        outcome.diagnostics
    );

    let capabilities: Vec<String> =
        args_of(&ownership_edges(dir.path()), own::OWNERSHIP_ANALYSIS_STATUS)
            .into_iter()
            .filter_map(|args| args.get(1).cloned())
            .collect();
    assert!(
        capabilities.iter().all(|c| c == "ast_extraction"),
        "provider = ast reports only on ast_extraction: {capabilities:?}"
    );
}

// §13.2: enabled ownership edges must hydrate into RETE with graph provenance,
// which is what makes Goal 7 reachable and what a future project-authored rule
// would match on. A relation missing from `hydrate::GRAPH_RELATIONS` is
// persisted and queryable but never becomes a fact — a silent failure.
#[test]
fn persisted_ownership_edges_carry_graph_file_provenance_into_facts() {
    let dir = project(&[("src/lib.rs", CHAIN_SRC)]);
    enable_ownership(
        dir.path(),
        "[ownership.rust]\nenabled = true\nprovider = \"ast\"\n",
    );
    sync::rebuild(dir.path()).expect("rebuild");

    let edges = ownership_edges(dir.path());
    assert!(!edges.is_empty(), "the rebuild must have produced edges");
    for edge in &edges {
        let fact = edge.to_fact();
        assert_eq!(
            fact.source.as_deref(),
            Some("graph:src/lib.rs"),
            "Addendum A.4 requires per-file graph provenance, not graph:structural: {edge:?}"
        );
    }
}
