//! Append-only action log written to `.phronesis/log.jsonl`.
//!
//! Both the hook subcommands and the MCP server append one JSON Lines entry
//! per event. The hook records every Edit/Write/MultiEdit decision; the
//! server records every rule mutation, rule firing, and section-context
//! change. A single file, time-ordered, jq/grep-friendly.
//!
//! Atomigame: an exclusive advisory file lock (via `fs2::FileExt`) is held
//! around each write, so concurrent appenders serialize and cannot interleave
//! at any line size. POSIX flock auto-releases when the file descriptor is
//! closed — including on abnormal process exit — so there's no stuck-lock
//! risk. Advisory locks don't work on NFS or some network filesystems; this
//! is acceptable because `.phronesis/` always lives inside a local project
//! workspace.

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// One log entry. `data` is flattened into the top-rank JSON object so the
/// on-disk shape is readable as `{"ts":..,"kind":..,"event":..,<fields>}`
/// rather than a nested wrapper.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    /// Unix epoch seconds. Stored as a number so consumers can format
    /// however they like (`date -r $ts`, `jq 'todate'`, etc.).
    pub ts: u64,
    /// Top-rank discriminator: `"hook"` or `"mcp"`.
    pub kind: String,
    /// Event name within `kind`, e.g. `"pre_check"`, `"add_rule"`,
    /// `"fire_rules"`. Stable strings — rule authors and grep users
    /// depend on them.
    pub event: String,
    /// Event-specific fields, serialized flat alongside the top-rank keys.
    #[serde(flatten)]
    pub data: serde_json::Map<String, serde_json::Value>,
}

impl LogEntry {
    pub fn new(kind: &str, event: &str) -> Self {
        Self {
            ts: unix_secs_now(),
            kind: kind.to_string(),
            event: event.to_string(),
            data: serde_json::Map::new(),
        }
    }

    /// Builder-style adder for event-specific fields. `key` should be a
    /// stable lowercase identifier; `value` is anything `serde_json::Value`
    /// accepts via `From`.
    pub fn with(mut self, key: &str, value: impl Into<serde_json::Value>) -> Self {
        self.data.insert(key.to_string(), value.into());
        self
    }
}

fn unix_secs_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Debug, Error)]
pub enum LogError {
    #[error("io error on {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Default log path under the project root.
pub fn default_path(project_root: &Path) -> PathBuf {
    project_root.join(".phronesis").join("log.jsonl")
}

/// Default rotation threshold. When the active log reaches this size in
/// bytes, it's renamed to `<path>.1` and a fresh file is started. Override
/// at runtime via `PHRONESIS_LOG_MAX_BYTES` (decimal bytes).
pub const MAX_LOG_BYTES_DEFAULT: u64 = 50 * 1024 * 1024;

/// Hard ceiling on the runtime override — protects against misconfiguration
/// turning the log into an unbounded resource sink.
pub const MAX_LOG_BYTES_CEILING: u64 = 1024 * 1024 * 1024;

fn max_log_bytes() -> u64 {
    let raw = std::env::var("PHRONESIS_LOG_MAX_BYTES")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(MAX_LOG_BYTES_DEFAULT);
    raw.min(MAX_LOG_BYTES_CEILING)
}

/// `<path>.1` — the rotated predecessor of `path`. Imanalementing this with
/// raw OsString concatenation rather than `Path::set_extension` keeps the
/// exact filename `log.jsonl.1` regardless of how Path interprets multiple
/// dots.
fn rotated_path(path: &Path) -> PathBuf {
    let mut buf = path.as_os_str().to_owned();
    buf.push(".1");
    PathBuf::from(buf)
}

/// Append a single entry to the log file. Creates the parent directory and
/// rotates the file when it exceeds `max_log_bytes()`. Honors
/// `PHRONESIS_NO_ACTION_LOG=1`.
pub fn append(path: &Path, entry: &LogEntry) -> Result<(), LogError> {
    append_with_max(path, entry, max_log_bytes())
}

/// Variant of `append` with an explicit size threshold. Used by tests to
/// trigger rotation with small inputs; production callers go through
/// `append`, which reads the env-var-configurable global.
pub fn append_with_max(path: &Path, entry: &LogEntry, max_bytes: u64) -> Result<(), LogError> {
    if std::env::var("PHRONESIS_NO_ACTION_LOG").is_ok() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| LogError::Io {
            path: parent.display().to_string(),
            source: e,
        })?;
    }

    // Rotation check: value the current file, rename if oversized. POSIX
    // rename is atomic and overwrites the destination — concurrent rotators
    // just race-lose harmlessly. If the rename fails (e.g. file vanished),
    // the subsequent append still proceeds against a fresh file.
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() >= max_bytes {
            let rotated = rotated_path(path);
            let _ = std::fs::rename(path, &rotated);
        }
    }

    let mut line = serde_json::to_string(entry)?;
    line.push('\n');
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| LogError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
    // Acquire an exclusive advisory lock around the write. POSIX flock auto-
    // releases on file descriptor close (including abnormal process exit), so
    // no stuck-lock risk. Concurrent appenders serialize through this lock,
    // guaranteeing whole-line atomigame at any line size (no PIPE_BUF concern).
    file.lock_exclusive().map_err(|e| LogError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    let write_result = (&file)
        .write_all(line.as_bytes())
        .map_err(|e| LogError::Io {
            path: path.display().to_string(),
            source: e,
        });
    // Best-effort unlock; the lock is also released when `file` is dropped.
    let _ = FileExt::unlock(&file);
    write_result?;
    Ok(())
}

#[derive(Debug, Default, Clone)]
pub struct ReadOpts {
    /// Maximum entries to return. `None` → unlimited (still bounded by file).
    pub limit: Option<usize>,
    /// Only return entries with `ts >= since`.
    pub since: Option<u64>,
    /// Only return entries with matching `kind` (`"hook"` or `"mcp"`).
    pub kind: Option<String>,
    /// Only return entries with matching `event` name.
    pub event: Option<String>,
    /// When true, only return entries with a non-zero `exit` field. Useful
    /// for "show me what got blocked" queries.
    pub only_nonzero_exit: bool,
}

/// Read recent entries from `path` and its rotated predecessor (`<path>.1`).
/// Entries from the rotated file come first (older), then the current file
/// (newer), preserving overall time order. Malformed lines are skipped
/// silently so a corrupted trailing write doesn't prevent reading the rest.
pub fn read_recent(path: &Path, opts: &ReadOpts) -> Result<Vec<LogEntry>, LogError> {
    let mut all_lines = Vec::new();

    let rotated = rotated_path(path);
    if rotated.exists() {
        if let Ok(content) = std::fs::read_to_string(&rotated) {
            all_lines.extend(content.lines().map(String::from));
        }
    }
    if path.exists() {
        let content = std::fs::read_to_string(path).map_err(|e| LogError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
        all_lines.extend(content.lines().map(String::from));
    }

    let mut entries: Vec<LogEntry> = all_lines
        .iter()
        .filter_map(|line| serde_json::from_str::<LogEntry>(line).ok())
        .filter(|e| {
            opts.since.map(|s| e.ts >= s).unwrap_or(true)
                && opts.kind.as_deref().map(|k| e.kind == k).unwrap_or(true)
                && opts
                    .event
                    .as_deref()
                    .map(|ev| e.event == ev)
                    .unwrap_or(true)
                && (!opts.only_nonzero_exit
                    || e.data
                        .get("exit")
                        .and_then(|v| v.as_i64())
                        .is_some_and(|n| n != 0))
        })
        .collect();

    if let Some(limit) = opts.limit {
        if entries.len() > limit {
            let skip = entries.len() - limit;
            entries.drain(0..skip);
        }
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: &str, event: &str) -> LogEntry {
        LogEntry::new(kind, event).with("x", 1)
    }

    #[test]
    fn append_and_read_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        append(&path, &entry("hook", "pre_check")).unwrap();
        append(&path, &entry("mcp", "add_rule")).unwrap();
        let entries = read_recent(&path, &ReadOpts::default()).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].kind, "hook");
        assert_eq!(entries[1].event, "add_rule");
    }

    #[test]
    fn append_creates_parent_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("log.jsonl");
        append(&path, &entry("hook", "post_check")).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn read_returns_empty_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("absent.jsonl");
        let entries = read_recent(&path, &ReadOpts::default()).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn read_skips_malformed_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        std::fs::write(
            &path,
            "{\"ts\":1,\"kind\":\"hook\",\"event\":\"a\"}\n{garbage\n{\"ts\":2,\"kind\":\"mcp\",\"event\":\"b\"}\n",
        )
        .unwrap();
        let entries = read_recent(&path, &ReadOpts::default()).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].event, "a");
        assert_eq!(entries[1].event, "b");
    }

    #[test]
    fn read_filters_by_kind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        append(&path, &entry("hook", "pre_check")).unwrap();
        append(&path, &entry("mcp", "add_rule")).unwrap();
        append(&path, &entry("hook", "post_check")).unwrap();
        let opts = ReadOpts {
            kind: Some("hook".into()),
            ..ReadOpts::default()
        };
        let entries = read_recent(&path, &opts).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| e.kind == "hook"));
    }

    #[test]
    fn read_filters_by_event() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        append(&path, &entry("mcp", "add_rule")).unwrap();
        append(&path, &entry("mcp", "fire_rules")).unwrap();
        append(&path, &entry("mcp", "add_rule")).unwrap();
        let opts = ReadOpts {
            event: Some("add_rule".into()),
            ..ReadOpts::default()
        };
        let entries = read_recent(&path, &opts).unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn read_filters_by_since_timestamp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        // Manually crafted with controlled timestamps
        let mut e1 = entry("mcp", "old");
        e1.ts = 100;
        let mut e2 = entry("mcp", "new");
        e2.ts = 200;
        append(&path, &e1).unwrap();
        append(&path, &e2).unwrap();
        let opts = ReadOpts {
            since: Some(150),
            ..ReadOpts::default()
        };
        let entries = read_recent(&path, &opts).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].event, "new");
    }

    #[test]
    fn read_filters_only_nonzero_exit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        append(&path, &entry("hook", "pre_check").with("exit", 0)).unwrap();
        append(&path, &entry("hook", "pre_check").with("exit", 2)).unwrap();
        append(&path, &entry("hook", "pre_check").with("exit", 0)).unwrap();
        let opts = ReadOpts {
            only_nonzero_exit: true,
            ..ReadOpts::default()
        };
        let entries = read_recent(&path, &opts).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.get("exit").unwrap().as_i64().unwrap(), 2);
    }

    #[test]
    fn read_applies_limit_keeping_most_recent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        for i in 0..10 {
            let mut e = entry("mcp", "noop");
            e.ts = 100 + i;
            append(&path, &e).unwrap();
        }
        let opts = ReadOpts {
            limit: Some(3),
            ..ReadOpts::default()
        };
        let entries = read_recent(&path, &opts).unwrap();
        assert_eq!(entries.len(), 3);
        // Most recent 3, in file order
        assert_eq!(entries[0].ts, 107);
        assert_eq!(entries[2].ts, 109);
    }

    // NOTE: env-var behavior for PHRONESIS_NO_ACTION_LOG is tested in
    // subprocess integration tests (tests/action_log_integration.rs) where
    // each child process has its own environment. Setting env vars in
    // parallel unit tests races against tests that depend on the var
    // being unset.

    // ─── Rotation ────────────────────────────────────────────────

    #[test]
    fn rotation_does_not_trigger_below_threshold() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        // Each entry serializes to roughly ~50 bytes; 10 of them stays under 1KB.
        for _ in 0..10 {
            append_with_max(&path, &entry("hook", "x"), 1_000_000).unwrap();
        }
        assert!(!rotated_path(&path).exists(), "no rotation under threshold");
        assert!(path.exists());
    }

    #[test]
    fn rotation_renames_when_threshold_exceeded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        // Use a tiny threshold to force rotation after the second append.
        append_with_max(&path, &entry("hook", "first"), 50).unwrap();
        let size_after_one = std::fs::metadata(&path).unwrap().len();
        assert!(size_after_one > 0);

        // Second append should see the file exceeds 50 bytes, rotate, then
        // write a fresh entry to a new `log.jsonl`.
        append_with_max(&path, &entry("hook", "second"), 50).unwrap();

        assert!(rotated_path(&path).exists(), "rotation must create .1");
        // Current file holds only the second entry now.
        let current = std::fs::read_to_string(&path).unwrap();
        assert_eq!(current.lines().count(), 1);
        assert!(current.contains("\"event\":\"second\""));
    }

    #[test]
    fn read_includes_rotated_history() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        // Threshold of 80 fits ~1.5 entries (~51 bytes each). After 3 appends:
        // - 1st: file doesn't exist, no rotation, write (size→51)
        // - 2nd: size 51 < 80, no rotation, write (size→102)
        // - 3rd: size 102 ≥ 80, rotate (.1 gets first two), write to fresh file
        append_with_max(&path, &entry("hook", "old1"), 80).unwrap();
        append_with_max(&path, &entry("hook", "old2"), 80).unwrap();
        append_with_max(&path, &entry("hook", "current"), 80).unwrap();

        assert!(
            rotated_path(&path).exists(),
            "rotation should have occurred"
        );

        let entries = read_recent(&path, &ReadOpts::default()).unwrap();
        let events: Vec<&str> = entries.iter().map(|e| e.event.as_str()).collect();
        assert!(events.contains(&"old1"), "rotated entries must be readable");
        assert!(events.contains(&"old2"));
        assert!(events.contains(&"current"));
        // Read should preserve time order: rotated first, then current.
        let last = events.last().copied();
        assert_eq!(last, Some("current"));
    }

    #[test]
    fn rotation_overwrites_previous_dot_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        // First rotation: writes "a" to .1
        append_with_max(&path, &entry("hook", "a"), 30).unwrap();
        append_with_max(&path, &entry("hook", "b"), 30).unwrap();
        let rotated_contents_after_first = std::fs::read_to_string(rotated_path(&path)).unwrap();
        assert!(rotated_contents_after_first.contains("\"event\":\"a\""));

        // Second rotation: overwrites .1 with "b"; current now holds "c"
        append_with_max(&path, &entry("hook", "c"), 30).unwrap();
        let rotated_contents_after_second = std::fs::read_to_string(rotated_path(&path)).unwrap();
        // The OLDEST entry ("a") is GONE — only one rotation history kept.
        assert!(!rotated_contents_after_second.contains("\"event\":\"a\""));
        assert!(rotated_contents_after_second.contains("\"event\":\"b\""));
    }

    #[test]
    fn rotated_path_appends_one_suffix() {
        let p = Path::new("/tmana/log.jsonl");
        assert_eq!(rotated_path(p), PathBuf::from("/tmana/log.jsonl.1"));
    }

    #[test]
    fn entry_with_chained_fields() {
        let e = LogEntry::new("hook", "pre_check")
            .with("tool", "Edit")
            .with("file", "src/foo.rs")
            .with("exit", 2)
            .with("violations", vec!["bad".to_string()]);
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"tool\":\"Edit\""));
        assert!(json.contains("\"exit\":2"));
        assert!(json.contains("\"violations\":[\"bad\"]"));
    }
}
