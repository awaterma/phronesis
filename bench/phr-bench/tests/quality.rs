use phr_bench::quality::{audit_clone, parse_audit};

fn fixture() -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/testdata/audit-report.json"),
    )
    .unwrap()
}

#[test]
fn parses_real_audit_shape() {
    let s = parse_audit(&fixture()).unwrap();
    assert_eq!(s.total_violations, 3); // block-level hits only
    assert_eq!(s.per_rule.get("no-unwrap-in-src"), Some(&2));
    assert_eq!(s.per_rule.get("unsafe-blocks"), Some(&1));
    assert!(!s.per_rule.contains_key("audit-file-loc-high")); // warns excluded from debt
}

#[test]
fn staging_refuses_without_captured_patch() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.diff");
    let err = audit_clone(dir.path(), "{}", &missing).unwrap_err();
    assert!(
        err.to_string().contains("patch"),
        "ordering constraint: diff first, audit second"
    );
}

#[test]
fn audit_of_a_clone_excludes_governance_files() {
    // After audit_clone runs, the staged rules file must not appear in a re-extraction
    // of the patch — proving staged files never pollute the artifact.
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init", "-q"]);
    std::fs::write(repo.join("a.rs"), "x\n").unwrap();
    run_git(&repo, &["add", "-A"]);
    run_git(
        &repo,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-qm",
            "i",
        ],
    );
    std::fs::write(repo.join("a.rs"), "y\n").unwrap();
    let before = phr_bench::runner::extract_diff(&repo, "HEAD").unwrap();
    let patch_file = dir.path().join("p.diff");
    std::fs::write(&patch_file, &before).unwrap();

    // Point to the stub phr-mcp
    let stub = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/testdata/fake-phr-mcp.sh");
    std::env::set_var(phr_bench::quality::PHR_MCP_PATH_ENV, &stub);

    let summary = audit_clone(&repo, "{\"rules\":[]}", &patch_file).unwrap();
    let after = phr_bench::runner::extract_diff(&repo, "HEAD").unwrap();
    assert_eq!(before, after, "staging must not change the tracked diff");
    let _ = summary; // audit ran against the staged rules; content covered by parse tests
}

fn run_git(cwd: &std::path::Path, argv: &[&str]) {
    let ok = std::process::Command::new("git")
        .args(argv)
        .current_dir(cwd)
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok, "command failed: git {:?}", argv);
}
