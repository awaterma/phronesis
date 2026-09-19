//! `phr-mcp journey` CLI tests — the table / `--json` / `--explain` surface
//! introduced in 0.13.0. Mirrors the integration-test pattern used by
//! `confidence_cli_integration.rs`: seed `.phronesis/journey/events.jsonl`
//! and `.phronesis/journey.json` + `.phronesis/rules.json` in a tempdir,
//! point `PHRONESIS_PROJECT_ROOT` at it, and check the binary's output.

use std::path::Path;
use std::process::Command;

fn run(args: &[&str], root: &Path) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(args)
        .env("PHRONESIS_PROJECT_ROOT", root)
        .output()
        .expect("run phr-mcp");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// Seed a project tree with a journey.json + rules.json + journal records,
/// returning the project root. Records are tagged with the supplied `tags`
/// vec; sid is `s-test`.
fn seed_project(root: &Path, config: (&str, &str), records: (usize, &str)) {
    let (rules_json, journey_json) = config;
    let (tagged_records, tag) = records;
    let phr = root.join(".phronesis");
    let journey = phr.join("journey");
    std::fs::create_dir_all(&journey).unwrap();
    std::fs::write(phr.join("rules.json"), rules_json).unwrap();
    std::fs::write(phr.join("journey.json"), journey_json).unwrap();
    std::fs::write(journey.join("session"), "s-test").unwrap();
    let mut lines = Vec::new();
    for i in 0..tagged_records {
        let line = serde_json::json!({
            "v": 1,
            "ts": 1000u64 + i as u64,
            "sid": "s-test",
            "seq": i as u64 + 1,
            "tool": "Edit",
            "path": "src/auth/x.rs",
            "ext": "rs",
            "tags": [tag],
        })
        .to_string();
        lines.push(line);
    }
    std::fs::write(journey.join("events.jsonl"), lines.join("\n") + "\n").unwrap();
}

macro_rules! seed {
    ($root:expr, $rules:expr, $journey:expr, $count:expr, $tag:expr) => {
        seed_project($root, ($rules, $journey), ($count, $tag))
    };
}

const AUTH_JOURNEY_JSON: &str = r#"{
    "version":1,
    "taggers":[{"tag":"auth","when":[{"file_path_matches":"src/auth/"}]}],
    "modules":[]
}"#;

const AUTH_CHURN_RULES: &str = r#"{"rules":[
    {"id":"auth-churn","phase":"pre","priority":10,
     "when":[{"__script__":"facts_count('journey_occurrence', ['auth','s']) >= 3"}],
     "then":{"warn":"churn"}}
]}"#;

#[test]
fn journey_command_renders_current_facts() {
    let dir = tempfile::tempdir().unwrap();
    seed!(dir.path(), AUTH_CHURN_RULES, AUTH_JOURNEY_JSON, 3, "auth");

    let (code, stdout, stderr) = run(&["journey", "--json"], dir.path());
    assert_eq!(code, 0, "stderr: {}", stderr);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert!(v.is_array(), "stdout: {}", stdout);
    // We expect 3 journey_occurrence facts for ('auth','s').
    let occurrences: Vec<_> = v
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["predicate"] == "journey_occurrence" && r["selector"] == "auth")
        .collect();
    assert_eq!(occurrences.len(), 3, "stdout: {}", stdout);
    // The auth-churn rule should be attributed.
    assert!(
        occurrences.iter().all(|r| r["rules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x == "auth-churn")),
        "stdout: {}",
        stdout
    );
}

#[test]
fn journey_command_table_default_is_human_readable() {
    let dir = tempfile::tempdir().unwrap();
    seed!(dir.path(), AUTH_CHURN_RULES, AUTH_JOURNEY_JSON, 3, "auth");

    let (code, stdout, stderr) = run(&["journey"], dir.path());
    assert_eq!(code, 0, "stderr: {}", stderr);
    // Header row
    assert!(stdout.contains("PREDICATE"), "stdout: {}", stdout);
    assert!(stdout.contains("ARGS"), "stdout: {}", stdout);
    assert!(stdout.contains("RULES"), "stdout: {}", stdout);
    // At least one data row
    assert!(stdout.contains("journey_occurrence"), "stdout: {}", stdout);
    assert!(stdout.contains("auth"), "stdout: {}", stdout);
    assert!(stdout.contains("auth-churn"), "stdout: {}", stdout);
}

#[test]
fn journey_command_explain_filters_to_one_rule() {
    // Two rules: one referencing 'auth', one referencing 'sql'. seed only
    // auth records — `--explain auth-churn` should return the auth rows;
    // `--explain sql-rule` would return nothing, but we test the positive
    // filter here.
    let rules = r#"{"rules":[
        {"id":"auth-churn","phase":"pre","priority":10,
         "when":[{"__script__":"facts_count('journey_occurrence', ['auth','s']) >= 3"}],
         "then":{"warn":"churn"}},
        {"id":"sql-rule","phase":"pre","priority":10,
         "when":[{"journey_seen":["sql","5c"]}],
         "then":{"warn":"sql"}}
    ]}"#;
    let journey_json = r#"{
        "version":1,
        "taggers":[
            {"tag":"auth","when":[{"file_path_matches":"src/auth/"}]},
            {"tag":"sql","when":[{"new_content_contains":"INSERT INTO"}]}
        ],
        "modules":[]
    }"#;
    let dir = tempfile::tempdir().unwrap();
    seed!(dir.path(), rules, journey_json, 3, "auth");

    let (code, stdout, stderr) = run(
        &["journey", "--json", "--explain", "auth-churn"],
        dir.path(),
    );
    assert_eq!(code, 0, "stderr: {}", stderr);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    let rows = v.as_array().unwrap();
    // Each row's rules list should contain auth-churn — and ONLY rows
    // that auth-churn references should appear (no journey_seen rows).
    assert!(!rows.is_empty(), "stdout: {}", stdout);
    for row in rows {
        assert_eq!(row["predicate"], "journey_occurrence", "stdout: {}", stdout);
        assert_eq!(row["selector"], "auth", "stdout: {}", stdout);
        assert!(
            row["rules"]
                .as_array()
                .unwrap()
                .iter()
                .any(|x| x == "auth-churn"),
            "stdout: {}",
            stdout
        );
    }
}

#[test]
fn journey_command_explain_unknown_rule_errors() {
    let dir = tempfile::tempdir().unwrap();
    seed!(dir.path(), AUTH_CHURN_RULES, AUTH_JOURNEY_JSON, 3, "auth");

    let (code, _stdout, stderr) = run(&["journey", "--explain", "no-such-rule"], dir.path());
    assert_eq!(code, 1, "stderr: {}", stderr);
    assert!(
        stderr.contains("no rule with id 'no-such-rule'"),
        "stderr: {}",
        stderr
    );
}

#[test]
fn journey_command_nudges_when_journey_config_missing() {
    // Project has rules.json but no journey.json. The CLI should still
    // succeed with empty output (continuing with TaggerConfig::default()),
    // *and* emit a stderr nudge pointing the operator at the scaffolder.
    //
    // Use a rules pack that doesn't reference journey_* facts so the
    // derive pass with the default empty config has no undefined-selector
    // complaints to make.
    let rules_no_journey = r#"{"rules":[
        {"id":"plain","phase":"pre","priority":10,
         "when":[{"new_content_contains":"FIXME"}],
         "then":{"warn":"fixme"}}
    ]}"#;
    let dir = tempfile::tempdir().unwrap();
    let phr = dir.path().join(".phronesis");
    std::fs::create_dir_all(&phr).unwrap();
    std::fs::write(phr.join("rules.json"), rules_no_journey).unwrap();
    // Deliberately no .phronesis/journey.json.

    let (code, stdout, stderr) = run(&["journey", "--json"], dir.path());
    assert_eq!(code, 0, "stderr: {}", stderr);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(v.as_array().unwrap().len(), 0, "stdout: {}", stdout);
    assert!(
        stderr.contains("no .phronesis/journey.json")
            && stderr.contains("phr-mcp init --packs journey"),
        "expected scaffold nudge on stderr, got: {}",
        stderr
    );
}

#[test]
fn journey_command_handles_missing_journal() {
    let dir = tempfile::tempdir().unwrap();
    // rules + journey.json only — no events.jsonl
    let phr = dir.path().join(".phronesis");
    std::fs::create_dir_all(&phr).unwrap();
    std::fs::write(phr.join("rules.json"), AUTH_CHURN_RULES).unwrap();
    std::fs::write(phr.join("journey.json"), AUTH_JOURNEY_JSON).unwrap();

    let (code, stdout, stderr) = run(&["journey", "--json"], dir.path());
    assert_eq!(code, 0, "stderr: {}", stderr);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(v.as_array().unwrap().len(), 0, "stdout: {}", stdout);
}

/// Append v2 lifecycle journal records to a seeded project, and open a kalpa.
fn append_lifecycle_records(root: &Path) {
    let events = root.join(".phronesis/journey/events.jsonl");
    let mut lines = std::fs::read_to_string(&events).unwrap_or_default();
    for (ts, seq, kind, mode) in [
        (2000u64, 100u64, "prompt", Some("fresh")),
        (2001, 101, "interrupt", None),
        (2002, 102, "prompt", Some("correction")),
        (2003, 103, "subagent_stop", None),
    ] {
        let mut rec = serde_json::json!({
            "v": 2, "ts": ts, "sid": "s-test", "seq": seq,
            "tool": "__lifecycle", "path": "", "tags": [format!("lifecycle:{kind}")],
            "kind": kind, "host": "claude", "kalpa": "demo",
        });
        if let Some(m) = mode {
            rec["mode"] = serde_json::json!(m);
        }
        lines.push_str(&rec.to_string());
        lines.push('\n');
    }
    std::fs::write(&events, lines).unwrap();
    std::fs::write(
        root.join(".phronesis/journey/kalpa"),
        serde_json::json!({"name": "demo", "started_ts": 1000}).to_string(),
    )
    .unwrap();
}

#[test]
fn journey_table_renders_lifecycle_records_and_kalpa_header() {
    let dir = tempfile::tempdir().unwrap();
    seed!(dir.path(), AUTH_CHURN_RULES, AUTH_JOURNEY_JSON, 3, "auth");
    append_lifecycle_records(dir.path());

    let (code, stdout, stderr) = run(&["journey"], dir.path());
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.starts_with("kalpa: demo ("), "stdout: {stdout}");
    assert!(
        stdout.contains("journey_occurrence"),
        "facts still render: {stdout}"
    );
    assert!(stdout.contains("⟂"), "stdout: {stdout}");
    assert!(stdout.contains("prompt/correction"), "stdout: {stdout}");
    assert!(stdout.contains("subagent_stop"), "stdout: {stdout}");
}

#[test]
fn journey_lifecycle_flag_shows_only_lifecycle_records() {
    let dir = tempfile::tempdir().unwrap();
    seed!(dir.path(), AUTH_CHURN_RULES, AUTH_JOURNEY_JSON, 3, "auth");
    append_lifecycle_records(dir.path());

    let (code, stdout, stderr) = run(&["journey", "--lifecycle"], dir.path());
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(!stdout.contains("journey_occurrence"), "stdout: {stdout}");
    assert!(stdout.contains("prompt/fresh"), "stdout: {stdout}");

    let (code, stdout, _) = run(&["journey", "--lifecycle", "--json"], dir.path());
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    let rows = v.as_array().unwrap();
    assert_eq!(rows.len(), 4, "stdout: {stdout}");
    assert_eq!(rows[0]["kind"], "prompt");
    assert_eq!(rows[0]["mode"], "fresh");
    assert_eq!(rows[3]["kind"], "subagent_stop");
}
