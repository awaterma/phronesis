//! CLI-level coverage for the `phr-mcp context` subcommands and the opt-in
//! boundary they sit behind.
//!
//! These drive the real binary rather than the library, so they also pin the
//! two properties that matter most in production and cannot be observed from a
//! unit test: that a project which has not opted in behaves exactly as it did
//! before, and that `context inspect` is a genuine dry run.

use std::path::Path;
use std::process::Command;

fn bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_phr-mcp"))
}

struct Output {
    code: i32,
    stdout: String,
    stderr: String,
}

fn run(root: &Path, args: &[&str]) -> Output {
    let out = Command::new(bin())
        .args(args)
        .env("PHRONESIS_PROJECT_ROOT", root)
        .current_dir(root)
        .output()
        .expect("spawn phr-mcp");
    Output {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    }
}

/// A project with rules, a durable document, and one logged blocking
/// decision — enough that every section of both payloads has content.
fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let ph = dir.path().join(".phronesis");
    std::fs::create_dir_all(&ph).expect("mkdir .phronesis");
    std::fs::write(
        ph.join("rules.json"),
        r#"{"rules":[{"id":"r1","phase":"pre","priority":10,
            "when":[{"new_content_contains":"__never__"}],
            "then":{"block":"Don't do X"}}]}"#,
    )
    .expect("write rules");
    std::fs::write(
        ph.join("durable.md"),
        "# Project\n\n## Review\n\nAlways review before merging.\n",
    )
    .expect("write durable");
    std::fs::write(
        ph.join("log.jsonl"),
        "{\"ts\":1700000000,\"kind\":\"hook\",\"event\":\"pre_check\",\"file\":\"src/x.rs\",\
         \"exit\":2,\"consequences\":[{\"rule_id\":\"r1\",\"action_type\":\"constraint_violation\",\
         \"message\":\"m\",\"bindings\":{}}]}\n",
    )
    .expect("write log");
    dir
}

fn opt_in(root: &Path) {
    let config = serde_json::to_string(&phronesis_mcp::context::config::ContextConfig::default())
        .expect("serialize config");
    std::fs::write(root.join(".phronesis/context.json"), config).expect("write context.json");
    std::fs::write(root.join(".phronesis/kernel.md"), "Always-on kernel line.")
        .expect("write kernel.md");
}

fn context_records(root: &Path) -> usize {
    std::fs::read_to_string(root.join(".phronesis/log.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter(|l| l.contains("\"kind\":\"context\""))
        .count()
}

fn body_of(envelope: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(envelope.trim()).expect("envelope is JSON");
    value["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

// ── predicates ──────────────────────────────────────────────────────────

#[test]
fn predicates_lists_the_whole_allowlist() {
    let dir = project();
    let out = run(dir.path(), &["context", "predicates"]);
    assert_eq!(out.code, 0);
    for predicate in phronesis_mcp::context::capsule::ALLOWED_PREDICATES {
        assert!(
            out.stdout.contains(predicate),
            "`{predicate}` missing from `context predicates`: {}",
            out.stdout
        );
    }
}

#[test]
fn predicates_json_is_machine_readable() {
    let dir = project();
    let out = run(dir.path(), &["context", "predicates", "--json"]);
    assert_eq!(out.code, 0);
    let value: serde_json::Value = serde_json::from_str(&out.stdout).expect("valid JSON");
    let listed = value["predicates"].as_array().expect("predicates array");
    assert_eq!(
        listed.len(),
        phronesis_mcp::context::capsule::ALLOWED_PREDICATES.len()
    );
}

// ── the opt-in boundary ─────────────────────────────────────────────────

#[test]
fn without_context_json_the_payloads_are_unchanged_and_unmeasured() {
    let dir = project();
    let session = run(dir.path(), &["session-context"]);
    let interaction = run(dir.path(), &["interaction-context"]);
    assert_eq!(session.code, 0);
    assert_eq!(interaction.code, 0);

    // The legacy renderer emits the whole durable file, uncut at this size.
    let body = body_of(&interaction.stdout);
    assert!(body.contains("Always review before merging."));
    assert!(body.contains("BLOCKED"));
    assert_eq!(
        context_records(dir.path()),
        0,
        "a project that has not opted in must record no context observations"
    );
}

#[test]
fn opting_in_splits_the_kernel_from_the_session_document() {
    let dir = project();
    opt_in(dir.path());

    let interaction = body_of(&run(dir.path(), &["interaction-context"]).stdout);
    assert!(interaction.contains("Always-on kernel line."));
    assert!(interaction.contains("BLOCKED"));
    assert!(
        !interaction.contains("Always review before merging."),
        "the session document must not ride along on every turn:\n{interaction}"
    );

    let session = body_of(&run(dir.path(), &["session-context"]).stdout);
    assert!(session.contains("Always-on kernel line."));
    assert!(
        session.contains("Always review before merging."),
        "but it must still be delivered once per session:\n{session}"
    );
}

// ── inspect ─────────────────────────────────────────────────────────────

#[test]
fn inspect_reports_that_a_project_has_not_opted_in() {
    let dir = project();
    let out = run(
        dir.path(),
        &["context", "inspect", "--event", "interaction"],
    );
    assert_eq!(out.code, 0);
    assert!(
        out.stdout.contains("has not opted in"),
        "inspect must say why there is nothing to pack: {}",
        out.stdout
    );

    let json = run(
        dir.path(),
        &["context", "inspect", "--event", "interaction", "--json"],
    );
    let value: serde_json::Value = serde_json::from_str(&json.stdout).expect("valid JSON");
    assert_eq!(value["config_status"]["state"], "missing");
}

#[test]
fn inspect_writes_no_observation() {
    // The property that makes inspect trustworthy: reading the diagnostic
    // must not alter the data the diagnostic reports on.
    let dir = project();
    opt_in(dir.path());
    for event in ["interaction", "session"] {
        for _ in 0..3 {
            assert_eq!(
                run(dir.path(), &["context", "inspect", "--event", event]).code,
                0
            );
        }
    }
    assert_eq!(
        context_records(dir.path()),
        0,
        "inspect must never append to log.jsonl"
    );

    // But the live path does record, so the zero above is meaningful.
    assert_eq!(run(dir.path(), &["interaction-context"]).code, 0);
    assert_eq!(context_records(dir.path()), 1);
}

#[test]
fn inspect_json_carries_candidates_costs_and_omission_reasons() {
    let dir = project();
    opt_in(dir.path());
    // A kernel far over its ceiling, so something is certain to be omitted.
    std::fs::write(
        dir.path().join(".phronesis/kernel.md"),
        "## Section\n\nA guidance paragraph.\n\n".repeat(40),
    )
    .expect("write kernel");

    let out = run(
        dir.path(),
        &["context", "inspect", "--event", "interaction", "--json"],
    );
    assert_eq!(out.code, 0);
    let value: serde_json::Value = serde_json::from_str(&out.stdout).expect("valid JSON");

    assert!(value["bytes"].as_u64().expect("bytes") > 0);
    assert!(value["estimated_tokens"].as_u64().expect("tokens") > 0);
    assert_eq!(value["raw_truncation"], false);
    assert!(
        !value["candidates"]
            .as_array()
            .expect("candidates")
            .is_empty(),
        "candidates must be enumerated"
    );
    let omitted = value["omitted"].as_array().expect("omitted");
    assert!(
        !omitted.is_empty(),
        "this fixture must omit kernel sections"
    );
    assert_eq!(omitted[0]["reason"], "kind_ceiling");
    assert!(value["config"]["hard_max_bytes"].as_u64().expect("ceiling") > 0);
}

#[test]
fn inspect_reports_a_capsule_that_cannot_load() {
    let dir = project();
    opt_in(dir.path());
    std::fs::create_dir_all(dir.path().join(".phronesis/nudges")).expect("mkdir nudges");
    std::fs::write(
        dir.path().join(".phronesis/nudges/broken.md"),
        "not a capsule at all\n",
    )
    .expect("write capsule");

    let out = run(
        dir.path(),
        &["context", "inspect", "--event", "interaction", "--json"],
    );
    let value: serde_json::Value = serde_json::from_str(&out.stdout).expect("valid JSON");
    let diagnostics = value["capsule_diagnostics"]
        .as_array()
        .expect("capsule_diagnostics");
    assert!(
        diagnostics.iter().any(|d| d
            .as_str()
            .is_some_and(|s| s.contains("broken.md") && s.contains("---json"))),
        "a capsule that fails to load must be named with its reason: {diagnostics:?}"
    );
}

#[test]
fn the_scaffolded_nudges_readme_is_not_treated_as_a_capsule() {
    // `init` writes this file itself; parsing it as a capsule would emit a
    // diagnostic on every single hook invocation.
    let dir = project();
    opt_in(dir.path());
    std::fs::create_dir_all(dir.path().join(".phronesis/nudges")).expect("mkdir nudges");
    std::fs::write(
        dir.path().join(".phronesis/nudges/README.md"),
        "# Situational context nudges\n\nDocs, not a capsule.\n",
    )
    .expect("write readme");

    let out = run(
        dir.path(),
        &["context", "inspect", "--event", "interaction", "--json"],
    );
    let value: serde_json::Value = serde_json::from_str(&out.stdout).expect("valid JSON");
    assert!(
        value["capsule_diagnostics"]
            .as_array()
            .expect("capsule_diagnostics")
            .is_empty(),
        "README.md must be skipped silently: {}",
        value["capsule_diagnostics"]
    );
}

// ── stats ───────────────────────────────────────────────────────────────

#[test]
fn stats_on_an_empty_log_exits_zero() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = run(dir.path(), &["context", "stats"]);
    assert_eq!(out.code, 0);
    assert!(out.stdout.contains("Payloads:"));
}

#[test]
fn stats_aggregates_recorded_payloads_in_both_formats() {
    let dir = project();
    opt_in(dir.path());
    for _ in 0..3 {
        run(dir.path(), &["interaction-context"]);
    }

    let table = run(dir.path(), &["context", "stats"]);
    assert_eq!(table.code, 0);
    assert!(table.stdout.contains("Payloads:                 3"));

    let json = run(dir.path(), &["context", "stats", "--json"]);
    let value: serde_json::Value = serde_json::from_str(&json.stdout).expect("valid JSON");
    assert_eq!(value["payloads"], 3);
    assert_eq!(value["raw_truncations"], 0);
    assert!(value["median_bytes"].as_u64().expect("median") > 0);
}

#[test]
fn stats_rejects_an_unparseable_since_rather_than_widening_to_all_time() {
    // Silently treating a bad window as "everything" would report more data
    // than was asked for while looking like a real answer.
    let dir = project();
    let out = run(dir.path(), &["context", "stats", "--since", "banana"]);
    assert_ne!(out.code, 0, "an invalid window must not exit zero");
    assert!(
        out.stderr.contains("invalid --since"),
        "and must say so: {}",
        out.stderr
    );
}

#[test]
fn stats_honours_a_valid_since_window() {
    let dir = project();
    opt_in(dir.path());
    run(dir.path(), &["interaction-context"]);
    let out = run(dir.path(), &["context", "stats", "--since", "7d", "--json"]);
    assert_eq!(out.code, 0);
    let value: serde_json::Value = serde_json::from_str(&out.stdout).expect("valid JSON");
    assert_eq!(value["payloads"], 1);
}

#[test]
fn existing_stats_readers_ignore_context_observations() {
    let dir = project();
    opt_in(dir.path());
    for _ in 0..3 {
        run(dir.path(), &["interaction-context"]);
    }
    // `stats` reads kind=hook; the fixture log has exactly one such record.
    let out = run(dir.path(), &["stats", "--json"]);
    assert_eq!(out.code, 0);
    let value: serde_json::Value = serde_json::from_str(&out.stdout).expect("valid JSON");
    let blocked = value["totals"]["blocked"].as_u64().unwrap_or(0);
    assert_eq!(
        blocked, 1,
        "context observations must not be counted as rule firings: {}",
        out.stdout
    );
}
