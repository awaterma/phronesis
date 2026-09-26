use std::path::Path;
use std::process::{Command, Output};

/// Run `phr-mcp` against `root` as the project root.
fn run_phr(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(args)
        .env("PHRONESIS_PROJECT_ROOT", root)
        .current_dir(root)
        .output()
        .expect("spawn phr-mcp")
}

/// A temp project with an empty rule set: `kalpa start` / `unit start` refuse
/// an ungoverned root, and these tests exercise the governed behavior.
fn governed_tempdir() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join(".phronesis")).unwrap();
    std::fs::write(d.path().join(".phronesis/rules.json"), r#"{"rules":[]}"#).unwrap();
    d
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

#[test]
fn kalpa_start_show_end_round_trip_and_events() {
    let d = governed_tempdir();
    let out = run_phr(d.path(), &["kalpa", "start", "lifecycle-events"]);
    assert!(out.status.success());
    let show = run_phr(d.path(), &["kalpa"]);
    assert!(stdout(&show).contains("kalpa: lifecycle-events"));
    let journal =
        std::fs::read_to_string(d.path().join(".phronesis/journey/events.jsonl")).unwrap();
    assert!(journal.contains(r#""kind":"kalpa_start""#));
    assert!(journal.contains(r#""kalpa":"lifecycle-events""#));
    let end = run_phr(d.path(), &["kalpa", "end"]);
    assert!(end.status.success());
    // The closing record is attributed to the kalpa it closes.
    let last: serde_json::Value = serde_json::from_str(
        std::fs::read_to_string(d.path().join(".phronesis/journey/events.jsonl"))
            .unwrap()
            .lines()
            .next_back()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(last["kind"], "kalpa_end");
    assert_eq!(last["kalpa"], "lifecycle-events");
    assert!(
        last["tags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t == "kalpa:lifecycle-events"),
        "{last}"
    );
    assert!(!d.path().join(".phronesis/journey/kalpa").exists());
}

#[test]
fn kalpa_rejects_bad_names_and_survives_session_reset() {
    let d = governed_tempdir();
    assert!(
        !run_phr(d.path(), &["kalpa", "start", "Bad Name"])
            .status
            .success()
    );
    assert!(
        run_phr(d.path(), &["kalpa", "start", "ok-1"])
            .status
            .success()
    );
    phronesis_mcp::lifecycle::state::reset_for_session_start(d.path());
    assert_eq!(
        phronesis_mcp::lifecycle::state::read_kalpa(d.path())
            .unwrap()
            .name,
        "ok-1"
    );
}

#[test]
fn kalpa_show_with_none_open_fails_and_starting_a_second_ends_the_first() {
    let d = governed_tempdir();
    let show = run_phr(d.path(), &["kalpa"]);
    assert!(!show.status.success());
    assert!(String::from_utf8_lossy(&show.stderr).contains("no kalpa open"));

    assert!(run_phr(d.path(), &["kalpa", "start", "a"]).status.success());
    let second = run_phr(d.path(), &["kalpa", "start", "b"]);
    assert!(second.status.success());
    let text = stdout(&second);
    assert!(text.contains("ended kalpa a"), "{text}");
    assert!(text.contains("started kalpa b"), "{text}");
    assert_eq!(
        phronesis_mcp::lifecycle::state::read_kalpa(d.path())
            .unwrap()
            .name,
        "b"
    );
}

/// `header_line` takes `now` so the stale marker is testable without waiting a
/// month. This is the only caller that exercises the 30-day branch.
#[test]
fn header_line_marks_a_month_old_kalpa_stale() {
    use phronesis_mcp::lifecycle::kalpa_cli::header_line;
    use phronesis_mcp::lifecycle::state::{Kalpa, write_kalpa};
    let d = governed_tempdir();
    let now = 1_800_000_000u64;
    write_kalpa(
        d.path(),
        &Kalpa {
            name: "old-one".into(),
            started_ts: now - 31 * 24 * 3600,
        },
    );
    let line = header_line(d.path(), now).expect("a header");
    assert!(line.contains("kalpa: old-one"), "{line}");
    assert!(line.contains("(stale? run phr-mcp kalpa end)"), "{line}");

    write_kalpa(
        d.path(),
        &Kalpa {
            name: "new-one".into(),
            started_ts: now - 3600,
        },
    );
    assert!(!header_line(d.path(), now).unwrap().contains("stale"));
}

/// Fixture log written through the real projection, so the field names under
/// test are the ones `to_log_entry` actually writes.
fn seed_lifecycle_log(root: &std::path::Path, kalpa: &str) {
    use phronesis_mcp::action_log;
    use phronesis_mcp::lifecycle::{Host, Kind, LifecycleEvent, Mode, PromptText, Stamped};

    let path = action_log::default_path(root);
    let events: Vec<(u64, LifecycleEvent)> = vec![
        (
            1_700_000_000,
            LifecycleEvent::new(Kind::Prompt, Host::Claude)
                .with_mode(Mode::Fresh)
                .with_prompt("do the thing"),
        ),
        (
            1_700_000_100,
            LifecycleEvent::new(Kind::Interrupt, Host::Claude)
                .with_extra("inferred_from", "inflight"),
        ),
        (
            1_700_000_110,
            LifecycleEvent::new(Kind::Prompt, Host::Claude)
                .with_mode(Mode::Correction)
                .with_prompt("no, the other thing"),
        ),
        (
            1_700_000_150,
            LifecycleEvent::new(Kind::SubagentStart, Host::Claude)
                .with_agent("a1", Some("reviewer".into())),
        ),
        (
            1_700_000_160,
            LifecycleEvent::new(Kind::SubagentStart, Host::Claude)
                .with_agent("a2", Some("reviewer".into())),
        ),
        (
            1_700_000_200,
            LifecycleEvent::new(Kind::SubagentStop, Host::Claude)
                .with_agent("a1", Some("reviewer".into()))
                .with_extra("duration_secs", 220u64)
                .with_extra("matched_start", true),
        ),
        (
            1_700_000_300,
            LifecycleEvent::new(Kind::SubagentStop, Host::Claude)
                .with_agent("a2", Some("reviewer".into()))
                .with_extra("duration_secs", 20u64)
                .with_extra("matched_start", true),
        ),
        (
            1_700_000_400,
            LifecycleEvent::new(Kind::Commit, Host::Claude)
                .with_extra("sha", "0f3c")
                .with_extra("confidence_band", "high"),
        ),
    ];
    for (i, (ts, ev)) in events.iter().enumerate() {
        let stamped = Stamped {
            ts: *ts,
            sid: "s-1".to_string(),
            seq: i as u64 + 1,
            kalpa: Some(kalpa.to_string()),
            subject: None,
        };
        action_log::append(&path, &ev.to_log_entry(&stamped, PromptText::Full)).unwrap();
    }
}

#[test]
fn stats_kalpa_prints_lifecycle_section_with_retention_boundary() {
    let d = governed_tempdir();
    seed_lifecycle_log(d.path(), "lifecycle-events");
    let out = run_phr(d.path(), &["stats", "--kalpa", "lifecycle-events"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("counts since log entry 20"), "{stdout}");
    assert!(stdout.contains("correction 1"), "{stdout}");
    assert!(stdout.contains("median 2m00s"), "{stdout}");
    assert!(stdout.contains("confidence at commit: high 1"), "{stdout}");
    assert!(
        !stdout.contains("do the thing"),
        "prompt text must never reach stats: {stdout}"
    );
}

/// The seeded log holds lifecycle entries and no rule firings, which is the
/// common shape for a session that never tripped a rule. Printing "no
/// phronesis activity recorded yet" above a populated lifecycle block is a
/// contradiction: the emptiness check must read both sections.
#[test]
fn stats_does_not_claim_no_activity_when_lifecycle_is_populated() {
    let d = governed_tempdir();
    seed_lifecycle_log(d.path(), "lifecycle-events");
    let out = run_phr(d.path(), &["stats", "--kalpa", "lifecycle-events"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("no phronesis activity recorded yet"),
        "lifecycle entries are activity: {stdout}"
    );
    assert!(stdout.contains("no rules have fired yet"), "{stdout}");
    assert!(stdout.contains("commits"), "{stdout}");
}

/// The other half: with nothing at all recorded, the original line stands.
#[test]
fn stats_still_reports_an_empty_log_as_no_activity() {
    let d = governed_tempdir();
    std::fs::create_dir_all(d.path().join(".phronesis")).unwrap();
    std::fs::write(d.path().join(".phronesis/log.jsonl"), "").unwrap();
    let out = run_phr(d.path(), &["stats"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("no phronesis activity recorded yet"),
        "{stdout}"
    );
}

#[test]
fn stats_kalpa_filter_excludes_other_kalpas() {
    let d = governed_tempdir();
    seed_lifecycle_log(d.path(), "lifecycle-events");
    let out = run_phr(d.path(), &["stats", "--kalpa", "other-theme", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["lifecycle"]["commits"], 0);
    assert_eq!(v["lifecycle"]["sessions"], 0);
    assert_eq!(
        v["lifecycle"]["oldest_entry_ts"], 1_700_000_000u64,
        "boundary ignores the filter"
    );
}

#[test]
fn kalpa_show_reports_counts_for_the_named_kalpa() {
    let d = governed_tempdir();
    assert!(
        run_phr(d.path(), &["kalpa", "start", "lifecycle-events"])
            .status
            .success()
    );
    seed_lifecycle_log(d.path(), "lifecycle-events");

    let out = run_phr(d.path(), &["kalpa", "show", "lifecycle-events"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("kalpa: lifecycle-events"), "{stdout}");
    assert!(stdout.contains("started "), "{stdout}");
    assert!(stdout.contains("counts since log entry 20"), "{stdout}");
    assert!(stdout.contains("sessions        1"), "{stdout}");
    assert!(stdout.contains("prompts         2"), "{stdout}");
    assert!(stdout.contains("interrupts      1"), "{stdout}");
    assert!(
        stdout.contains("sub-agents      2   starts, 2 matched   median 2m00s"),
        "{stdout}"
    );
    assert!(stdout.contains("commits         1   (shell tool calls only)   confidence at commit: high 1  medium 0  low 0"), "{stdout}");
    assert!(
        !stdout.contains("do the thing"),
        "prompt text must never reach kalpa show: {stdout}"
    );
}

#[test]
fn kalpa_show_of_a_closed_kalpa_still_counts_its_entries() {
    let d = governed_tempdir();
    seed_lifecycle_log(d.path(), "old-theme");
    let out = run_phr(d.path(), &["kalpa", "show", "old-theme"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("kalpa: old-theme (closed)"), "{stdout}");
    assert!(stdout.contains("commits         1"), "{stdout}");
    // `seed_lifecycle_log` writes no `kalpa_start` entry, which is exactly the
    // case where the boundary has rotated off.
    assert!(stdout.contains("start not retained"), "{stdout}");
}

/// The other half: when the `kalpa_start` entry is still in the log, the header
/// prints the date it holds rather than the disclaimer.
#[test]
fn kalpa_show_prints_the_start_date_when_the_boundary_is_still_retained() {
    let d = governed_tempdir();
    seed_lifecycle_log(d.path(), "old-theme");
    {
        use phronesis_mcp::action_log;
        use phronesis_mcp::lifecycle::{Host, Kind, LifecycleEvent, PromptText, Stamped};
        let stamped = Stamped {
            ts: 1_699_999_000,
            sid: "s-1".to_string(),
            seq: 0,
            kalpa: Some("old-theme".to_string()),
            subject: None,
        };
        action_log::append(
            &action_log::default_path(d.path()),
            &LifecycleEvent::new(Kind::KalpaStart, Host::Cli)
                .to_log_entry(&stamped, PromptText::Full),
        )
        .unwrap();
    }
    let out = run_phr(d.path(), &["kalpa", "show", "old-theme"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("started 20"), "{stdout}");
    assert!(!stdout.contains("start not retained"), "{stdout}");
}

/// Seed one governed work item and one ungoverned one into `kalpa`.
fn seed_work_items(root: &std::path::Path, kalpa: &str) {
    use phronesis_mcp::action_log::{self, LogEntry};
    use phronesis_mcp::lifecycle::{Host, Kind, LifecycleEvent, Mode, PromptText, Stamped};

    let path = action_log::default_path(root);
    let mut seq = 100u64;
    let mut push = |ts: u64, subject: &str, ev: LifecycleEvent| {
        seq += 1;
        let stamped = Stamped {
            ts,
            sid: "s-1".to_string(),
            seq,
            kalpa: Some(kalpa.to_string()),
            subject: Some(subject.to_string()),
        };
        action_log::append(&path, &ev.to_log_entry(&stamped, PromptText::Full)).unwrap();
    };
    push(
        1_700_010_000,
        "w-1",
        LifecycleEvent::new(Kind::UnitStart, Host::Cli).with_extra("unit_id", "w-1"),
    );
    push(
        1_700_010_100,
        "w-1",
        LifecycleEvent::new(Kind::Prompt, Host::Claude)
            .with_mode(Mode::Correction)
            .with_prompt("no, the other one"),
    );
    push(
        1_700_010_200,
        "w-1",
        LifecycleEvent::new(Kind::Commit, Host::Claude)
            .with_extra("sha", "0f3c")
            .with_extra("confidence_band", "high"),
    );
    push(
        1_700_010_300,
        "w-2",
        LifecycleEvent::new(Kind::Commit, Host::Claude)
            .with_extra("sha", "aa11")
            .with_extra("confidence_band", "low"),
    );

    // `w-1`'s evaluation comes from the Codex host and `w-2`'s from Claude
    // Code: a work item is governed by the rules that ran against it, whatever
    // host ran them.
    for (ts, subject, event) in [
        (1_700_010_050u64, "w-1", "codex_hook"),
        (1_700_010_250, "w-2", "pre_check"),
    ] {
        let mut e = LogEntry::new("hook", event)
            .with("phase", "pre")
            .with("tool", "Edit")
            .with("exit", 0)
            .with("consequences", serde_json::json!([]))
            .with("subject", subject);
        e.ts = ts;
        action_log::append(&path, &e).unwrap();
    }
}

#[test]
fn kalpa_show_reports_work_items_and_governed_throughput() {
    let d = governed_tempdir();
    seed_work_items(d.path(), "lifecycle-events");
    let out = run_phr(d.path(), &["kalpa", "show", "lifecycle-events"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("work items      2   explicit 1   implicit 1"),
        "{stdout}"
    );
    assert!(
        stdout.contains("governed        1   (commit + rules evaluated + band ≥ medium)"),
        "{stdout}"
    );
    assert!(
        stdout.contains("interventions / work item   0.50"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("no, the other one"),
        "prompt text never reaches kalpa show: {stdout}"
    );
}

#[test]
fn stats_kalpa_reports_governed_throughput_as_json() {
    let d = governed_tempdir();
    seed_work_items(d.path(), "lifecycle-events");
    let out = run_phr(
        d.path(),
        &["stats", "--kalpa", "lifecycle-events", "--json"],
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["lifecycle"]["work_items"]["explicit"], 1);
    assert_eq!(v["lifecycle"]["work_items"]["implicit"], 1);
    assert_eq!(v["lifecycle"]["work_items"]["completed"], 2);
    assert_eq!(v["lifecycle"]["governed"], 1);
}
