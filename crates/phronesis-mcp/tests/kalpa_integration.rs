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

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

#[test]
fn kalpa_start_show_end_round_trip_and_events() {
    let d = tempfile::tempdir().unwrap();
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
    let d = tempfile::tempdir().unwrap();
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
    let d = tempfile::tempdir().unwrap();
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
    let d = tempfile::tempdir().unwrap();
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
