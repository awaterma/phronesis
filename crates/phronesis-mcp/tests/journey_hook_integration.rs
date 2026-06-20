//! End-to-end hook tests for the journey wiring (SPEC-journey-facts §"Where
//! it plugs into the hook"). Drive the `phr-mcp` binary against tempdir
//! projects and verify:
//!
//! 1. post-check journals an executed call, stamping tagger output;
//! 2. pre-check blocks on a journey rule that fires off prior records;
//! 3. `PHRONESIS_NO_JOURNEY=1` disables both derive *and* journaling;
//! 4. a malformed `journey.json` fails open — pre-check does not block.
//!
//! `PHRONESIS_NO_ACTION_LOG=1` is set on every run so the noisy log.jsonl
//! doesn't bleed into the project tempdir; that env var is unrelated to the
//! journey path under test.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

fn run_hook(
    subcommand: &str,
    payload: &str,
    root: &Path,
    no_journey: bool,
) -> (i32, String, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_phr-mcp"));
    cmd.arg(subcommand)
        .env("PHRONESIS_PROJECT_ROOT", root)
        .env("PHRONESIS_NO_ACTION_LOG", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if no_journey {
        cmd.env("PHRONESIS_NO_JOURNEY", "1");
    }
    let mut child = cmd.spawn().expect("spawn hook");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn setup_project(rules_json: &str, journey_json: Option<&str>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let phr = dir.path().join(".phronesis");
    std::fs::create_dir_all(&phr).unwrap();
    std::fs::write(phr.join("rules.json"), rules_json).unwrap();
    if let Some(j) = journey_json {
        std::fs::write(phr.join("journey.json"), j).unwrap();
    }
    dir
}

/// Pre-seed the session id the hook reads. Avoids a date-bucket-fallback sid
/// race when assertions check for the session window.
fn write_session(root: &Path, sid: &str) {
    let journey = root.join(".phronesis").join("journey");
    std::fs::create_dir_all(&journey).unwrap();
    std::fs::write(journey.join("session"), sid).unwrap();
}

#[test]
fn post_check_journals_executed_call() {
    let rules = r#"{"rules":[]}"#;
    let journey = r#"{
        "version":1,
        "taggers":[{"tag":"auth","when":[{"file_path_matches":"auth"}]}],
        "modules":[]
    }"#;
    let dir = setup_project(rules, Some(journey));
    write_session(dir.path(), "s-test");

    let payload = r#"{
        "tool_name":"Edit",
        "tool_input":{
            "file_path":"src/auth/login.rs",
            "old_string":"",
            "new_string":"pub fn login() {}"
        }
    }"#;
    let (code, _stdout, stderr) = run_hook("post-check", payload, dir.path(), false);
    assert_eq!(code, 0, "stderr: {stderr}");

    let events = dir
        .path()
        .join(".phronesis")
        .join("journey")
        .join("events.jsonl");
    let journal = std::fs::read_to_string(&events).expect("events.jsonl written");
    assert!(
        journal.contains("\"auth\""),
        "journal should carry auth tag: {journal}"
    );
    assert!(
        journal.contains("\"tool\":\"Edit\""),
        "journal should carry tool: {journal}"
    );
    assert!(
        journal.contains("src/auth/login.rs"),
        "journal should carry path: {journal}"
    );
}

#[test]
fn post_check_journals_bash_with_build_tag() {
    // The default `build` tagger from `phr-mcp init --packs journey` keys on
    // `bash_command_matches: "cargo (build|check|test)"`. Without the synthetic
    // `bash_command_matches:<pattern>` fact, the tagger silently never fires
    // because the engine's equality matcher has nothing to match against.
    // Regression guard: a `cargo check --workspace` Bash call must land the
    // `build` tag in the journal record.
    let rules = r#"{"rules":[]}"#;
    let journey = r#"{
        "version":1,
        "taggers":[
            {"tag":"build","when":[{"bash_command_matches":"cargo (build|check|test)"}]}
        ],
        "modules":[]
    }"#;
    let dir = setup_project(rules, Some(journey));
    write_session(dir.path(), "s-test");

    let payload = r#"{
        "tool_name":"Bash",
        "tool_input":{ "command": "cargo check --workspace" }
    }"#;
    let (code, _stdout, stderr) = run_hook("post-check", payload, dir.path(), false);
    assert_eq!(code, 0, "stderr: {stderr}");

    let events = dir
        .path()
        .join(".phronesis")
        .join("journey")
        .join("events.jsonl");
    let journal = std::fs::read_to_string(&events).expect("events.jsonl written");
    assert!(
        journal.contains("\"tags\":[\"build\"]"),
        "build tag should appear in record: {journal}"
    );
}

#[test]
fn pre_check_blocks_on_journey_rule() {
    // The rule fires when a `sql`-tagged record was seen in the last 5 calls.
    let rules = r#"{
        "rules":[
            {
                "id":"sql-recent",
                "phase":"pre",
                "priority":10,
                "when":[{"journey_seen":["sql","5c"]}],
                "then":{"block":"Recent SQL — verify the target."}
            }
        ]
    }"#;
    let journey = r#"{
        "version":1,
        "taggers":[{"tag":"sql","when":[{"new_content_contains":"INSERT INTO"}]}],
        "modules":[]
    }"#;
    let dir = setup_project(rules, Some(journey));
    write_session(dir.path(), "s-test");

    // Seed: one prior call that produced a `sql` tag in the current session.
    let rec = serde_json::json!({
        "v":1,"ts":1718700000,"sid":"s-test","seq":1,
        "tool":"Edit","path":"src/db.rs","ext":"rs",
        "tags":["sql"]
    });
    let journey_dir = dir.path().join(".phronesis").join("journey");
    std::fs::create_dir_all(&journey_dir).unwrap();
    std::fs::write(journey_dir.join("events.jsonl"), format!("{}\n", rec)).unwrap();

    let payload = r#"{
        "tool_name":"Edit",
        "tool_input":{
            "file_path":"src/unrelated.rs",
            "old_string":"",
            "new_string":"pub fn foo() {}"
        }
    }"#;
    let (code, _stdout, stderr) = run_hook("pre-check", payload, dir.path(), false);
    assert_eq!(code, 2, "exit 2 = blocked; stderr: {stderr}");
    assert!(
        stderr.contains("Recent SQL"),
        "expected block message in stderr: {stderr}"
    );
}

#[test]
fn no_journey_env_var_disables_both_paths() {
    // Same setup as the blocking test — a pre-seeded events.jsonl that would
    // ordinarily trip the rule. With PHRONESIS_NO_JOURNEY=1, derive is
    // skipped (the rule doesn't see the prior record and so doesn't fire) and
    // post-check journaling is skipped too (no new record appended).
    let rules = r#"{
        "rules":[
            {
                "id":"sql-recent",
                "phase":"pre",
                "priority":10,
                "when":[{"journey_seen":["sql","5c"]}],
                "then":{"block":"Recent SQL"}
            }
        ]
    }"#;
    let dir = setup_project(rules, None);
    write_session(dir.path(), "s-test");
    let journey_dir = dir.path().join(".phronesis").join("journey");
    let rec = serde_json::json!({
        "v":1,"ts":1718700000,"sid":"s-test","seq":1,
        "tool":"Edit","path":"src/db.rs","ext":"rs",
        "tags":["sql"]
    });
    std::fs::write(journey_dir.join("events.jsonl"), format!("{}\n", rec)).unwrap();

    // pre-check: NO_JOURNEY should make the rule not block.
    let payload = r#"{
        "tool_name":"Edit",
        "tool_input":{
            "file_path":"src/a.rs",
            "old_string":"",
            "new_string":"pub fn a() {}"
        }
    }"#;
    let (code, _stdout, stderr) = run_hook("pre-check", payload, dir.path(), true);
    assert_eq!(
        code, 0,
        "PHRONESIS_NO_JOURNEY must skip derivation; stderr: {stderr}"
    );

    // post-check: NO_JOURNEY should skip the journal append. The seeded
    // single record stays; no new record is added.
    let events = journey_dir.join("events.jsonl");
    let before = std::fs::read_to_string(&events).unwrap();
    let (code, _, _) = run_hook("post-check", payload, dir.path(), true);
    assert_eq!(code, 0);
    let after = std::fs::read_to_string(&events).unwrap();
    assert_eq!(
        before, after,
        "PHRONESIS_NO_JOURNEY must skip journaling: before={before:?}, after={after:?}"
    );
}

#[test]
fn corrupt_journey_json_is_fail_open() {
    // Malformed journey.json must not block: derive falls back to default
    // config (no taggers, no rule selectors validate against an empty set
    // — but the rule references none here, so derive is a no-op).
    let rules = r#"{"rules":[]}"#;
    let dir = setup_project(rules, Some("{not json"));
    write_session(dir.path(), "s-test");

    let payload = r#"{
        "tool_name":"Edit",
        "tool_input":{
            "file_path":"src/a.rs",
            "old_string":"",
            "new_string":"pub fn a() {}"
        }
    }"#;
    let (code, _stdout, stderr) = run_hook("pre-check", payload, dir.path(), false);
    assert_eq!(
        code, 0,
        "fail-open: corrupt journey.json must not block; stderr: {stderr}"
    );
}
