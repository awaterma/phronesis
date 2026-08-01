use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const CONTEXT_CONFIG_FILENAME: &str = ".phronesis/context.json";

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ContextConfig {
    pub version: u8,
    pub hard_max_bytes: usize,
    #[serde(default)]
    pub estimated_max_tokens: Option<usize>,
    pub interaction: InteractionConfig,
    pub session: SessionConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InteractionConfig {
    pub kernel_max_bytes: usize,
    pub activity_reserve_bytes: usize,
    pub nudges_max_bytes: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SessionConfig {
    pub kernel_max_bytes: usize,
    pub state_reserve_bytes: usize,
    /// Ceiling for the session-level project document (`.phronesis/durable.md`).
    /// Defaulted so configuration written before the charter existed keeps
    /// loading rather than failing closed on an unknown-field error.
    #[serde(default = "default_charter_max_bytes")]
    pub charter_max_bytes: usize,
    pub rules_max_bytes: usize,
}

fn default_charter_max_bytes() -> usize {
    2048
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            version: 1,
            hard_max_bytes: 4096,
            estimated_max_tokens: Some(900),
            interaction: InteractionConfig {
                kernel_max_bytes: 768,
                activity_reserve_bytes: 1024,
                nudges_max_bytes: 1536,
            },
            session: SessionConfig {
                kernel_max_bytes: 768,
                state_reserve_bytes: 384,
                charter_max_bytes: default_charter_max_bytes(),
                rules_max_bytes: 2304,
            },
        }
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("context.json not found at {0}")]
    NotFound(String),
    #[error("context.json at {path}: io error: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("context.json at {path}: malformed: {source}")]
    Malformed {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("context.json at {path}: {message}")]
    Invalid { path: String, message: String },
}

pub fn path(root: &Path) -> PathBuf {
    root.join(CONTEXT_CONFIG_FILENAME)
}

pub fn load(root: &Path) -> Result<ContextConfig, ConfigError> {
    let requested = path(root);
    if !requested.exists() {
        return Err(ConfigError::NotFound(requested.display().to_string()));
    }
    let path = crate::security::resolve_safe_path(&requested.display().to_string(), root).map_err(
        |error| ConfigError::Invalid {
            path: requested.display().to_string(),
            message: error.to_string(),
        },
    )?;
    let text = std::fs::read_to_string(&path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            ConfigError::NotFound(path.display().to_string())
        } else {
            ConfigError::Io {
                path: path.display().to_string(),
                source,
            }
        }
    })?;
    let config: ContextConfig =
        serde_json::from_str(&text).map_err(|source| ConfigError::Malformed {
            path: path.display().to_string(),
            source,
        })?;
    validate(&config, &path)?;
    Ok(config)
}

fn validate(config: &ContextConfig, path: &Path) -> Result<(), ConfigError> {
    let invalid = |message: &str| ConfigError::Invalid {
        path: path.display().to_string(),
        message: message.to_string(),
    };
    if config.version != 1 {
        return Err(invalid("version must be 1"));
    }
    if config.hard_max_bytes == 0 || config.hard_max_bytes > 1024 * 1024 {
        return Err(invalid("hard_max_bytes must be between 1 and 1048576"));
    }
    if config.estimated_max_tokens == Some(0) {
        return Err(invalid(
            "estimated_max_tokens must be positive when present",
        ));
    }
    for (name, value) in [
        (
            "interaction.kernel_max_bytes",
            config.interaction.kernel_max_bytes,
        ),
        (
            "interaction.activity_reserve_bytes",
            config.interaction.activity_reserve_bytes,
        ),
        (
            "interaction.nudges_max_bytes",
            config.interaction.nudges_max_bytes,
        ),
        ("session.kernel_max_bytes", config.session.kernel_max_bytes),
        (
            "session.state_reserve_bytes",
            config.session.state_reserve_bytes,
        ),
        (
            "session.charter_max_bytes",
            config.session.charter_max_bytes,
        ),
        ("session.rules_max_bytes", config.session.rules_max_bytes),
    ] {
        if value > config.hard_max_bytes {
            return Err(invalid(&format!("{name} cannot exceed hard_max_bytes")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_spec() {
        let c = ContextConfig::default();
        assert_eq!(c.hard_max_bytes, 4096);
        assert_eq!(c.estimated_max_tokens, Some(900));
        assert_eq!(c.interaction.activity_reserve_bytes, 1024);
    }

    #[test]
    fn missing_is_distinct_from_malformed() {
        let d = tempfile::tempdir().unwrap();
        assert!(matches!(load(d.path()), Err(ConfigError::NotFound(_))));
        std::fs::create_dir_all(d.path().join(".phronesis")).unwrap();
        std::fs::write(path(d.path()), "{").unwrap();
        assert!(matches!(load(d.path()), Err(ConfigError::Malformed { .. })));
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".phronesis")).unwrap();
        let mut v = serde_json::to_value(ContextConfig::default()).unwrap();
        v.as_object_mut()
            .unwrap()
            .insert("surprise".into(), true.into());
        std::fs::write(path(d.path()), serde_json::to_vec(&v).unwrap()).unwrap();
        assert!(matches!(load(d.path()), Err(ConfigError::Malformed { .. })));
    }
}
