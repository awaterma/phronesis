use super::evidence::line_of;
use super::*;
use crate::graph::ownership::extract;
use crate::graph::ownership::*;

fn edge(relation: &str, args: &[&str]) -> Edge {
    Edge::base(relation, args, "src/scheduler.rs")
}

const F: &str = "rust:demo::llm::scheduler::Scheduler::acquire";
const LOCK: &str = "rust:demo::llm::scheduler::Scheduler::acquire#ownership:lock:1200";
const AWAIT: &str = "rust:demo::llm::scheduler::Scheduler::acquire#ownership:await:2400";

fn scheduler_graph() -> Vec<Edge> {
    vec![
        edge(OWNERSHIP_SITE, &[LOCK]),
        edge(OWNERSHIP_SITE_IN_FUNCTION, &[LOCK, F]),
        edge(
            OWNERSHIP_SITE_SPAN,
            &[LOCK, "src/scheduler.rs", "1200", "1240"],
        ),
        edge(SYNC_LOCK_SITE, &[LOCK, "lock", "guard"]),
        edge(OWNERSHIP_EVIDENCE, &[LOCK, "ast", "tree_sitter_rust"]),
        edge(OWNERSHIP_SITE, &[AWAIT]),
        edge(OWNERSHIP_SITE_IN_FUNCTION, &[AWAIT, F]),
        edge(
            OWNERSHIP_SITE_SPAN,
            &[AWAIT, "src/scheduler.rs", "2400", "2420"],
        ),
        edge(AWAIT_SITE, &[AWAIT]),
        edge(OWNERSHIP_EVIDENCE, &[AWAIT, "ast", "tree_sitter_rust"]),
        edge(LOCK_SCOPE_ENDS_BEFORE_AWAIT, &[F, LOCK, AWAIT]),
        edge(
            OWNERSHIP_ANALYSIS_STATUS,
            &[F, "ast_extraction", "available", extract::REASON_COMPLETE],
        ),
        edge(
            OWNERSHIP_ANALYSIS_STATUS,
            &[F, "type_inference", "available", "rust_analyzer"],
        ),
        edge(
            OWNERSHIP_ANALYSIS_STATUS,
            &[F, "mir_lowering", "unavailable", "async_lowering"],
        ),
    ]
}

fn on() -> Availability {
    Availability {
        ownership_enabled: true,
        graph_present: true,
    }
}

fn build_all(edges: &[Edge], pattern: &str) -> OwnershipReport {
    build(edges, pattern, 0, on())
}

// Pins the three empty states apart. `store::load` returns an empty vector
// for a missing file, so "no graph" and "empty graph" are the same value —
// conflating them would tell a user to rebuild a graph they already have,
// or worse, read as "your code is clean".
#[test]
fn the_three_empty_states_are_distinguishable_and_never_read_as_clean() {
    let disabled = build(
        &scheduler_graph(),
        "*",
        0,
        Availability {
            ownership_enabled: false,
            graph_present: true,
        },
    );
    assert_eq!(
        disabled.state,
        OwnershipState::Disabled,
        "a project without [ownership.rust] must report disabled"
    );
    assert!(
        disabled.message.contains("[ownership.rust]"),
        "the disabled message must say how to enable it: {}",
        disabled.message
    );

    let no_graph = build(
        &[],
        "*",
        0,
        Availability {
            ownership_enabled: true,
            graph_present: false,
        },
    );
    assert_eq!(
        no_graph.state,
        OwnershipState::NoGraph,
        "an absent graph file must not read as an empty result"
    );
    assert!(
        no_graph.message.contains("graph rebuild"),
        "the no-graph message must name the fix: {}",
        no_graph.message
    );

    let no_match = build(&scheduler_graph(), "rust:other::*", 0, on());
    assert_eq!(
        no_match.state,
        OwnershipState::NoMatch,
        "a graph that matched nothing is its own state"
    );
    assert!(
        no_match
            .message
            .contains("No indexed ownership evidence found"),
        "empty must render as absence of evidence: {}",
        no_match.message
    );
    assert!(
        no_match.message.contains("not proof"),
        "empty must never read as proof the code is clean: {}",
        no_match.message
    );
}

// The extractor writes the `reason` argument and this renderer prints it
// verbatim, so the two agree only by convention. They disagreed for the
// whole time nothing was wired: the extractor emitted `complete` while
// every fixture here said `extracted`, and the sample rendering in §10 of
// the spec read `AST: available (extracted)`. `complete` won. This test is
// the join — it renders the constant the extractor actually emits and
// asserts the exact string a user sees, so a rename on either side fails
// here rather than shipping a value no fixture matches.
#[test]
fn the_successful_ast_status_reason_renders_as_the_exact_string_complete() {
    assert_eq!(
        extract::REASON_COMPLETE,
        "complete",
        "the successful ast_extraction reason is the wire value `complete`"
    );
    let mut edges = scheduler_graph();
    edges.push(edge(
        OWNERSHIP_ANALYSIS_STATUS,
        &[
            "src/scheduler.rs",
            extract::CAPABILITY_AST_EXTRACTION,
            extract::STATUS_AVAILABLE,
            extract::REASON_COMPLETE,
        ],
    ));
    let table = render_table(&build_all(&edges, F));
    assert!(
        table.contains("AST: available (complete) for src/scheduler.rs"),
        "the extractor's own reason constant must render verbatim: {table}"
    );
    assert!(
        !table.contains("extracted"),
        "`extracted` is the retired spelling and must not reappear: {table}"
    );
}

// §9 records the site cap and D9's stale compiler generation against the
// *file*, not the function. Those must surface on every function in that
// file, and must say which subject they are about — a file-scoped partial
// means something different from a function-scoped one.
#[test]
fn file_scoped_partial_and_stale_statuses_surface_on_the_functions_in_that_file() {
    let mut edges = scheduler_graph();
    edges.push(edge(
        OWNERSHIP_ANALYSIS_STATUS,
        &["src/scheduler.rs", "ast_extraction", "partial", "site_cap"],
    ));
    edges.push(edge(
        OWNERSHIP_ANALYSIS_STATUS,
        &[
            "src/scheduler.rs",
            "type_inference",
            "stale",
            "incremental_edit",
        ],
    ));
    let report = build_all(&edges, F);
    let table = render_table(&report);
    assert!(
        table.contains("AST: partial (site_cap) for src/scheduler.rs"),
        "a file-scoped site cap must render and name its subject: {table}"
    );
    assert!(
        table.contains("type inference: stale (incremental_edit) for src/scheduler.rs"),
        "D9's stale status must be visible in the output: {table}"
    );
    assert!(
        table.contains("AST: available (complete)\n"),
        "a function-scoped status must not be cluttered with its own name: {table}"
    );
    assert!(
        report.functions[0]
            .limits
            .iter()
            .any(|l| l.contains("partial, stale, failed, or unavailable")),
        "a degraded capability must be named in the limits"
    );
}

// Pins Addendum A.1: the traversal from a derived relationship to its
// supporting sites, and from each site to span, evidence level, and
// provider. Losing any hop leaves a relationship the user cannot check.
#[test]
fn a_relationship_names_its_supporting_sites_with_span_and_provider() {
    let report = build_all(&scheduler_graph(), F);
    let function = &report.functions[0];
    let relation = &function.relationships[0];
    assert_eq!(
        relation.relation, LOCK_SCOPE_ENDS_BEFORE_AWAIT,
        "the scheduler fixture derives one lock-scope relation"
    );
    assert_eq!(
        relation.supported_by.len(),
        2,
        "both site arguments must be named"
    );
    assert_eq!(
        relation.supported_by[0].site, LOCK,
        "the first argument is the lock site"
    );
    assert!(
        relation.supported_by.iter().all(|s| s.resolved),
        "both supporting sites resolve to indexed evidence"
    );
    assert!(
        relation.supported_by[0]
            .summary
            .contains("src/scheduler.rs"),
        "a supporting site must carry its source location: {}",
        relation.supported_by[0].summary
    );
    assert_eq!(
        relation.evidence,
        vec![EvidenceRef {
            level: "ast".to_string(),
            provider: "tree_sitter_rust".to_string(),
        }],
        "relationship evidence is the union of its sites' evidence, never stronger"
    );
}

// Pins Addendum A.4: an unavailable capability must be rendered next to
// the positive findings. Dropping it makes AST-only evidence look
// corroborated, which is the single worst failure this feature can have.
#[test]
fn unavailable_capabilities_render_alongside_the_positive_facts() {
    let table = render_table(&build_all(&scheduler_graph(), F));
    assert!(
        table.contains("AST: available"),
        "positive AST status must render: {table}"
    );
    assert!(
        table.contains("MIR: unavailable (async_lowering)"),
        "the unavailable MIR capability must render with its reason: {table}"
    );
    assert!(
        table.contains("lexical scope is not general control-flow or borrow-liveness proof"),
        "the §10 limit line must render: {table}"
    );
}

// A capability with no status edge at all is the quietest way to make weak
// evidence look strong: nothing is wrong, there is simply no line.
#[test]
fn a_capability_with_no_recorded_status_renders_as_not_reported() {
    let edges: Vec<Edge> = scheduler_graph()
        .into_iter()
        .filter(|e| e.a.get(1).map(String::as_str) != Some("mir_lowering"))
        .collect();
    let report = build_all(&edges, F);
    assert_eq!(
        report.functions[0].analysis_not_reported,
        vec!["mir_lowering".to_string()],
        "a capability with no status edge must be listed as unreported"
    );
    let table = render_table(&report);
    assert!(
        table.contains("MIR: not reported"),
        "an unreported capability must still occupy a line: {table}"
    );
}

// Pins that an exact function ID and an embedded glob both select, the
// same way every other graph query behaves (§13.2).
#[test]
fn exact_and_embedded_glob_function_queries_both_select() {
    let edges = scheduler_graph();
    let exact = build_all(&edges, F);
    assert_eq!(
        exact.matched_functions, 1,
        "an exact function ID selects it"
    );

    let glob = build_all(&edges, "rust:demo::llm::*::acquire");
    assert_eq!(
        glob.matched_functions, 1,
        "an embedded glob selects the same function"
    );
    assert_eq!(
        glob.functions, exact.functions,
        "glob and exact selection must produce identical grouped evidence"
    );

    let wrong = build_all(&edges, "rust:demo::llm::*::release");
    assert_eq!(
        wrong.matched_functions, 0,
        "a non-matching glob selects none"
    );
}

// A relationship whose supporting site has no site edges must render as
// unattributed rather than silently disappearing (Addendum A.4).
#[test]
fn a_relationship_over_an_unindexed_site_renders_as_unattributed() {
    let mut edges = scheduler_graph();
    edges.retain(|e| e.a.first().map(String::as_str) != Some(AWAIT));
    edges.push(edge(LOCK_SCOPE_ENDS_BEFORE_AWAIT, &[F, LOCK, AWAIT]));
    let report = build_all(&edges, F);
    let support = &report.functions[0].relationships[0].supported_by[1];
    assert!(
        !support.resolved,
        "a site with no indexed edges must be marked unresolved"
    );
    assert!(
        support.summary.contains("unattributed"),
        "an empty evidence path renders as unattributed: {}",
        support.summary
    );
}

// Operand text is capped at 240 bytes but not at one line, and it may
// contain the characters a table uses for alignment.
#[test]
fn multi_line_operand_text_cannot_break_table_alignment() {
    let site = "f#ownership:clone:10";
    let edges = vec![
        edge(OWNERSHIP_SITE, &[site]),
        edge(OWNERSHIP_SITE_IN_FUNCTION, &[site, "rust:demo::f"]),
        edge(OWNERSHIP_SITE_SPAN, &[site, "src/a.rs", "10", "20"]),
        edge(
            CLONE_SITE,
            &[site, "clone", "self\n  .items\n\t.first()\n.unwrap()"],
        ),
        edge(OWNERSHIP_EVIDENCE, &[site, "ast", "tree_sitter_rust"]),
    ];
    let table = render_table(&build_all(&edges, "rust:demo::f"));
    let observed: Vec<&str> = table
        .lines()
        .filter(|line| line.contains("self .items"))
        .collect();
    assert_eq!(
        observed.len(),
        1,
        "the operand must collapse onto one row: {table}"
    );
    assert!(
        !table.contains("self\n"),
        "no raw newline may survive into a table cell: {table}"
    );
}

// The digest marker and the operand cap protect fact IDs; the renderer
// must not reintroduce the separator into displayed text either.
#[test]
fn rendered_text_never_carries_the_fact_id_separator() {
    let table = render_table(&build_all(&scheduler_graph(), "*"));
    assert!(
        !table.contains('\u{1f}'),
        "U+001F joins fact-id arguments and must never reach rendered text"
    );
}

// A bare `collect` is a clone site by D5, but the rendering must not
// describe it as having produced ownership — that is a type-level claim.
#[test]
fn a_collect_site_is_not_described_as_producing_ownership() {
    let site = "rust:demo::g#ownership:clone:44";
    let edges = vec![
        edge(OWNERSHIP_SITE, &[site]),
        edge(OWNERSHIP_SITE_IN_FUNCTION, &[site, "rust:demo::g"]),
        edge(OWNERSHIP_SITE_SPAN, &[site, "src/g.rs", "44", "70"]),
        edge(CLONE_SITE, &[site, "collect", "rows.iter().collect()"]),
        edge(OWNERSHIP_EVIDENCE, &[site, "ast", "tree_sitter_rust"]),
    ];
    let report = build_all(&edges, "rust:demo::g");
    let table = render_table(&report);
    assert!(
        table.contains("does not establish that ownership was produced"),
        "a collect site must carry its type-level caveat: {table}"
    );
    assert!(
        report.functions[0]
            .limits
            .iter()
            .any(|l| l.contains("type-level claim")),
        "the function limits must name the collect caveat"
    );
}

// Sites must order by numeric byte offset. String ordering would put
// byte 1200 before byte 240 and scramble the narrative.
#[test]
fn sites_order_by_numeric_byte_offset_not_string_order() {
    let mut edges = scheduler_graph();
    let early = "rust:demo::llm::scheduler::Scheduler::acquire#ownership:clone:240";
    edges.push(edge(OWNERSHIP_SITE, &[early]));
    edges.push(edge(OWNERSHIP_SITE_IN_FUNCTION, &[early, F]));
    edges.push(edge(
        OWNERSHIP_SITE_SPAN,
        &[early, "src/scheduler.rs", "240", "260"],
    ));
    edges.push(edge(CLONE_SITE, &[early, "clone", "cfg"]));
    let report = build_all(&edges, F);
    let order: Vec<&str> = report.functions[0]
        .sites
        .iter()
        .map(|s| s.site.as_str())
        .collect();
    assert_eq!(
        order,
        vec![early, LOCK, AWAIT],
        "byte 240 precedes byte 1200"
    );
}

// The function limit must be visible in the shared struct, so the two
// surfaces cannot disagree about whether the answer was truncated.
#[test]
fn truncation_is_reported_in_the_shared_report_not_per_surface() {
    let mut edges = Vec::new();
    for i in 0..5 {
        let function = format!("rust:demo::f{i}");
        let site = format!("{function}#ownership:await:{i}");
        edges.push(edge(OWNERSHIP_SITE, &[&site]));
        edges.push(edge(OWNERSHIP_SITE_IN_FUNCTION, &[&site, &function]));
        edges.push(edge(AWAIT_SITE, &[&site]));
    }
    let report = build(&edges, "rust:demo::*", 2, on());
    assert_eq!(report.matched_functions, 5, "the total ignores the limit");
    assert_eq!(
        report.returned_functions, 2,
        "the limit caps what is rendered"
    );
    assert!(
        render_table(&report).contains("2 of 5 matching function(s)"),
        "truncation must be visible in the table"
    );
}

// Line numbers come from the file on disk, and the graph can outlive an
// edit to it. A stale offset must degrade to the byte span, not to a
// confidently wrong line.
#[test]
fn an_offset_past_the_end_of_the_file_degrades_to_a_byte_span() {
    assert_eq!(
        line_of("one\ntwo\nthree", "4"),
        Some(2),
        "the byte after the first newline is on line 2"
    );
    assert_eq!(line_of("one\ntwo", "0"), Some(1), "offset 0 is line 1");
    assert_eq!(
        line_of("short", "9999"),
        None,
        "an offset past EOF yields no line rather than a wrong one"
    );
    assert_eq!(
        line_of("short", "not-a-number"),
        None,
        "a malformed offset yields no line"
    );
}

// The JSON form must be an object: `EpistemeMcp::ok_json` rejects a
// top-level array at runtime, and a bare list of functions would be one.
#[test]
fn the_json_form_is_an_object_so_the_mcp_envelope_accepts_it() {
    let json = render_json(&build_all(&scheduler_graph(), F));
    let value: serde_json::Value = serde_json::from_str(&json).expect("rendered JSON must parse");
    assert!(value.is_object(), "structured MCP results must be objects");
    assert!(
        value.get("functions").is_some_and(|f| f.is_array()),
        "functions must be an array under an object key"
    );
    assert_eq!(
        value["state"], "matched",
        "the state must be machine-readable"
    );
}
