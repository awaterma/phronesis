//! `set_property_status` (SPEC-property-ontology.md §4): the promotion act —
//! an MCP tool whose every invocation lands in `log.jsonl` (kind `mcp`) with
//! the property, old status, new status, and the required reason. Never
//! rule-driven: this is sketch §17's "observed ≠ intended" given teeth.

use std::io::Write;
use std::path::Path;

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::properties::store::{
    PROPERTIES_FORMAT, PropertiesFile, PropertyStatus, PropertyStoreError, load_properties,
    properties_path,
};

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
    #[error("transition not journaled to log.jsonl, so not committed: {message}")]
    Journal { message: String },
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

/// Set the status of one property. Returns the old and new status names.
///
/// The whole read-modify-write runs under an exclusive lock on a sibling
/// `properties.json.lock`, so concurrent transitions never lose an update.
/// The store is read through `load_properties` — the same all-or-nothing
/// validator every reader uses — so a store that validator rejects (an
/// unsupported version, a hostile id) is refused, never rewritten.
///
/// Journal before commit (SPEC-property-ontology.md §4: every invocation
/// lands in `log.jsonl`): the new store is staged in a uniquely named temp
/// file, the transition is journaled, and only then is the temp file renamed
/// over the store. A journal failure discards the staged file and returns an
/// error — no transition commits unaudited. The journal write ignores
/// `PHRONESIS_NO_ACTION_LOG` (`action_log::append_audit`).
///
/// The journal line is therefore a record of intent: a rename that fails
/// after it is followed by a `set_property_status_aborted` line, but a crash
/// in the window between the journal append and the rename leaves a
/// `set_property_status` line for a transition that never landed. That
/// window is accepted — the reverse order could commit a transition with no
/// record at all. `properties.json` is the source of truth for the current
/// status; the log is the audit trail of attempts. A staging file orphaned
/// by such a crash is removed by the next transition.
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
    // No store: nothing to transition, and no `.phronesis/` or lock file to
    // create as a side effect of a lookup that must fail.
    if !path.exists() {
        return Err(SetPropertyStatusError::NoSuchProperty {
            id: property_id.to_string(),
        });
    }
    let dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| root.to_path_buf());
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(dir.join("properties.json.lock"))?;
    // Released on drop (and by the OS on process exit) — no stuck lock.
    lock.lock_exclusive()?;
    remove_stale_staging_files(&dir);

    let mut properties = load_properties(root)?;
    let prop = properties
        .iter_mut()
        .find(|p| p.id == property_id)
        .ok_or_else(|| SetPropertyStatusError::NoSuchProperty {
            id: property_id.to_string(),
        })?;
    let old_name = status_name(&prop.status).to_string();
    let new_name = status_name(&new_status).to_string();
    prop.status = new_status;
    let file = PropertiesFile {
        version: PROPERTIES_FORMAT,
        properties,
    };
    let body = serde_json::to_string_pretty(&file).map_err(|e| PropertyStoreError::Malformed {
        message: e.to_string(),
    })?;

    let mut staged = tempfile::Builder::new()
        .prefix(STAGING_PREFIX)
        .suffix(".tmp")
        .tempfile_in(&dir)?;
    staged.write_all(body.as_bytes())?;
    staged.as_file().sync_all()?;
    // Dropping `staged` on any early return removes the temp file.
    journal_transition(root, property_id, &old_name, &new_name, because)?;
    if let Err(e) = staged.persist(&path) {
        // The journal already names a transition that did not land; say so.
        let _ = crate::action_log::append_audit(
            &crate::action_log::default_path(root),
            &crate::action_log::LogEntry::new("mcp", "set_property_status_aborted")
                .with("property", property_id)
                .with("error", e.error.to_string()),
        );
        return Err(e.error.into());
    }
    Ok((old_name, new_name))
}

/// Name prefix of the staging file a transition writes before the rename.
const STAGING_PREFIX: &str = ".properties.json.";

/// Best-effort removal of staging files a crashed transition left behind.
/// Called under the store lock, so no live transition owns one.
fn remove_stale_staging_files(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(STAGING_PREFIX) && name.ends_with(".tmp") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// The on-disk store shape (kept as an alias for API stability).
pub type PropertyFile = PropertiesFile;

/// Append the audited transition to the action log (kind `mcp`), through the
/// shared locked, rotating appender. Errors propagate: the caller must not
/// commit a transition whose journal line did not land.
pub fn journal_transition(
    root: &Path,
    property_id: &str,
    old_status: &str,
    new_status: &str,
    because: &str,
) -> Result<(), SetPropertyStatusError> {
    let entry = crate::action_log::LogEntry::new("mcp", "set_property_status")
        .with("property", property_id)
        .with("old_status", old_status)
        .with("new_status", new_status)
        .with("because", because);
    crate::action_log::append_audit(&crate::action_log::default_path(root), &entry).map_err(|e| {
        SetPropertyStatusError::Journal {
            message: e.to_string(),
        }
    })
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

    /// SPEC-B §4: every invocation lands in log.jsonl. A transition whose
    /// journal line cannot be written must not commit.
    #[test]
    fn journal_failure_refuses_and_leaves_the_store_unchanged() {
        let root = tempdir().unwrap();
        write_props(root.path(), ONE);
        // A directory where the log file should be: every append fails.
        std::fs::create_dir_all(root.path().join(".phronesis/log.jsonl")).unwrap();
        let before = std::fs::read_to_string(properties_path(root.path())).unwrap();
        let out = set_property_status_handler(root.path(), "p1", "accepted", "reviewed");
        assert!(out.is_err(), "an unjournaled transition must be refused");
        let after = std::fs::read_to_string(properties_path(root.path())).unwrap();
        assert_eq!(before, after, "no commit without the journal line");
    }

    /// The store `load_properties` rejects must not be rewritten (and so
    /// laundered into a new status) by the promotion act.
    #[test]
    fn a_store_load_properties_rejects_is_refused_unchanged() {
        let unsupported_version = ONE.replace(r#""version":1"#, r#""version":99"#);
        let hostile_id = ONE.replace(r#""id":"p1""#, r#""id":"p1\"x""#);
        for (label, body, id) in [
            ("version 99", unsupported_version.as_str(), "p1"),
            ("id with a quote", hostile_id.as_str(), "p1\"x"),
        ] {
            let root = tempdir().unwrap();
            write_props(root.path(), body);
            assert!(
                crate::properties::load_properties(root.path()).is_err(),
                "{label}: precondition — the loader rejects this store"
            );
            let out = set_property_status_handler(root.path(), id, "accepted", "reviewed");
            assert!(out.is_err(), "{label}: must refuse, got {out:?}");
            let after = std::fs::read_to_string(properties_path(root.path())).unwrap();
            assert_eq!(after, body, "{label}: store must be left byte-identical");
        }
    }

    /// Concurrent transitions on different properties must all land: the
    /// read-modify-write is serialized and each writer uses its own temp file.
    #[test]
    fn concurrent_transitions_do_not_lose_updates() {
        const N: usize = 12;
        for _round in 0..5 {
            let root = tempdir().unwrap();
            let props: Vec<String> = (0..N)
                .map(|i| {
                    format!(
                        r#"{{"id":"p{i}","subject":"s","kind":"postcondition","depends_on":["fn:s"],"source":"explicit_spec","status":"candidate"}}"#
                    )
                })
                .collect();
            write_props(
                root.path(),
                &format!(r#"{{"version":1,"properties":[{}]}}"#, props.join(",")),
            );
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(N));
            let handles: Vec<_> = (0..N)
                .map(|i| {
                    let root = root.path().to_path_buf();
                    let barrier = barrier.clone();
                    std::thread::spawn(move || {
                        barrier.wait();
                        set_property_status_handler(&root, &format!("p{i}"), "accepted", "r")
                            .map(|_| ())
                            .map_err(|e| e.to_string())
                    })
                })
                .collect();
            for h in handles {
                h.join().unwrap().expect("each transition succeeds");
            }
            let loaded = crate::properties::load_properties(root.path()).unwrap();
            let accepted = loaded
                .iter()
                .filter(|p| p.status == PropertyStatus::Accepted)
                .count();
            assert_eq!(accepted, N, "lost update: {loaded:?}");
            let log = std::fs::read_to_string(root.path().join(".phronesis/log.jsonl")).unwrap();
            assert_eq!(log.lines().count(), N, "every transition journaled once");
        }
    }

    /// No store at all: the unknown id is refused without creating
    /// `.phronesis/` or a lock file as a side effect.
    #[test]
    fn unknown_property_without_a_store_creates_nothing() {
        let root = tempdir().unwrap();
        let out = set_property_status_handler(root.path(), "nope", "accepted", "r");
        assert!(
            matches!(out, Err(SetPropertyStatusError::NoSuchProperty { .. })),
            "{out:?}"
        );
        assert!(!root.path().join(".phronesis").exists());
    }

    /// A staging file left by a crashed transition is removed by the next one
    /// (it runs under the same lock, so no live writer owns it).
    #[test]
    fn a_crashed_transitions_staging_file_is_cleaned_up() {
        let root = tempdir().unwrap();
        write_props(root.path(), ONE);
        let stale = root.path().join(".phronesis/.properties.json.crashed.tmp");
        std::fs::write(&stale, "partial").unwrap();
        let unrelated = root.path().join(".phronesis/.tmpOther");
        std::fs::write(&unrelated, "not ours").unwrap();
        set_property_status_handler(root.path(), "p1", "accepted", "r").unwrap();
        assert!(!stale.exists(), "stale staging file must be removed");
        assert!(unrelated.exists(), "only our own staging files are touched");
        let leftovers: Vec<_> = std::fs::read_dir(root.path().join(".phronesis"))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".properties.json."))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn unknown_property_errors() {
        let root = tempdir().unwrap();
        write_props(root.path(), ONE);
        assert!(set_property_status_handler(root.path(), "nope", "accepted", "r").is_err());
    }
}
