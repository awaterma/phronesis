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
    phronesis_rhai::RhaiFactProvider::new()
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
    phronesis_rhai::RhaiFactProvider::new()
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
        let evaluator = phronesis_rhai::RhaiFactProvider::new();
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
            let metadata = std::fs::metadata(&path).map_err(|error| ProviderError::Read {
                path: path.display().to_string(),
                message: error.to_string(),
            })?;
            if metadata.len() > MAX_PROVIDER_BYTES {
                return Err(ProviderError::Read {
                    path: path.display().to_string(),
                    message: format!("file exceeds {MAX_PROVIDER_BYTES} bytes"),
                });
            }
            let script = std::fs::read_to_string(&path).map_err(|error| ProviderError::Read {
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
