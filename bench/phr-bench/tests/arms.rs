use phr_bench::arms::prep;
use phr_bench::manifest::TaskSpec;
use phr_bench::record::Arm;
use tempfile::tempdir;

fn spec(repo_url: &str) -> TaskSpec {
    TaskSpec {
        instance_id: "fixture".into(),
        language: "rust".into(),
        repo: repo_url.into(),
        base_commit: "HEAD".into(),
        issue_text: "".into(),
        fail_to_pass: vec![],
        pass_to_pass: vec![],
        packs: vec!["llm".into(), "rust".into()],
    }
}

/// Build a tiny local git repo to clone from (file:// URL), one commit, src/a.rs.
fn fixture_repo() -> (tempfile::TempDir, String) {
    let dir = tempdir().unwrap();
    let repo = dir.path().join("origin");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/a.rs"), "pub fn a() {}\n").unwrap();
    run(&["git", "init", "-q"], &repo);
    run(&["git", "add", "-A"], &repo);
    run(
        &[
            "git", "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "init",
        ],
        &repo,
    );
    let url = format!("file://{}", repo.display());
    (dir, url)
}

fn run(argv: &[&str], cwd: &std::path::Path) {
    let ok = std::process::Command::new(argv[0])
        .args(&argv[1..])
        .current_dir(cwd)
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok, "command failed: {:?}", argv);
}

#[test]
fn control_is_bare_and_treatment_is_governed() {
    let (_guard, url) = fixture_repo();
    let work = tempdir().unwrap();
    let control = prep(&spec(&url), Arm::Control, work.path()).unwrap();
    let treated = prep(&spec(&url), Arm::Treatment, work.path()).unwrap();
    assert!(
        control.join("src/a.rs").exists(),
        "clone checkout happened"
    );
    assert!(!control.join(".phronesis").exists());
    assert!(!control.join(".claude").exists());
    assert!(
        treated.join(".phronesis/rules.json").exists(),
        "init ran"
    );
    assert!(
        treated.join(".claude/settings.json").exists()
            || treated.join(".claude/settings.local.json").exists(),
        "hooks installed"
    );
    assert!(!control.join(".claude/settings.json").exists());
}

#[test]
fn bad_commit_is_an_error() {
    let (_guard, url) = fixture_repo();
    let work = tempdir().unwrap();
    let mut s = spec(&url);
    s.base_commit = "0000000000000000000000000000000000000000".into();
    assert!(prep(&s, Arm::Control, work.path()).is_err());
}
