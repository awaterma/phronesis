//! Journey facts — durable, recomputed-per-call temporal predicates.
//!
//! See `docs/specs/SPEC-journey-facts.md`. The stateless hook stays stateless:
//! every invocation rebuilds the network and re-derives `journey_*` facts from
//! a bounded suffix of `.phronesis/journey/events.jsonl`. State lives on disk;
//! decay is the sliding window; determinism is a pure function of
//! (journal bytes, ts, sid).

pub mod derive;
pub mod journal;
pub mod tagger;

use std::path::Path;

/// Read-or-create the active session id at `.phronesis/journey/session`.
///
/// Single source of truth for the sid across hook (pre/post), CLI (`phr-mcp
/// journey`), and the `get_journey` MCP tool. Format matches the SPEC:
/// `s-YYYY-MM-DD-<6 hex>` where the hex is `(epoch_secs ^ pid) & 0xFFFFFF`
/// — enough entropy to distinguish concurrent sessions in the same second.
///
/// Idempotent: if the file exists with a non-empty body, that id is reused
/// (a session boundary is whatever the runtime calls SessionStart for — we
/// don't roll a new id mid-session). If the file is missing or empty, a
/// fresh id is generated and atomically written via `create_new`; if a
/// concurrent caller won the race (`AlreadyExists`), the file is re-read
/// and that id returned.
///
/// Infallible by contract: any IO error falls back to a deterministic
/// in-memory id (same format, just not persisted). Persistence is best-
/// effort because callers (CLI, MCP, hook) all need a sid even on a
/// read-only mount.
pub fn current_sid(project_root: &Path) -> String {
    use std::io::ErrorKind;
    use std::time::{SystemTime, UNIX_EPOCH};

    let path = project_root
        .join(".phronesis")
        .join("journey")
        .join("session");

    // Fast path: already stamped.
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let trimmed = existing.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let hex: u32 = (ts as u32) ^ std::process::id();
    let date = crate::audit::short_iso_date(ts);
    let sid = format!("s-{}-{:06x}", date, hex & 0x00FF_FFFF);

    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // Atomic create-on-miss: `create_new` races safely against a sibling
    // hook/CLI process. If we lose the race, re-read what the winner wrote.
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut f) => {
            use std::io::Write;
            let _ = f.write_all(sid.as_bytes());
            sid
        }
        Err(e) if e.kind() == ErrorKind::AlreadyExists => std::fs::read_to_string(&path)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or(sid),
        Err(_) => sid,
    }
}

/// Loader error for `.phronesis/journey.json` — separates "not present" from
/// "present but malformed" so the hook can fail-open in both cases while
/// surfacing a useful stderr line for the malformed case.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("journey.json not found at {0}")]
    NotFound(String),
    #[error("journey.json at {path}: io error: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("journey.json at {path}: malformed: {source}")]
    Malformed {
        path: String,
        #[source]
        source: serde_json::Error,
    },
}

/// Load `.phronesis/journey.json` into a `TaggerConfig`. The hook caller is
/// expected to `unwrap_or_else(|e| { ...; TaggerConfig::default() })` so a
/// missing or malformed config never blocks an edit (SPEC §Fail-open).
pub fn load_config(project_root: &Path) -> Result<tagger::TaggerConfig, ConfigError> {
    let path = project_root.join(".phronesis").join("journey.json");
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ConfigError::NotFound(path.display().to_string()));
        }
        Err(e) => {
            return Err(ConfigError::Io {
                path: path.display().to_string(),
                source: e,
            });
        }
    };
    serde_json::from_str::<tagger::TaggerConfig>(&raw).map_err(|e| ConfigError::Malformed {
        path: path.display().to_string(),
        source: e,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_config_not_found_returns_not_found_variant() {
        let dir = tempfile::tempdir().unwrap();
        let err = load_config(dir.path()).unwrap_err();
        assert!(matches!(err, ConfigError::NotFound(_)));
    }

    #[test]
    fn load_config_malformed_returns_malformed_variant() {
        let dir = tempfile::tempdir().unwrap();
        let phr = dir.path().join(".phronesis");
        std::fs::create_dir_all(&phr).unwrap();
        std::fs::write(phr.join("journey.json"), "{not json").unwrap();
        let err = load_config(dir.path()).unwrap_err();
        assert!(matches!(err, ConfigError::Malformed { .. }));
    }

    #[test]
    fn load_config_well_formed_parses() {
        let dir = tempfile::tempdir().unwrap();
        let phr = dir.path().join(".phronesis");
        std::fs::create_dir_all(&phr).unwrap();
        std::fs::write(
            phr.join("journey.json"),
            r#"{"version":1,"taggers":[{"tag":"auth","when":[]}],"modules":[]}"#,
        )
        .unwrap();
        let cfg = load_config(dir.path()).expect("parses");
        assert_eq!(cfg.taggers.len(), 1);
        assert_eq!(cfg.taggers[0].tag, "auth");
    }

    #[test]
    fn tagger_config_default_is_empty_v1() {
        let d: tagger::TaggerConfig = Default::default();
        assert_eq!(d.version, 1);
        assert!(d.taggers.is_empty());
        assert!(d.modules.is_empty());
    }

    #[test]
    fn current_sid_reads_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let journey = dir.path().join(".phronesis").join("journey");
        std::fs::create_dir_all(&journey).unwrap();
        std::fs::write(journey.join("session"), "s-pre-seeded").unwrap();
        let sid = current_sid(dir.path());
        assert_eq!(sid, "s-pre-seeded");
    }

    #[test]
    fn current_sid_creates_session_file_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let sid = current_sid(dir.path());
        assert!(sid.starts_with("s-"), "sid `{}` should be s-prefixed", sid);
        assert!(
            !sid.ends_with("-fallback"),
            "the placeholder must be gone: {}",
            sid
        );
        let parts: Vec<&str> = sid.splitn(5, '-').collect();
        assert_eq!(parts.len(), 5, "expected s-YYYY-MM-DD-<hex>, got {}", sid);
        assert_eq!(parts[4].len(), 6, "6 hex chars: {}", sid);
        assert!(parts[4].chars().all(|c| c.is_ascii_hexdigit()));

        // Persisted.
        let on_disk = std::fs::read_to_string(
            dir.path()
                .join(".phronesis")
                .join("journey")
                .join("session"),
        )
        .unwrap();
        assert_eq!(on_disk, sid);
    }

    #[test]
    fn current_sid_two_consecutive_calls_return_same_id() {
        // The collision the placeholder used to cause: two `current_sid`
        // calls on the same day must return the same id, not collapse to a
        // shared `s-YYYY-MM-DD-fallback`.
        let dir = tempfile::tempdir().unwrap();
        let first = current_sid(dir.path());
        let second = current_sid(dir.path());
        assert_eq!(first, second);
    }
}
