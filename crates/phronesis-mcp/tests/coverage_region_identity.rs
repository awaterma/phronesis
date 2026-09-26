//! Region identity is per code site (SPEC-coverage-evidence §3.2): a hit,
//! a changed region, and a selection must never join two different sites
//! that merely share a leaf function name or a branch condition.
//!
//! Hits are produced through the real collector so these tests exercise the
//! whole producer -> store -> consumer path rather than hand-written ids.

use std::collections::HashSet;
use std::path::Path;
use std::process::Command;

use phronesis_mcp::coverage::collect::{LlvmCovDocument, collect_from_documents};
use phronesis_mcp::coverage::hydrate::{EditedFile, HydrationInput, facts_for_event};
use phronesis_mcp::coverage::select::select;
use phronesis_mcp::coverage::store::{COVERAGE_FORMAT, CoverageIndex, HitRecord, write_store};

const REV: &str = "0ef2e37d80ee4be6d551cb9c7429a8a22720e712";

const A_SRC: &str = "pub struct A;\n\nimpl A {\n    pub fn new() -> Self {\n        A\n    }\n}\n";
const B_SRC: &str = "pub struct B;\n\nimpl B {\n    pub fn new() -> Self {\n        B\n    }\n}\n";
const B_SRC_EDITED: &str = "pub struct B;\n\nimpl B {\n    pub fn new() -> Self {\n        let b = B;\n        b\n    }\n}\n";

/// One llvm-cov function entry spanning `start..=end` with `branches`.
fn doc(name: &str, file: &str, start: u64, end: u64, branches: &str) -> LlvmCovDocument {
    let json = format!(
        r#"{{"data": [{{"functions": [{{"name": "{name}", "count": 1, "regions": [[{start}, 0, {end}, 1, 1]], "branches": {branches}, "filenames": ["{file}"]}}]}}]}}"#
    );
    serde_json::from_str(&json).expect("synthetic llvm-cov doc")
}

fn write_src(root: &Path, rel: &str, src: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, src).unwrap();
}

fn store(root: &Path, hits: &[HitRecord]) {
    write_store(
        root,
        hits,
        &CoverageIndex {
            format: COVERAGE_FORMAT,
            revision: REV.into(),
            imported_at: 1,
            tool: "cargo-llvm-cov".into(),
        },
    )
    .unwrap();
}

fn gaps_for_edit(root: &Path, path: &str, old: &str, new: &str) -> Vec<String> {
    let input = HydrationInput {
        root,
        rule_relations: ["region_without_dynamic_evidence"]
            .iter()
            .map(|s| s.to_string())
            .collect::<HashSet<_>>(),
        edited: vec![EditedFile {
            path: path.into(),
            old: Some(old),
            new,
        }],
        head_sha: Some(REV.into()),
    };
    facts_for_event(&input)
        .unwrap()
        .into_iter()
        .filter(|f| f.predicate == "region_without_dynamic_evidence")
        .map(|f| f.args[0].clone())
        .collect()
}

// Consequence 1: a hit on `new` in a.rs must not count as dynamic evidence
// for an unexecuted `new` in b.rs.
#[test]
fn hit_on_same_named_fn_in_another_file_does_not_suppress_gap() {
    let root = tempfile::tempdir().unwrap();
    let a = "crates/x/src/a.rs";
    let b = "crates/x/src/b.rs";
    write_src(root.path(), a, A_SRC);
    write_src(root.path(), b, B_SRC);

    let docs = vec![("t_a".to_string(), doc("x::a::A::new", a, 4, 6, "[]"))];
    let hits = collect_from_documents(root.path(), REV, &docs).unwrap();
    assert_eq!(hits.len(), 1, "one fn hit for A::new: {hits:?}");
    store(root.path(), &hits);

    let gaps = gaps_for_edit(root.path(), b, B_SRC, B_SRC_EDITED);
    assert!(
        !gaps.is_empty(),
        "B::new in b.rs was never executed; a hit on A::new in a.rs must not \
         suppress its gap (hits: {hits:?})"
    );

    // Control: the executed site itself is not a gap.
    let a_edited = A_SRC.replace("        A\n", "        let a = A;\n        a\n");
    let own = gaps_for_edit(root.path(), a, A_SRC, &a_edited);
    assert!(
        own.is_empty(),
        "A::new has dynamic evidence; must not gap: {own:?}"
    );
}

fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("git");
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

const GAMMA: &str = "pub fn gamma(x: i32) -> i32 {\n    x + 1\n}\n";
const GAMMA_EDITED: &str = "pub fn gamma(x: i32) -> i32 {\n    x + 2\n}\n";

// Consequence 2: `coverage select` must not pick a test that hit `gamma` in
// unrelated.rs for an edit to other.rs::gamma.
#[test]
fn select_does_not_pick_test_that_hit_same_named_fn_in_another_file() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path();
    git(dir, &["init", "-q"]);
    git(dir, &["config", "user.email", "t@t"]);
    git(dir, &["config", "user.name", "t"]);
    let unrelated = "crates/x/src/unrelated.rs";
    let other = "crates/x/src/other.rs";
    write_src(dir, unrelated, GAMMA);
    write_src(dir, other, GAMMA);
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "init"]);

    let docs = vec![(
        "t_unrelated".to_string(),
        doc("x::unrelated::gamma", unrelated, 1, 3, "[]"),
    )];
    let hits = collect_from_documents(dir, REV, &docs).unwrap();
    assert_eq!(hits.len(), 1, "{hits:?}");
    store(dir, &hits);

    write_src(dir, other, GAMMA_EDITED);
    let sel = select(dir, None).unwrap();
    assert!(
        !sel.tests.iter().any(|t| t.test == "t_unrelated"),
        "t_unrelated only executed unrelated.rs::gamma; it is not relevant \
         to an edit of other.rs::gamma: {:?}",
        sel.tests
    );

    // Control: editing the executed site selects the test.
    write_src(dir, other, GAMMA);
    write_src(dir, unrelated, GAMMA_EDITED);
    let sel = select(dir, None).unwrap();
    assert!(
        sel.tests.iter().any(|t| t.test == "t_unrelated"),
        "editing unrelated.rs::gamma must select t_unrelated: {:?}",
        sel.tests
    );
}

const TWIN_IFS: &str = "pub fn f(x: i32) -> i32 {\n    if x == 0 {\n        return 1;\n    }\n    if x == 0 {\n        return 2;\n    }\n    x\n}\n";

// Consequence 3: two identical conditions in one function are two sites.
#[test]
fn identical_conditions_in_one_function_are_distinct_branch_regions() {
    let root = tempfile::tempdir().unwrap();
    let rel = "crates/x/src/twin.rs";
    write_src(root.path(), rel, TWIN_IFS);

    let docs = vec![(
        "t".to_string(),
        doc(
            "x::twin::f",
            rel,
            1,
            9,
            "[[2, 7, 2, 13, 1], [5, 7, 5, 13, 1]]",
        ),
    )];
    let hits = collect_from_documents(root.path(), REV, &docs).unwrap();
    let branches: HashSet<&str> = hits
        .iter()
        .filter(|h| h.hit_kind == "branch")
        .map(|h| h.region.as_str())
        .collect();
    assert_eq!(
        branches.len(),
        2,
        "each `if x == 0` site needs its own branch region: {hits:?}"
    );
}
