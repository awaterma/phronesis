use phr_bench::manifest::{Caps, TaskSpec};
use phr_bench::record::{validate, Arm, RunExit, RunRecord};
use phr_bench::runner::{classify_exit, extract_diff, not_wired_record, run};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use tempfile::{tempdir, TempDir};

/// Runner tests point `PHR_BENCH_CLAUDE` at a shell-script fake and steer it
/// with `FAKE_CLAUDE_*` knobs. Process env is global, so every test that
/// spawns a fake claude holds this lock and clears its knobs on drop.
static ENV_LOCK: Mutex<()> = Mutex::new(());

const FAKE_KNOBS: &[&str] = &[
    "PHR_BENCH_CLAUDE",
    "FAKE_CLAUDE_TRANSCRIPT",
    "FAKE_CLAUDE_EXIT",
    "FAKE_CLAUDE_SLEEP",
    "FAKE_CLAUDE_STDERR",
    "FAKE_CLAUDE_TOUCH",
    "FAKE_CLAUDE_DUMP_ENV",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_MODEL",
    "ANTHROPIC_SMALL_FAST_MODEL",
];

/// Points the runner at the fake claude, sets the given env knobs, and removes
/// every knob on drop so parallel tests never inherit another test's fake.
struct FakeClaude {
    _guard: MutexGuard<'static, ()>,
}

impl FakeClaude {
    fn new(knobs: &[(&str, String)]) -> Self {
        let guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("PHR_BENCH_CLAUDE", fake_claude_path());
        for (key, value) in knobs {
            std::env::set_var(key, value);
        }
        Self { _guard: guard }
    }
}

impl Drop for FakeClaude {
    fn drop(&mut self) {
        for key in FAKE_KNOBS {
            std::env::remove_var(key);
        }
    }
}

fn fake_claude_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/testdata/fake-claude.sh")
}

fn spec() -> TaskSpec {
    TaskSpec {
        instance_id: "fixture".into(),
        language: "rust".into(),
        repo: "https://github.com/example/example.git".into(),
        base_commit: "HEAD".into(),
        issue_text: "Fix the bug.".into(),
        fail_to_pass: vec![],
        pass_to_pass: vec![],
        packs: vec!["llm".into(), "rust".into()],
    }
}

fn caps() -> Caps {
    Caps {
        max_turns: 100,
        max_wall_clock_secs: 60,
    }
}

/// Tiny local git repo to clone from (file:// URL), one commit, tracked.txt.
fn fixture_repo() -> (TempDir, String) {
    let dir = tempdir().unwrap();
    let repo = dir.path().join("origin");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join("tracked.txt"), "base\n").unwrap();
    git(&["git", "init", "-q"], &repo);
    git(&["git", "add", "-A"], &repo);
    git(
        &[
            "git",
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-qm",
            "init",
        ],
        &repo,
    );
    let url = format!("file://{}", repo.display());
    (dir, url)
}

/// Clone the fixture origin — same pattern as tests/arms.rs.
fn clone_fixture(url: &str, workdir: &Path) -> PathBuf {
    let clone = workdir.join("clone");
    let target = clone.to_str().unwrap();
    git(&["git", "clone", "--quiet", url, target], workdir);
    clone
}

fn git(argv: &[&str], cwd: &Path) {
    let ok = std::process::Command::new(argv[0])
        .args(&argv[1..])
        .current_dir(cwd)
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok, "command failed: {argv:?}");
}

fn head(repo: &Path) -> String {
    let out = std::process::Command::new("git")
        .args(["-C", repo.to_str().unwrap(), "rev-parse", "HEAD"])
        .output()
        .unwrap();
    assert!(out.status.success(), "rev-parse HEAD failed");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A plausible post-run `.phronesis/log.jsonl`: one rule block, one warn.
fn write_governance_log(clone: &Path) {
    let log = concat!(
        r#"{"ts":1,"kind":"hook","event":"pre_check","exit":2,"blocked_by":[{"kind":"rule","rule":"no-unwrap-in-src"}]}"#,
        "\n",
        r#"{"ts":2,"kind":"hook","event":"post_check","exit":1,"consequences":[{"rule_id":"audit-file-loc-high"}]}"#,
        "\n",
    );
    std::fs::create_dir_all(clone.join(".phronesis")).unwrap();
    std::fs::write(clone.join(".phronesis/log.jsonl"), log).unwrap();
}

fn read_record(run_dir: &Path) -> RunRecord {
    let src = std::fs::read_to_string(run_dir.join("record.json")).unwrap();
    serde_json::from_str(&src).unwrap()
}

#[test]
fn classify_exit_orders_time_over_turns_over_error() {
    assert!(matches!(
        classify_exit(true, false, false, "ok"),
        RunExit::CapTime
    ));
    assert!(matches!(
        classify_exit(false, true, false, "ok"),
        RunExit::CapTurns
    ));
    match classify_exit(false, false, false, "router exploded") {
        RunExit::Error { reason } => assert_eq!(reason, "router exploded"),
        other => panic!("expected Error, got {other:?}"),
    }
    assert!(matches!(
        classify_exit(false, false, true, ""),
        RunExit::Completed
    ));
}

#[test]
fn classify_exit_truncates_long_error_reasons() {
    let loud = "x".repeat(600);
    match classify_exit(false, false, false, &loud) {
        RunExit::Error { reason } => assert_eq!(reason.len(), 500),
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn treatment_record_requires_governance_or_fails_loud() {
    let rec = not_wired_record("i-1", Arm::Treatment).unwrap();
    assert!(matches!(rec.exit, RunExit::Error { .. }));
    assert!(rec.exit.to_string().contains("governance_not_wired"));
    assert!(not_wired_record("i-1", Arm::Control).is_err());
}

#[test]
fn diff_extraction_captures_untracked_files_and_agent_commits() {
    let (_guard, url) = fixture_repo();
    let work = tempdir().unwrap();
    let clone = clone_fixture(&url, work.path());
    let pre_head = head(&clone);

    // The agent commits some of its work...
    std::fs::write(clone.join("tracked.txt"), "changed by agent\n").unwrap();
    std::fs::write(clone.join("committed-new.txt"), "agent committed me\n").unwrap();
    git(&["git", "add", "-A"], &clone);
    git(
        &[
            "git",
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-qm",
            "agent work",
        ],
        &clone,
    );
    // ...and leaves an untracked file behind.
    std::fs::write(clone.join("untracked.txt"), "agent left me behind\n").unwrap();

    let diff = extract_diff(&clone, &pre_head).unwrap();
    assert!(diff.contains("tracked.txt"), "modified tracked file missing");
    assert!(
        diff.contains("committed-new.txt"),
        "agent commit content missing"
    );
    assert!(diff.contains("untracked.txt"), "untracked file missing");
}

#[test]
fn diff_extraction_excludes_governance_wiring_dirs() {
    let (_guard, url) = fixture_repo();
    let work = tempdir().unwrap();
    let clone = clone_fixture(&url, work.path());
    let pre_head = head(&clone);

    // phr-mcp init writes these BEFORE the run; they are bench wiring, not
    // agent output, so they must not pollute the patch.
    std::fs::create_dir_all(clone.join(".phronesis")).unwrap();
    std::fs::write(clone.join(".phronesis/rules.json"), "{}\n").unwrap();
    std::fs::create_dir_all(clone.join(".claude")).unwrap();
    std::fs::write(clone.join(".claude/settings.local.json"), "{}\n").unwrap();
    std::fs::write(clone.join("tracked.txt"), "agent change\n").unwrap();

    let diff = extract_diff(&clone, &pre_head).unwrap();
    assert!(diff.contains("tracked.txt"), "agent change missing");
    assert!(
        !diff.contains(".phronesis"),
        "governance wiring must not enter the patch"
    );
    assert!(
        !diff.contains(".claude"),
        "claude settings must not enter the patch"
    );
}

#[test]
fn control_run_completes_and_writes_all_artifacts() {
    let (_guard, url) = fixture_repo();
    let work = tempdir().unwrap();
    let clone = clone_fixture(&url, work.path());
    let run_dir = work.path().join("run");
    let env_dump = work.path().join("child-env.txt");
    let touch = clone.join("proof.txt");

    let _fake = FakeClaude::new(&[
        ("FAKE_CLAUDE_TOUCH", touch.to_str().unwrap().into()),
        ("FAKE_CLAUDE_DUMP_ENV", env_dump.to_str().unwrap().into()),
        ("ANTHROPIC_BASE_URL", "http://127.0.0.1:11434".into()),
        ("ANTHROPIC_MODEL", "glm-5.3:cloud".into()),
    ]);

    let rec = run(&spec(), Arm::Control, &clone, &run_dir, &caps()).unwrap();
    assert!(
        matches!(rec.exit, RunExit::Completed),
        "exit was {:?}",
        rec.exit
    );
    assert_eq!(rec.turns, 2);
    assert_eq!(rec.tokens_in, Some(300));
    assert_eq!(rec.tokens_out, Some(130));
    assert_eq!(rec.governance, None);
    assert!(rec.wall_clock_secs < caps().max_wall_clock_secs);

    let transcript = std::fs::read_to_string(run_dir.join("transcript.jsonl")).unwrap();
    assert!(transcript.contains("\"assistant\""));
    let patch = std::fs::read_to_string(run_dir.join("patch.diff")).unwrap();
    assert!(patch.contains("proof.txt"), "agent work missing from patch");
    assert_eq!(rec.diff_bytes, patch.len() as u64);

    assert_eq!(read_record(&run_dir), rec, "record.json must equal the record");

    let child_env = std::fs::read_to_string(&env_dump).unwrap();
    assert!(child_env.contains("ANTHROPIC_BASE_URL=http://127.0.0.1:11434"));
    assert!(child_env.contains("ANTHROPIC_MODEL=glm-5.3:cloud"));
}

#[test]
fn treatment_run_summarizes_clone_governance_log() {
    let (_guard, url) = fixture_repo();
    let work = tempdir().unwrap();
    let clone = clone_fixture(&url, work.path());
    let run_dir = work.path().join("run");
    let touch = clone.join("proof.txt");
    write_governance_log(&clone);

    let _fake = FakeClaude::new(&[("FAKE_CLAUDE_TOUCH", touch.to_str().unwrap().into())]);

    let rec = run(&spec(), Arm::Treatment, &clone, &run_dir, &caps()).unwrap();
    assert!(
        matches!(rec.exit, RunExit::Completed),
        "exit was {:?}",
        rec.exit
    );
    let governance = rec.governance.as_ref().expect("treatment carries governance");
    assert_eq!(governance.blocks.get("no-unwrap-in-src"), Some(&1));
    assert_eq!(governance.warns.get("audit-file-loc-high"), Some(&1));

    let patch = std::fs::read_to_string(run_dir.join("patch.diff")).unwrap();
    assert!(patch.contains("proof.txt"));
    assert!(
        !patch.contains(".phronesis"),
        "governance wiring must not enter the patch"
    );
}

#[test]
fn treatment_run_without_hook_log_records_governance_not_wired() {
    let (_guard, url) = fixture_repo();
    let work = tempdir().unwrap();
    let clone = clone_fixture(&url, work.path());
    let run_dir = work.path().join("run");

    let _fake = FakeClaude::new(&[]);

    let rec = run(&spec(), Arm::Treatment, &clone, &run_dir, &caps()).unwrap();
    match &rec.exit {
        RunExit::Error { reason } => {
            assert!(reason.contains("governance_not_wired"), "reason: {reason}")
        }
        other => panic!("expected Error, got {other:?}"),
    }
    assert!(rec.governance.is_none());
    assert!(
        validate(&rec).is_err(),
        "a not-wired treatment record must stay loud, not silently control-equivalent"
    );
    // The failure must be inspectable on disk, never silently dropped.
    assert!(run_dir.join("record.json").exists());
    assert!(run_dir.join("patch.diff").exists());
    assert!(run_dir.join("transcript.jsonl").exists());
    assert_eq!(read_record(&run_dir), rec);
}

#[test]
fn wall_clock_breach_kills_child_and_records_cap_time() {
    let (_guard, url) = fixture_repo();
    let work = tempdir().unwrap();
    let clone = clone_fixture(&url, work.path());
    let run_dir = work.path().join("run");

    let _fake = FakeClaude::new(&[("FAKE_CLAUDE_SLEEP", "10".into())]);
    let tight = Caps {
        max_turns: 100,
        max_wall_clock_secs: 1,
    };

    let rec = run(&spec(), Arm::Control, &clone, &run_dir, &tight).unwrap();
    assert!(matches!(rec.exit, RunExit::CapTime), "exit was {:?}", rec.exit);
    assert!(
        rec.wall_clock_secs >= 1 && rec.wall_clock_secs < 8,
        "child must be killed at the cap, took {}s",
        rec.wall_clock_secs
    );
    assert!(run_dir.join("record.json").exists());
    assert!(run_dir.join("patch.diff").exists());
}

#[test]
fn turn_cap_records_cap_turns() {
    let (_guard, url) = fixture_repo();
    let work = tempdir().unwrap();
    let clone = clone_fixture(&url, work.path());
    let run_dir = work.path().join("run");
    let transcript = work.path().join("turn-cap.jsonl");
    std::fs::write(
        &transcript,
        concat!(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#,
            "\n",
            r#"{"type":"result","subtype":"success","num_turns":5,"usage":{"input_tokens":10,"output_tokens":5}}"#,
            "\n",
        ),
    )
    .unwrap();

    let _fake = FakeClaude::new(&[(
        "FAKE_CLAUDE_TRANSCRIPT",
        transcript.to_str().unwrap().into(),
    )]);
    let tight = Caps {
        max_turns: 3,
        max_wall_clock_secs: 60,
    };

    let rec = run(&spec(), Arm::Control, &clone, &run_dir, &tight).unwrap();
    assert!(matches!(rec.exit, RunExit::CapTurns), "exit was {:?}", rec.exit);
    assert_eq!(rec.turns, 5);
}

#[test]
fn transcript_without_assistant_events_is_an_error_not_a_silent_pass() {
    let (_guard, url) = fixture_repo();
    let work = tempdir().unwrap();
    let clone = clone_fixture(&url, work.path());
    let run_dir = work.path().join("run");
    let garbage = work.path().join("garbage.jsonl");
    std::fs::write(&garbage, "not json at all\n{\"type\":\"system\"}\n").unwrap();

    let _fake = FakeClaude::new(&[(
        "FAKE_CLAUDE_TRANSCRIPT",
        garbage.to_str().unwrap().into(),
    )]);

    let rec = run(&spec(), Arm::Control, &clone, &run_dir, &caps()).unwrap();
    match &rec.exit {
        RunExit::Error { reason } => assert_eq!(reason, "transcript_unparseable"),
        other => panic!("expected Error, got {other:?}"),
    }
    assert_eq!(rec.turns, 0, "zeroed stats on unparseable transcript");
}

#[test]
fn claude_failure_is_error_with_stderr_reason() {
    let (_guard, url) = fixture_repo();
    let work = tempdir().unwrap();
    let clone = clone_fixture(&url, work.path());
    let run_dir = work.path().join("run");

    let _fake = FakeClaude::new(&[
        ("FAKE_CLAUDE_EXIT", "1".into()),
        ("FAKE_CLAUDE_STDERR", "router exploded".into()),
    ]);

    let rec = run(&spec(), Arm::Control, &clone, &run_dir, &caps()).unwrap();
    match &rec.exit {
        RunExit::Error { reason } => assert!(reason.contains("router exploded"), "reason: {reason}"),
        other => panic!("expected Error, got {other:?}"),
    }
    assert_eq!(rec.turns, 2, "stats still parsed from the valid transcript");
}