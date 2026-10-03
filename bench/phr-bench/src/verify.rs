use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

/// Write predictions in SWE-bench JSONL format: one instance per line.
/// Each line: `{"instance_id": ..., "model_name_or_path": "<run-id>", "model_patch": "<patch>"}`
pub fn write_predictions(records: &[(String, String)], out: &Path) -> Result<()> {
    let lines: Vec<String> = records
        .iter()
        .map(|(instance_id, patch)| {
            let obj = serde_json::json!({
                "instance_id": instance_id,
                "model_name_or_path": "unknown", // will be filled in by caller with run-id
                "model_patch": patch
            });
            serde_json::to_string(&obj).context("serialize prediction line")
        })
        .collect::<Result<_>>()?;

    let content = lines.join("\n");
    if !content.is_empty() {
        std::fs::write(out, content + "\n")
            .with_context(|| format!("write predictions to {}", out.display()))?;
    } else {
        std::fs::write(out, "")
            .with_context(|| format!("write empty predictions to {}", out.display()))?;
    }

    Ok(())
}

/// Parse the SWE-bench harness results.json report.
/// Format: `{"<instance_id>": {"resolved": <bool>}, ...}`
pub fn parse_harness_report(json: &str) -> Result<BTreeMap<String, bool>> {
    #[derive(Deserialize)]
    struct InstanceResult {
        resolved: Option<bool>,
    }

    let report: BTreeMap<String, InstanceResult> =
        serde_json::from_str(json).context("parse harness report JSON")?;

    let mut result = BTreeMap::new();
    for (instance_id, entry) in report {
        result.insert(instance_id, entry.resolved.unwrap_or(false));
    }

    Ok(result)
}

/// Apply resolved status from harness report to run records.
/// Loads each `runs/<instance_id>/<arm>/record.json`, updates `resolved` field,
/// revalidates, and writes back.
pub fn apply_resolved(records_dir: &Path, resolved: &BTreeMap<String, bool>) -> Result<()> {
    use crate::record::{validate, RunRecord};

    for entry in std::fs::read_dir(records_dir)
        .with_context(|| format!("read records directory {}", records_dir.display()))?
    {
        let entry = entry.context("read directory entry")?;
        let instance_dir = entry.path();
        if !instance_dir.is_dir() {
            continue;
        }

        let instance_id = instance_dir
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_owned())
            .context("invalid instance_id in records dir")?;

        for arm_entry in std::fs::read_dir(&instance_dir)
            .with_context(|| format!("read instance dir {}", instance_dir.display()))?
        {
            let arm_entry = arm_entry.context("read arm directory entry")?;
            let arm_dir = arm_entry.path();
            if !arm_dir.is_dir() {
                continue;
            }

            let record_path = arm_dir.join("record.json");
            if !record_path.exists() {
                continue;
            }

            let record_text = std::fs::read_to_string(&record_path)
                .with_context(|| format!("read record {}", record_path.display()))?;
            let mut record: RunRecord = serde_json::from_str(&record_text)
                .with_context(|| format!("parse record {}", record_path.display()))?;

            // Update resolved: if instance is in the report, use that value; else None
            record.resolved = resolved.get(&instance_id).copied();

            validate(&record).with_context(|| format!("validate record for {}", instance_id))?;

            let updated_text =
                serde_json::to_string_pretty(&record).context("serialize updated record")?;
            std::fs::write(&record_path, updated_text)
                .with_context(|| format!("write updated record to {}", record_path.display()))?;
        }
    }

    Ok(())
}
