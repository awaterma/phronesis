//! Minimal characterization smoke tests for CLI arms that have no other
//! integration coverage. Each test spawns the binary with the smallest
//! set of arguments that exercises the arm, asserts exit status, and
//! checks one stdout/stderr marker. These are nets, not feature tests.

use std::path::Path;
use std::process::Command;

fn bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_phr-mcp"))
}

fn run_bin(args: &[&str], root: &Path) -> std::process::Output {
    Command::new(bin())
        .args(args)
        .env("PHRONESIS_PROJECT_ROOT", root)
        .current_dir(root)
        .output()
        .expect("run phr-mcp")
}

/// Write a minimal `.phronesis/rules.json` with one audit-tagged rule so
/// the audit arm can run without warnings about missing or untagged rules.
fn make_project_with_audit_rule(dir: &Path) {
    let ph = dir.join(".phronesis");
    std::fs::create_dir_all(&ph).unwrap();
    std::fs::write(
        ph.join("rules.json"),
        r#"{"rules":[{
            "id":"smoke-test-never-matches",
            "phase":"pre",
            "audit":true,
            "when":[{"new_content_contains":"__smoke_test_xyzzy_never__"}],
            "then":{"log":"smoke"}
        }]}"#,
    )
    .unwrap();
}

/// Write a minimal `.phronesis/rules.json` (no audit tag needed).
fn make_project(dir: &Path) {
    let ph = dir.join(".phronesis");
    std::fs::create_dir_all(&ph).unwrap();
    std::fs::write(
        ph.join("rules.json"),
        r#"{"rules":[{
            "id":"smoke-rule",
            "phase":"pre",
            "when":[{"new_content_contains":"__smoke__"}],
            "then":{"log":"ok"}
        }]}"#,
    )
    .unwrap();
}

/// Every pack `--packs` accepts must be discoverable from `--help`.
///
/// Asserted name by name rather than as one contiguous string: clap rewraps
/// the paragraph at the terminal width, so a substring spanning the wrap point
/// fails for reasons that have nothing to do with the pack list.
#[test]
fn init_help_lists_every_installable_pack() {
    let out = Command::new(bin())
        .args(["init", "--help"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "init --help exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    for pack in [
        "llm",
        "rust",
        "rhai",
        "python",
        "typescript",
        "swift",
        "confidence",
        "journey",
        "context",
        "structural",
        "none",
    ] {
        assert!(
            stdout.contains(pack),
            "installable pack `{pack}` missing from help: {stdout}"
        );
    }
}

#[test]
fn audit_exits_zero_and_emits_audit_output() {
    let dir = tempfile::tempdir().unwrap();
    make_project_with_audit_rule(dir.path());
    let out = Command::new(bin())
        .arg("audit")
        .env("PHRONESIS_PROJECT_ROOT", dir.path())
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "audit exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // With no source files to scan the diagnostic goes to stderr ("walked 0 files").
    // With source files it goes to stdout ("no audit violations found").
    // Either way the word "phronesis" appears somewhere in the output.
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("phronesis")
            || combined.contains("files scanned")
            || combined.contains("Total"),
        "expected audit output marker, got: {combined}"
    );
}

#[test]
fn stats_exits_zero_on_empty_log() {
    let dir = tempfile::tempdir().unwrap();
    make_project(dir.path());
    let out = Command::new(bin())
        .arg("stats")
        .env("PHRONESIS_PROJECT_ROOT", dir.path())
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stats exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // render_table returns "no phronesis activity recorded yet\n" when log is empty.
    assert!(
        stdout.contains("no phronesis activity"),
        "expected empty-log message in stdout, got: {stdout}"
    );
}

#[test]
fn trend_exits_zero_on_empty_log() {
    let dir = tempfile::tempdir().unwrap();
    make_project(dir.path());
    let out = Command::new(bin())
        .arg("trend")
        .env("PHRONESIS_PROJECT_ROOT", dir.path())
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "trend exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // render_trend_table returns "no audit snapshots recorded yet; …" when empty.
    assert!(
        stdout.contains("no audit snapshots"),
        "expected empty-snapshots message in stdout, got: {stdout}"
    );
}

#[test]
fn claude_md_drift_exits_nonzero_and_names_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    make_project(dir.path());
    // No CLAUDE.md written — the arm exits 1 and names the missing path.
    let out = Command::new(bin())
        .args(["claude-md-drift", "."])
        .env("PHRONESIS_PROJECT_ROOT", dir.path())
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "expected non-zero exit when CLAUDE.md is absent"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("CLAUDE.md"),
        "expected 'CLAUDE.md' in stderr, got: {stderr}"
    );
}

#[test]
fn memory_drift_exits_nonzero_when_memory_dir_missing() {
    let dir = tempfile::tempdir().unwrap();
    make_project(dir.path());
    let out = Command::new(bin())
        .args([
            "memory-drift",
            "--memory-dir",
            "/nonexistent/smoke/dir",
            ".",
        ])
        .env("PHRONESIS_PROJECT_ROOT", dir.path())
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "expected non-zero exit when memory dir is absent"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("memory directory"),
        "expected 'memory directory' in stderr, got: {stderr}"
    );
}

#[test]
fn drift_cmd_defaults_to_all_sources_and_succeeds_on_a_bare_project() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = run_bin(&["drift", "--json"], dir.path());
    assert!(
        out.status.success(),
        "drift must not fail when corpora are absent: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let body = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&body).expect("valid json");
    assert_eq!(
        v["sources"].as_array().map(|a| a.len()),
        Some(4),
        "all four sources must be reported: {body}"
    );
}

#[test]
fn drift_cmd_rejects_an_unknown_source() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = run_bin(&["drift", "--source", "nope"], dir.path());
    assert!(!out.status.success(), "unknown source must fail");
    // `!success` alone is too weak: it also holds when the `drift`
    // subcommand does not exist at all, which is exactly the state this
    // test was written in. Pin the reason, so a future removal of the
    // command cannot make this test keep passing for the wrong cause.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unknown source") && stderr.contains("nope"),
        "must fail because the source is unknown, not because the command is: {stderr}"
    );
}

#[test]
fn interaction_context_is_canonical_and_turn_context_remains_an_alias() {
    let dir = tempfile::tempdir().unwrap();
    make_project(dir.path());
    std::fs::write(
        dir.path().join(".phronesis/durable.md"),
        "interaction guidance",
    )
    .unwrap();
    let run = |command: &str| {
        Command::new(bin())
            .arg(command)
            .env("PHRONESIS_PROJECT_ROOT", dir.path())
            .current_dir(dir.path())
            .output()
            .unwrap()
    };
    let canonical = run("interaction-context");
    let legacy = run("turn-context");
    assert!(canonical.status.success());
    assert!(legacy.status.success());
    assert_eq!(canonical.stdout, legacy.stdout);
    assert!(String::from_utf8_lossy(&canonical.stdout).contains("interaction guidance"));
}
