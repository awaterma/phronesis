//! `set_property_status` (SPEC-property-ontology.md §4): the promotion act —
//! an MCP tool whose every invocation lands in `log.jsonl` (kind `mcp`) with
//! the property, old status, new status, and the required reason. Never
//! rule-driven: this is sketch §17's "observed ≠ intended" given teeth.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::properties::store::{Property, PropertyStatus, PropertyStoreError, properties_path};

#[derive(Debug, Error)]
pub enum SetPropertyStatusError {
    #[error("because is required: every status transition must name its reason")]
    MissingBecause,
    #[error("no such property: {id}")]
    NoSuchProperty { id: String },
    #[error("unsupported status: {found}")]
    UnsupportedStatus { found: String },
    #[error(transparent)]
    Store(#[from] PropertyStoreError),
    #[error("atomic write failed: {source}")]
    Io {
        #[from]
        source: std::io::Error,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusTransition {
    pub property_id: String,
    pub old_status: String,
    pub new_status: String,
    pub because: String,
}

fn status_name(s: &PropertyStatus) -> &'static str {
    match s {
        PropertyStatus::Observed => "observed",
        PropertyStatus::Candidate => "candidate",
        PropertyStatus::Corroborated => "corroborated",
        PropertyStatus::Accepted => "accepted",
        PropertyStatus::Verified => "verified",
        PropertyStatus::Rejected => "rejected",
        PropertyStatus::Superseded => "superseded",
    }
}

fn parse_status_name(s: &str) -> Option<PropertyStatus> {
    Some(match s {
        "observed" => PropertyStatus::Observed,
        "candidate" => PropertyStatus::Candidate,
        "corroborated" => PropertyStatus::Corroborated,
        "accepted" => PropertyStatus::Accepted,
        "verified" => PropertyStatus::Verified,
        "rejected" => PropertyStatus::Rejected,
        "superseded" => PropertyStatus::Superseded,
        _ => return None,
    })
}

/// Set the status of one property, writing atomically. Returns the old and
/// new status names so the caller journals them.
pub fn set_status(
    root: &Path,
    property_id: &str,
    new_status: PropertyStatus,
    because: &str,
) -> Result<(String, String), SetPropertyStatusError> {
    if because.trim().is_empty() {
        return Err(SetPropertyStatusError::MissingBecause);
    }
    let path = properties_path(root);
    let raw = std::fs::read_to_string(&path)?;
    let mut file: PropertyFile =
        serde_json::from_str(&raw).map_err(|e| PropertyStoreError::Malformed {
            message: e.to_string(),
        })?;
    let prop = file
        .properties
        .iter_mut()
        .find(|p| p.id == property_id)
        .ok_or_else(|| SetPropertyStatusError::NoSuchProperty {
            id: property_id.to_string(),
        })?;
    let old_name = status_name(&prop.status).to_string();
    let new_name = status_name(&new_status).to_string();
    prop.status = new_status;
    write_atomic(&path, &file)?;
    Ok((old_name, new_name))
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PropertyFile {
    pub version: u32,
    #[serde(default)]
    pub properties: Vec<Property>,
}

fn write_atomic(path: &PathBuf, file: &PropertyFile) -> Result<(), PropertyStoreError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(
        &tmp,
        serde_json::to_string_pretty(file).map_err(|e| PropertyStoreError::Malformed {
            message: e.to_string(),
        })?,
    )?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Append the audited transition to the action log (kind `mcp`).
pub fn journal_transition(
    root: &Path,
    property_id: &str,
    old_status: &str,
    new_status: &str,
    because: &str,
) {
    use std::io::Write;
    let path = root.join(".phronesis").join("log.jsonl");
    let entry = serde_json::json!({
        "ts": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        "kind": "mcp",
        "event": "set_property_status",
        "property": property_id,
        "old_status": old_status,
        "new_status": new_status,
        "because": because,
    });
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "{entry}");
    }
}

/// The handler body, minus MCP plumbing (the server.rs delegation pattern).
pub fn set_property_status_handler(
    root: &Path,
    property_id: &str,
    new_status_str: &str,
    because: &str,
) -> Result<serde_json::Value, SetPropertyStatusError> {
    let new_status =
        parse_status_name(new_status_str).ok_or(SetPropertyStatusError::UnsupportedStatus {
            found: new_status_str.to_string(),
        })?;
    let (old_name, new_name) = set_status(root, property_id, new_status, because)?;
    journal_transition(root, property_id, &old_name, &new_name, because);
    Ok(serde_json::json!({
        "property": property_id,
        "old_status": old_name,
        "new_status": new_name,
        "because": because,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write_props(root: &Path, body: &str) {
        std::fs::create_dir_all(root.join(".phronesis")).unwrap();
        std::fs::write(properties_path(root), body).unwrap();
    }

    const ONE: &str = r#"{"version":1,"properties":[{"id":"p1","subject":"s","kind":"postcondition","depends_on":["fn:s"],"source":"explicit_spec","status":"candidate"}]}"#;

    #[test]
    fn status_change_records_the_transition() {
        let root = tempdir().unwrap();
        write_props(root.path(), ONE);
        let out = set_property_status_handler(
            root.path(),
            "p1",
            "accepted",
            "human review: the proof is verified end-to-end",
        )
        .unwrap();
        assert_eq!(out["old_status"], "candidate");
        assert_eq!(out["new_status"], "accepted");
        let log = std::fs::read_to_string(root.path().join(".phronesis/log.jsonl")).unwrap();
        let entry: serde_json::Value = serde_json::from_str(log.lines().last().unwrap()).unwrap();
        assert_eq!(entry["kind"], "mcp");
        assert_eq!(entry["event"], "set_property_status");
    }

    #[test]
    fn missing_because_refuses_and_changes_nothing() {
        let root = tempdir().unwrap();
        write_props(root.path(), ONE);
        let before = std::fs::read_to_string(properties_path(root.path())).unwrap();
        assert!(set_property_status_handler(root.path(), "p1", "accepted", "  ").is_err());
        let after = std::fs::read_to_string(properties_path(root.path())).unwrap();
        assert_eq!(before, after, "nothing changes without a reason");
    }

    #[test]
    fn unknown_property_errors() {
        let root = tempdir().unwrap();
        write_props(root.path(), ONE);
        assert!(set_property_status_handler(root.path(), "nope", "accepted", "r").is_err());
    }
}
