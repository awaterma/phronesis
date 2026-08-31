//! Reader for the phronesis action log (`.phronesis/log.jsonl`).
//!
//! This crate deliberately parses the log itself rather than depending on
//! `phronesis-mcp`: the dependency arrow points one way
//! (`phronesis-mcp` -> `phronesis-metrics`), so the MCP crate can pull this
//! one in behind an optional feature without a cycle.
//!
//! The on-disk shape is one JSON object per line,
//! `{"ts":..,"kind":..,"event":..,<event-specific fields>}`. Every writer
//! appends under an exclusive advisory lock, so lines are never interleaved.

use serde::Deserialize;
use std::path::{Path, PathBuf};

/// One parsed log line. Event-specific fields stay in `data` as raw JSON;
/// the derivation layer reaches into them by name.
#[derive(Debug, Clone, Deserialize)]
pub struct LogRecord {
    /// Unix epoch seconds.
    pub ts: u64,
    /// Top-level discriminator: `"hook"`, `"mcp"`, or `"context"`.
    pub kind: String,
    /// Event name within `kind`, e.g. `"pre_check"`, `"fire_rules"`.
    pub event: String,
    /// Everything else, flattened on disk.
    #[serde(flatten)]
    pub data: serde_json::Map<String, serde_json::Value>,
}

impl LogRecord {
    /// Read a `u64` field, tolerating the number arriving as a float.
    pub fn num(&self, key: &str) -> Option<u64> {
        self.data.get(key)?.as_u64()
    }

    /// Read a string field.
    pub fn str_field(&self, key: &str) -> Option<&str> {
        self.data.get(key)?.as_str()
    }

    /// The `consequences` array, if this record carries one.
    pub fn consequences(&self) -> &[serde_json::Value] {
        self.data
            .get("consequences")
            .and_then(|v| v.as_array())
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

/// Default log path under a project root.
pub fn default_path(project_root: &Path) -> PathBuf {
    project_root.join(".phronesis").join("log.jsonl")
}

/// What one read of the log yielded.
#[derive(Debug, Clone, Default)]
pub struct LogRead {
    pub records: Vec<LogRecord>,
    /// Size of the log file in bytes, exposed as a gauge so a dashboard can
    /// see truncation (`append_with_max`) coming.
    pub bytes: u64,
    /// Lines that failed to parse. Non-fatal — a partially written tail is
    /// possible if a writer was killed mid-append — but worth surfacing.
    pub malformed: u64,
}

/// Read and parse the log. A missing file is not an error: it means the
/// project simply has no recorded activity yet, and every family renders
/// empty. Malformed lines are counted and skipped rather than aborting the
/// scrape.
pub fn read(path: &Path) -> std::io::Result<LogRead> {
    let bytes = match std::fs::metadata(path) {
        Ok(m) => m.len(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(LogRead::default()),
        Err(e) => return Err(e),
    };
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(LogRead::default()),
        Err(e) => return Err(e),
    };
    let mut out = LogRead {
        bytes,
        ..Default::default()
    };
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<LogRecord>(line) {
            Ok(r) => out.records.push(r),
            Err(_) => out.malformed += 1,
        }
    }
    Ok(out)
}
