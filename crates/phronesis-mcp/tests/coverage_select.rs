//! Tests for `phr-mcp coverage select` (SPEC §7 / acceptance A6).

use std::process::Command;

use phronesis_mcp::coverage::select::{SelectedTest, render_json, render_table, select};
use phronesis_mcp::coverage::store::{COVERAGE_FORMAT, CoverageIndex, HitRecord, write_store};

const OLD_SRC: &str = r#"pub fn safe_divide(numerator: i32, denominator: i32) -> Result<i32, &'static str> {
    if denominator == 0 {
        return Err("division by zero");
    }

    Ok(numerator / denominator)
}
"#;

const NEW_SRC: &str = r#"pub fn safe_divide(numerator: i32, denominator: i32) -> Result<i32, &'static str> {
    if denominator == 0 {
        return Err("invalid denominator");
    }

    Ok(numerator / denominator)
}
"#;

const FIXTURE_REV: &str = "0ef2e37d80ee4be6d551cb9c7429a8a22720e712";

/// The temp repo's HEAD: a store imported at any other revision is stale,
/// and stale hits are labeled `coverage_observation_stale` (D3).
fn head_rev(root: &std::path::Path) -> String {
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .expect("git rev-parse");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn at_rev(hits: Vec<HitRecord>, rev: &str) -> Vec<HitRecord> {
    hits.into_iter()
        .map(|h| HitRecord {
            revision: rev.to_string(),
            ..h
        })
        .collect()
}

fn hit(test: &str, region: &str, file: &str, kind: &str) -> HitRecord {
    HitRecord {
        v: COVERAGE_FORMAT,
        kind: "hit".into(),
        test: test.into(),
        region: region.into(),
        file: file.into(),
        start_line: 1,
        end_line: 7,
        hit_kind: kind.into(),
        revision: FIXTURE_REV.into(),
        tool: "cargo-llvm-cov".into(),
    }
}

fn write_coverage_store(root: &std::path::Path) {
    let hits = vec![
        hit(
            "divides_positive_values",
            "fn:src/lib.rs::safe_divide",
            "src/lib.rs",
            "region",
        ),
        hit(
            "divides_negative_values",
            "fn:src/lib.rs::safe_divide",
            "src/lib.rs",
            "region",
        ),
        hit(
            "rejects_zero_denominator",
            "fn:src/lib.rs::safe_divide",
            "src/lib.rs",
            "region",
        ),
        hit(
            "rejects_zero_denominator",
            "branch:src/lib.rs::safe_divide:cd6054b02dde",
            "src/lib.rs",
            "branch",
        ),
    ];
    let rev = head_rev(root);
    write_store(
        root,
        &at_rev(hits, &rev),
        &CoverageIndex {
            format: COVERAGE_FORMAT,
            revision: rev.clone(),
            imported_at: 1,
            tool: "cargo-llvm-cov".into(),
        },
    )
    .unwrap();
}

fn init_git_repo(root: &std::path::Path) {
    Command::new("git")
        .args(["init"])
        .current_dir(root)
        .output()
        .expect("git init");
    Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(root)
        .output()
        .expect("git config");
    Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(root)
        .output()
        .expect("git config");
    // Write the original source and commit it.
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), OLD_SRC).unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(root)
        .output()
        .expect("git add");
    Command::new("git")
        .args(["commit", "-m", "initial"])
        .current_dir(root)
        .output()
        .expect("git commit");
}

fn apply_edit(root: &std::path::Path) {
    std::fs::write(root.join("src/lib.rs"), NEW_SRC).unwrap();
}

#[test]
fn test_select_returns_rejects_zero_at_branch_granularity() {
    let root = tempfile::tempdir().unwrap();
    init_git_repo(root.path());
    write_coverage_store(root.path());
    apply_edit(root.path());

    let sel = select(root.path(), None).unwrap();

    // Changed regions must include the branch site and the function.
    assert!(
        sel.changed_functions
            .contains(&"fn:src/lib.rs::safe_divide".to_string()),
        "changed_functions must include fn:src/lib.rs::safe_divide: {:?}",
        sel.changed_functions
    );
    assert!(
        sel.changed_branches
            .iter()
            .any(|b| b.starts_with("branch:src/lib.rs::safe_divide:")),
        "changed_branches must include a branch:src/lib.rs::safe_divide:* site: {:?}",
        sel.changed_branches
    );

    // The coverage_observation entries: exactly rejects_zero_denominator at
    // branch granularity, plus all three tests at function granularity.
    let coverage_tests: Vec<&SelectedTest> = sel
        .tests
        .iter()
        .filter(|t| t.evidence == "coverage_observation")
        .collect();

    // rejects_zero_denominator must be present and must carry the branch region.
    let rzd = coverage_tests
        .iter()
        .find(|t| t.test == "rejects_zero_denominator")
        .expect("rejects_zero_denominator must be selected");
    assert!(
        rzd.regions
            .iter()
            .any(|r| r.starts_with("branch:src/lib.rs::safe_divide:")),
        "rejects_zero_denominator must carry the branch region: {:?}",
        rzd.regions
    );
    assert!(
        rzd.regions
            .contains(&"fn:src/lib.rs::safe_divide".to_string()),
        "rejects_zero_denominator must also carry the function region: {:?}",
        rzd.regions
    );

    // The other two tests only have function-level coverage hits, not branch.
    for name in &["divides_positive_values", "divides_negative_values"] {
        let entry = coverage_tests
            .iter()
            .find(|t| &t.test == name)
            .expect("{name} must be selected at function level");
        assert!(
            entry
                .regions
                .contains(&"fn:src/lib.rs::safe_divide".to_string()),
            "{name} must carry fn:src/lib.rs::safe_divide: {:?}",
            entry.regions
        );
        assert!(
            !entry
                .regions
                .iter()
                .any(|r| r.starts_with("branch:src/lib.rs::safe_divide:")),
            "{name} must NOT carry a branch region: {:?}",
            entry.regions
        );
    }

    // Static reach is not available (no graph was built).
    assert!(
        !sel.static_reach_available,
        "static reach should be unavailable without a graph"
    );
}

#[test]
fn test_select_empty_store_returns_empty_not_error() {
    let root = tempfile::tempdir().unwrap();
    init_git_repo(root.path());
    // No coverage store written.
    apply_edit(root.path());

    let sel = select(root.path(), None).unwrap();

    assert!(
        sel.tests.is_empty(),
        "empty store must produce empty selection: {:?}",
        sel.tests
    );

    let table = render_table(&sel);
    assert!(
        table.contains("No tests selected"),
        "empty selection must have a clear message: {table}"
    );

    let json = render_json(&sel);
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(parsed["tests"].as_array().unwrap().is_empty());
}

#[test]
fn test_select_with_change_override() {
    let root = tempfile::tempdir().unwrap();
    init_git_repo(root.path());
    write_coverage_store(root.path());
    apply_edit(root.path());

    let sel = select(root.path(), Some("my-change-id")).unwrap();
    assert_eq!(sel.change, "my-change-id");
}

#[test]
fn test_select_json_shape() {
    let root = tempfile::tempdir().unwrap();
    init_git_repo(root.path());
    write_coverage_store(root.path());
    apply_edit(root.path());

    let sel = select(root.path(), None).unwrap();
    let json = render_json(&sel);
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

    assert!(parsed["change"].is_string());
    assert!(parsed["changed_functions"].is_array());
    assert!(parsed["changed_branches"].is_array());
    assert!(parsed["tests"].is_array());
    assert!(parsed["static_reach_available"].is_boolean());

    // Every test entry has test, evidence, regions.
    for entry in parsed["tests"].as_array().unwrap() {
        assert!(entry["test"].is_string());
        assert!(entry["evidence"].is_string());
        assert!(entry["regions"].is_array());
    }
}

#[test]
fn test_select_table_lists_coverage_and_static_separately() {
    let root = tempfile::tempdir().unwrap();
    init_git_repo(root.path());
    write_coverage_store(root.path());
    apply_edit(root.path());

    let sel = select(root.path(), None).unwrap();
    let table = render_table(&sel);

    // The coverage_observation section must be present.
    assert!(
        table.contains("coverage_observation (dynamic):"),
        "table must label the dynamic section: {table}"
    );

    // The static_reach section header must appear (even if empty or unavailable).
    // When the graph is unavailable, the note line appears instead.
    assert!(
        table.contains("static_reach") || table.contains("static reach:"),
        "table must mention static reach: {table}"
    );
}

const TWO_FN_OLD: &str = r#"pub fn alpha(x: i32) -> i32 {
    x + 1
}

pub fn beta(x: i32) -> i32 {
    x * 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exercises_beta() {
        assert_eq!(beta(2), 4);
    }
}
"#;

const TWO_FN_NEW: &str = r#"pub fn alpha(x: i32) -> i32 {
    x + 10
}

pub fn beta(x: i32) -> i32 {
    x * 20
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exercises_beta() {
        assert_eq!(beta(2), 40);
    }
}
"#;

/// A test with a real store hit on `alpha` and only a static graph edge to
/// `beta` must not have `beta` reported as dynamic evidence: the store never
/// observed that test executing `beta`.
#[test]
fn test_select_static_region_never_labeled_as_coverage_observation() {
    use phronesis_mcp::graph::store as graph_store;

    let root = tempfile::tempdir().unwrap();
    let dir = root.path();
    Command::new("git")
        .args(["init"])
        .current_dir(dir)
        .output()
        .expect("git init");
    for (k, v) in [("user.email", "test@test.com"), ("user.name", "Test")] {
        Command::new("git")
            .args(["config", k, v])
            .current_dir(dir)
            .output()
            .expect("git config");
    }
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/lib.rs"), TWO_FN_OLD).unwrap();
    for args in [&["add", "."][..], &["commit", "-m", "initial"][..]] {
        Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git");
    }
    std::fs::write(dir.join("src/lib.rs"), TWO_FN_NEW).unwrap();
    // Rebuild after the edit so the graph is fresh against the working tree.
    phronesis_mcp::graph::sync::rebuild(dir).expect("graph rebuild");

    // Use the graph's own test identity so the dynamic and static halves key
    // the same test.
    let edges = graph_store::load(&graph_store::graph_path(dir)).expect("graph");
    let test_id = edges
        .iter()
        .find_map(|e| {
            let reaches_beta = |f: &str| f.rsplit("::").next() == Some("beta");
            match e.p.as_str() {
                "tested_by" if e.a.len() == 2 && reaches_beta(&e.a[0]) => Some(e.a[1].clone()),
                "test_reaches" if e.a.len() == 2 && reaches_beta(&e.a[1]) => Some(e.a[0].clone()),
                _ => None,
            }
        })
        .expect("graph must carry a static edge from a test to beta");

    let rev = head_rev(dir);
    write_store(
        dir,
        &at_rev(
            vec![hit(
                &test_id,
                "fn:src/lib.rs::alpha",
                "src/lib.rs",
                "region",
            )],
            &rev,
        ),
        &CoverageIndex {
            format: COVERAGE_FORMAT,
            revision: rev.clone(),
            imported_at: 1,
            tool: "cargo-llvm-cov".into(),
        },
    )
    .unwrap();

    let sel = select(dir, None).unwrap();
    assert!(
        sel.static_reach_available,
        "graph must be fresh: {:?}",
        sel.static_note
    );

    let dynamic: Vec<&SelectedTest> = sel
        .tests
        .iter()
        .filter(|t| t.test == test_id && t.evidence == "coverage_observation")
        .collect();
    let stat: Vec<&SelectedTest> = sel
        .tests
        .iter()
        .filter(|t| t.test == test_id && t.evidence == "static_reach")
        .collect();
    assert_eq!(dynamic.len(), 1, "one dynamic entry: {:?}", sel.tests);
    assert_eq!(stat.len(), 1, "one static entry: {:?}", sel.tests);
    assert_eq!(dynamic[0].regions, vec!["fn:src/lib.rs::alpha".to_string()]);
    assert!(
        stat[0].regions.contains(&"fn:src/lib.rs::beta".to_string()),
        "static entry must carry fn:src/lib.rs::beta: {:?}",
        stat[0].regions
    );
    assert!(
        !stat[0]
            .regions
            .contains(&"fn:src/lib.rs::alpha".to_string()),
        "alpha has no static edge from this test: {:?}",
        stat[0].regions
    );

    // Table: beta must not appear in the dynamic section.
    let table = render_table(&sel);
    let dynamic_section = table
        .split("coverage_observation (dynamic):")
        .nth(1)
        .and_then(|rest| rest.split("static_reach (graph edges):").next())
        .expect("dynamic section must be rendered");
    assert!(
        !dynamic_section.contains("fn:src/lib.rs::beta"),
        "fn:src/lib.rs::beta leaked into the dynamic section: {table}"
    );
    assert!(
        table.contains("static_reach (graph edges):"),
        "static section must be rendered: {table}"
    );
    // Two entries, one test.
    assert!(
        table.contains("1 test(s) selected."),
        "footer counts distinct tests: {table}"
    );
}

// A store collected before per-site region ids holds leaf-name ids that can
// match nothing. Selection must say so and name the remedy, not claim the
// store is empty or merely unmatched.
#[test]
fn test_select_on_legacy_store_tells_the_user_to_recollect() {
    let root = tempfile::tempdir().unwrap();
    init_git_repo(root.path());
    write_store(
        root.path(),
        &[hit(
            "divides_positive_values",
            "fn:safe_divide",
            "src/lib.rs",
            "region",
        )],
        &CoverageIndex {
            format: COVERAGE_FORMAT,
            revision: FIXTURE_REV.into(),
            imported_at: 1,
            tool: "cargo-llvm-cov".into(),
        },
    )
    .unwrap();
    apply_edit(root.path());

    let sel = select(root.path(), None).unwrap();
    assert!(
        sel.tests.is_empty(),
        "legacy hits never match: {:?}",
        sel.tests
    );
    let table = render_table(&sel);
    assert!(
        table.contains("phr-mcp coverage collect"),
        "table must name the remedy: {table}"
    );
    assert!(
        !table.contains("the coverage store is empty"),
        "the store is not empty: {table}"
    );
    let json: serde_json::Value = serde_json::from_str(&render_json(&sel)).unwrap();
    assert!(
        json["coverage_note"]
            .as_str()
            .is_some_and(|n| n.contains("phr-mcp coverage collect")),
        "json must carry coverage_note: {json}"
    );
}

// A file too large to map per site changes as one `file:` region. A test the
// graph statically reaches in that file must still be selected: the static
// half over-selects like the dynamic half rather than miss it.
#[test]
fn test_select_static_reach_into_a_whole_file_region() {
    use phronesis_mcp::graph::store as graph_store;

    let root = tempfile::tempdir().unwrap();
    let dir = root.path();
    Command::new("git")
        .args(["init"])
        .current_dir(dir)
        .output()
        .expect("git init");
    for (k, v) in [("user.email", "test@test.com"), ("user.name", "Test")] {
        Command::new("git")
            .args(["config", k, v])
            .current_dir(dir)
            .output()
            .expect("git config");
    }
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/lib.rs"), TWO_FN_OLD).unwrap();
    for args in [&["add", "."][..], &["commit", "-m", "initial"][..]] {
        Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git");
    }
    // Past the region-mapping byte cap: the edit maps to `file:src/lib.rs`.
    let padding = "// padding line to push the file past the region-map cap\n".repeat(20_000);
    std::fs::write(dir.join("src/lib.rs"), format!("{TWO_FN_NEW}{padding}")).unwrap();
    phronesis_mcp::graph::sync::rebuild(dir).expect("graph rebuild");

    let edges = graph_store::load(&graph_store::graph_path(dir)).expect("graph");
    let test_id = edges
        .iter()
        .find_map(|e| {
            let reaches_beta = |f: &str| f.rsplit("::").next() == Some("beta");
            match e.p.as_str() {
                "tested_by" if e.a.len() == 2 && reaches_beta(&e.a[0]) => Some(e.a[1].clone()),
                "test_reaches" if e.a.len() == 2 && reaches_beta(&e.a[1]) => Some(e.a[0].clone()),
                _ => None,
            }
        })
        .expect("graph must carry a static edge from a test to beta");

    let sel = select(dir, None).unwrap();
    assert!(
        sel.static_reach_available,
        "graph must be fresh: {:?}",
        sel.static_note
    );
    let stat: Vec<&SelectedTest> = sel
        .tests
        .iter()
        .filter(|t| t.test == test_id && t.evidence == "static_reach")
        .collect();
    assert_eq!(stat.len(), 1, "one static entry: {:?}", sel.tests);
    assert_eq!(stat[0].regions, vec!["file:src/lib.rs".to_string()]);
}

const BIN_LIB_OLD: &str = "pub fn engine() -> u32 {\n    1\n}\n";
const BIN_LIB_NEW: &str = "pub fn engine() -> u32 {\n    2\n}\n";

/// A test that runs the package's binary reaches what `main` calls. That
/// reach is stored once as `bin_reaches(main, fn)`, not copied into every
/// such test's `test_reaches`, so `select` must join the two to list the
/// test for a changed function only `main` reaches.
#[test]
fn test_select_lists_a_binary_running_test_for_a_function_main_reaches() {
    use phronesis_mcp::graph::store as graph_store;

    let root = tempfile::tempdir().unwrap();
    let dir = root.path();
    Command::new("git")
        .args(["init"])
        .current_dir(dir)
        .output()
        .expect("git init");
    for (k, v) in [("user.email", "test@test.com"), ("user.name", "Test")] {
        Command::new("git")
            .args(["config", k, v])
            .current_dir(dir)
            .output()
            .expect("git config");
    }
    for (rel, body) in [
        (
            "Cargo.toml",
            "[package]\nname = \"tool\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[[bin]]\nname = \"tool-cli\"\npath = \"src/main.rs\"\n",
        ),
        ("src/lib.rs", BIN_LIB_OLD),
        (
            "src/main.rs",
            "fn main() {\n    run();\n}\n\nfn run() {\n    let _ = tool::engine();\n}\n",
        ),
        (
            "tests/cli.rs",
            "use std::process::Command;\n\n#[test]\nfn runs_the_binary() {\n    let _ = Command::new(env!(\"CARGO_BIN_EXE_tool-cli\")).status();\n}\n",
        ),
    ] {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
    for args in [&["add", "."][..], &["commit", "-m", "initial"][..]] {
        Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git");
    }
    std::fs::write(dir.join("src/lib.rs"), BIN_LIB_NEW).unwrap();
    phronesis_mcp::graph::sync::rebuild(dir).expect("graph rebuild");

    let test = "rust:tool#test:cli::runs_the_binary";
    let main = "rust:tool#bin:tool::main";
    let engine = "rust:tool::engine";
    let edges = graph_store::load(&graph_store::graph_path(dir)).expect("graph");
    let has = |p: &str, a: &[&str]| edges.iter().any(|e| e.p == p && e.a == a);
    assert!(has("tested_by", &[main, test]));
    assert!(has("test_reaches", &[test, main]));
    assert!(
        !has("test_reaches", &[test, engine]),
        "main's closure is not copied into the test's test_reaches"
    );
    assert!(has("bin_reaches", &[main, engine]));

    let sel = select(dir, None).unwrap();
    assert!(
        sel.static_reach_available,
        "graph must be fresh: {:?}",
        sel.static_note
    );
    let stat: Vec<&SelectedTest> = sel
        .tests
        .iter()
        .filter(|t| t.test == test && t.evidence == "static_reach")
        .collect();
    assert_eq!(stat.len(), 1, "one static entry: {:?}", sel.tests);
    assert!(
        stat[0].regions.iter().any(|r| r == "fn:src/lib.rs::engine"),
        "{:?}",
        stat[0].regions
    );
}

// ── Part L task 2: evaluated, not executed ──────────────────────────

const CUE_OLD: &str = "package config\na: int\n";
const CUE_NEW: &str = "package config\na: string\n";

fn commit_file(root: &std::path::Path, path: &str, content: &str) {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(root.join(parent)).unwrap();
    }
    std::fs::write(root.join(path), content).unwrap();
    for args in [vec!["add", path], vec!["commit", "-m", path]] {
        Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .expect("git");
    }
}

/// Red-first (plan Part L task 2): edits to evaluated languages (cue, json,
/// yaml/yml, tpl→helm3, rhai per `graph::unit::lang_of_path`) are named
/// under `no_coverage_semantics`; unknown extensions (`.md`) and executed
/// languages (`.rs`) stay unclassified; no test is ever selected for the
/// evaluated files and no `region_without_dynamic_evidence` appears.
#[test]
fn test_select_names_evaluated_files_under_no_coverage_semantics() {
    let root = tempfile::tempdir().unwrap();
    init_git_repo(root.path());
    commit_file(root.path(), "config/model.cue", CUE_OLD);
    commit_file(root.path(), "templates/deploy.tpl", "{{ .Values.x }}");
    commit_file(root.path(), "hooks/check.rhai", "true\n");
    commit_file(root.path(), "settings/cluster.yml", "a: 1\n");
    commit_file(root.path(), "settings/cluster.json", "{\"a\": 1}\n");
    commit_file(root.path(), "docs/notes.md", "notes\n");

    std::fs::write(root.path().join("config/model.cue"), CUE_NEW).unwrap();
    std::fs::write(
        root.path().join("templates/deploy.tpl"),
        "{{ .Values.y }}\n",
    )
    .unwrap();
    std::fs::write(root.path().join("hooks/check.rhai"), "false\n").unwrap();
    std::fs::write(root.path().join("settings/cluster.yml"), "a: 2\n").unwrap();
    std::fs::write(root.path().join("settings/cluster.json"), "{\"a\": 2}\n").unwrap();
    std::fs::write(root.path().join("docs/notes.md"), "more notes\n").unwrap();
    apply_edit(root.path());

    let sel = select(root.path(), None).unwrap();
    assert_eq!(
        sel.no_coverage_semantics
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec![
            "config/model.cue",
            "hooks/check.rhai",
            "settings/cluster.json",
            "settings/cluster.yml",
            "templates/deploy.tpl",
        ],
        "evaluated edits named, .md and .rs stay unclassified"
    );
    assert!(
        sel.changed_functions
            .iter()
            .chain(sel.changed_branches.iter())
            .all(|r| !r.contains("model.cue")
                && !r.contains("deploy.tpl")
                && !r.contains("check.rhai")),
        "no region for an evaluated file: {:?}",
        sel.changed_functions
    );
    assert!(
        sel.tests
            .iter()
            .all(|t| !t.regions.iter().any(|r| r.contains("model.cue"))),
        "no test is ever selected for an evaluated file: {:?}",
        sel.tests
    );

    let json = render_json(&sel);
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        parsed["no_coverage_semantics"],
        serde_json::json!([
            "config/model.cue",
            "hooks/check.rhai",
            "settings/cluster.json",
            "settings/cluster.yml",
            "templates/deploy.tpl",
        ]),
        "json key names the evaluated files"
    );
    assert!(
        !json.contains("region_without_dynamic_evidence"),
        "select never reports an evidence-gap fact for evaluated files: {json}"
    );

    let table = render_table(&sel);
    assert!(
        table.contains(
            "config/model.cue: evaluated, not executed; the compile signal comes from `cue vet` / `helm lint` / the hook's Rhai evaluation."
        ),
        "table footer names the file and the validating tools: {table}"
    );
}

/// Red-first: the footer also prints on the no-tests early return — exactly
/// the case an evaluated-only edit produces.
#[test]
fn test_select_table_footer_prints_when_no_tests_selected() {
    let root = tempfile::tempdir().unwrap();
    init_git_repo(root.path());
    commit_file(root.path(), "config/model.cue", CUE_OLD);
    std::fs::write(root.path().join("config/model.cue"), CUE_NEW).unwrap();

    let sel = select(root.path(), None).unwrap();
    assert!(sel.tests.is_empty(), "a .cue edit selects no tests");
    let table = render_table(&sel);
    assert!(
        table.contains("config/model.cue: evaluated, not executed;"),
        "footer must print on the no-tests branch: {table}"
    );
}

/// PINNING REGRESSION (plan Part L task 2, labelled a pin — already green at
/// 5deb7b7 and required to stay green): `extract_function_sites_for` has no
/// registry row for `.cue`, so `changed_regions` returns no functions and no
/// branches for it. A future registry row for an evaluated language must go
/// through plan Revision 2's scope rule, not silently add regions here.
#[test]
fn pin_changed_regions_for_a_cue_file_are_empty() {
    let regions =
        phronesis_mcp::coverage::region_map::changed_regions("config/model.cue", CUE_OLD, CUE_NEW)
            .unwrap();
    assert!(
        regions.functions.is_empty() && regions.branches.is_empty(),
        "a .cue edit must produce no regions: functions={:?} branches={:?}",
        regions.functions,
        regions.branches
    );
}
