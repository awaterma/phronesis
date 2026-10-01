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

/// `audit --rule X` where X names a rule stored as `X#or0`/`X#or1` (an `or`
/// clause in `when`) must select those expansions; an unmatched filter must
/// say so rather than blame `.gitignore` for "walked 0 files".
#[test]
fn audit_rule_filter_matches_or_expansions_and_reports_unmatched_filter() {
    let dir = tempfile::tempdir().unwrap();
    let ph = dir.path().join(".phronesis");
    std::fs::create_dir_all(&ph).unwrap();
    std::fs::write(
        ph.join("rules.json"),
        r#"{"rules":[{
            "id":"risky-call",
            "phase":"pre",
            "audit":true,
            "when":[{"or":[{"new_content_contains":".unwrap()"},{"new_content_contains":"panic!("}]}],
            "then":{"constraint_warning":"risky"}
        }]}"#,
    )
    .unwrap();
    std::fs::write(dir.path().join("a.rs"), "fn f() { x.unwrap(); }\n").unwrap();

    let out = run_bin(&["audit", "--rule", "risky-call", "--json"], dir.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("risky-call#or0"),
        "expected the #or0 expansion to be audited; stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(!stderr.contains("walked 0 files"), "stderr: {stderr}");

    let out = run_bin(&["audit", "--rule", "risky"], dir.path());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no opted-in rule matches `risky`"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("risky-call"),
        "near-miss hint missing: {stderr}"
    );
    assert!(!stderr.contains("walked 0 files"), "stderr: {stderr}");
}

#[test]
fn audit_json_keeps_stdout_machine_readable_when_script_diagnostic_is_emitted() {
    let dir = tempfile::tempdir().unwrap();
    let ph = dir.path().join(".phronesis");
    std::fs::create_dir_all(&ph).unwrap();
    std::fs::write(
        ph.join("rules.json"),
        r#"{"rules":[{
            "id":"unsupported-audit-script",
            "phase":"audit",
            "audit":true,
            "when":[
                {"new_content_contains":"print("},
                {"__script__":"facts.len > 0"}
            ],
            "then":{"warn":"unsupported"}
        }]}"#,
    )
    .unwrap();
    std::fs::write(dir.path().join("example.py"), "print('x')\n").unwrap();

    let out = run_bin(&["audit", "--json"], dir.path());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let value: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|error| panic!("stdout must remain JSON: {error}; stdout: {stdout}"));
    assert_eq!(value["rules"], serde_json::json!([]));
    assert!(
        stderr.contains("unsupported-audit-script"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("unsupported"), "stderr: {stderr}");
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
fn consolidated_claude_md_drift_discovers_agents_guidance() {
    let dir = tempfile::tempdir().unwrap();
    make_project(dir.path());
    std::fs::write(
        dir.path().join("AGENTS.md"),
        "- Always run workspace tests before committing\n",
    )
    .unwrap();
    let out = Command::new(bin())
        .args(["drift", "--source", "claude_md"])
        .env("PHRONESIS_PROJECT_ROOT", dir.path())
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "consolidated drift exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("1 present, 0 missing"), "got: {stdout}");
    assert!(
        stdout.contains("AGENTS.md"),
        "origin path missing: {stdout}"
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

/// The three removed MCP tool names must not survive in any artifact that
/// ships to a project or reaches the model. A dead tool name in
/// `durable.md` is re-injected into context every session, so that one
/// matters most.
///
/// Deliberately NOT checked: `CHANGELOG.md` and `.phronesis/wiki/decisions/`
/// are historical records — a release note or an ADR describing what was
/// true then must keep saying so. `docs/specs/` and `docs/superpowers/`
/// describe this very migration.
#[test]
fn no_shipped_artifact_names_the_removed_drift_tools() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root");

    let mut checked = vec![repo.join("crates/phronesis-mcp/CLAUDE.md")];
    let init_dir = repo.join("crates/phronesis-mcp/src/init");
    let mut pending = vec![init_dir.clone()];
    while let Some(dir) = pending.pop() {
        for entry in
            std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
        {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                checked.push(path);
            }
        }
    }
    assert!(
        checked.len() > 2,
        "expected the init/ module tree under {}",
        init_dir.display()
    );

    for path in checked {
        let body = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        for gone in ["get_claude_md_drift", "get_memory_drift", "get_wiki_drift"] {
            assert!(
                !body.contains(gone),
                "{} still names the removed MCP tool {gone}",
                path.display()
            );
        }
    }
}

/// `graph status --json` must emit a pure JSON object on stdout: the human
/// "Resolution hotspots" block must not interleave with the JSON envelope.
/// Found empirically during dogfooding (the JSON had to be rescued by
/// truncating to the last `}`). Human mode must still print the block.
#[test]
fn graph_status_json_is_pure_and_human_mode_still_shows_hotspots() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    make_project(root);
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("lib.rs"), "fn caller() { never_defined(); }\n").unwrap();

    let rebuild = run_bin(&["graph", "rebuild", "--path", "."], root);
    assert!(rebuild.status.success(), "rebuild failed");

    let json = run_bin(&["graph", "status", "--json", "--path", "."], root);
    assert!(json.status.success(), "status --json failed");
    let stdout = String::from_utf8_lossy(&json.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("status --json stdout must be a pure JSON object");
    assert!(
        parsed.get("per_file_resolution").is_some(),
        "status --json must carry per_file_resolution, got: {stdout}"
    );

    let human = run_bin(&["graph", "status", "--path", "."], root);
    assert!(human.status.success(), "status failed");
    let stdout = String::from_utf8_lossy(&human.stdout);
    assert!(
        stdout.contains("Resolution hotspots"),
        "human status must keep the hotspots block, got: {stdout}"
    );
    assert!(
        stdout.contains("src/lib.rs"),
        "human hotspots must name the file with the unresolved call, got: {stdout}"
    );
}

fn ingest_project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join(".phronesis")).expect("mkdir");
    std::fs::write(dir.path().join(".phronesis/rules.json"), r#"{"rules": []}"#).expect("rules");
    std::fs::write(dir.path().join(".phronesis/confidence.json"), "{}").expect("enable");
    dir
}
fn ingest_phr(root: &std::path::Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .env("PHRONESIS_PROJECT_ROOT", root)
        .current_dir(root)
        .args(args)
        .output()
        .expect("run phr-mcp")
}

#[test]
fn signal_ingest_records_parsed_evidence_and_refuses_empty_or_unknown_output() {
    let dir = ingest_project();
    let root = dir.path();
    std::fs::write(
        root.join("gate.log"),
        "test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\\n",
    )
    .expect("log");
    let out = ingest_phr(
        root,
        &[
            "signal",
            "ingest",
            "--command",
            "cargo test --workspace",
            "--output",
            "gate.log",
            "--exit",
            "0",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("outcome:test_pass") && stdout.contains("outcome:ingested"),
        "{stdout}"
    );
    let conf_out = ingest_phr(root, &["confidence", "--json"]);
    let conf: serde_json::Value = serde_json::from_slice(&conf_out.stdout).expect("json");
    assert!(conf["signals"].to_string().contains("tests"), "{conf}");
    std::fs::write(
        root.join("bad.log"),
        "test result: FAILED. 11 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out\\n",
    )
    .expect("log");
    let out = ingest_phr(
        root,
        &[
            "signal",
            "ingest",
            "--command",
            "cargo test",
            "--output",
            "bad.log",
            "--exit",
            "101",
        ],
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("outcome:test_fail"));
    std::fs::write(root.join("empty.log"), "").expect("log");
    let out = ingest_phr(
        root,
        &[
            "signal",
            "ingest",
            "--command",
            "cargo test",
            "--output",
            "empty.log",
        ],
    );
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no outcome"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = ingest_phr(
        root,
        &[
            "signal",
            "ingest",
            "--command",
            "make check",
            "--output",
            "gate.log",
        ],
    );
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no toolchain definition handles"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A `.phronesisignore` entry exempts lexical rules only; it must never
/// hide a production file from structural audit rules (issue #114). The
/// ignore file below carries a `src/*` pattern that used to hide
/// `src/init.rs` from every audit rule. The unwrap rule
/// (`enforce-no-unwrap-in-src`) uses the `rust_governed_invocation` AST
/// predicate, so it is structural and must still scan the ignored file.
#[test]
fn rust_pack_audit_scans_ignored_production_file_and_counts_unwrap() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let init = Command::new(bin())
        .args(["init", "--packs", "rust", root.to_str().unwrap()])
        .current_dir(std::env::temp_dir())
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    let fixture = src.join("init.rs");
    let write_fixture = |with_unwrap: bool| {
        let mut lines = vec!["pub fn fixture() {".to_owned()];
        if with_unwrap {
            lines.push("    let _ = Some(1).unwrap();".to_owned());
        }
        lines.extend((0..1000).map(|i| format!("    let _padding_{i} = {i};")));
        lines.push("}".to_owned());
        std::fs::write(&fixture, lines.join("\n")).unwrap();
    };
    write_fixture(true);
    // `src/*` is the directory pattern that used to hide src/init.rs from
    // every audit rule; the other two lines are lexical exemptions.
    std::fs::write(
        root.join(".phronesisignore"),
        "src/init/rules_rust.rs\nsrc/init.rs\nsrc/*\n",
    )
    .unwrap();

    // Positive case: the structural unwrap rule still scans the ignored
    // fixture and reports exactly one hit on src/init.rs.
    let with_hit = run_bin(&["audit", "--json"], root);
    assert!(
        with_hit.status.success(),
        "audit exited {:?}: {}",
        with_hit.status.code(),
        String::from_utf8_lossy(&with_hit.stderr)
    );
    let report: serde_json::Value = {
        let stdout = String::from_utf8_lossy(&with_hit.stdout);
        serde_json::from_str(&stdout)
            .unwrap_or_else(|e| panic!("audit stdout is not JSON ({e}): {stdout}"))
    };
    assert!(
        report["files_scanned"].as_u64().unwrap_or(0) > 0,
        "files_scanned must be > 0: {report}"
    );
    let unwrap_rule = report["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["rule_id"] == "enforce-no-unwrap-in-src")
        .unwrap_or_else(|| panic!("enforce-no-unwrap-in-src missing: {report}"));
    assert_eq!(
        unwrap_rule["hits"].as_u64(),
        Some(1),
        "expected 1 unwrap hit: {report}"
    );
    assert!(
        unwrap_rule["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["path"]
                .as_str()
                .is_some_and(|p| p.ends_with("src/init.rs"))),
        "fixture hit missing from report: {report}"
    );

    // Negative case: after removing the unwrap, the rule is either absent
    // (audit lists only rules with ≥1 hit) or present with hits == 0. The
    // fixture path must still appear under lexical_excluded, proving the
    // ignore pattern matched but did not hide the file from the walk.
    write_fixture(false);
    let without_hit = run_bin(&["audit", "--json"], root);
    assert!(
        without_hit.status.success(),
        "audit exited {:?}: {}",
        without_hit.status.code(),
        String::from_utf8_lossy(&without_hit.stderr)
    );
    let report: serde_json::Value = {
        let stdout = String::from_utf8_lossy(&without_hit.stdout);
        serde_json::from_str(&stdout)
            .unwrap_or_else(|e| panic!("audit stdout is not JSON ({e}): {stdout}"))
    };
    assert!(
        report["files_scanned"].as_u64().unwrap_or(0) > 0,
        "files_scanned must be > 0 even with no hits: {report}"
    );
    if let Some(rule) = report["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["rule_id"] == "enforce-no-unwrap-in-src")
    {
        assert_eq!(
            rule["hits"].as_u64(),
            Some(0),
            "unwrap rule present but hits != 0: {report}"
        );
    }
    let excluded: Vec<String> = report["lexical_excluded"]
        .as_array()
        .map_or(Vec::new().as_slice(), |v| v.as_slice())
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    assert!(
        excluded.iter().any(|p| p.ends_with("src/init.rs")),
        "src/init.rs must appear under lexical_excluded: {report}"
    );
}

/// The Rust pack's `src/init/rules_rust.rs` embeds its own trigger strings.
/// Three lexical rules (`audit-newtype-id-string`,
/// `audit-allow-dead-code-in-src`, `audit-string-concat-with-plus`) would
/// fire on that self-reference. They are exempted by `//! phronesis-allow:`
/// markers at the top of the file — never by a `.phronesisignore` entry.
/// This test copies the file into a fresh temp project (no ignore file),
/// runs each rule's audit, and proves the marker is what exempts it by
/// stripping the markers and confirming the rules then fire.
#[test]
fn pack_rule_self_reference_is_exempted_by_marker_not_ignore() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let init = Command::new(bin())
        .args(["init", "--packs", "rust", root.to_str().unwrap()])
        .current_dir(std::env::temp_dir())
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    // Copy the worktree's rules_rust.rs into <tmp>/src/init/rules_rust.rs.
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/init/rules_rust.rs");
    let content =
        std::fs::read_to_string(&source).unwrap_or_else(|e| panic!("read rules_rust.rs: {e}"));
    let dest_dir = root.join("src/init");
    std::fs::create_dir_all(&dest_dir).unwrap();
    let dest = dest_dir.join("rules_rust.rs");
    std::fs::write(&dest, &content).unwrap();

    let rules = [
        "audit-newtype-id-string",
        "audit-allow-dead-code-in-src",
        "audit-string-concat-with-plus",
    ];

    // Positive case: with markers present, each rule scans the file but
    // does not report rules_rust.rs among its hits.
    for rule in rules {
        let out = run_bin(&["audit", "--rule", rule, "--json"], root);
        assert!(
            out.status.success(),
            "audit --rule {rule} exited {:?}: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        let report: serde_json::Value = serde_json::from_str(&stdout)
            .unwrap_or_else(|e| panic!("audit {rule} stdout is not JSON ({e}): {stdout}"));
        assert!(
            report["files_scanned"].as_u64().unwrap_or(0) > 0,
            "{rule}: files_scanned must be > 0: {report}"
        );
        let rule_entry = report["rules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["rule_id"] == rule);
        if let Some(entry) = rule_entry {
            let mentions = entry["files"]
                .as_array()
                .map_or(Vec::new().as_slice(), |v| v.as_slice())
                .iter()
                .any(|f| {
                    f["path"]
                        .as_str()
                        .is_some_and(|p| p.ends_with("src/init/rules_rust.rs"))
                });
            assert!(
                !mentions,
                "{rule} still fires on rules_rust.rs with markers present: {report}"
            );
        }
    }

    // Negative case: strip the three `//! phronesis-allow:` lines and
    // re-run. Every rule now fires on rules_rust.rs (its own rule args
    // still contain the trigger strings), proving the marker, not an
    // ignore file, is what exempts the file.
    let stripped: String = content
        .lines()
        .filter(|line| !line.trim_start().starts_with("//! phronesis-allow:"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&dest, &stripped).unwrap();

    for rule in rules {
        let out = run_bin(&["audit", "--rule", rule, "--json"], root);
        assert!(
            out.status.success(),
            "audit --rule {rule} exited {:?}: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        let report: serde_json::Value = serde_json::from_str(&stdout)
            .unwrap_or_else(|e| panic!("audit {rule} stdout is not JSON ({e}): {stdout}"));
        assert!(
            report["files_scanned"].as_u64().unwrap_or(0) > 0,
            "{rule}: files_scanned must be > 0 after strip: {report}"
        );
        let entry = report["rules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["rule_id"] == rule)
            .unwrap_or_else(|| {
                panic!("{rule} absent from audit after stripping markers: {report}")
            });
        let mentions = entry["files"]
            .as_array()
            .map_or(Vec::new().as_slice(), |v| v.as_slice())
            .iter()
            .any(|f| {
                f["path"]
                    .as_str()
                    .is_some_and(|p| p.ends_with("src/init/rules_rust.rs"))
            });
        assert!(
            mentions,
            "{rule} does not fire on rules_rust.rs after stripping markers: {report}"
        );
    }
}
