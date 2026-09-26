//! Durable property record store — `.phronesis/properties.json` (version-
//! controlled, reviewed intent) and the derived `.phronesis/property-results.jsonl`
//! sidecar (verifier results; gitignored, replace-per-revision like coverage).
//!
//! Spec: `docs/specs/SPEC-property-ontology.md` §1.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const PROPERTIES_FORMAT: u32 = 1;
pub const RESULTS_FORMAT: u32 = 1;

pub fn properties_path(root: &Path) -> PathBuf {
    root.join(".phronesis").join("properties.json")
}

pub fn results_path(root: &Path) -> PathBuf {
    root.join(".phronesis").join("property-results.jsonl")
}

/// Where the claim came from (spec §2 — closed set).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PropertySource {
    ExplicitSpec,
    ExistingVerifierContract,
    TestAssertion,
    Documentation,
    CodeInference,
    RuntimeObservation,
    AgentInference,
}

/// Promotion lifecycle status (spec §2 — closed set).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PropertyStatus {
    Observed,
    Candidate,
    Corroborated,
    Accepted,
    Verified,
    Rejected,
    Superseded,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Encoding {
    pub language: String,
    pub verifier: String,
    pub artifact: String,
}

/// The curated, version-controlled property record (spec §1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Property {
    pub id: String,
    pub subject: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guarantee: Option<String>,
    pub depends_on: Vec<String>,
    pub source: PropertySource,
    pub status: PropertyStatus,
    #[serde(default)]
    pub corroborated_by: Vec<String>,
    #[serde(default)]
    pub encodings: Vec<Encoding>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PropertiesFile {
    pub version: u32,
    #[serde(default)]
    pub properties: Vec<Property>,
}

/// One verifier result, from the derived sidecar. Result statuses carry the
/// three-state `unknown` discipline from `outcomes/toolchain.rs`: never a
/// silent pass (spec §2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultRecord {
    pub v: u32,
    pub kind: String, // "verification_result"
    pub property: String,
    pub verifier: String,
    pub status: String, // passed | failed | inconclusive | timeout | unknown
    pub revision: String,
    pub tool: String,
}

impl ResultRecord {
    pub fn sample(property: &str, verifier: &str, status: &str, revision: &str) -> Self {
        Self {
            v: RESULTS_FORMAT,
            kind: "verification_result".into(),
            property: property.into(),
            verifier: verifier.into(),
            status: status.into(),
            revision: revision.into(),
            tool: verifier.into(),
        }
    }
}

#[derive(Debug, Error)]
pub enum PropertyStoreError {
    #[error("properties.json unreadable: {source}")]
    Io {
        #[from]
        source: std::io::Error,
    },
    #[error("properties.json malformed: {message}")]
    Malformed { message: String },
    #[error("unsupported properties format: {found}")]
    UnsupportedFormat { found: u32 },
    #[error("property {id}: {message}")]
    InvalidRecord { id: String, message: String },
    #[error("results line {line}: {message}")]
    InvalidResult { line: usize, message: String },
}

/// Identifier-shaped field check (S5): the coverage charset only — quotes,
/// backslashes, and shell delimiters rejected at ingest, so injected content
/// can never reach a rendered artifact through an id.
fn identifier_field_problem(value: &str, field: &str) -> Option<String> {
    if let Some(problem) = string_field_problem(value, field) {
        return Some(problem);
    }
    if let Some(bad) = value
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '.' | '/' | '-')))
    {
        return Some(format!(
            "{field} carries non-identifier character {bad:?} (S5 field-class contract)"
        ));
    }
    None
}

/// Field-level string check; the caller maps the problem onto the record
/// error so field validation never produces a bare-`String` error path.
fn string_field_problem(value: &str, field: &str) -> Option<String> {
    if value.is_empty() {
        return Some(format!("{field} is empty"));
    }
    if value.len() > 256 {
        return Some(format!("{field} exceeds 256 bytes"));
    }
    if value.chars().any(|c| c.is_control()) {
        return Some(format!("{field} carries control characters"));
    }
    None
}

fn validate_property(p: &Property) -> Result<(), PropertyStoreError> {
    let invalid = |message: String| PropertyStoreError::InvalidRecord {
        id: p.id.clone(),
        message,
    };
    // Identifier-shaped fields get the tighter charset (S5 field-class contract).
    for (value, field) in [(&p.id, "id"), (&p.subject, "subject")] {
        if let Some(problem) = identifier_field_problem(value, field) {
            return Err(invalid(problem));
        }
    }
    if let Some(problem) = string_field_problem(&p.kind, "kind") {
        return Err(invalid(problem));
    }
    if p.depends_on.is_empty() {
        return Err(invalid(
            "depends_on must list at least one region".to_string(),
        ));
    }
    for region in &p.depends_on {
        if let Some(problem) = string_field_problem(region, "depends_on entry") {
            return Err(invalid(problem));
        }
    }
    for c in &p.corroborated_by {
        if let Some(problem) = string_field_problem(c, "corroborated_by entry") {
            return Err(invalid(problem));
        }
    }
    for e in &p.encodings {
        for (value, field) in [
            (&e.language, "encoding.language"),
            (&e.verifier, "encoding.verifier"),
            (&e.artifact, "encoding.artifact"),
        ] {
            if let Some(problem) = string_field_problem(value, field) {
                return Err(invalid(problem));
            }
        }
    }
    Ok(())
}

/// Read and validate the curated property record. All-or-nothing: one invalid
/// record fails the whole load.
pub fn load_properties(root: &Path) -> Result<Vec<Property>, PropertyStoreError> {
    let path = properties_path(root);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(&path)?;
    let file: PropertiesFile =
        serde_json::from_str(&raw).map_err(|e| PropertyStoreError::Malformed {
            message: e.to_string(),
        })?;
    if file.version != PROPERTIES_FORMAT {
        return Err(PropertyStoreError::UnsupportedFormat {
            found: file.version,
        });
    }
    for p in &file.properties {
        validate_property(p)?;
    }
    Ok(file.properties)
}

/// Load the derived results sidecar (empty when absent).
pub fn load_results(root: &Path) -> Result<Vec<ResultRecord>, PropertyStoreError> {
    let path = results_path(root);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(&path)?;
    let mut out = Vec::new();
    for (i, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let rec: ResultRecord =
            serde_json::from_str(line).map_err(|e| PropertyStoreError::InvalidResult {
                line: i + 1,
                message: e.to_string(),
            })?;
        if rec.v != RESULTS_FORMAT {
            return Err(PropertyStoreError::InvalidResult {
                line: i + 1,
                message: format!("unsupported format {}", rec.v),
            });
        }
        out.push(rec);
    }
    Ok(out)
}
