use anyhow::{anyhow, Context, Result};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::coverage::store::{write_store, CoverageIndex, HitRecord, COVERAGE_FORMAT};

#[derive(Debug)]
pub struct ImportSummary {
    pub records: usize,
    pub tests: usize,
    pub revision: String,
}

const MAX_EXPORT_SIZE: u64 = 5 * 1024 * 1024;

pub fn validate_record(rec: &HitRecord) -> Result<()> {
    if rec.v != COVERAGE_FORMAT {
        return Err(anyhow!("invalid format version: expected {}", COVERAGE_FORMAT));
    }
    if rec.kind != "hit" {
        return Err(anyhow!("invalid kind: expected 'hit'"));
    }
    validate_identifier_field(&rec.test, "test")?;
    validate_identifier_field(&rec.region, "region")?;
    if rec.file.starts_with('/') {
        return Err(anyhow!("file path must be repo-relative (no leading '/')"));
    }
    if rec.file.split('/').any(|c| c == "..") {
        return Err(anyhow!("file path must not contain '..'"));
    }
    if rec.file.is_empty() {
        return Err(anyhow!("file path must be non-empty"));
    }
    if rec.revision.len() != 40 || !rec.revision.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(anyhow!("revision must be exactly 40 hex chars"));
    }
    if rec.hit_kind != "region" && rec.hit_kind != "branch" {
        return Err(anyhow!("hit_kind must be 'region' or 'branch'"));
    }
    if rec.start_line > rec.end_line {
        return Err(anyhow!("start_line must be <= end_line"));
    }
    Ok(())
}

fn validate_identifier_field(field: &str, name: &str) -> Result<()> {
    if field.is_empty() {
        return Err(anyhow!("{name} must be non-empty"));
    }
    if field.len() > 256 {
        return Err(anyhow!("{name} must be <= 256 bytes"));
    }
    for c in field.chars() {
        if c.is_control() {
            return Err(anyhow!("{name} contains control characters"));
        }
        if !matches!(c, 'A'..='Z' | 'a'..='z' | '0'..='9' | '_' | ':' | '.' | '/' | '-') {
            return Err(anyhow!("{name} contains invalid character '{c}'"));
        }
    }
    Ok(())
}

/// All-or-nothing import: parse and validate every record first; only when
/// the whole export passes (including single-revision consistency) is the
/// store replaced (SPEC-coverage-evidence §8).
pub fn import_export(root: &Path, export_path: &Path, now_unix: u64) -> Result<ImportSummary> {
    let metadata = fs::metadata(export_path)
        .with_context(|| format!("export file not found: {}", export_path.display()))?;
    if metadata.len() > MAX_EXPORT_SIZE {
        return Err(anyhow!("export file exceeds {} byte limit", MAX_EXPORT_SIZE));
    }

    let file = File::open(export_path)?;
    let reader = BufReader::new(file);
    let mut records: Vec<HitRecord> = Vec::new();
    let mut revision: Option<String> = None;
    let mut test_names: HashSet<String> = HashSet::new();

    for (i, line_result) in reader.lines().enumerate() {
        let line_num = i + 1;
        let line = line_result.with_context(|| format!("failed to read export line {line_num}"))?;
        if line.trim().is_empty() {
            continue;
        }
        let rec: HitRecord = serde_json::from_str(&line)
            .with_context(|| format!("malformed JSON at export line {line_num}"))?;
        if let Err(e) = validate_record(&rec) {
            return Err(anyhow!("export line {line_num}: {e}"));
        }

        match &revision {
            None => revision = Some(rec.revision.clone()),
            Some(prev) if *prev != rec.revision => {
                return Err(anyhow!("export mixes revisions"));
            }
            _ => {}
        }

        test_names.insert(rec.test.clone());
        records.push(rec);
    }

    let revision = revision.ok_or_else(|| anyhow!("no records found in export"))?;
    let tool = records[0].tool.clone();

    let index = CoverageIndex {
        format: COVERAGE_FORMAT,
        revision: revision.clone(),
        imported_at: now_unix,
        tool,
    };
    write_store(root, &records, &index)?;

    Ok(ImportSummary {
        records: records.len(),
        tests: test_names.len(),
        revision,
    })
}