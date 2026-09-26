//! Project-defined Rhai fact providers for extensible RETE predicates.

use std::path::{Path, PathBuf};

#[cfg(feature = "rhai")]
use phr::Fact;
use phr::ReteNetwork;
use thiserror::Error;

const MAX_PROVIDER_BYTES: u64 = 64 * 1024;
const MAX_PROVIDERS: usize = 64;

#[derive(Debug, Clone, Default)]
pub struct ProviderEvent {
    pub phase: String,
    pub tool_name: String,
    pub file_path: String,
    /// `file_path` relative to the project root — the form the code graph
    /// keys files by, so provider facts can join graph facts on a path.
    pub file_rel: String,
    pub files: Vec<String>,
    pub old_content: String,
    pub new_content: String,
    pub command: String,
    pub output: String,
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("predicate provider directory could not be read: {0}")]
    Directory(#[source] std::io::Error),
    #[error("predicate provider {path} could not be read: {message}")]
    Read { path: String, message: String },
    #[error("predicate provider {path} failed: {message}")]
    Evaluation { path: String, message: String },
    #[error("predicate provider fact assertion failed: {0}")]
    Assertion(#[from] phr::ReteError),
    #[error("predicate providers require a binary built with the `rhai` feature")]
    RhaiDisabled,
    #[error("at most {MAX_PROVIDERS} predicate providers are allowed")]
    TooMany,
    #[error("invalid predicate provider name `{0}`")]
    InvalidName(String),
    #[error("predicate provider `{0}` already exists; pass replace=true to replace it")]
    Exists(String),
    #[error("predicate provider `{0}` was not found")]
    NotFound(String),
    #[error("predicate provider path escapes the project root: {0}")]
    UnsafePath(String),
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ProviderInfo {
    pub name: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TestFact {
    pub predicate: String,
    pub args: Vec<String>,
}

pub fn providers_dir(root: &Path) -> PathBuf {
    root.join(".phronesis").join("predicates")
}

/// Predicates the host asserts itself, which rules trust as ground truth —
/// named individually because each is one family's fixed vocabulary.
///
/// Every name here comes from a host fact source: hook content/diff facts
/// (`hook_facts.rs`, `hook/`), the cargo scanner, section context,
/// `clock_facts`, the outcome ledger (`outcomes/`), rule-layer override
/// provenance, and the `__script__` guard marker. The hydrated families
/// (coverage, properties, graph, ownership, AST syntax) and the capsule
/// predicates (`context_confidence_band`) are appended from
/// their own relation lists in [`reserved_predicates`], so a relation added
/// there is reserved without touching this list.
#[cfg(feature = "rhai")]
const RESERVED_EXACT: &[&str] = &[
    // Guard marker.
    "__script__",
    // Hook content and diff facts.
    "file_path",
    "new_content",
    "file_content",
    "hook_phase",
    "change_type",
    "file_path_matches",
    "file_extension_is",
    "new_content_contains",
    "bash_command_matches",
    "file_missing_pattern",
    "file_line_count_above",
    "function_added",
    "function_removed",
    "import_added",
    "import_removed",
    "test_exists_for",
    "no_test_for",
    "cargo_command_lacks_workspace",
    "markdown_rule",
    // Clock facts.
    "confidence_enabled",
    "business_hours_local",
    "weekday_local",
    "hour_local",
    // Outcome ledger: the grounded signals the confidence gate counts.
    "build_outcome",
    "test_outcome",
    "proof_outcome",
    "proof_run_outcome",
    "bug_check_outcome",
    // Store-integrity diagnostics.
    "store_corrupt",
    crate::rule_layers::OVERRIDE_PREDICATE,
];

/// Namespaces the host owns outright: it builds names under these at run
/// time, so a prefix is the only complete reservation, and a new `signal_*`
/// or `journey_*` fact is protected the day it ships. Every other host name
/// — including those under `store_`, `context_`, `confidence_`, `proof_`,
/// `property_`, `coverage_`, and `verification_` — is reserved by exact
/// name only, so a provider's own `store_opened` or `context_switch` (and
/// vocabularies such as this repository's `change_set_*`) stays available.
#[cfg(feature = "rhai")]
const RESERVED_PREFIXES: &[&str] = &["signal_", "journey_"];

/// The host-owned predicates a project provider may not emit (C17).
///
/// Providers are writable by the agent being governed (the
/// `add_predicate_provider` MCP tool), so a provider that could emit
/// `signal_pass` or `rule_overridden` would forge the evidence a gate rule
/// relies on. A provider that emits a reserved name fails its run: all of
/// its facts for that event are dropped and the hook fails closed.
#[cfg(feature = "rhai")]
pub fn reserved_predicates() -> phronesis_rhai::ReservedPredicates {
    phronesis_rhai::ReservedPredicates::new()
        .with_exact(RESERVED_EXACT.iter().copied())
        .with_exact(crate::coverage::hydrate::RELATIONS.iter().copied())
        .with_exact(crate::properties::hydrate::RELATIONS.iter().copied())
        .with_exact(crate::graph::hydrate::GRAPH_RELATIONS.iter().copied())
        .with_exact(crate::graph::ownership::OWNERSHIP_RELATIONS.iter().copied())
        .with_exact(crate::context::capsule::ALLOWED_PREDICATES.iter().copied())
        .with_exact(
            crate::syntax::facts::SyntaxFacts::PREDICATES
                .iter()
                .copied(),
        )
        .with_prefixes(RESERVED_PREFIXES.iter().copied())
}

#[cfg(feature = "rhai")]
fn provider_evaluator() -> phronesis_rhai::RhaiFactProvider {
    phronesis_rhai::RhaiFactProvider::with_reserved(reserved_predicates())
}

fn discover(root: &Path) -> Result<Vec<PathBuf>, ProviderError> {
    let dir = providers_dir(root);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    ensure_inside_root(root, &dir)?;
    let mut paths = std::fs::read_dir(&dir)
        .map_err(ProviderError::Directory)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rhai"))
        .collect::<Vec<_>>();
    paths.sort();
    if paths.len() > MAX_PROVIDERS {
        return Err(ProviderError::TooMany);
    }
    for path in &paths {
        ensure_inside_root(root, path)?;
    }
    Ok(paths)
}

fn ensure_inside_root(root: &Path, path: &Path) -> Result<(), ProviderError> {
    let canonical_root = root.canonicalize().map_err(|error| ProviderError::Read {
        path: root.display().to_string(),
        message: error.to_string(),
    })?;
    let canonical_path = path.canonicalize().map_err(|error| ProviderError::Read {
        path: path.display().to_string(),
        message: error.to_string(),
    })?;
    if !canonical_path.starts_with(canonical_root) {
        return Err(ProviderError::UnsafePath(path.display().to_string()));
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), ProviderError> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .chars()
            .all(|ch| ch == '-' || ch == '_' || ch.is_ascii_alphanumeric())
    {
        return Err(ProviderError::InvalidName(name.to_string()));
    }
    Ok(())
}

fn provider_path(root: &Path, name: &str) -> Result<PathBuf, ProviderError> {
    validate_name(name)?;
    Ok(providers_dir(root).join(format!("{name}.rhai")))
}

#[cfg(feature = "rhai")]
pub fn validate_script(script: &str) -> Result<(), ProviderError> {
    if script.len() as u64 > MAX_PROVIDER_BYTES {
        return Err(ProviderError::Read {
            path: "<script>".to_string(),
            message: format!("script exceeds {MAX_PROVIDER_BYTES} bytes"),
        });
    }
    provider_evaluator()
        .validate(script)
        .map_err(|message| ProviderError::Evaluation {
            path: "<script>".to_string(),
            message,
        })
}

#[cfg(not(feature = "rhai"))]
pub fn validate_script(_script: &str) -> Result<(), ProviderError> {
    Err(ProviderError::RhaiDisabled)
}

#[cfg(feature = "rhai")]
pub fn test_script(script: &str, event: &ProviderEvent) -> Result<Vec<TestFact>, ProviderError> {
    validate_script(script)?;
    provider_evaluator()
        .evaluate(
            script,
            &phronesis_rhai::FactProviderEvent {
                phase: event.phase.clone(),
                tool_name: event.tool_name.clone(),
                file_path: event.file_path.clone(),
                file_rel: event.file_rel.clone(),
                files: event.files.clone(),
                old_content: event.old_content.clone(),
                new_content: event.new_content.clone(),
                command: event.command.clone(),
                output: event.output.clone(),
            },
        )
        .map(|facts| {
            facts
                .into_iter()
                .map(|fact| TestFact {
                    predicate: fact.predicate,
                    args: fact.args,
                })
                .collect()
        })
        .map_err(|message| ProviderError::Evaluation {
            path: "<script>".to_string(),
            message,
        })
}

#[cfg(not(feature = "rhai"))]
pub fn test_script(_script: &str, _event: &ProviderEvent) -> Result<Vec<TestFact>, ProviderError> {
    Err(ProviderError::RhaiDisabled)
}

pub fn add(root: &Path, name: &str, script: &str, replace: bool) -> Result<PathBuf, ProviderError> {
    if script.len() as u64 > MAX_PROVIDER_BYTES {
        return Err(ProviderError::Read {
            path: "<script>".to_string(),
            message: format!("script exceeds {MAX_PROVIDER_BYTES} bytes"),
        });
    }
    validate_script(script)?;
    let path = provider_path(root, name)?;
    if path.exists() && !replace {
        return Err(ProviderError::Exists(name.to_string()));
    }
    let dir = providers_dir(root);
    std::fs::create_dir_all(&dir).map_err(ProviderError::Directory)?;
    ensure_inside_root(root, &dir)?;
    let temporary = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&temporary, script).map_err(|error| ProviderError::Read {
        path: temporary.display().to_string(),
        message: error.to_string(),
    })?;
    std::fs::rename(&temporary, &path).map_err(|error| ProviderError::Read {
        path: path.display().to_string(),
        message: error.to_string(),
    })?;
    Ok(path)
}

pub fn list(root: &Path) -> Result<Vec<ProviderInfo>, ProviderError> {
    discover(root)?
        .into_iter()
        .map(|path| {
            let metadata = std::fs::metadata(&path).map_err(|error| ProviderError::Read {
                path: path.display().to_string(),
                message: error.to_string(),
            })?;
            Ok(ProviderInfo {
                name: path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or_default()
                    .to_string(),
                bytes: metadata.len(),
            })
        })
        .collect()
}

pub fn get(root: &Path, name: &str) -> Result<String, ProviderError> {
    let path = provider_path(root, name)?;
    if !path.exists() {
        return Err(ProviderError::NotFound(name.to_string()));
    }
    ensure_inside_root(root, &path)?;
    let bytes = std::fs::metadata(&path)
        .map_err(|error| ProviderError::Read {
            path: path.display().to_string(),
            message: error.to_string(),
        })?
        .len();
    if bytes > MAX_PROVIDER_BYTES {
        return Err(ProviderError::Read {
            path: path.display().to_string(),
            message: format!("file exceeds {MAX_PROVIDER_BYTES} bytes"),
        });
    }
    std::fs::read_to_string(&path).map_err(|error| ProviderError::Read {
        path: path.display().to_string(),
        message: error.to_string(),
    })
}

pub fn remove(root: &Path, name: &str) -> Result<(), ProviderError> {
    let path = provider_path(root, name)?;
    if !path.exists() {
        return Err(ProviderError::NotFound(name.to_string()));
    }
    ensure_inside_root(root, &path)?;
    std::fs::remove_file(&path).map_err(|error| ProviderError::Read {
        path: path.display().to_string(),
        message: error.to_string(),
    })
}

pub async fn assert_facts(
    network: &ReteNetwork,
    root: &Path,
    event: &ProviderEvent,
) -> Result<usize, ProviderError> {
    let paths = discover(root)?;
    if paths.is_empty() {
        return Ok(0);
    }
    #[cfg(not(feature = "rhai"))]
    {
        let _ = (network, event);
        Err(ProviderError::RhaiDisabled)
    }
    #[cfg(feature = "rhai")]
    {
        let evaluator = provider_evaluator();
        let event = phronesis_rhai::FactProviderEvent {
            phase: event.phase.clone(),
            tool_name: event.tool_name.clone(),
            file_path: event.file_path.clone(),
            file_rel: event.file_rel.clone(),
            files: event.files.clone(),
            old_content: event.old_content.clone(),
            new_content: event.new_content.clone(),
            command: event.command.clone(),
            output: event.output.clone(),
        };
        let mut asserted = 0;
        for path in paths {
            let metadata =
                tokio::fs::metadata(&path)
                    .await
                    .map_err(|error| ProviderError::Read {
                        path: path.display().to_string(),
                        message: error.to_string(),
                    })?;
            if metadata.len() > MAX_PROVIDER_BYTES {
                return Err(ProviderError::Read {
                    path: path.display().to_string(),
                    message: format!("file exceeds {MAX_PROVIDER_BYTES} bytes"),
                });
            }
            let script =
                tokio::fs::read_to_string(&path)
                    .await
                    .map_err(|error| ProviderError::Read {
                        path: path.display().to_string(),
                        message: error.to_string(),
                    })?;
            let emitted = evaluator.evaluate(&script, &event).map_err(|message| {
                ProviderError::Evaluation {
                    path: path.display().to_string(),
                    message,
                }
            })?;
            let provider = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("provider");
            for (index, emitted) in emitted.into_iter().enumerate() {
                network
                    .assert_fact(Fact {
                        id: format!("provider:{provider}:{index}"),
                        predicate: emitted.predicate,
                        args: emitted.args,
                        timestamp: 0,
                        source: Some(format!("rhai:{provider}")),
                    })
                    .await?;
                asserted += 1;
            }
        }
        Ok(asserted)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::symlink;

    use tempfile::tempdir;

    use super::{ProviderError, list};

    #[test]
    fn list_rejects_provider_symlink_that_escapes_project_root() {
        let project = tempdir().expect("project tempdir");
        let outside = tempdir().expect("outside tempdir");
        let provider_dir = project.path().join(".phronesis/predicates");
        std::fs::create_dir_all(&provider_dir).expect("create provider directory");

        let outside_script = outside.path().join("escape.rhai");
        std::fs::write(&outside_script, "emit_fact(\"escaped\", []);")
            .expect("write outside provider");
        symlink(&outside_script, provider_dir.join("escape.rhai"))
            .expect("create provider symlink");

        let error = list(project.path()).expect_err("escaping symlink must be rejected");
        assert!(matches!(error, ProviderError::UnsafePath(_)));
    }
}

#[cfg(all(test, feature = "rhai"))]
mod reserved_tests {
    use super::{ProviderEvent, add, test_script, validate_script};

    #[test]
    fn test_script_rejects_a_forged_confidence_signal() {
        let error = test_script(
            "let s = \"signal_\" + \"pass\"; emit_fact(s, [\"unit\", \"tests\"]);",
            &ProviderEvent::default(),
        )
        .expect_err("host-owned predicate must be rejected at run time");
        assert!(error.to_string().contains("signal_pass"), "{error}");
    }

    #[test]
    fn add_rejects_a_literal_reserved_emit() {
        let project = tempfile::tempdir().expect("project tempdir");
        let error = add(
            project.path(),
            "forge",
            r#"emit_fact("rule_overridden", []);"#,
            false,
        )
        .expect_err("literal reserved emit must be rejected before it is written");
        assert!(error.to_string().contains("rule_overridden"), "{error}");
        assert!(
            !project
                .path()
                .join(".phronesis/predicates/forge.rhai")
                .exists()
        );
        for host_owned in [
            "journey_seen",
            "test_hits_region",
            "property_status",
            "defines_fn",
            "function_is_public",
            "confidence_enabled",
            "context_confidence_band",
            "store_corrupt",
            "new_content_contains",
            "coverage_revision",
            "coverage_stale",
            "property_obligation",
            "verification_result",
            "proof_outcome",
            "proof_run_outcome",
            "signal_anything_new",
            "journey_anything_new",
        ] {
            let script = format!("emit_fact(\"{host_owned}\", []);");
            assert!(
                validate_script(&script).is_err(),
                "{host_owned} must be reserved"
            );
        }
    }

    #[test]
    fn this_repos_change_set_provider_still_emits_its_facts() {
        let script = include_str!("../../../.phronesis/predicates/change_set.rhai");
        let facts = test_script(
            script,
            &ProviderEvent {
                phase: "pre".to_string(),
                tool_name: "apply_patch".to_string(),
                files: vec!["src/lib.rs".to_string()],
                ..ProviderEvent::default()
            },
        )
        .expect("change_set.rhai must pass the reserved-predicate check");
        let predicates: Vec<_> = facts.iter().map(|f| f.predicate.as_str()).collect();
        assert_eq!(
            predicates,
            [
                "change_set_production_rust",
                "change_set_has_production_rust",
                "change_set_production_without_test",
            ]
        );
    }

    #[test]
    fn provider_names_under_host_family_words_are_not_reserved() {
        // Only `signal_` and `journey_` are reserved as prefixes. A provider
        // already on disk that emits its own `store_*` / `context_*` / ...
        // name must keep working after upgrade: providers are never
        // re-validated, so an over-broad prefix would block every hook call.
        for own in [
            "store_opened",
            "context_switch",
            "confidence_note",
            "proof_reviewed",
            "property_touched",
            "coverage_wanted",
            "verification_pending",
        ] {
            let script = format!("emit_fact(\"{own}\", []);");
            validate_script(&script).unwrap_or_else(|e| panic!("{own}: {e}"));
            let facts = test_script(&script, &ProviderEvent::default())
                .unwrap_or_else(|e| panic!("{own}: {e}"));
            assert_eq!(facts.len(), 1, "{own}");
            assert_eq!(facts[0].predicate, own);
        }
    }
}
