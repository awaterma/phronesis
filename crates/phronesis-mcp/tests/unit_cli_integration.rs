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
