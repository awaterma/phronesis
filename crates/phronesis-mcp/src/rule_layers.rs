//! Layered rule-file resolution.
//!
//! A project opts in by creating `.phronesis/loader.json`. Layers are applied
//! in declaration order; a later rule with the same ID replaces the earlier
//! rule and produces a `rule_overridden` provenance fact.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use phr::Fact;
use serde::Deserialize;
use thiserror::Error;

use crate::rules_file::{self, DiskRule, RulesFileError};

pub const CONFIG_FILE: &str = "loader.json";
pub const OVERRIDE_PREDICATE: &str = "rule_overridden";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayersConfig {
    #[serde(default = "config_version")]
    pub version: u8,
    pub layers: Vec<LayerConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayerConfig {
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub decision: Option<String>,
    #[serde(default)]
    pub optional: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleOrigin {
    pub layer: String,
    pub path: PathBuf,
    pub decision: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleOverride {
    pub rule_id: String,
    pub overridden: RuleOrigin,
    pub winner: RuleOrigin,
}

#[derive(Debug, Clone)]
pub struct ResolvedRules {
    pub rules: Vec<DiskRule>,
    pub origins: HashMap<String, RuleOrigin>,
    pub overrides: Vec<RuleOverride>,
    pub configured: bool,
}

#[derive(Debug, Error)]
pub enum LayerError {
    #[error("could not read rule-layer config at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("rule-layer config at {path} is malformed: {source}")]
    Malformed {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("unsupported rule-layer config version {0}; expected 1")]
    Version(u8),
    #[error("rule-layer config must contain at least one layer")]
    Empty,
    #[error("rule-layer name must not be empty")]
    EmptyName,
    #[error("duplicate rule-layer name `{0}`")]
    DuplicateName(String),
    #[error("cannot expand `~` in layer path `{0}` because the home directory is unavailable")]
    HomeUnavailable(String),
    #[error("required rule layer `{name}` does not exist at {path}")]
    Missing { name: String, path: String },
    #[error(transparent)]
    Rules(#[from] RulesFileError),
}

fn config_version() -> u8 {
    1
}

pub fn config_path(project_root: &Path) -> PathBuf {
    project_root.join(".phronesis").join(CONFIG_FILE)
}

/// Resolve effective rules. Without `loader.json`, this is exactly the legacy
/// project-local `rules.json` behavior.
pub fn resolve(project_root: &Path) -> Result<ResolvedRules, LayerError> {
    let config_path = config_path(project_root);
    if !config_path.exists() {
        let path = rules_file::default_path(project_root);
        let file = rules_file::read(&path)?;
        let origin = RuleOrigin {
            layer: "project".to_string(),
            path,
            decision: None,
        };
        let origins = file
            .rules
            .iter()
            .map(|rule| (rule.id.clone(), origin.clone()))
            .collect();
        return Ok(ResolvedRules {
            rules: file.rules,
            origins,
            overrides: Vec::new(),
            configured: false,
        });
    }

    let content = std::fs::read_to_string(&config_path).map_err(|source| LayerError::Io {
        path: config_path.display().to_string(),
        source,
    })?;
    let config: LayersConfig =
        serde_json::from_str(&content).map_err(|source| LayerError::Malformed {
            path: config_path.display().to_string(),
            source,
        })?;
    validate(&config)?;

    let mut rules = Vec::<DiskRule>::new();
    let mut positions = HashMap::<String, usize>::new();
    let mut origins = HashMap::<String, RuleOrigin>::new();
    let mut overrides = Vec::new();

    for layer in config.layers {
        let path = resolve_path(project_root, &layer.path)?;
        if !path.exists() {
            if layer.optional {
                continue;
            }
            return Err(LayerError::Missing {
                name: layer.name,
                path: path.display().to_string(),
            });
        }
        let file = rules_file::read(&path)?;
        let origin = RuleOrigin {
            layer: layer.name,
            path,
            decision: layer.decision,
        };
        for rule in file.rules {
            if let Some(position) = positions.get(&rule.id).copied() {
                let overridden = origins
                    .get(&rule.id)
                    .cloned()
                    .expect("an indexed layered rule always has an origin");
                overrides.push(RuleOverride {
                    rule_id: rule.id.clone(),
                    overridden,
                    winner: origin.clone(),
                });
                rules[position] = rule.clone();
            } else {
                positions.insert(rule.id.clone(), rules.len());
                rules.push(rule.clone());
            }
            origins.insert(rule.id.clone(), origin.clone());
        }
    }

    Ok(ResolvedRules {
        rules,
        origins,
        overrides,
        configured: true,
    })
}

fn validate(config: &LayersConfig) -> Result<(), LayerError> {
    if config.version != 1 {
        return Err(LayerError::Version(config.version));
    }
    if config.layers.is_empty() {
        return Err(LayerError::Empty);
    }
    let mut names = std::collections::HashSet::new();
    for layer in &config.layers {
        if layer.name.trim().is_empty() {
            return Err(LayerError::EmptyName);
        }
        if !names.insert(layer.name.clone()) {
            return Err(LayerError::DuplicateName(layer.name.clone()));
        }
    }
    Ok(())
}

fn resolve_path(project_root: &Path, configured: &str) -> Result<PathBuf, LayerError> {
    if configured == "~" || configured.starts_with("~/") {
        let home =
            dirs::home_dir().ok_or_else(|| LayerError::HomeUnavailable(configured.to_string()))?;
        return Ok(if configured == "~" {
            home
        } else {
            home.join(&configured[2..])
        });
    }
    let path = PathBuf::from(configured);
    Ok(if path.is_absolute() {
        path
    } else {
        project_root.join(path)
    })
}

pub fn override_facts(overrides: &[RuleOverride]) -> Vec<Fact> {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    overrides
        .iter()
        .enumerate()
        .map(|(index, item)| Fact {
            id: format!("__rule_override__:{index}:{}", item.rule_id),
            predicate: OVERRIDE_PREDICATE.to_string(),
            args: vec![
                item.rule_id.clone(),
                item.overridden.layer.clone(),
                item.overridden.path.display().to_string(),
                item.winner.layer.clone(),
                item.winner.path.display().to_string(),
                item.winner.decision.clone().unwrap_or_default(),
            ],
            timestamp,
            source: Some("rule_layers".to_string()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_rules(path: &Path, id: &str, priority: i32) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(
            path,
            format!(
                r#"{{"rules":[{{"id":"{id}","priority":{priority},"phase":"pre","when":[{{"file_path_matches":"src/"}}],"then":{{"block":"test"}}}}]}}"#
            ),
        )
        .expect("rules");
    }

    #[test]
    fn later_layer_overrides_and_emits_provenance() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_rules(&dir.path().join(".phronesis/rules.json"), "same", 1);
        write_rules(&dir.path().join("personal/rules.json"), "same", 9);
        std::fs::write(
            dir.path().join(".phronesis/loader.json"),
            r#"{"version":1,"layers":[{"name":"project","path":".phronesis/rules.json"},{"name":"user","path":"personal/rules.json","decision":"ADR-user-policy"}]}"#,
        )
        .expect("config");

        let resolved = resolve(dir.path()).expect("resolve");
        assert_eq!(resolved.rules.len(), 1);
        assert_eq!(resolved.rules[0].priority, 9);
        assert_eq!(resolved.overrides.len(), 1);
        let facts = override_facts(&resolved.overrides);
        assert_eq!(facts[0].predicate, "rule_overridden");
        assert_eq!(facts[0].args[0], "same");
        assert_eq!(facts[0].args[1], "project");
        assert_eq!(facts[0].args[3], "user");
        assert_eq!(facts[0].args[5], "ADR-user-policy");
    }

    #[test]
    fn missing_optional_layer_is_ignored() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_rules(&dir.path().join(".phronesis/rules.json"), "local", 1);
        std::fs::write(
            dir.path().join(".phronesis/loader.json"),
            r#"{"layers":[{"name":"project","path":".phronesis/rules.json"},{"name":"user","path":"missing.json","optional":true}]}"#,
        )
        .expect("config");
        let resolved = resolve(dir.path()).expect("resolve");
        assert_eq!(resolved.rules[0].id, "local");
    }

    #[test]
    fn missing_required_layer_fails_closed() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join(".phronesis")).expect("mkdir");
        std::fs::write(
            dir.path().join(".phronesis/loader.json"),
            r#"{"layers":[{"name":"team","path":"missing.json"}]}"#,
        )
        .expect("config");
        assert!(matches!(
            resolve(dir.path()),
            Err(LayerError::Missing { .. })
        ));
    }

    #[test]
    fn malformed_loader_fails_closed() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join(".phronesis")).expect("mkdir");
        std::fs::write(dir.path().join(".phronesis/loader.json"), "{bad").expect("config");
        assert!(matches!(
            resolve(dir.path()),
            Err(LayerError::Malformed { .. })
        ));
    }
}
