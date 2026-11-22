use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use phronesis_mcp::{decision_store, graph, wiki, wiki_drift};
use serde_json::Value;
use tempfile::{TempDir, tempdir};

fn git(root: &Path, args: &[&str]) -> Output {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn cli(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .current_dir(root)
        .args(args)
        .output()
        .unwrap()
}

fn success(root: &Path, args: &[&str]) -> Output {
    let output = cli(root, args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn fixture() -> (TempDir, TempDir) {
    let project = tempdir().unwrap();
    let external = tempdir().unwrap();
    git(project.path(), &["init", "--quiet"]);
    git(external.path(), &["init", "--quiet"]);
    fs::write(project.path().join(".git/info/exclude"), "/.phronesis/\n").unwrap();
    fs::create_dir(project.path().join(".phronesis")).unwrap();
    fs::write(
        project.path().join(".phronesis/rules.json"),
        r#"{"rules":[]}"#,
    )
    .unwrap();
    fs::create_dir_all(project.path().join("docs/decisions")).unwrap();
    (project, external)
}

fn configure(project: &Path, external: &Path) {
    success(
        project,
        &[
            "decision",
            "configure",
            "--directory",
            external.to_str().unwrap(),
        ],
    );
}

#[test]
fn configured_external_store_creates_lists_and_checks_without_git_mutation() {
    let (project, external) = fixture();
    configure(project.path(), external.path());
    configure(project.path(), external.path()); // idempotent
    let state = phronesis_mcp::state::inspect(project.path());
    assert!(
        state
            .iter()
            .any(|entry| entry.path == decision_store::CONFIG_PATH
                && entry.class == "authored"
                && entry.sensitive
                && !entry.tracked)
    );
    success(project.path(), &["decision", "new", "private-design"]);
    let output = success(project.path(), &["decision", "list", "--json"]);
    let list: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(list["decisions"][0]["id"], "private-design");
    let path = Path::new(list["decisions"][0]["path"].as_str().unwrap());
    assert!(path.starts_with(external.path().canonicalize().unwrap()));
    assert!(
        fs::read_to_string(path)
            .unwrap()
            .contains("status: proposed")
    );
    assert_eq!(
        fs::read_dir(project.path().join("docs/decisions"))
            .unwrap()
            .count(),
        0
    );
    let output = success(project.path(), &["wiki-drift", "--json"]);
    let drift: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(drift["items"][0]["id"], "private-design");
    for repo in [project.path(), external.path()] {
        assert!(git(repo, &["for-each-ref"]).stdout.is_empty());
        assert!(git(repo, &["diff", "--cached"]).stdout.is_empty());
        assert!(git(repo, &["remote"]).stdout.is_empty());
    }
    let tracked = git(
        project.path(),
        &["ls-files", "--others", "--exclude-standard"],
    );
    assert!(!String::from_utf8_lossy(&tracked.stdout).contains("decision-store"));
}

#[test]
fn private_decisions_do_not_enter_automatic_graph_drift_or_hook_provenance() {
    let (project, external) = fixture();
    configure(project.path(), external.path());
    fs::write(external.path().join("private.md"), "---\nid: secret-choice\ndate: 2026-11-21\nstatus: accepted\nenforces: [rule-a]\n---\nprivate-body\n").unwrap();
    assert!(
        wiki::resolve_decisions_lenient(project.path())
            .decisions
            .is_empty()
    );
    assert!(wiki_drift::run(project.path()).unwrap().items.is_empty());
    let edges = graph::decisions::extract(project.path());
    let text = serde_json::to_string(&edges).unwrap();
    assert!(!text.contains("secret-choice"));
    assert!(!text.contains(external.path().to_str().unwrap()));
    success(project.path(), &["graph", "rebuild", "--path", "."]);
    let persisted = fs::read_to_string(project.path().join(".phronesis/graph.jsonl")).unwrap();
    assert!(!persisted.contains("secret-choice"));
    assert!(!persisted.contains("private-body"));
    assert!(!persisted.contains(external.path().to_str().unwrap()));
}

#[test]
fn explicit_directory_overrides_config_and_is_project_relative() {
    let (project, external) = fixture();
    configure(project.path(), external.path());
    success(
        project.path(),
        &[
            "decision",
            "new",
            "shared-choice",
            "--decisions-dir",
            "docs/decisions",
        ],
    );
    let output = success(
        project.path(),
        &[
            "decision",
            "list",
            "--decisions-dir",
            "docs/decisions",
            "--json",
        ],
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("shared-choice"));
    let output = success(
        project.path(),
        &["wiki-drift", "--wiki-dir", "docs/decisions", "--json"],
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("shared-choice"));
    let output = success(project.path(), &["decision", "list", "--json"]);
    assert!(!String::from_utf8_lossy(&output.stdout).contains("shared-choice"));
}

#[test]
fn unavailable_or_invalid_configuration_never_falls_back_to_shared_docs() {
    let (project, external) = fixture();
    configure(project.path(), external.path());
    let config = project.path().join(decision_store::CONFIG_PATH);
    for content in [
        "{broken".to_string(),
        serde_json::json!({"directory": external.path().join("missing")}).to_string(),
        serde_json::json!({"directory": "relative/path"}).to_string(),
    ] {
        fs::write(&config, content).unwrap();
        for args in [
            vec!["decision", "new", "private-choice"],
            vec!["decision", "list"],
            vec!["wiki-drift"],
        ] {
            assert!(!cli(project.path(), &args).status.success());
        }
        assert_eq!(
            fs::read_dir(project.path().join("docs/decisions"))
                .unwrap()
                .count(),
            0
        );
    }
}

#[test]
fn configure_rejects_unignored_or_tracked_local_configuration() {
    let (project, external) = fixture();
    fs::write(project.path().join(".git/info/exclude"), "").unwrap();
    assert!(
        !cli(
            project.path(),
            &[
                "decision",
                "configure",
                "--directory",
                external.path().to_str().unwrap()
            ]
        )
        .status
        .success()
    );
    let config = project.path().join(decision_store::CONFIG_PATH);
    assert!(!config.exists());
    fs::write(&config, "{}").unwrap();
    git(project.path(), &["add", "--", decision_store::CONFIG_PATH]);
    fs::write(project.path().join(".git/info/exclude"), "/.phronesis/\n").unwrap();
    assert!(matches!(
        decision_store::configured_directory(project.path()),
        Err(decision_store::StoreError::NotPrivate)
    ));
    assert_eq!(fs::read_to_string(config).unwrap(), "{}");
}

#[test]
fn configure_preserves_existing_mapping_and_rejects_internal_target() {
    let (project, external) = fixture();
    assert!(
        decision_store::configure(project.path(), &project.path().join("docs/decisions")).is_err()
    );
    configure(project.path(), external.path());
    let config = project.path().join(decision_store::CONFIG_PATH);
    let before = fs::read(&config).unwrap();
    let other = tempdir().unwrap();
    assert!(matches!(
        decision_store::configure(project.path(), other.path()),
        Err(decision_store::StoreError::AlreadyConfigured)
    ));
    assert_eq!(fs::read(config).unwrap(), before);
}

#[test]
fn external_duplicate_ids_fail_discovery() {
    let (project, external) = fixture();
    configure(project.path(), external.path());
    let body = "---\nid: repeated\ndate: 2026-11-21\nstatus: proposed\n---\n";
    for name in ["a.md", "b.md"] {
        fs::write(external.path().join(name), body).unwrap();
    }
    assert!(!cli(project.path(), &["decision", "list"]).status.success());
    assert!(!cli(project.path(), &["wiki-drift"]).status.success());
}

#[cfg(unix)]
#[test]
fn linked_configuration_is_rejected_and_external_permissions_are_private() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let (project, external) = fixture();
    configure(project.path(), external.path());
    let config = project.path().join(decision_store::CONFIG_PATH);
    assert_eq!(
        fs::metadata(&config).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let backup = external.path().join("config-backup.json");
    fs::rename(&config, &backup).unwrap();
    symlink(&backup, &config).unwrap();
    assert!(matches!(
        decision_store::configured_directory(project.path()),
        Err(decision_store::StoreError::InvalidFile)
    ));
}

#[test]
fn wiki_drift_relative_override_keeps_invocation_directory_semantics() {
    let (project, external) = fixture();
    configure(project.path(), external.path());
    fs::create_dir(external.path().join("override")).unwrap();
    fs::write(
        external.path().join("override/choice.md"),
        "---\nid: explicit-override\ndate: 2026-11-21\nstatus: proposed\n---\n",
    )
    .unwrap();
    let output = success(
        external.path(),
        &[
            "wiki-drift",
            project.path().to_str().unwrap(),
            "--wiki-dir",
            "override",
            "--json",
        ],
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("explicit-override"));
}
