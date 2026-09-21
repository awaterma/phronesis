//! `phr-mcp unit` CLI — explicit work items with a spec pointer.
//! See `docs/specs/SPEC-agent-lifecycle-events.md` §"Work items and governed
//! throughput".

use std::path::Path;
use std::process::{Command, Output};

/// Run `phr-mcp` against `root` as the project root. `run` honours
/// `PHRONESIS_PROJECT_ROOT` via `security::project_root`, and `current_dir`
/// pins the fallback too.
fn run_phr(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(args)
        .env("PHRONESIS_PROJECT_ROOT", root)
        .current_dir(root)
        .output()
        .expect("spawn phr-mcp")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

/// Every lifecycle entry in the action log, oldest first.
fn lifecycle_entries(root: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(root.join(".phronesis/log.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["kind"] == "lifecycle")
        .collect()
}

fn write_spec(root: &Path, rel: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, "# spec\n").unwrap();
}

#[test]
fn unit_start_sets_the_subject_and_records_unit_start_with_the_spec() {
    let d = tempfile::tempdir().unwrap();
    write_spec(d.path(), "docs/specs/SPEC-thing.md");
    let out = run_phr(
        d.path(),
        &[
            "unit",
            "start",
            "item-1",
            "--spec",
            "docs/specs/SPEC-thing.md",
        ],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("started work unit item-1"),
        "{}",
        stdout(&out)
    );

    assert_eq!(
        std::fs::read_to_string(d.path().join(".phronesis/outcomes/current")).unwrap(),
        "item-1"
    );
    let entries = lifecycle_entries(d.path());
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0]["event"], "unit_start");
    assert_eq!(entries[0]["unit_id"], "item-1");
    assert_eq!(entries[0]["subject"], "item-1");
    assert_eq!(entries[0]["spec"], "docs/specs/SPEC-thing.md");
    assert_eq!(entries[0]["host"], "cli");

    let journal =
        std::fs::read_to_string(d.path().join(".phronesis/journey/events.jsonl")).unwrap();
    assert!(journal.contains(r#""kind":"unit_start""#), "{journal}");
    assert!(journal.contains(r#""subject":"item-1""#), "{journal}");

    // `spec` is a join key, so a detoured spelling of the same file records in
    // its normalized form — two spellings must not read as two work streams.
    let out = run_phr(
        d.path(),
        &[
            "unit",
            "start",
            "item-2",
            "--spec",
            "docs/../docs/specs/SPEC-thing.md",
        ],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let started = lifecycle_entries(d.path())
        .into_iter()
        .find(|v| v["event"] == "unit_start" && v["unit_id"] == "item-2")
        .expect("item-2 unit_start");
    assert_eq!(started["spec"], "docs/specs/SPEC-thing.md");
}

#[test]
fn unit_start_without_an_id_mints_one() {
    let d = tempfile::tempdir().unwrap();
    let out = run_phr(d.path(), &["unit", "start"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let id = std::fs::read_to_string(d.path().join(".phronesis/outcomes/current")).unwrap();
    assert!(id.starts_with("unit-"), "{id}");
    assert!(
        stdout(&out).contains(&id),
        "the minted id is reported: {}",
        stdout(&out)
    );
}

/// A spec pointer that does not exist, escapes the root, or is absolute is
/// rejected *before* the subject moves: a unit must never point at a spec the
/// reader cannot open.
#[test]
fn unit_start_rejects_a_bad_spec_and_leaves_the_subject_untouched() {
    let d = tempfile::tempdir().unwrap();
    for bad in ["docs/specs/NOPE.md", "../outside.md", "/etc/passwd"] {
        let out = run_phr(d.path(), &["unit", "start", "x", "--spec", bad]);
        assert!(!out.status.success(), "{bad} should be rejected");
        assert!(stderr(&out).contains("--spec"), "{bad}: {}", stderr(&out));
    }
    assert!(
        !d.path().join(".phronesis/outcomes/current").exists(),
        "a rejected start opens no unit"
    );
    assert!(
        lifecycle_entries(d.path()).is_empty(),
        "and records nothing"
    );
}

/// "Starting a unit while one is open ends the open one first" (spec
/// §"Work items", 1). The closing record must carry the *old* subject.
#[test]
fn unit_start_ends_an_open_unit_first() {
    let d = tempfile::tempdir().unwrap();
    assert!(
        run_phr(d.path(), &["unit", "start", "first"])
            .status
            .success()
    );
    let out = run_phr(d.path(), &["unit", "start", "second"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("ended work unit first"), "{text}");
    assert!(text.contains("started work unit second"), "{text}");

    let entries = lifecycle_entries(d.path());
    let events: Vec<&str> = entries
        .iter()
        .map(|e| e["event"].as_str().unwrap())
        .collect();
    assert_eq!(
        events,
        ["unit_start", "unit_end", "unit_start"],
        "{entries:?}"
    );
    assert_eq!(
        entries[1]["subject"], "first",
        "the end record carries the old subject"
    );
    assert_eq!(entries[1]["unit_id"], "first");
    assert!(
        entries[1].get("implicit").is_none(),
        "`first` was started explicitly, so it is not implicit: {:?}",
        entries[1]
    );
    assert_eq!(entries[2]["subject"], "second");
}

#[test]
fn unit_end_records_and_clears_the_subject() {
    let d = tempfile::tempdir().unwrap();
    assert!(
        run_phr(d.path(), &["unit", "start", "item-1"])
            .status
            .success()
    );
    let out = run_phr(d.path(), &["unit", "end"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("ended work unit item-1"),
        "{}",
        stdout(&out)
    );
    assert!(
        !d.path().join(".phronesis/outcomes/current").exists(),
        "the subject is cleared"
    );
    let entries = lifecycle_entries(d.path());
    assert_eq!(entries.last().unwrap()["event"], "unit_end");
    assert_eq!(entries.last().unwrap()["subject"], "item-1");
}

/// An implicit unit — one minted by the outcomes path, never `unit start`ed —
/// is closed with `implicit: true` so the report can split the two populations.
#[test]
fn unit_end_marks_a_never_started_unit_implicit() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join(".phronesis/outcomes")).unwrap();
    std::fs::write(d.path().join(".phronesis/outcomes/current"), "unit-999").unwrap();
    let out = run_phr(d.path(), &["unit", "end"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let entries = lifecycle_entries(d.path());
    assert_eq!(entries.last().unwrap()["event"], "unit_end");
    assert_eq!(entries.last().unwrap()["implicit"], true);
}

#[test]
fn unit_end_with_nothing_open_fails() {
    let d = tempfile::tempdir().unwrap();
    let out = run_phr(d.path(), &["unit", "end"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("no work unit open"),
        "{}",
        stderr(&out)
    );
}

/// Seed `.phronesis/bugs.json`. `entries` is written verbatim so a test can
/// exercise an entry with and without the optional `spec`.
fn write_bugs(root: &Path, entries: serde_json::Value) {
    let phr = root.join(".phronesis");
    std::fs::create_dir_all(&phr).unwrap();
    std::fs::write(phr.join("bugs.json"), entries.to_string()).unwrap();
}

/// Spec §"Where the name comes from": `--bug` names the unit `bug-<bug_id>`
/// and records the registry's cargo test name, which is what later joins the
/// unit to its red→green signal.
#[test]
fn unit_start_from_a_known_bug_names_the_unit_and_records_its_test() {
    let d = tempfile::tempdir().unwrap();
    write_bugs(
        d.path(),
        serde_json::json!([{"bug_id": "1042", "test": "auth::rejects_expired", "status": "open"}]),
    );
    let out = run_phr(d.path(), &["unit", "start", "--bug", "1042"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("started work unit bug-1042"),
        "{}",
        stdout(&out)
    );
    assert_eq!(
        std::fs::read_to_string(d.path().join(".phronesis/outcomes/current")).unwrap(),
        "bug-1042"
    );
    let entries = lifecycle_entries(d.path());
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0]["event"], "unit_start");
    assert_eq!(entries[0]["unit_id"], "bug-1042");
    assert_eq!(entries[0]["subject"], "bug-1042");
    assert_eq!(entries[0]["bug_id"], "1042");
    assert_eq!(entries[0]["test"], "auth::rejects_expired");
    assert!(
        entries[0].get("spec").is_none(),
        "no spec in the entry, none recorded"
    );
}

/// An entry carrying a `spec` supplies the pointer without a flag — the
/// registry answers "which spec is this bug against?" once, for everyone.
#[test]
fn unit_start_from_a_known_bug_takes_the_registry_spec() {
    let d = tempfile::tempdir().unwrap();
    write_spec(d.path(), "docs/specs/SPEC-auth.md");
    write_bugs(
        d.path(),
        serde_json::json!([{
            "bug_id": "1042", "test": "auth::rejects_expired", "status": "open",
            "spec": "docs/specs/SPEC-auth.md", "title": "expired tokens accepted"
        }]),
    );
    let out = run_phr(d.path(), &["unit", "start", "--bug", "1042"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let entries = lifecycle_entries(d.path());
    assert_eq!(entries[0]["spec"], "docs/specs/SPEC-auth.md");
    assert!(
        stdout(&out).contains("docs/specs/SPEC-auth.md"),
        "{}",
        stdout(&out)
    );
}

/// "An unknown id is an error, not a fresh unit: the registry is the source of
/// truth for bug-shaped work" (spec §"Where the name comes from").
#[test]
fn unit_start_from_an_unknown_bug_errors_and_opens_nothing() {
    let d = tempfile::tempdir().unwrap();
    write_bugs(
        d.path(),
        serde_json::json!([{"bug_id": "1042", "test": "auth::rejects_expired", "status": "open"}]),
    );
    let out = run_phr(d.path(), &["unit", "start", "--bug", "9999"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("unknown bug id `9999` (not in .phronesis/bugs.json)"),
        "{}",
        stderr(&out)
    );
    assert!(
        !d.path().join(".phronesis/outcomes/current").exists(),
        "a rejected start opens no unit"
    );
    assert!(
        lifecycle_entries(d.path()).is_empty(),
        "and records nothing"
    );
}

/// A missing registry is the same error, not a silent fresh unit: `bugs::load`
/// is fail-open (empty on a missing file), so the lookup has to be the gate.
#[test]
fn unit_start_with_bug_and_no_registry_errors() {
    let d = tempfile::tempdir().unwrap();
    let out = run_phr(d.path(), &["unit", "start", "--bug", "1042"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("unknown bug id `1042`"),
        "{}",
        stderr(&out)
    );
}

/// The registry is the name; a second name is a contradiction, and clap
/// rejects it before anything runs.
#[test]
fn unit_start_rejects_bug_together_with_a_positional_id() {
    let d = tempfile::tempdir().unwrap();
    let out = run_phr(d.path(), &["unit", "start", "item-1", "--bug", "1042"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("cannot be used with"),
        "clap's conflict message: {}",
        stderr(&out)
    );
    assert!(!d.path().join(".phronesis/outcomes/current").exists());
}

// --- submit_suggestion MCP tool (Task 2c) ---

use phronesis_mcp::server::EpistemeMcp;
use phronesis_mcp::server_params::SubmitSuggestionParams;

fn suggestion(subject: &str, spec: Option<&str>, bug_id: Option<&str>) -> SubmitSuggestionParams {
    SubmitSuggestionParams {
        subject: subject.to_string(),
        summary: Some("a suggestion".to_string()),
        spec: spec.map(str::to_string),
        bug_id: bug_id.map(str::to_string),
    }
}

/// The plain path an agent already uses — declaring a subject — now also
/// leaves the `unit_start` record the CLI leaves, so a work item named in the
/// LLM window is indistinguishable from one named at a shell.
#[test]
fn submit_suggestion_records_unit_start_for_a_plain_subject() {
    let d = tempfile::tempdir().unwrap();
    write_spec(d.path(), "docs/specs/SPEC-thing.md");
    let out = EpistemeMcp::submit_suggestion_report(
        d.path(),
        &suggestion("xlate-7", Some("docs/specs/SPEC-thing.md"), None),
    )
    .expect("report");
    assert_eq!(out["subject"], "xlate-7");
    assert_eq!(out["unit_id"], "xlate-7");
    assert_eq!(out["spec"], "docs/specs/SPEC-thing.md");
    assert!(
        out.get("band").is_some(),
        "the existing confidence fields survive: {out}"
    );

    assert_eq!(
        std::fs::read_to_string(d.path().join(".phronesis/outcomes/current")).unwrap(),
        "xlate-7"
    );
    let entries = lifecycle_entries(d.path());
    assert_eq!(
        entries.len(),
        1,
        "exactly one unit_start, not two: {entries:?}"
    );
    assert_eq!(entries[0]["event"], "unit_start");
    assert_eq!(entries[0]["unit_id"], "xlate-7");
    assert_eq!(entries[0]["subject"], "xlate-7");
    assert_eq!(entries[0]["spec"], "docs/specs/SPEC-thing.md");
}

/// `bug_id` overrides the caller's subject with the registry name, and the
/// response says so — the agent must report the id the reports will use.
#[test]
fn submit_suggestion_with_a_bug_id_names_the_unit_and_records_the_test() {
    let d = tempfile::tempdir().unwrap();
    write_bugs(
        d.path(),
        serde_json::json!([{"bug_id": "1042", "test": "auth::rejects_expired", "status": "open"}]),
    );
    let out = EpistemeMcp::submit_suggestion_report(
        d.path(),
        &suggestion("my-guess", None, Some("1042")),
    )
    .expect("report");
    assert_eq!(out["unit_id"], "bug-1042");
    assert_eq!(
        out["subject"], "bug-1042",
        "the response reports the real name"
    );
    assert_eq!(
        std::fs::read_to_string(d.path().join(".phronesis/outcomes/current")).unwrap(),
        "bug-1042"
    );
    let entries = lifecycle_entries(d.path());
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0]["unit_id"], "bug-1042");
    assert_eq!(entries[0]["test"], "auth::rejects_expired");
    assert_eq!(entries[0]["bug_id"], "1042");
}

/// An unknown id fails the call and leaves nothing behind — the same rule the
/// CLI enforces, reached through the same function.
#[test]
fn submit_suggestion_with_an_unknown_bug_id_errors_and_sets_no_subject() {
    let d = tempfile::tempdir().unwrap();
    let err = EpistemeMcp::submit_suggestion_report(
        d.path(),
        &suggestion("my-guess", None, Some("9999")),
    )
    .expect_err("unknown bug id must fail");
    assert!(
        err.to_string()
            .contains("unknown bug id `9999` (not in .phronesis/bugs.json)"),
        "{err}"
    );
    assert!(
        !d.path().join(".phronesis/outcomes/current").exists(),
        "no subject is set on a rejected call"
    );
    assert!(lifecycle_entries(d.path()).is_empty());
}

/// A bad `--spec`-equivalent is rejected here too: the parameter goes through
/// the same `validate_spec`, so the MCP surface cannot record a pointer the
/// CLI would refuse.
#[test]
fn submit_suggestion_rejects_a_spec_that_does_not_exist() {
    let d = tempfile::tempdir().unwrap();
    let err = EpistemeMcp::submit_suggestion_report(
        d.path(),
        &suggestion("xlate-7", Some("docs/specs/NOPE.md"), None),
    )
    .expect_err("missing spec must fail");
    assert!(err.to_string().contains("--spec"), "{err}");
    assert!(!d.path().join(".phronesis/outcomes/current").exists());
}

/// Seed one work item's worth of evidence: lifecycle entries written through
/// the real projection (so the field names under test are the ones
/// `to_log_entry` writes), plus hand-built `pre_check` entries in the shape
/// `log_hook_event` writes.
fn seed_unit(root: &Path, unit_id: &str) {
    use phronesis_mcp::action_log::{self, LogEntry};
    use phronesis_mcp::lifecycle::{Host, Kind, LifecycleEvent, Mode, PromptText, Stamped};

    let path = action_log::default_path(root);
    let stamp = |ts: u64, seq: u64| Stamped {
        ts,
        sid: "s-1".to_string(),
        seq,
        kalpa: Some("lifecycle-events".to_string()),
        subject: Some(unit_id.to_string()),
    };
    let events: Vec<(u64, LifecycleEvent)> = vec![
        (
            1_700_000_000,
            LifecycleEvent::new(Kind::UnitStart, Host::Cli)
                .with_extra("unit_id", unit_id)
                .with_extra("spec", "docs/specs/SPEC-thing.md"),
        ),
        (
            1_700_000_600,
            LifecycleEvent::new(Kind::Prompt, Host::Claude)
                .with_mode(Mode::MidTurn)
                .with_prompt("also update the changelog"),
        ),
        (
            1_700_000_900,
            LifecycleEvent::new(Kind::Prompt, Host::Claude)
                .with_mode(Mode::Correction)
                .with_prompt("no, keep the journal free of text"),
        ),
        (
            1_700_001_200,
            LifecycleEvent::new(Kind::Commit, Host::Claude)
                .with_extra("sha", "0f3c9a1e")
                .with_extra("confidence_band", "high"),
        ),
    ];
    for (i, (ts, ev)) in events.iter().enumerate() {
        action_log::append(
            &path,
            &ev.to_log_entry(&stamp(*ts, i as u64 + 1), PromptText::Full),
        )
        .unwrap();
    }

    // Rule evaluations, in `log_hook_event`'s shape.
    let hook = |ts: u64, exit: i32, cons: serde_json::Value| {
        let mut e = LogEntry::new("hook", "pre_check")
            .with("phase", "pre")
            .with("tool", "Edit")
            .with("file", "src/lib.rs")
            .with("exit", exit)
            .with("consequences", cons)
            .with("subject", unit_id);
        e.ts = ts;
        e
    };
    let cons = |id: &str, action: &str| serde_json::json!([{ "rule_id": id, "action_type": action, "message": "m", "bindings": {} }]);
    for (ts, exit, cons) in [
        (1_700_000_100u64, 0, serde_json::json!([])),
        (1_700_000_200, 1, cons("warn-piped-verification", "warning")),
        (1_700_000_300, 1, cons("warn-piped-verification", "warning")),
        (
            1_700_000_400,
            2,
            cons("block-await-on-sync", "constraint_violation"),
        ),
    ] {
        action_log::append(&path, &hook(ts, exit, cons)).unwrap();
    }
    // A rule evaluation for a *different* unit, which must not be counted.
    let mut other = hook(1_700_000_500, 1, cons("warn-piped-verification", "warning"));
    other
        .data
        .insert("subject".to_string(), serde_json::json!("other-unit"));
    action_log::append(&path, &other).unwrap();

    // Grounded outcome signals live in the journey journal, keyed by subject.
    let journey = root.join(".phronesis/journey");
    std::fs::create_dir_all(&journey).unwrap();
    let mut body = String::new();
    for (i, tag) in ["outcome:compile_ok", "outcome:test_pass"]
        .iter()
        .enumerate()
    {
        body.push_str(
            &serde_json::json!({
                "v": 1, "ts": 1_700_000_050u64 + i as u64, "sid": "s-1", "seq": 900 + i as u64,
                "tool": "Bash", "path": "<cmd>", "tags": [tag], "subject": unit_id,
            })
            .to_string(),
        );
        body.push('\n');
    }
    std::fs::write(journey.join("events.jsonl"), body).unwrap();
    std::fs::create_dir_all(root.join(".phronesis")).unwrap();
    std::fs::write(root.join(".phronesis/confidence.json"), "{}").unwrap();
}

#[test]
fn unit_show_renders_the_spec_block() {
    let d = tempfile::tempdir().unwrap();
    seed_unit(d.path(), "unit-1789095489589855000");
    let out = run_phr(d.path(), &["unit", "show", "unit-1789095489589855000"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);

    assert!(
        text.contains("unit: unit-1789095489589855000   explicit   spec: docs/specs/SPEC-thing.md"),
        "{text}"
    );
    assert!(text.contains("kalpa: lifecycle-events"), "{text}");
    assert!(text.contains("window: "), "{text}");
    assert!(
        text.contains("rules evaluated   4   fired 3   blocked 1   warned 2"),
        "{text}"
    );
    assert!(text.contains("block-await-on-sync  1"), "{text}");
    assert!(text.contains("warn-piped-verification  2"), "{text}");
    assert!(
        text.contains("evidence         compile pass   tests pass   band: medium"),
        "{text}"
    );
    assert!(
        text.contains("interventions     2   mid_turn 1   correction 1"),
        "{text}"
    );
    assert!(
        text.contains("correction  \"no, keep the journal free of text\""),
        "{text}"
    );
    assert!(
        text.contains("commits           1   0f3c9a1e  band high"),
        "{text}"
    );
    assert!(
        !text.contains("other-unit"),
        "another unit's rule evaluations are not this unit's: {text}"
    );
}

/// A Codex session's evaluations are this unit's evaluations: `codex_hook` is
/// the Codex host's single pre/post entry and carries the same `exit` and
/// `consequences` shape, so it counts exactly like `pre_check`.
#[test]
fn unit_show_counts_codex_hook_evaluations() {
    use phronesis_mcp::action_log::{self, LogEntry};
    let d = tempfile::tempdir().unwrap();
    seed_unit(d.path(), "unit-1");
    let mut e = LogEntry::new("hook", "codex_hook")
        .with("phase", "pre")
        .with("tool", "Bash")
        .with("host", "codex")
        .with("exit", 2)
        .with(
            "consequences",
            serde_json::json!([{ "rule_id": "block-unwrap", "action_type": "constraint_violation",
                                 "message": "m", "bindings": {} }]),
        )
        .with("subject", "unit-1");
    e.ts = 1_700_000_450;
    action_log::append(&action_log::default_path(d.path()), &e).unwrap();

    let out = run_phr(d.path(), &["unit", "show", "unit-1", "--json"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        v["rules_evaluated"], 5,
        "four Claude entries plus one Codex"
    );
    assert_eq!(v["fired"], 4);
    assert_eq!(v["blocked"], 2);
    assert_eq!(v["per_rule"]["block-unwrap"], 1);
}

#[test]
fn unit_show_json_emits_one_object() {
    let d = tempfile::tempdir().unwrap();
    seed_unit(d.path(), "unit-1");
    let out = run_phr(d.path(), &["unit", "show", "unit-1", "--json"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["unit_id"], "unit-1");
    assert_eq!(v["explicit"], true);
    assert_eq!(v["spec"], "docs/specs/SPEC-thing.md");
    assert_eq!(v["kalpa"], "lifecycle-events");
    assert_eq!(v["rules_evaluated"], 4);
    assert_eq!(v["fired"], 3);
    assert_eq!(v["blocked"], 1);
    assert_eq!(v["warned"], 2);
    assert_eq!(v["per_rule"]["warn-piped-verification"], 2);
    assert_eq!(v["band"], "medium");
    assert_eq!(v["signals"][0], "compile");
    assert_eq!(v["interventions"].as_array().unwrap().len(), 2);
    assert_eq!(v["commits"][0]["sha"], "0f3c9a1e");
    assert_eq!(v["commits"][0]["band"], "high");
    assert_eq!(v["first_ts"], 1_700_000_000u64);
    assert_eq!(v["last_ts"], 1_700_001_200u64);
}

/// A unit nobody ran `unit start` for is reported as implicit, with no spec.
/// The split is the point: implicit units split on every build/test cycle and
/// would otherwise flatter every per-item number (spec §"Work items / Limits").
#[test]
fn unit_show_reports_an_implicit_unit_as_implicit() {
    let d = tempfile::tempdir().unwrap();
    use phronesis_mcp::action_log;
    use phronesis_mcp::lifecycle::{Host, Kind, LifecycleEvent, PromptText, Stamped};
    let stamped = Stamped {
        ts: 1_700_000_000,
        sid: "s-1".into(),
        seq: 1,
        kalpa: None,
        subject: Some("unit-implicit".into()),
    };
    action_log::append(
        &action_log::default_path(d.path()),
        &LifecycleEvent::new(Kind::Commit, Host::Claude)
            .with_extra("sha", "abc0123")
            .to_log_entry(&stamped, PromptText::Full),
    )
    .unwrap();

    let out = run_phr(d.path(), &["unit", "show", "unit-implicit"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("unit: unit-implicit   implicit"), "{text}");
    assert!(!text.contains("spec:"), "{text}");
    assert!(text.contains("commits           1   abc0123"), "{text}");
    assert!(
        !text.contains("band "),
        "no band segment without one: {text}"
    );
    assert!(
        !text.contains("evidence"),
        "no evidence line without confidence scoring: {text}"
    );
}

/// `unit show` with no id reports the open unit; with none open it fails.
#[test]
fn unit_show_defaults_to_the_open_unit() {
    let d = tempfile::tempdir().unwrap();
    let out = run_phr(d.path(), &["unit", "show"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("no work unit open"),
        "{}",
        stderr(&out)
    );

    assert!(
        run_phr(d.path(), &["unit", "start", "item-1"])
            .status
            .success()
    );
    let out = run_phr(d.path(), &["unit", "show"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("unit: item-1   explicit"),
        "{}",
        stdout(&out)
    );
}

/// Intervention text is prompt text, so it obeys the `prompt_text` switch at
/// read time — the same accessor `journey --corrections` goes through.
#[test]
fn unit_show_hides_intervention_text_under_prompt_text_none() {
    let d = tempfile::tempdir().unwrap();
    seed_unit(d.path(), "unit-1");
    std::fs::write(
        d.path().join(".phronesis/journey.json"),
        r#"{"version":1,"taggers":[],"modules":[],"lifecycle":{"prompt_text":"none"}}"#,
    )
    .unwrap();
    let text = stdout(&run_phr(d.path(), &["unit", "show", "unit-1"]));
    assert!(
        text.contains("interventions     2"),
        "the count survives: {text}"
    );
    assert!(!text.contains("keep the journal free of text"), "{text}");
    assert!(text.contains("correction  (text withheld)"), "{text}");
}
