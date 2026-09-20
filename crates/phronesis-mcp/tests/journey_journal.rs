use phronesis_mcp::journey::journal::{self, JournalRecord};

fn make_record(
    timing: (u64, u64),
    operation: (&str, &str),
    classification: (&[&str], Option<&str>),
) -> JournalRecord {
    let (seq, ts) = timing;
    let (tool, path) = operation;
    let (tags, subject) = classification;
    JournalRecord {
        v: 1,
        ts,
        sid: "s-test".to_string(),
        seq,
        tool: tool.to_string(),
        path: path.to_string(),
        ext: path.rsplit('.').next().map(|s| s.to_string()),
        module: None,
        tags: tags.iter().map(|s| s.to_string()).collect(),
        subject: subject.map(|s| s.to_string()),
        command_exit: None,
        kind: None,
        mode: None,
        host: None,
        turn: None,
        agent: None,
        agent_type: None,
        kalpa: None,
    }
}

macro_rules! rec {
    ($seq:expr, $ts:expr, $tool:expr, $path:expr, $tags:expr, $subject:expr) => {
        make_record(($seq, $ts), ($tool, $path), ($tags, $subject))
    };
}

#[test]
fn append_and_read_recent_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    journal::append(
        dir.path(),
        &rec!(1, 1000, "Edit", "src/auth/a.rs", &["auth"], None),
    )
    .unwrap();
    journal::append(
        dir.path(),
        &rec!(2, 1010, "Edit", "tests/a.rs", &["tests"], None),
    )
    .unwrap();
    journal::append(
        dir.path(),
        &rec!(3, 1020, "Bash", "<cmd>", &["build"], Some("u1")),
    )
    .unwrap();

    let recs = journal::read_recent(dir.path(), 10).unwrap();
    assert_eq!(recs.len(), 3);
    assert_eq!(recs[0].seq, 1);
    assert_eq!(recs[2].subject.as_deref(), Some("u1"));
    assert_eq!(recs[2].tags, vec!["build".to_string()]);
}

#[test]
fn read_recent_bounded_returns_tail() {
    let dir = tempfile::tempdir().unwrap();
    for seq in 1..=10 {
        journal::append(
            dir.path(),
            &rec!(seq, 1000 + seq, "Edit", "src/a.rs", &["auth"], None),
        )
        .unwrap();
    }
    let recs = journal::read_recent(dir.path(), 3).unwrap();
    assert_eq!(recs.len(), 3);
    assert_eq!(
        recs.iter().map(|r| r.seq).collect::<Vec<_>>(),
        vec![8, 9, 10]
    );
}

#[test]
fn read_recent_subject_filters() {
    let dir = tempfile::tempdir().unwrap();
    journal::append(
        dir.path(),
        &rec!(1, 1000, "Edit", "src/a.rs", &["auth"], None),
    )
    .unwrap();
    journal::append(
        dir.path(),
        &rec!(2, 1010, "Bash", "<cmd>", &["build"], Some("u1")),
    )
    .unwrap();
    journal::append(
        dir.path(),
        &rec!(3, 1020, "Bash", "<cmd>", &["build"], Some("u2")),
    )
    .unwrap();
    journal::append(
        dir.path(),
        &rec!(4, 1030, "Bash", "<cmd>", &["test"], Some("u1")),
    )
    .unwrap();

    let recs = journal::read_recent_subject(dir.path(), "u1", 10).unwrap();
    assert_eq!(recs.len(), 2);
    assert_eq!(recs[0].seq, 2);
    assert_eq!(recs[1].seq, 4);
}

#[test]
fn missing_file_reads_empty() {
    let dir = tempfile::tempdir().unwrap();
    assert!(journal::read_recent(dir.path(), 10).unwrap().is_empty());
    assert!(
        journal::read_recent_subject(dir.path(), "u1", 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn malformed_lines_are_skipped() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let journey_dir = dir.path().join(".phronesis").join("journey");
    std::fs::create_dir_all(&journey_dir).unwrap();
    let path = journey_dir.join("events.jsonl");
    let good = serde_json::to_string(&rec!(1, 1000, "Edit", "src/a.rs", &["auth"], None)).unwrap();
    let mut f = std::fs::File::create(&path).unwrap();
    writeln!(f, "{}", good).unwrap();
    writeln!(f, "{{not json").unwrap();
    writeln!(f, "{}", good).unwrap();
    drop(f);
    let recs = journal::read_recent(dir.path(), 10).unwrap();
    assert_eq!(recs.len(), 2);
}

#[test]
fn concurrent_appends_serialize() {
    use std::sync::Arc;
    use std::thread;

    let dir = Arc::new(tempfile::tempdir().unwrap());
    let mut handles = Vec::new();
    for t in 0..8u64 {
        let dir = Arc::clone(&dir);
        handles.push(thread::spawn(move || {
            for i in 0..50u64 {
                let seq = t * 100 + i;
                journal::append(
                    dir.path(),
                    &rec!(seq, 1000 + seq, "Edit", "src/a.rs", &["auth"], None),
                )
                .unwrap();
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    let recs = journal::read_recent(dir.path(), 10_000).unwrap();
    assert_eq!(recs.len(), 400, "all appends preserved");
    // Each line parsed as a record — no interleaved partials.
}

#[test]
fn append_errors_when_events_path_is_a_directory() {
    // OpenOptions::append on a path that's already a directory yields an Io
    // error — exercises the second `.map_err(...)` in `append`.
    let dir = tempfile::tempdir().unwrap();
    let journey_dir = dir.path().join(".phronesis").join("journey");
    std::fs::create_dir_all(&journey_dir).unwrap();
    std::fs::create_dir(journey_dir.join("events.jsonl")).unwrap();
    let err = journal::append(
        dir.path(),
        &rec!(1, 1000, "Edit", "src/a.rs", &["auth"], None),
    )
    .unwrap_err();
    match &err {
        journal::JournalError::Io { path, .. } => {
            assert!(path.contains("events.jsonl"), "path = {path}");
        }
        other => panic!("expected JournalError::Io, got {other:?}"),
    }
}

#[test]
fn append_errors_when_phronesis_is_a_file() {
    // create_dir_all on .phronesis/journey/ fails when .phronesis exists as a
    // regular file. Exercises the JournalError::Io path in `append`.
    let dir = tempfile::tempdir().unwrap();
    let phr = dir.path().join(".phronesis");
    std::fs::write(&phr, b"not a dir").unwrap();
    let err = journal::append(
        dir.path(),
        &rec!(1, 1000, "Edit", "src/a.rs", &["auth"], None),
    )
    .unwrap_err();
    // Confirm we get the Io variant with a path that points at the journey dir.
    match &err {
        journal::JournalError::Io { path, .. } => {
            assert!(path.contains(".phronesis"), "path = {path}");
        }
        other => panic!("expected JournalError::Io, got {other:?}"),
    }
    // Display impl is rendered via `?`/format; assert it's nonempty and mentions io.
    let s = format!("{err}");
    assert!(s.contains("io"), "display = {s}");
}

#[test]
fn read_recent_errors_when_events_is_a_directory() {
    // Opening events.jsonl with read_to_string yields a non-NotFound Io error
    // when the path exists as a directory — exercises the catch-all Err arm
    // in `read_recent`.
    let dir = tempfile::tempdir().unwrap();
    let journey_dir = dir.path().join(".phronesis").join("journey");
    std::fs::create_dir_all(&journey_dir).unwrap();
    // Make events.jsonl a directory rather than a file.
    std::fs::create_dir(journey_dir.join("events.jsonl")).unwrap();
    let err = journal::read_recent(dir.path(), 10).unwrap_err();
    match &err {
        journal::JournalError::Io { path, .. } => {
            assert!(path.contains("events.jsonl"), "path = {path}");
        }
        other => panic!("expected JournalError::Io, got {other:?}"),
    }
}

#[test]
fn read_recent_subject_propagates_io_error() {
    // read_recent_subject delegates to read_recent — the same directory-as-file
    // trick surfaces the `?` propagation site.
    let dir = tempfile::tempdir().unwrap();
    let journey_dir = dir.path().join(".phronesis").join("journey");
    std::fs::create_dir_all(&journey_dir).unwrap();
    std::fs::create_dir(journey_dir.join("events.jsonl")).unwrap();
    let err = journal::read_recent_subject(dir.path(), "u1", 5).unwrap_err();
    assert!(matches!(err, journal::JournalError::Io { .. }));
}

#[test]
fn journal_error_display_renders_both_variants() {
    // Io variant — formatted via thiserror.
    let io = journal::JournalError::Io {
        path: "/tmp/some-path".to_string(),
        source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied"),
    };
    let s = format!("{io}");
    assert!(s.contains("/tmp/some-path"));
    assert!(s.contains("denied"));

    // Json variant — synthesize a serde_json::Error.
    let json_err = serde_json::from_str::<JournalRecord>("not json").unwrap_err();
    let je: journal::JournalError = json_err.into();
    let s = format!("{je}");
    assert!(s.contains("json"));
}

#[test]
fn v1_record_reads_under_v2_with_kind_none() {
    let line = r#"{"v":1,"ts":1,"sid":"s-x","seq":1,"tool":"Edit","path":"a.rs","ext":"rs","tags":["edits"]}"#;
    let rec: JournalRecord = serde_json::from_str(line).unwrap();
    assert_eq!(rec.v, 1);
    assert!(rec.kind.is_none());
    assert!(!rec.is_lifecycle());
}

#[test]
fn v2_lifecycle_record_round_trips_in_field_order() {
    let rec = JournalRecord {
        v: journal::JOURNAL_V,
        ts: 10,
        sid: "s-x".into(),
        seq: 7,
        tool: "__lifecycle".into(),
        path: String::new(),
        ext: None,
        module: None,
        tags: vec!["lifecycle:prompt".into(), "lifecycle:prompt:fresh".into()],
        subject: None,
        command_exit: None,
        kind: Some("prompt".into()),
        mode: Some("fresh".into()),
        host: Some("claude".into()),
        turn: Some("t-1".into()),
        agent: None,
        agent_type: None,
        kalpa: Some("demo".into()),
    };
    assert!(rec.is_lifecycle());
    let s = serde_json::to_string(&rec).unwrap();
    assert_eq!(
        s,
        r#"{"v":2,"ts":10,"sid":"s-x","seq":7,"tool":"__lifecycle","path":"","tags":["lifecycle:prompt","lifecycle:prompt:fresh"],"kind":"prompt","mode":"fresh","host":"claude","turn":"t-1","kalpa":"demo"}"#
    );
    let back: JournalRecord = serde_json::from_str(&s).unwrap();
    assert_eq!(back, rec);
}

/// Spec §"Determinism and versioning": a downgraded binary reads a lifecycle
/// record as an odd `__lifecycle` tool record with no projection, which shifts
/// positional windows and adds `""` to `journey_distinct` on `path`. That is
/// the rollout hazard; pinning it here keeps it visible rather than
/// rediscovered.
#[test]
fn a_v1_reader_sees_a_lifecycle_record_as_a_tool_record() {
    /// The v1 shape, verbatim: no `kind`, no `mode`, no lifecycle fields.
    #[derive(serde::Deserialize)]
    struct V1Record {
        v: u32,
        tool: String,
        path: String,
        tags: Vec<String>,
    }
    let line = r#"{"v":2,"ts":10,"sid":"s-x","seq":7,"tool":"__lifecycle","path":"","tags":["lifecycle:prompt"],"kind":"prompt","mode":"fresh","host":"claude"}"#;
    let old: V1Record = serde_json::from_str(line).unwrap();
    assert_eq!(old.v, 2, "a v1 reader has no way to reject the record");
    assert_eq!(old.tool, "__lifecycle");
    assert_eq!(
        old.path, "",
        "which is what pollutes journey_distinct on path"
    );
    assert_eq!(old.tags, vec!["lifecycle:prompt"]);
}

// ---------- Compaction retention (Task 3) ----------

/// A lifecycle record, mirroring Task 2's `make_lifecycle` in
/// `tests/journey_derive.rs`.
fn lifecycle_record(ts: u64, seq: u64, kind: &str, tags: &[&str]) -> JournalRecord {
    JournalRecord {
        v: journal::JOURNAL_V,
        ts,
        sid: "s-a".into(),
        seq,
        tool: journal::LIFECYCLE_TOOL.into(),
        path: String::new(),
        ext: None,
        module: None,
        tags: tags.iter().map(|s| s.to_string()).collect(),
        subject: None,
        command_exit: None,
        kind: Some(kind.into()),
        mode: None,
        host: Some("claude".into()),
        turn: None,
        agent: None,
        agent_type: None,
        kalpa: None,
    }
}

/// A plain tool record, used here only as the compaction tail.
fn tool_record(ts: u64, seq: u64) -> JournalRecord {
    JournalRecord {
        v: journal::JOURNAL_V,
        ts,
        sid: "s-a".into(),
        seq,
        tool: "Edit".into(),
        path: format!("src/f{seq}.rs"),
        ext: Some("rs".into()),
        module: None,
        tags: vec!["edits".into()],
        subject: None,
        command_exit: None,
        kind: None,
        mode: None,
        host: None,
        turn: None,
        agent: None,
        agent_type: None,
        kalpa: None,
    }
}

#[test]
fn compaction_retains_commit_kalpa_and_unit_records_in_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let mut commit = lifecycle_record(1, 1, "commit", &["lifecycle:commit"]);
    commit.kalpa = Some("demo".into());

    journal::append(
        dir.path(),
        &lifecycle_record(
            0,
            0,
            "kalpa_start",
            &["lifecycle:kalpa_start", "kalpa:demo"],
        ),
    )
    .unwrap();
    journal::append(dir.path(), &commit).unwrap();
    journal::append(
        dir.path(),
        &lifecycle_record(
            2,
            2,
            "prompt",
            &["lifecycle:prompt", "lifecycle:prompt:fresh"],
        ),
    )
    .unwrap();
    // The friction record is the point of the feature: an interrupt and the
    // correction that follows it must survive compaction, or a rule like "two
    // corrections this session" stops firing because the journal compacted.
    journal::append(
        dir.path(),
        &lifecycle_record(5, 5, "interrupt", &["lifecycle:interrupt"]),
    )
    .unwrap();
    journal::append(
        dir.path(),
        &lifecycle_record(
            6,
            6,
            "prompt",
            &[
                "lifecycle:prompt",
                "lifecycle:prompt:correction",
                "lifecycle:intervention",
            ],
        ),
    )
    .unwrap();
    journal::append(
        dir.path(),
        &lifecycle_record(3, 3, "kalpa_end", &["lifecycle:kalpa_end", "kalpa:demo"]),
    )
    .unwrap();
    // Work-item boundaries are the denominator of every per-work-item report,
    // and a compacted-away `unit_start` reclassifies an explicit unit as
    // implicit — so they are retained alongside the kalpa boundaries.
    journal::append(
        dir.path(),
        &lifecycle_record(7, 7, "unit_start", &["lifecycle:unit_start"]),
    )
    .unwrap();
    journal::append(
        dir.path(),
        &lifecycle_record(8, 8, "unit_end", &["lifecycle:unit_end"]),
    )
    .unwrap();
    // The one record the tail keeps, so every lifecycle record above lands in
    // the compaction prefix and is subject to the retention rule.
    journal::append(dir.path(), &tool_record(4, 4)).unwrap();

    // max_bytes = 1 forces compaction; tail_records = 1 keeps only the last.
    assert!(journal::maybe_compact(dir.path(), 1, 1).unwrap());

    let all = journal::read_recent(dir.path(), journal::SUFFIX_HARD_CAP).unwrap();
    let kinds: Vec<&str> = all.iter().filter_map(|r| r.kind.as_deref()).collect();
    assert!(kinds.contains(&"commit"), "{kinds:?}");
    assert!(kinds.contains(&"kalpa_start"), "{kinds:?}");
    assert!(kinds.contains(&"kalpa_end"), "{kinds:?}");
    assert!(kinds.contains(&"interrupt"), "{kinds:?}");
    assert!(kinds.contains(&"unit_start"), "{kinds:?}");
    assert!(kinds.contains(&"unit_end"), "{kinds:?}");
    // Exactly one prompt survives: the correction, not the fresh one.
    let prompt_tags: Vec<&Vec<String>> = all
        .iter()
        .filter(|r| r.kind.as_deref() == Some("prompt"))
        .map(|r| &r.tags)
        .collect();
    assert_eq!(prompt_tags.len(), 1, "{prompt_tags:?}");
    assert!(
        prompt_tags[0]
            .iter()
            .any(|t| t == "lifecycle:prompt:correction"),
        "a fresh prompt compacts away, a correction does not: {prompt_tags:?}"
    );
    assert!(all.iter().any(|r| r.tool == "Edit"), "the tail survives");
}

// ---------- record() journal placement (Task 7) ----------

/// No field of a lifecycle journal record ever contains prompt text.
/// Not "no `prompt` key": no field at all, checked over the serialized line.
#[test]
fn a_prompt_event_journals_no_field_containing_the_text() {
    use phronesis_mcp::lifecycle::{Host, Kind, LifecycleEvent, Mode, record::record};
    let d = tempfile::tempdir().unwrap();
    record(
        d.path(),
        LifecycleEvent::new(Kind::Prompt, Host::Claude)
            .with_mode(Mode::Correction)
            .with_prompt("zzz-distinctive-prompt-text"),
    );
    let line = std::fs::read_to_string(d.path().join(".phronesis/journey/events.jsonl")).unwrap();
    assert!(!line.contains("zzz-distinctive"), "{line}");
    let rec: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    for (_, v) in rec.as_object().unwrap() {
        assert!(!v.to_string().contains("zzz-distinctive"), "{rec}");
    }
    // …and the action log does hold it, so this is a placement test, not a
    // "the text vanished" test.
    assert!(
        std::fs::read_to_string(d.path().join(".phronesis/log.jsonl"))
            .unwrap()
            .contains("zzz-distinctive-prompt-text")
    );
}
