//! The S3 hash-keyed review-gate allowlist (SPEC-verification-artifact-
//! generation.md): approved artifacts carry the provenance tuple; execution
//! re-hashes on disk and refuses on mismatch — approval binds to bytes, not
//! paths.
//!
//! Trust-anchor defense: the allowlist file is protected by ordinary
//! governance rules (a pre-phase rule blocks agent-seam Edit/Write to
//! trust-anchor paths — shipped in the `llm` pack, see `init.rs`), and `record`
//! requires a non-empty approver principal: an approval whose attribution
//! comes from the hooked session is not a review.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const ALLOWLIST_FORMAT: u32 = 1;

pub fn allowlist_path(root: &Path) -> PathBuf {
    root.join(".phronesis").join("verification-allowlist.json")
}

/// Who approved an artifact (D10). `Human` is the S3 review; an entry
/// written before the field existed has none and loads as `Human` — the
/// explicit back-compat default. `AgentQuorum` is written only by
/// `phr-mcp verify approve --quorum` after the quorum rules and the host
/// checks pass, and its evidence never hydrates as human `verified`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalKind {
    #[default]
    Human,
    AgentQuorum,
}

impl PrincipalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::AgentQuorum => "agent_quorum",
        }
    }
}

/// One reviewer of an agent-quorum approval, copied from its review record.
/// Model and family are self-declared by whoever wrote the record — they
/// are not authenticated (SPEC C "Agent-verified evidence", honest limits).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuorumReviewer {
    pub model: String,
    pub family: String,
    pub verdict: String,
    /// SHA-256 of the review record's JSON line.
    pub record_sha256: String,
}

/// Outcomes of the host-run mechanical checks that gated the approval.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuorumChecks {
    /// Production functions the artifact structurally calls.
    pub reach: Vec<String>,
    /// Verifier status on the mutant (must be `failed`).
    pub mutation: String,
    /// Verifier status on the unmodified artifact (must be `passed`).
    pub baseline: String,
    /// Number of vacuity-sentinel sites; every one must have failed.
    pub sentinel_sites: usize,
    /// Confinement tier the checks ran under.
    pub tier: String,
}

/// The quorum behind an `AgentQuorum` entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuorumEvidence {
    /// The declared model family of the template/property author.
    pub author_family: String,
    pub reviewers: Vec<QuorumReviewer>,
    pub checks: QuorumChecks,
}

/// Normalized family/model comparison key: trimmed, lowercased.
pub fn family_key(value: &str) -> String {
    value.trim().to_lowercase()
}

/// The quorum rules over approving reviewers (SPEC C "Quorum rules" 2–4):
/// at least two, at least two distinct families, none sharing the author's
/// family. `None` when they hold.
pub fn quorum_rule_problem(author_family: &str, reviewers: &[(String, String)]) -> Option<String> {
    let author = family_key(author_family);
    if author.is_empty() {
        return Some("author family is empty".to_string());
    }
    if let Some((model, _)) = reviewers.iter().find(|(_, f)| family_key(f) == author) {
        return Some(format!(
            "reviewer {model:?} shares the author's family {author:?}: an author's family never reviews its own work"
        ));
    }
    if reviewers.len() < 2 {
        return Some(format!(
            "{} approving reviewer record(s); a quorum needs at least 2",
            reviewers.len()
        ));
    }
    let families: std::collections::BTreeSet<String> =
        reviewers.iter().map(|(_, f)| family_key(f)).collect();
    if families.len() < 2 {
        return Some(format!(
            "approving reviewers span {} model family ({}); a quorum needs at least 2 distinct families",
            families.len(),
            families.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    None
}

fn quorum_problem(q: &QuorumEvidence) -> Option<String> {
    if q.reviewers.iter().any(|r| r.verdict != "approve") {
        return Some("an agent_quorum entry lists a non-approve verdict".to_string());
    }
    let pairs: Vec<(String, String)> = q
        .reviewers
        .iter()
        .map(|r| (r.model.clone(), r.family.clone()))
        .collect();
    if let Some(problem) = quorum_rule_problem(&q.author_family, &pairs) {
        return Some(problem);
    }
    let c = &q.checks;
    if c.reach.is_empty()
        || c.mutation != "failed"
        || c.baseline != "passed"
        || c.sentinel_sites == 0
    {
        return Some("an agent_quorum entry's host checks did not all pass".to_string());
    }
    None
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
    /// Who approved (D10). Absent in pre-D10 files: loads as `Human`.
    #[serde(default)]
    pub principal_kind: PrincipalKind,
    /// The quorum and host-check outcomes; required for `AgentQuorum`,
    /// absent for `Human`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quorum: Option<QuorumEvidence>,
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
    match (e.principal_kind, &e.quorum) {
        (PrincipalKind::AgentQuorum, None) => {
            return Some("principal_kind agent_quorum without quorum evidence".to_string());
        }
        (PrincipalKind::AgentQuorum, Some(q)) => {
            if let Some(problem) = quorum_problem(q) {
                return Some(problem);
            }
        }
        (PrincipalKind::Human, Some(_)) => {
            return Some(
                "a human entry carries quorum evidence: principal kinds are never mixed"
                    .to_string(),
            );
        }
        (PrincipalKind::Human, None) => {}
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

/// The principal kind approving `artifact_sha256` for `property_id`, the
/// strongest when several entries match (`Human` over `AgentQuorum`).
/// `None` when nothing approves it.
pub fn approval_kind(
    file: &AllowlistFile,
    artifact_sha256: &str,
    property_id: &str,
) -> Option<PrincipalKind> {
    let kinds = file
        .entries
        .iter()
        .filter(|e| e.artifact_sha256 == artifact_sha256 && e.property_id == property_id)
        .map(|e| e.principal_kind);
    let mut best = None;
    for kind in kinds {
        if kind == PrincipalKind::Human {
            return Some(kind);
        }
        best = Some(kind);
    }
    best
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
            principal_kind: PrincipalKind::Human,
            quorum: None,
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

    /// D10 back-compat: an entry written before `principal_kind` existed
    /// loads as `Human` — explicitly, not by accident.
    #[test]
    fn a_pre_d10_entry_loads_as_human() {
        let root = tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".phronesis")).unwrap();
        std::fs::write(
            allowlist_path(root.path()),
            format!(
                r#"{{"version":1,"entries":[{{"artifact_sha256":"{ABC}","template_sha256":"t","property_id":"p","property_revision":"r","approver_principal":"a human","date":"2026-09-24"}}]}}"#
            ),
        )
        .unwrap();
        let file = load(root.path()).unwrap();
        assert_eq!(file.entries[0].principal_kind, PrincipalKind::Human);
        assert_eq!(approval_kind(&file, ABC, "p"), Some(PrincipalKind::Human));
    }

    /// An entry claiming `agent_quorum` without a valid quorum, or a human
    /// entry carrying one, fails the whole file closed.
    #[test]
    fn principal_kind_and_quorum_evidence_must_agree() {
        let root = tempdir().unwrap();
        let mut claimed = entry(ABC, "agent_quorum:x+y");
        claimed.principal_kind = PrincipalKind::AgentQuorum;
        assert!(matches!(
            record(root.path(), claimed.clone()),
            Err(AllowlistError::InvalidEntry { .. })
        ));
        let quorum = QuorumEvidence {
            author_family: "author".into(),
            reviewers: vec![
                QuorumReviewer {
                    model: "m1".into(),
                    family: "one".into(),
                    verdict: "approve".into(),
                    record_sha256: "1".repeat(64),
                },
                QuorumReviewer {
                    model: "m2".into(),
                    family: "one".into(),
                    verdict: "approve".into(),
                    record_sha256: "2".repeat(64),
                },
            ],
            checks: QuorumChecks {
                reach: vec!["f".into()],
                mutation: "failed".into(),
                baseline: "passed".into(),
                sentinel_sites: 1,
                tier: "raw".into(),
            },
        };
        claimed.quorum = Some(quorum.clone());
        assert!(
            record(root.path(), claimed.clone()).is_err(),
            "one family is not a quorum, even hand-written"
        );
        claimed.quorum.as_mut().unwrap().reviewers[1].family = "two".into();
        record(root.path(), claimed.clone()).unwrap();
        let mut human = entry(&"b".repeat(64), "a human");
        human.quorum = Some(quorum);
        assert!(record(root.path(), human).is_err());
        let file = load(root.path()).unwrap();
        assert_eq!(
            approval_kind(&file, ABC, "safe_divide.zero_returns_error"),
            Some(PrincipalKind::AgentQuorum)
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
