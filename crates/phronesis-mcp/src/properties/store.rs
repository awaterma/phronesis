//! Durable property record store — `.phronesis/properties.json` (version-
//! controlled, reviewed intent) and the derived `.phronesis/property-results.jsonl`
//! sidecar (verifier results; gitignored, replace-per-revision like coverage).
//!
//! Spec: `docs/specs/SPEC-property-ontology.md` §1.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const PROPERTIES_FORMAT: u32 = 1;
/// Results sidecar format. v2 records carry the binding fields (`tier`,
/// `artifact_sha256`); v1 records still load but read as unbound legacy
/// evidence, never as a verification (D9).
pub const RESULTS_FORMAT: u32 = 2;
/// The v1 results format: loads, never binds.
pub const LEGACY_RESULTS_FORMAT: u32 = 1;

/// The closed result-status set (SPEC-verification-artifact-generation §S8).
pub const RESULT_STATUSES: &[&str] = &["passed", "failed", "inconclusive", "timeout", "unknown"];

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
///
/// A record is evidence only when it is *bound* (D9): it names the property,
/// the 40-hex commit the proof ran against, the confinement tier, and the
/// SHA-256 of the approved artifact bytes, and those match the curated
/// record and the allowlist. `properties::hydrate::binding` decides; an
/// unbound record hydrates as `unbound_evidence`, never `verification_result`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultRecord {
    pub v: u32,
    pub kind: String, // "verification_result"
    pub property: String,
    pub verifier: String,
    pub status: String, // passed | failed | inconclusive | timeout | unknown
    pub revision: String,
    pub tool: String,
    /// The confinement tier that ran (`ConfinementTier`, snake_case). Absent
    /// on v1 records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    /// SHA-256 (64 lowercase hex) of the artifact bytes that ran. Absent on
    /// v1 records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_sha256: Option<String>,
}

impl ResultRecord {
    /// A bound-shaped record: `tier` `sandbox_exec`, the given artifact hash.
    pub fn sample(
        property: &str,
        verifier: &str,
        status: &str,
        revision: &str,
        artifact_sha256: &str,
    ) -> Self {
        Self {
            v: RESULTS_FORMAT,
            kind: "verification_result".into(),
            property: property.into(),
            verifier: verifier.into(),
            status: status.into(),
            revision: revision.into(),
            tool: verifier.into(),
            tier: Some("sandbox_exec".into()),
            artifact_sha256: Some(artifact_sha256.into()),
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
    #[error("property-results.jsonl unreadable: {source}")]
    ResultsIo { source: std::io::Error },
    #[error("property-results.jsonl line {line}: {message}")]
    InvalidResult { line: usize, message: String },
    #[error("property-results.jsonl line {line}: unsupported format {found}")]
    UnsupportedResultFormat { line: usize, found: u32 },
}

impl PropertyStoreError {
    /// The stable reason code carried by `store_corrupt(properties, <reason>)`
    /// (the coverage store's vocabulary, SPEC-coverage-evidence §3.1).
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Io { .. } => "properties_unreadable",
            Self::Malformed { .. } => "invalid_properties",
            Self::UnsupportedFormat { .. } | Self::UnsupportedResultFormat { .. } => {
                "unsupported_format"
            }
            Self::InvalidRecord { .. } => "invalid_record",
            Self::ResultsIo { .. } => "results_unreadable",
            Self::InvalidResult { .. } => "invalid_result",
        }
    }
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

/// Record-level validation at load: the closed kind and status sets and the
/// field shapes. A failure makes the whole sidecar corrupt. Binding fields
/// are only shape-checked here (bounded, no control characters); whether a
/// record *binds* is a hydration decision — an unbound record still loads.
fn result_problem(rec: &ResultRecord) -> Option<String> {
    if rec.kind != "verification_result" {
        return Some(format!("kind {:?} is not verification_result", rec.kind));
    }
    if !RESULT_STATUSES.contains(&rec.status.as_str()) {
        return Some(format!(
            "status {:?} is outside the closed set {RESULT_STATUSES:?}",
            rec.status
        ));
    }
    if let Some(problem) = identifier_field_problem(&rec.property, "property") {
        return Some(problem);
    }
    for (value, field) in [(&rec.verifier, "verifier"), (&rec.tool, "tool")] {
        if let Some(problem) = string_field_problem(value, field) {
            return Some(problem);
        }
    }
    // Possibly-empty binding fields: an empty value is an unbound record,
    // not a corrupt one.
    for (value, field) in [
        (Some(&rec.revision), "revision"),
        (rec.tier.as_ref(), "tier"),
        (rec.artifact_sha256.as_ref(), "artifact_sha256"),
    ] {
        if let Some(value) = value.filter(|v| !v.is_empty())
            && let Some(problem) = string_field_problem(value, field)
        {
            return Some(problem);
        }
    }
    None
}

/// Load the derived results sidecar (empty when absent). All-or-nothing: one
/// malformed line, unsupported format, or out-of-set status makes the sidecar
/// corrupt — the caller treats that as no evidence (D8).
pub fn load_results(root: &Path) -> Result<Vec<ResultRecord>, PropertyStoreError> {
    let path = results_path(root);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(&path)
        .map_err(|source| PropertyStoreError::ResultsIo { source })?;
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
        if rec.v != RESULTS_FORMAT && rec.v != LEGACY_RESULTS_FORMAT {
            return Err(PropertyStoreError::UnsupportedResultFormat {
                line: i + 1,
                found: rec.v,
            });
        }
        if let Some(message) = result_problem(&rec) {
            return Err(PropertyStoreError::InvalidResult {
                line: i + 1,
                message,
            });
        }
        out.push(rec);
    }
    Ok(out)
}
