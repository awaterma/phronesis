use phronesis_mcp::journey::journal::{self, JournalRecord};

fn rec(
    seq: u64,
    ts: u64,
    tool: &str,
    path: &str,
    tags: &[&str],
    subject: Option<&str>,
) -> JournalRecord {
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
    }
}

#[test]
fn append_and_read_recent_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    journal::append(
        dir.path(),
        &rec(1, 1000, "Edit", "src/auth/a.rs", &["auth"], None),
    )
    .unwrap();
    journal::append(
        dir.path(),
        &rec(2, 1010, "Edit", "tests/a.rs", &["tests"], None),
    )
    .unwrap();
    journal::append(
        dir.path(),
        &rec(3, 1020, "Bash", "<cmd>", &["build"], Some("u1")),
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
            &rec(seq, 1000 + seq, "Edit", "src/a.rs", &["auth"], None),
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
        &rec(1, 1000, "Edit", "src/a.rs", &["auth"], None),
    )
    .unwrap();
    journal::append(
        dir.path(),
        &rec(2, 1010, "Bash", "<cmd>", &["build"], Some("u1")),
    )
    .unwrap();
    journal::append(
        dir.path(),
        &rec(3, 1020, "Bash", "<cmd>", &["build"], Some("u2")),
    )
    .unwrap();
    journal::append(
        dir.path(),
        &rec(4, 1030, "Bash", "<cmd>", &["test"], Some("u1")),
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
    let good = serde_json::to_string(&rec(1, 1000, "Edit", "src/a.rs", &["auth"], None)).unwrap();
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
                    &rec(seq, 1000 + seq, "Edit", "src/a.rs", &["auth"], None),
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
