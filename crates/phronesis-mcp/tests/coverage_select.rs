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
            "fn:safe_divide",
            "src/lib.rs",
            "region",
        ),
        hit(
            "divides_negative_values",
            "fn:safe_divide",
            "src/lib.rs",
            "region",
        ),
        hit(
            "rejects_zero_denominator",
            "fn:safe_divide",
            "src/lib.rs",
            "region",
        ),
        hit(
            "rejects_zero_denominator",
            "branch:safe_divide:cd6054b02dde",
            "src/lib.rs",
            "branch",
        ),
    ];
    write_store(
        root,
        &hits,
        &CoverageIndex {
            format: COVERAGE_FORMAT,
            revision: FIXTURE_REV.into(),
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
            .contains(&"fn:safe_divide".to_string()),
        "changed_functions must include fn:safe_divide: {:?}",
        sel.changed_functions
    );
    assert!(
        sel.changed_branches
            .iter()
            .any(|b| b.starts_with("branch:safe_divide:")),
        "changed_branches must include a branch:safe_divide:* site: {:?}",
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
            .any(|r| r.starts_with("branch:safe_divide:")),
        "rejects_zero_denominator must carry the branch region: {:?}",
        rzd.regions
    );
    assert!(
        rzd.regions.contains(&"fn:safe_divide".to_string()),
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
            entry.regions.contains(&"fn:safe_divide".to_string()),
            "{name} must carry fn:safe_divide: {:?}",
            entry.regions
        );
        assert!(
            !entry
                .regions
                .iter()
                .any(|r| r.starts_with("branch:safe_divide:")),
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
