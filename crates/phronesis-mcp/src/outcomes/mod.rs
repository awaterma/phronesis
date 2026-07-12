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
//! `.phronesis/toolchains.json`). Adding a pytest adapter generalizes
//! confidence scoring beyond Rust without touching a rule.
//!
//! See `docs/specs/SPEC-confidence-scoring.md` for the full design, including
//! how these facts become a discretized confidence band and gate the
//! done-claim / commit.

pub mod adapter;
pub mod bugs;
pub mod derive;
pub mod facts;
pub mod subject;
pub mod toolchain;

pub use adapter::{extract, handles};
pub use derive::{band, signals};
pub use facts::{Band, OutcomeFact};

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

#[cfg(test)]
mod tests {
    use super::*;

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
            },
        )
        .unwrap();
        let r = report(dir.path(), Some("u")).expect("report");
        assert_eq!(r.subject, "u");
        assert_eq!(r.band, Band::Medium);
        assert_eq!(r.signals, vec!["compile", "tests"]);
    }
}
