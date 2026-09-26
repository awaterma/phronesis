use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

pub const COVERAGE_FORMAT: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HitRecord {
    pub v: u32,
    pub kind: String,
    pub test: String,
    pub region: String,
    pub file: String,
    pub start_line: u64,
    pub end_line: u64,
    pub hit_kind: String,
    pub revision: String,
    pub tool: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoverageIndex {
    pub format: u32,
    pub revision: String,
    pub imported_at: u64,
    pub tool: String,
}

pub fn store_paths(root: &Path) -> (PathBuf, PathBuf) {
    let dir = root.join(".phronesis");
    (dir.join("coverage.jsonl"), dir.join("coverage.index"))
}

/// Atomically replace the coverage store: records first, then the index
/// (the index is the commit marker — a crash between the two leaves the
/// old index pointing at a newer store, which hydration reports as stale
/// rather than corrupt).
pub fn write_store(root: &Path, records: &[HitRecord], index: &CoverageIndex) -> Result<()> {
    let (records_path, index_path) = store_paths(root);
    let dir = records_path
        .parent()
        .context("coverage path has no parent")?;
    fs::create_dir_all(dir)?;

    let tmp_records = records_path.with_file_name("coverage.jsonl.tmp");
    {
        let mut file = File::create(&tmp_records)?;
        for rec in records {
            serde_json::to_writer(&mut file, rec)?;
            writeln!(file)?;
        }
        file.sync_all()?;
    }
    fs::rename(&tmp_records, &records_path)?;

    let tmp_index = index_path.with_file_name("coverage.index.tmp");
    {
        let mut file = File::create(&tmp_index)?;
        serde_json::to_writer(&mut file, index)?;
        file.sync_all()?;
    }
    fs::rename(&tmp_index, &index_path)?;

    Ok(())
}

/// Fail-open read: a missing or unreadable index means "no coverage
/// imported" — the hook path must never hard-fail on derived state.
pub fn load_index(root: &Path) -> Option<CoverageIndex> {
    let (_, index_path) = store_paths(root);
    let file = File::open(&index_path).ok()?;
    serde_json::from_reader(file).ok()
}

pub fn load_hits(root: &Path) -> Result<Vec<HitRecord>> {
    let (records_path, _) = store_paths(root);
    let file = match File::open(&records_path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e).context("opening coverage store"),
    };

    let reader = BufReader::new(file);
    let mut hits = Vec::new();
    for (i, line_res) in reader.lines().enumerate() {
        let line = line_res.context(format!("coverage store line {}", i + 1))?;
        if line.trim().is_empty() {
            continue;
        }
        let rec: HitRecord =
            serde_json::from_str(&line).context(format!("coverage store line {}", i + 1))?;
        if rec.v != COVERAGE_FORMAT {
            return Err(anyhow::anyhow!(
                "unsupported coverage format at line {}: expected {}",
                i + 1,
                COVERAGE_FORMAT
            ));
        }
        hits.push(rec);
    }
    Ok(hits)
}
