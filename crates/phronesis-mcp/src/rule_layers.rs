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

use phronesis_governance::composition::{
    LoadedLayer, RuleOrigin as GovRuleOrigin, RuleOverride as GovRuleOverride, compose,
    override_facts_at,
};

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
    #[error("language pack: {0}")]
    Pack(#[from] crate::language_packs::PackError),
}

impl LayerError {
    /// The file a human (or agent) has to edit to fix this error: the rules
    /// file that failed to parse, or `loader.json` for a layer-config error.
    pub fn failing_file(&self, project_root: &Path) -> PathBuf {
        match self {
            LayerError::Io { path, .. } | LayerError::Malformed { path, .. } => PathBuf::from(path),
            LayerError::Rules(error) => error
                .path()
                .map(PathBuf::from)
                .unwrap_or_else(|| config_path(project_root)),
            LayerError::Pack(error) => {
                crate::language_packs::repair::locate_broken_pack(project_root, error)
                    .unwrap_or_else(|| {
                        project_root.join(crate::language_packs::registry::PACKS_DIR)
                    })
            }
            _ => config_path(project_root),
        }
    }

    /// Whether this is an installed language pack failing to load, whose
    /// repair is the pack CLI rather than an edit.
    pub fn is_pack_failure(&self) -> bool {
        matches!(self, LayerError::Pack(_))
    }
}

/// Whether `target` (a tool's `file_path`, absolute or project-relative) is
/// the file whose load error is blocking every hook. Editing that file is
/// the one tool call a load error must not block, or an agent can never
/// repair it.
///
/// Only the project's own `.phronesis/rules.json` and `.phronesis/loader.json`
/// ever qualify. The failing file can be any layer path `loader.json` names,
/// and `loader.json` is itself agent-writable, so trusting the failing path
/// alone would let a layer entry pointing at `src/main.rs` open that source
/// file to unchecked edits.
pub fn is_repair_target(project_root: &Path, failing: &Path, target: &str) -> bool {
    if target.is_empty() {
        return false;
    }
    let target = Path::new(target);
    let target = if target.is_absolute() {
        target.to_path_buf()
    } else {
        project_root.join(target)
    };
    is_repairable(project_root, failing) && canonical(&target) == canonical(failing)
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Whether a load error in `failing` may be repaired through the hooks: only
/// the project's `.phronesis/rules.json` or `.phronesis/loader.json`.
pub fn is_repairable(project_root: &Path, failing: &Path) -> bool {
    let failing = canonical(failing);
    [
        rules_file::default_path(project_root),
        config_path(project_root),
    ]
    .iter()
    .any(|p| canonical(p) == failing)
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
    let registry = crate::language_packs::load(project_root)?;
    resolve_with_registry(project_root, &registry)
}

pub fn resolve_with_registry(
    project_root: &Path,
    registry: &crate::language_packs::PackRegistry,
) -> Result<ResolvedRules, LayerError> {
    let pack_layers = crate::language_packs::integration::pack_rule_layers(registry)?;
    // Resource validation belongs to the same strict operation snapshot;
    // malformed toolchains cannot silently disappear from hook governance.
    crate::language_packs::integration::pack_toolchains(registry)?;
    resolve_with_pack_layers(project_root, pack_layers)
}

/// Compose validated pack layers first, then project layers. No registry call
/// occurs below this point, so pack loading cannot recurse into rule loading.
/// The ordered later-wins merge itself is
/// [`phronesis_governance::composition::compose`]. Filesystem paths cross that
/// boundary only as opaque per-layer source IDs (`layer:N`): the exact
/// `RuleOrigin` for every loaded layer — arbitrary `OsString` paths included —
/// is kept in a local map keyed by its opaque ID, and the composed origins and
/// overrides are rehydrated from that map, so no path is ever round-tripped
/// through a lossy display string.
fn resolve_with_pack_layers(
    project_root: &Path,
    pack_layers: Vec<crate::language_packs::integration::PackRuleLayer>,
) -> Result<ResolvedRules, LayerError> {
    let config_file = config_path(project_root);
    let configured = config_file.exists();
    let project_layers = if configured {
        let content = std::fs::read_to_string(&config_file).map_err(|source| LayerError::Io {
            path: config_file.display().to_string(),
            source,
        })?;
        let config: LayersConfig =
            serde_json::from_str(&content).map_err(|source| LayerError::Malformed {
                path: config_file.display().to_string(),
                source,
            })?;
        validate(&config)?;
        config.layers
    } else {
        vec![default_project_layer()]
    };
    let mut loaded = Vec::new();
    let mut layer_origins: HashMap<String, RuleOrigin> = HashMap::new();
    let mut next_layer = 0usize;
    let mut register =
        |layer: String, path: PathBuf, decision: Option<String>, rules: Vec<DiskRule>| {
            let source_id = format!("layer:{next_layer}");
            next_layer += 1;
            let exact = RuleOrigin {
                layer: layer.clone(),
                path,
                decision: decision.clone(),
            };
            layer_origins.insert(source_id.clone(), exact);
            loaded.push(LoadedLayer {
                origin: GovRuleOrigin {
                    layer,
                    source_id,
                    decision,
                },
                rules,
            });
        };
    for layer in pack_layers {
        register(layer.name, layer.path, None, layer.rules);
    }
    for layer in project_layers {
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
        register(layer.name, path, layer.decision, file.rules);
    }
    let composed = compose(&loaded);
    // Composition only ever echoes back origins we handed it, so every
    // `source_id` resolves to the exact recorded layer origin (real
    // filesystem path, real decision) rather than a display reconstruction.
    let rehydrate = |origin: GovRuleOrigin| -> RuleOrigin {
        layer_origins
            .get(&origin.source_id)
            .cloned()
            .expect("every composed origin names a loaded layer")
    };
    let origins = composed
        .origins
        .into_iter()
        .map(|(rule_id, origin)| (rule_id, rehydrate(origin)))
        .collect();
    let overrides = composed
        .overrides
        .into_iter()
        .map(|o| RuleOverride {
            rule_id: o.rule_id,
            overridden: rehydrate(o.overridden),
            winner: rehydrate(o.winner),
        })
        .collect();
    Ok(ResolvedRules {
        rules: composed.rules,
        origins,
        overrides,
        configured,
    })
}

/// Without `loader.json`, the project's own `.phronesis/rules.json` is the
/// single optional default layer. The layer path stays project-relative so
/// `resolve_path` joins the project root exactly once: pre-joining it here
/// would double a relative root and mangle a non-UTF-8 one through `.display()`.
fn default_project_layer() -> LayerConfig {
    LayerConfig {
        name: "project".to_string(),
        path: rules_file::DEFAULT_RULES_RELATIVE_PATH.to_string(),
        decision: None,
        optional: true,
    }
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
    let gov_overrides: Vec<GovRuleOverride> = overrides
        .iter()
        .map(|item| GovRuleOverride {
            rule_id: item.rule_id.clone(),
            overridden: GovRuleOrigin {
                layer: item.overridden.layer.clone(),
                source_id: item.overridden.path.display().to_string(),
                decision: item.overridden.decision.clone(),
            },
            winner: GovRuleOrigin {
                layer: item.winner.layer.clone(),
                source_id: item.winner.path.display().to_string(),
                decision: item.winner.decision.clone(),
            },
        })
        .collect();
    override_facts_at(&gov_overrides, timestamp)
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
    /// Unix-only: a project root (or any layer path component) may contain
    /// arbitrary non-UTF-8 `OsString` bytes. Origins and overrides must carry the
    /// exact loaded `PathBuf` — never a `to_string_lossy` reconstruction — for
    /// both winners and shadowed rules, while `override_facts` keeps the legacy
    /// lossy display rendering. Linux only: macOS filesystems reject
    /// non-UTF-8 names outright, so such paths cannot exist there.
    #[cfg(all(unix, target_os = "linux"))]
    #[test]
    fn non_utf8_project_root_preserves_exact_origins() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let base = tempfile::tempdir().expect("tempdir");
        let root = base.path().join(OsStr::from_bytes(b"project-\xff-root"));
        assert!(root.as_os_str().as_bytes().contains(&0xff));
        std::fs::create_dir_all(root.join(".phronesis")).expect("mkdir");
        std::fs::create_dir_all(root.join("personal")).expect("mkdir");
        write_rules(&root.join(".phronesis/rules.json"), "same", 1);
        write_rules(&root.join("personal/rules.json"), "same", 9);
        std::fs::write(
        root.join(".phronesis/loader.json"),
        r#"{"version":1,"layers":[{"name":"project","path":".phronesis/rules.json"},{"name":"user","path":"personal/rules.json","decision":"ADR-user-policy"}]}"#,
    )
    .expect("config");

        let resolved = resolve(&root).expect("resolve");
        assert_eq!(resolved.rules.len(), 1);
        assert_eq!(resolved.rules[0].priority, 9);

        let project_rules = root.join(".phronesis/rules.json");
        let user_rules = root.join("personal/rules.json");
        let winner = resolved.origins.get("same").expect("winner origin");
        assert_eq!(winner.layer, "user");
        assert_eq!(winner.path, user_rules);
        assert_eq!(
            winner.path.as_os_str().as_bytes(),
            user_rules.as_os_str().as_bytes()
        );
        assert_eq!(winner.decision.as_deref(), Some("ADR-user-policy"));

        assert_eq!(resolved.overrides.len(), 1);
        let shadowed = &resolved.overrides[0];
        assert_eq!(shadowed.rule_id, "same");
        assert_eq!(shadowed.overridden.layer, "project");
        assert_eq!(shadowed.overridden.path, project_rules);
        assert_eq!(
            shadowed.overridden.path.as_os_str().as_bytes(),
            project_rules.as_os_str().as_bytes()
        );
        assert_eq!(shadowed.winner.path, user_rules);
        assert_eq!(shadowed.winner.decision.as_deref(), Some("ADR-user-policy"));

        // Provenance facts keep the original display output (lossy for
        // non-UTF-8 bytes) with the adapter's explicit timestamp.
        let facts = override_facts(&resolved.overrides);
        assert_eq!(facts[0].predicate, "rule_overridden");
        assert_eq!(facts[0].args[0], "same");
        assert_eq!(facts[0].args[1], "project");
        assert!(facts[0].args[2].contains('\u{FFFD}'));
        assert_eq!(facts[0].args[3], "user");
        assert!(facts[0].args[4].contains('\u{FFFD}'));
        assert_eq!(facts[0].args[5], "ADR-user-policy");
        assert!(facts[0].timestamp > 0);
    }

    #[test]
    fn default_layer_is_the_relative_rules_path() {
        let layer = default_project_layer();
        assert_eq!(layer.name, "project");
        assert_eq!(layer.path, rules_file::DEFAULT_RULES_RELATIVE_PATH);
        assert_eq!(layer.decision, None);
        assert!(layer.optional);
        // The relative path resolves to exactly `default_path`, joined once:
        // a pre-joined absolute string would double a relative root.
        let joined = resolve_path(Path::new("some-root"), &layer.path).expect("resolve");
        assert_eq!(joined, PathBuf::from("some-root/.phronesis/rules.json"));
    }

    #[test]
    fn missing_default_rules_file_resolves_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let resolved = resolve(dir.path()).expect("resolve");
        assert!(!resolved.configured);
        assert!(resolved.rules.is_empty());
        assert!(resolved.origins.is_empty());
        assert!(resolved.overrides.is_empty());
    }

    #[test]
    fn relative_project_root_loads_default_layer_once() {
        let temp_root = tempfile::Builder::new()
            .prefix("rule-layers-relative-root-check-")
            .tempdir_in(std::env::current_dir().expect("cwd"))
            .expect("tempdir in cwd");
        let dir_name = temp_root
            .path()
            .file_name()
            .expect("file name")
            .to_str()
            .expect("utf8 name")
            .to_owned();
        let root = PathBuf::from(&dir_name);
        std::fs::create_dir_all(root.join(".phronesis")).expect("mkdir");
        write_rules(&root.join(".phronesis/rules.json"), "local", 3);

        let resolved = resolve_with_pack_layers(Path::new(&dir_name), Vec::new()).expect("resolve");
        assert!(!resolved.configured);
        assert_eq!(resolved.rules.len(), 1);
        assert_eq!(resolved.rules[0].id, "local");
        assert_eq!(resolved.rules[0].priority, 3);
        let origin = resolved.origins.get("local").expect("origin");
        assert_eq!(origin.layer, "project");
        assert_eq!(origin.path, root.join(".phronesis/rules.json"));
    }

    /// The default (no `loader.json`) route must keep the exact non-UTF-8 root
    /// bytes in `RuleOrigin::path`: the layer path is the project-relative
    /// constant, so the join happens on the original `PathBuf`, never through a
    /// lossy display string. A missing default file stays an empty resolution.
    /// Linux only: macOS filesystems reject non-UTF-8 names outright.
    #[cfg(all(unix, target_os = "linux"))]
    #[test]
    fn non_utf8_project_root_default_layer_keeps_exact_origin() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let base = tempfile::tempdir().expect("tempdir");
        let root = base.path().join(OsStr::from_bytes(b"root-\xff"));
        assert!(root.as_os_str().as_bytes().contains(&0xff));
        std::fs::create_dir_all(root.join(".phronesis")).expect("mkdir");

        let empty = resolve(&root).expect("resolve without rules file");
        assert!(!empty.configured);
        assert!(empty.rules.is_empty());

        write_rules(&root.join(".phronesis/rules.json"), "local", 2);
        let resolved = resolve(&root).expect("resolve");
        assert!(!resolved.configured);
        assert_eq!(resolved.rules.len(), 1);
        assert_eq!(resolved.rules[0].id, "local");

        let expected = root.join(".phronesis/rules.json");
        let origin = resolved.origins.get("local").expect("origin");
        assert_eq!(origin.layer, "project");
        assert_eq!(
            origin.path.as_os_str().as_bytes(),
            expected.as_os_str().as_bytes()
        );
    }
}
