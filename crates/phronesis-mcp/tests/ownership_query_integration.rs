//! `phr-mcp graph ownership` — the grouped ownership explanation surface
//! (SPEC-rust-ownership-evidence §10, Addendum A.1/A.4).
//!
//! These tests drive the real binary against a seeded graph in a tempdir, the
//! way `journey_cli_integration.rs` does. They close the binary-to-library
//! hop of the §13.2 identity requirement: the CLI must print exactly what
//! `graph::ownership::query` renders, adding nothing of its own. The
//! library-to-MCP hop is pinned by `server.rs::ownership_evidence_tool_tests`,
//! which compares the MCP payload against the same rendered document.
//!
//! Together those two assertions are the whole chain: CLI == library == MCP.

use std::path::Path;
use std::process::Command;

use phronesis_mcp::graph::model::Edge;
use phronesis_mcp::graph::ownership as own;
use phronesis_mcp::graph::ownership::query as ownership;
use phronesis_mcp::graph::store;

const ACQUIRE: &str = "rust:demo::llm::scheduler::Scheduler::acquire";
const LOCK: &str = "rust:demo::llm::scheduler::Scheduler::acquire#ownership:lock:30";
const AWAIT: &str = "rust:demo::llm::scheduler::Scheduler::acquire#ownership:await:96";
const RELEASE: &str = "rust:demo::llm::scheduler::Scheduler::release";
const CLONE: &str = "rust:demo::llm::scheduler::Scheduler::release#ownership:clone:150";

fn run(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(args)
        .output()
        .expect("run phr-mcp");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn edge(relation: &str, args: &[&str]) -> Edge {
    Edge::base(relation, args, "src/scheduler.rs")
}

fn enable_ownership(root: &Path) {
    let phr = root.join(".phronesis");
    std::fs::create_dir_all(&phr).expect("create .phronesis");
    std::fs::write(
        phr.join("graph.toml"),
        "[ownership.rust]\nenabled = true\nprovider = \"ast\"\ninclude = [\"src/**/*.rs\"]\n",
    )
    .expect("write graph.toml");
}

/// A scheduler-shaped graph: two lock/await sites with a derived scope
/// relation, a clone site in a second function, and an unavailable MIR
/// capability — the §10 example, minimized.
fn seed(root: &Path) {
    enable_ownership(root);
    let edges = vec![
        edge(own::OWNERSHIP_SITE, &[LOCK]),
        edge(own::OWNERSHIP_SITE_IN_FUNCTION, &[LOCK, ACQUIRE]),
        edge(
            own::OWNERSHIP_SITE_SPAN,
            &[LOCK, "src/scheduler.rs", "30", "58"],
        ),
        edge(own::SYNC_LOCK_SITE, &[LOCK, "lock", "guard"]),
        edge(
            own::OWNERSHIP_EVIDENCE,
            &[LOCK, "ast", own::PROVIDER_TREE_SITTER_RUST],
        ),
        edge(own::OWNERSHIP_SITE, &[AWAIT]),
        edge(own::OWNERSHIP_SITE_IN_FUNCTION, &[AWAIT, ACQUIRE]),
        edge(
            own::OWNERSHIP_SITE_SPAN,
            &[AWAIT, "src/scheduler.rs", "96", "108"],
        ),
        edge(own::AWAIT_SITE, &[AWAIT]),
        edge(
            own::OWNERSHIP_EVIDENCE,
            &[AWAIT, "ast", own::PROVIDER_TREE_SITTER_RUST],
        ),
        edge(own::LOCK_SCOPE_ENDS_BEFORE_AWAIT, &[ACQUIRE, LOCK, AWAIT]),
        edge(
            own::OWNERSHIP_ANALYSIS_STATUS,
            &[
                ACQUIRE,
                "ast_extraction",
                "available",
                phronesis_mcp::graph::ownership::extract::REASON_COMPLETE,
            ],
        ),
        edge(
            own::OWNERSHIP_ANALYSIS_STATUS,
            &[ACQUIRE, "type_inference", "available", "rust_analyzer"],
        ),
        edge(
            own::OWNERSHIP_ANALYSIS_STATUS,
            &[ACQUIRE, "mir_lowering", "unavailable", "async_lowering"],
        ),
        edge(own::OWNERSHIP_SITE, &[CLONE]),
        edge(own::OWNERSHIP_SITE_IN_FUNCTION, &[CLONE, RELEASE]),
        edge(
            own::OWNERSHIP_SITE_SPAN,
            &[CLONE, "src/scheduler.rs", "150", "176"],
        ),
        edge(own::CLONE_SITE, &[CLONE, "clone", "self.pending"]),
        edge(
            own::OWNERSHIP_EVIDENCE,
            &[CLONE, "ast", own::PROVIDER_TREE_SITTER_RUST],
        ),
    ];
    store::write_atomic(&store::graph_path(root), &edges).expect("write graph");
}

fn root_arg(dir: &tempfile::TempDir) -> String {
    dir.path().display().to_string()
}

/// §13.2, binary half: the CLI adds no shaping of its own. If someone gives
/// the CLI its own JSON envelope — the way `graph query` grew one that the
/// MCP surface then diverged from — this fails immediately.
#[test]
fn the_cli_json_is_exactly_the_library_rendered_document() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    seed(dir.path());

    let (code, stdout, stderr) = run(&[
        "graph",
        "ownership",
        ACQUIRE,
        "--path",
        &root_arg(&dir),
        "--json",
    ]);
    assert_eq!(
        code, 0,
        "graph ownership --json must exit 0; stderr: {stderr}"
    );

    let printed: serde_json::Value =
        serde_json::from_str(&stdout).expect("CLI must print parseable JSON");
    let expected: serde_json::Value = serde_json::from_str(&ownership::render_json(
        &ownership::load(dir.path(), ACQUIRE, 20).expect("library report"),
    ))
    .expect("library JSON parses");
    assert_eq!(
        printed, expected,
        "the CLI must print the library's document verbatim"
    );
}

/// §10's worked example: source location, observed operation, the derived
/// relationship with its supporting sites, capability availability, and the
/// limit — all present in one grouped block.
#[test]
fn the_table_groups_sites_relationships_capabilities_and_limits() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    seed(dir.path());

    let (code, stdout, stderr) = run(&["graph", "ownership", ACQUIRE, "--path", &root_arg(&dir)]);
    assert_eq!(code, 0, "graph ownership must exit 0; stderr: {stderr}");

    for expected in [
        "Function: rust:demo::llm::scheduler::Scheduler::acquire",
        "Observed:",
        "sync lock acquired",
        "src/scheduler.rs",
        "Relationships:",
        "lock_scope_ends_before_await",
        "lock site:",
        "await site:",
        "Evidence:",
        "AST: available",
        "type inference: available",
        "MIR: unavailable (async_lowering)",
        "Limit:",
        "lexical scope is not general control-flow or borrow-liveness proof",
    ] {
        assert!(
            stdout.contains(expected),
            "grouped output must contain {expected:?}; got:\n{stdout}"
        );
    }
}

/// Exact and embedded-glob function queries behave like every other graph
/// query (§13.2), and select the same evidence when they name the same
/// function.
#[test]
fn exact_and_embedded_glob_function_queries_behave_alike() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    seed(dir.path());
    let root = root_arg(&dir);

    let (_, exact, _) = run(&["graph", "ownership", ACQUIRE, "--path", &root, "--json"]);
    let (_, glob, _) = run(&[
        "graph",
        "ownership",
        "rust:demo::llm::*::acquire",
        "--path",
        &root,
        "--json",
    ]);
    let exact: serde_json::Value = serde_json::from_str(&exact).expect("exact json");
    let glob: serde_json::Value = serde_json::from_str(&glob).expect("glob json");
    assert_eq!(
        exact["functions"], glob["functions"],
        "an embedded glob naming one function yields that function's evidence"
    );

    let (_, wide, _) = run(&[
        "graph",
        "ownership",
        "rust:demo::llm::scheduler::*",
        "--path",
        &root,
        "--json",
    ]);
    let wide: serde_json::Value = serde_json::from_str(&wide).expect("wide json");
    assert_eq!(
        wide["matched_functions"], 2,
        "a wider glob picks up both indexed functions: {wide}"
    );
}

/// Ownership disabled is its own answer, and it is actionable. Reporting it
/// as "nothing found" would tell the user their code is clean when in fact
/// nothing ever looked at it.
#[test]
fn ownership_disabled_is_reported_as_disabled_with_the_way_to_enable_it() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    // A graph exists, but no `[ownership.rust]` section does.
    store::write_atomic(
        &store::graph_path(dir.path()),
        &[Edge::base(
            "defines_fn",
            &["src/a.rs", "rust:demo::f"],
            "src/a.rs",
        )],
    )
    .expect("write graph");

    let (code, stdout, _) = run(&["graph", "ownership", "*", "--path", &root_arg(&dir)]);
    assert_eq!(code, 0, "a disabled feature is not an error");
    assert!(
        stdout.contains("not enabled") && stdout.contains("[ownership.rust]"),
        "disabled output must name the section that turns it on; got:\n{stdout}"
    );
}

/// `store::load` returns an empty vector for a missing file, so "no graph"
/// and "empty graph" are indistinguishable from its return value alone. The
/// unbuilt case must still name its own fix.
#[test]
fn an_unbuilt_graph_is_reported_as_no_graph_with_the_rebuild_command() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    enable_ownership(dir.path());

    let (code, stdout, _) = run(&["graph", "ownership", "*", "--path", &root_arg(&dir)]);
    assert_eq!(code, 0, "an unbuilt graph is not an error");
    assert!(
        stdout.contains("No code graph") && stdout.contains("graph rebuild"),
        "an unbuilt graph must point at `graph rebuild`; got:\n{stdout}"
    );
}

/// The §10 / A.4 requirement in its most load-bearing form: a query that
/// matched nothing must read as an absence of indexed evidence, never as a
/// clean bill of health.
#[test]
fn a_query_that_matched_nothing_is_never_rendered_as_proof_of_cleanliness() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    seed(dir.path());

    let (code, stdout, _) = run(&[
        "graph",
        "ownership",
        "rust:unrelated::*",
        "--path",
        &root_arg(&dir),
    ]);
    assert_eq!(code, 0, "an empty match is not an error");
    assert!(
        stdout.contains("No indexed ownership evidence found"),
        "an empty match must say what is absent; got:\n{stdout}"
    );
    assert!(
        stdout.contains("not proof that the matched code has no ownership concern"),
        "an empty match must refuse the clean-code reading; got:\n{stdout}"
    );
}

/// Addendum A.4: unavailable capabilities are rendered next to the positive
/// findings. A function whose MIR was never lowered must not present as
/// though its AST evidence had been corroborated.
#[test]
fn a_function_with_no_compiler_evidence_still_shows_every_capability_line() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    seed(dir.path());

    // `release` has a clone site but no analysis-status edges at all.
    let (_, stdout, _) = run(&["graph", "ownership", RELEASE, "--path", &root_arg(&dir)]);
    for expected in [
        "AST: not reported",
        "type inference: not reported",
        "MIR: not reported",
        "absence of a capability result is not a clean result",
    ] {
        assert!(
            stdout.contains(expected),
            "a capability with no status must still occupy a line ({expected:?}); got:\n{stdout}"
        );
    }
}
