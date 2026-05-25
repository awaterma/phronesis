//! Concurrent-appender stress test for the action log. Spawns N threads,
//! each appending an entry to the same file, then verifies every line
//! parses as valid JSON. Regression for the macOS PIPE_BUF interleave
//! corruption observed on main before the fs2 advisory-lock switch.

use phronesis_mcp::action_log::{self, LogEntry};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

fn long_entry(i: usize) -> LogEntry {
    // Build an entry that comfortably exceeds 512 bytes (the macOS PIPE_BUF
    // value) so plain O_APPEND would interleave.
    let payload: Vec<String> = (0..20)
        .map(|j| format!("entry-{}-payload-line-{}-with-some-filler-text", i, j))
        .collect();
    LogEntry::new("hook", "stress")
        .with("i", i as u64)
        .with("payload", payload)
}

#[test]
fn concurrent_appenders_never_interleave() {
    let dir = tempfile::tempdir().unwrap();
    let path: Arc<PathBuf> = Arc::new(dir.path().join("log.jsonl"));

    let n_threads = 50;
    let mut handles = Vec::new();
    for i in 0..n_threads {
        let path = Arc::clone(&path);
        handles.push(thread::spawn(move || {
            action_log::append(&path, &long_entry(i)).unwrap();
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    let contents = std::fs::read_to_string(path.as_ref()).unwrap();
    let lines: Vec<&str> = contents.lines().collect();
    assert_eq!(
        lines.len(),
        n_threads,
        "expected {} lines, got {}",
        n_threads,
        lines.len()
    );
    for (i, line) in lines.iter().enumerate() {
        serde_json::from_str::<LogEntry>(line)
            .unwrap_or_else(|e| panic!("line {} is not valid JSON: {} :: {}", i, e, line));
    }
}
