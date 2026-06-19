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
}
