//! Confidence scoring — grounded build/test outcome facts.
//!
//! phronesis's other fact families describe *syntax* (diff_extract, syntax/)
//! or *time* (clock_facts). This family describes **grounded outcomes**: did
//! the suggested code compile, did the tests pass. The signals come from the
//! output of the slow-clock tool calls that run the toolchain (`cargo test`,
//! `cargo build`, …) — see `HookPayload.tool_output`.
//!
//! The design keeps the engine domain-neutral the same way `syntax/` does:
//! declarative toolchain defs (cargo built-in; project defs in
//! `.phronesis/toolchains.json`). Adding a pytest def generalizes
//! confidence scoring beyond Rust without touching a rule.
//!
//! See `docs/specs/SPEC-confidence-scoring.md` for the full design, including
//! how these facts become a discretized confidence band and gate the
//! done-claim / commit.

pub mod adapter;
pub mod bugs;
pub mod derive;
pub mod facts;
pub mod segment;
pub mod subject;
pub mod toolchain;

pub use adapter::{extract, handles};
pub use derive::{band, signals};
pub use facts::{Band, OutcomeFact, is_grounded_outcome_tag};

use std::path::Path;

/// Confidence scoring is **opt-in per project**: active only when
/// `.phronesis/confidence.json` exists (written by `init --packs confidence`).
/// Until then the ledger and gate stay dormant, so projects that haven't
/// enabled it see no behavior change and no `.phronesis/outcomes/` directory.
pub fn enabled(root: &Path) -> bool {
    root.join(".phronesis").join("confidence.json").exists()
}

/// A read-only confidence snapshot for the `confidence` CLI / a report tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfidenceReport {
    pub subject: String,
    pub band: Band,
    /// Names of the passed signals (`compile`, `tests`, `bug:<id>`).
    pub signals: Vec<String>,
}

/// Build the confidence report for `subject_override`, or the currently open
/// work unit when `None`. Returns `None` when there is no subject to report on.
pub fn report(root: &Path, subject_override: Option<&str>) -> Option<ConfidenceReport> {
    let subject = subject_override
        .map(str::to_string)
        .or_else(|| subject::current(root))?;
    let sigs = signals(root, &subject).unwrap_or_default();
    let names = sigs.iter().map(|f| f.args[1].clone()).collect();
    Some(ConfidenceReport {
        subject,
        band: Band::from_signal_count(sigs.len()),
        signals: names,
    })
}

/// Errors from the explicit-signal path (`phr-mcp signal`).
#[derive(Debug, thiserror::Error)]
pub enum SignalError {
    #[error("confidence scoring is not enabled here (no .phronesis/confidence.json)")]
    NotEnabled,
    #[error("unknown signal `{0}`; expected `compile` or `tests`")]
    UnknownSignal(String),
    #[error(transparent)]
    Subject(#[from] subject::SubjectError),
    #[error(transparent)]
    Journal(#[from] crate::journey::journal::JournalError),
}

/// Record a `compile` or `tests` outcome explicitly — the escape hatch for a
/// test runner no toolchain def recognizes, or a run that happened outside
/// the hook. Writes the same `outcome:*` journal tag the post-check hook
/// stamps, against the open work unit (minting one if none is open), so
/// `signals` / the commit gate see it exactly as a hook-captured run.
/// Returns the subject the signal was recorded against.
pub fn record_signal(root: &Path, name: &str, passed: bool) -> Result<String, SignalError> {
    if !enabled(root) {
        return Err(SignalError::NotEnabled);
    }
    let tag = match (name, passed) {
        ("compile", true) => Some("outcome:compile_ok"),
        ("compile", false) => Some("outcome:compile_error"),
        ("tests", true) => Some("outcome:test_pass"),
        ("tests", false) => Some("outcome:test_fail"),
        _ => None,
    }
    .ok_or_else(|| SignalError::UnknownSignal(name.to_string()))?;
    let subject_id = subject::open(root)?;
    let record = crate::journey::journal::JournalRecord {
        v: 1,
        ts: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        sid: crate::journey::current_sid(root),
        seq: crate::hook::seq::next_seq(root),
        tool: "phr-mcp".to_string(),
        path: "<signal>".to_string(),
        ext: None,
        module: None,
        tags: vec![tag.to_string()],
        subject: Some(subject_id.clone()),
        command_exit: None,
    };
    crate::journey::journal::append(root, &record)?;
    Ok(subject_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enable(root: &Path) {
        std::fs::create_dir_all(root.join(".phronesis")).unwrap();
        std::fs::write(root.join(".phronesis/confidence.json"), "{}").unwrap();
    }

    #[test]
    fn record_signal_requires_confidence_enabled() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            record_signal(dir.path(), "tests", true),
            Err(SignalError::NotEnabled)
        ));
    }

    #[test]
    fn record_signal_rejects_unknown_names() {
        let dir = tempfile::tempdir().unwrap();
        enable(dir.path());
        assert!(matches!(
            record_signal(dir.path(), "vibes", true),
            Err(SignalError::UnknownSignal(_))
        ));
    }

    #[test]
    fn record_signal_opens_a_unit_and_grounds_the_signal() {
        let dir = tempfile::tempdir().unwrap();
        enable(dir.path());
        let subject = record_signal(dir.path(), "tests", true).unwrap();
        assert_eq!(
            subject::current(dir.path()).as_deref(),
            Some(subject.as_str())
        );
        let r = report(dir.path(), None).unwrap();
        assert_eq!(r.signals, vec!["tests"]);
        // Latest wins: an explicit failure retracts the earlier pass.
        record_signal(dir.path(), "tests", false).unwrap();
        assert!(report(dir.path(), None).unwrap().signals.is_empty());
        record_signal(dir.path(), "compile", true).unwrap();
        assert_eq!(report(dir.path(), None).unwrap().signals, vec!["compile"]);
    }

    #[test]
    fn report_none_without_subject() {
        let dir = tempfile::tempdir().unwrap();
        assert!(report(dir.path(), None).is_none());
    }

    #[test]
    fn report_reflects_journal_signals() {
        use crate::journey::journal::{self, JournalRecord};
        let dir = tempfile::tempdir().unwrap();
        journal::append(
            dir.path(),
            &JournalRecord {
                v: 1,
                ts: 0,
                sid: "s-test".to_string(),
                seq: 1,
                tool: "Bash".to_string(),
                path: "<cmd>".to_string(),
                ext: None,
                module: None,
                tags: vec![
                    "outcome:compile_ok".to_string(),
                    "outcome:test_pass".to_string(),
                ],
                subject: Some("u".to_string()),
                command_exit: None,
            },
        )
        .unwrap();
        let r = report(dir.path(), Some("u")).expect("report");
        assert_eq!(r.subject, "u");
        assert_eq!(r.band, Band::Medium);
        assert_eq!(r.signals, vec!["compile", "tests"]);
    }
}
