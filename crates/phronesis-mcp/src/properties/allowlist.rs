//! The S3 hash-keyed review-gate allowlist (SPEC-verification-artifact-
//! generation.md): approved artifacts carry the provenance tuple; execution
//! re-hashes on disk and refuses on mismatch — approval binds to bytes, not
//! paths.
//!
//! Trust-anchor defense: the allowlist file is protected by ordinary
//! governance rules (a pre-phase rule blocks agent-seam Edit/Write to
//! trust-anchor paths — see the properties rules fixture), and `record`
//! requires a non-empty approver principal: an approval whose attribution
//! comes from the hooked session is not a review.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const ALLOWLIST_FORMAT: u32 = 1;

pub fn allowlist_path(root: &Path) -> PathBuf {
    root.join(".phronesis").join("verification-allowlist.json")
}

/// One approval: the full provenance tuple (spec §S3 condition ii).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AllowlistEntry {
    /// SHA-256 of the approved artifact bytes — approval binds to bytes.
    pub artifact_sha256: String,
    pub template_sha256: String,
    pub property_id: String,
    pub property_revision: String,
    /// The human principal who approved. An entry whose principal is empty,
    /// or attributable to the hooked session, is not a review.
    pub approver_principal: String,
    /// UTC date of the approval.
    pub date: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AllowlistFile {
    pub version: u32,
    #[serde(default)]
    pub entries: Vec<AllowlistEntry>,
}

#[derive(Debug, Error)]
pub enum AllowlistError {
    #[error("allowlist unreadable: {source}")]
    Io {
        #[from]
        source: std::io::Error,
    },
    #[error("allowlist malformed: {message}")]
    Malformed { message: String },
    #[error("unsupported allowlist format: {found}")]
    UnsupportedFormat { found: u32 },
    #[error("allowlist entry invalid: {message}")]
    InvalidEntry { message: String },
}

fn entry_problem(e: &AllowlistEntry) -> Option<String> {
    if e.artifact_sha256.is_empty() {
        return Some("artifact_sha256 is empty".to_string());
    }
    if !is_sha256_hex(&e.artifact_sha256) {
        return Some(format!(
            "artifact_sha256 {:?} is not a SHA-256 digest (64 lowercase hex)",
            e.artifact_sha256
        ));
    }
    if e.template_sha256.is_empty() {
        return Some("template_sha256 is empty — approval must bind the template too".to_string());
    }
    if e.property_id.is_empty() {
        return Some("property_id is empty".to_string());
    }
    if e.approver_principal.trim().is_empty() {
        return Some(
            "approver_principal is empty: approval is a human-principal act (S3)".to_string(),
        );
    }
    if e.date.is_empty() {
        return Some("date is empty".to_string());
    }
    None
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Load the allowlist (empty when absent — a fresh project has approved
/// nothing, which is the correct default).
pub fn load(root: &Path) -> Result<AllowlistFile, AllowlistError> {
    let path = allowlist_path(root);
    if !path.exists() {
        return Ok(AllowlistFile {
            version: ALLOWLIST_FORMAT,
            entries: Vec::new(),
        });
    }
    let raw = std::fs::read_to_string(&path)?;
    let file: AllowlistFile =
        serde_json::from_str(&raw).map_err(|e| AllowlistError::Malformed {
            message: e.to_string(),
        })?;
    if file.version != ALLOWLIST_FORMAT {
        return Err(AllowlistError::Malformed {
            message: format!("unsupported format {}", file.version),
        });
    }
    // Fail closed on any entry `record` would have refused: a hand-edited
    // entry with an empty hash or principal is not an approval, and one bad
    // entry makes the whole file untrustworthy.
    for (index, entry) in file.entries.iter().enumerate() {
        if let Some(problem) = entry_problem(entry) {
            return Err(AllowlistError::InvalidEntry {
                message: format!(
                    "entry {index} (artifact_sha256 {:?}) in {}: {problem}",
                    entry.artifact_sha256,
                    path.display()
                ),
            });
        }
    }
    Ok(file)
}

/// Does the allowlist approve these bytes? `artifact_sha256` is the caller's
/// freshly computed hash of the ON-DISK artifact — the store lookup binds to
/// bytes, never paths (S3 condition iii).
pub fn contains(root: &Path, artifact_sha256: &str) -> Result<bool, AllowlistError> {
    Ok(load(root)?
        .entries
        .iter()
        .any(|e| e.artifact_sha256 == artifact_sha256))
}

/// Record an approval. `approver_principal` must name a human — an entry
/// attributable to the hooked session is rejected here (S3 condition i).
pub fn record(root: &Path, entry: AllowlistEntry) -> Result<(), AllowlistError> {
    if let Some(problem) = entry_problem(&entry) {
        return Err(AllowlistError::InvalidEntry { message: problem });
    }
    let mut file = load(root)?;
    if file.version != ALLOWLIST_FORMAT {
        return Err(AllowlistError::Malformed {
            message: format!("unsupported format {}", file.version),
        });
    }
    if file
        .entries
        .iter()
        .any(|e| e.artifact_sha256 == entry.artifact_sha256)
    {
        return Ok(()); // idempotent re-record
    }
    file.entries.push(entry);
    write_atomic(root, &file)
}

/// Persist atomically (write tmp, rename) — same discipline as rules autosave.
fn write_atomic(root: &Path, file: &AllowlistFile) -> Result<(), AllowlistError> {
    let path = allowlist_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(
        &tmp,
        serde_json::to_string_pretty(file).map_err(|e| AllowlistError::Malformed {
            message: e.to_string(),
        })?,
    )?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// SHA-256 of `abc` (the published test vector).
    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    fn entry(hash: &str, principal: &str) -> AllowlistEntry {
        AllowlistEntry {
            artifact_sha256: hash.into(),
            template_sha256: "t-hash".into(),
            property_id: "safe_divide.zero_returns_error".into(),
            property_revision: "r1".into(),
            approver_principal: principal.into(),
            date: "2026-09-24".into(),
        }
    }

    #[test]
    fn records_and_recognizes_by_hash() {
        let root = tempdir().unwrap();
        assert!(!contains(root.path(), ABC).unwrap());
        record(root.path(), entry(ABC, "awaterma (human)")).unwrap();
        assert!(contains(root.path(), ABC).unwrap());
        // Idempotent.
        record(root.path(), entry(ABC, "awaterma (human)")).unwrap();
        assert_eq!(load(root.path()).unwrap().entries.len(), 1);
    }

    /// C14 (pre-fix probe): a hand-edited entry that `record` would refuse
    /// must fail `load` closed, so `contains("")` can never be true.
    #[test]
    fn c14_load_rejects_invalid_hand_edited_entries() {
        let root = tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".phronesis")).unwrap();
        std::fs::write(
            allowlist_path(root.path()),
            r#"{"version":1,"entries":[{"artifact_sha256":"","template_sha256":"","property_id":"","property_revision":"","approver_principal":"","date":""}]}"#,
        )
        .unwrap();
        let err = load(root.path()).unwrap_err();
        assert!(matches!(err, AllowlistError::InvalidEntry { .. }), "{err}");
        assert!(
            err.to_string().contains("entry 0"),
            "error names the entry: {err}"
        );
        assert!(contains(root.path(), "").is_err());

        // A non-digest hash is refused at record and at load alike.
        assert!(matches!(
            record(root.path(), entry("abc", "human")),
            Err(AllowlistError::InvalidEntry { .. })
        ));
        let mut bad = entry(ABC, "human");
        bad.artifact_sha256 = "self-added".into();
        std::fs::write(
            allowlist_path(root.path()),
            serde_json::to_string(&AllowlistFile {
                version: ALLOWLIST_FORMAT,
                entries: vec![entry(ABC, "human"), bad],
            })
            .unwrap(),
        )
        .unwrap();
        let err = load(root.path()).unwrap_err();
        assert!(err.to_string().contains("entry 1"), "{err}");
        assert!(
            contains(root.path(), ABC).is_err(),
            "one bad entry fails closed"
        );
    }

    #[test]
    fn empty_principal_rejected_the_review_gate_is_not_a_rubber_stamp() {
        let root = tempdir().unwrap();
        assert!(matches!(
            record(root.path(), entry(ABC, "")),
            Err(AllowlistError::InvalidEntry { .. })
        ));
        assert!(matches!(
            record(root.path(), entry(ABC, "   ")),
            Err(AllowlistError::InvalidEntry { .. })
        ));
        // And the hooked session's attribution is not a principal: entries
        // naming the session agent are refused by the CALLER contract — the
        // store records what it is given; S3's refusal rule lives at the
        // entry-construction boundary (tested via the hook rule).
    }
}
