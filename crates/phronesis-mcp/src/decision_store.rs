//! Local routing for explicit ADR commands. Automatic graph and hook discovery
//! deliberately do not consult this configuration: private ADRs are not policy.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::wiki;

pub const CONFIG_PATH: &str = ".phronesis/decision-store.json";

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Config {
    directory: PathBuf,
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("decision store I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid decision store configuration: {0}")]
    Json(#[from] serde_json::Error),
    #[error("decision store configuration must be a regular file of at most 64 KiB")]
    InvalidFile,
    #[error(
        "configured decision directory must be an existing absolute directory outside the project"
    )]
    InvalidDirectory,
    #[error("refusing a symbolic link at the local .phronesis configuration directory")]
    LinkedConfig,
    #[error(
        "{CONFIG_PATH} must be untracked and gitignored; add /.phronesis/decision-store.json to the project's local Git exclude file first"
    )]
    NotPrivate,
    #[error(
        "a different decision store is already configured; edit the ignored local configuration explicitly to change it"
    )]
    AlreadyConfigured,
}

fn config_path(root: &Path) -> Result<PathBuf, StoreError> {
    match fs::symlink_metadata(root.join(".phronesis")) {
        Ok(meta) if meta.file_type().is_symlink() => return Err(StoreError::LinkedConfig),
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
        _ => {}
    }
    Ok(root.join(CONFIG_PATH))
}

fn ensure_private(root: &Path) -> Result<(), StoreError> {
    let tracked = Command::new("git")
        .current_dir(root)
        .args(["ls-files", "--", CONFIG_PATH])
        .output()?;
    let ignored = Command::new("git")
        .current_dir(root)
        .args(["check-ignore", "--quiet", "--", CONFIG_PATH])
        .output()?;
    if !tracked.status.success() || !tracked.stdout.is_empty() || !ignored.status.success() {
        return Err(StoreError::NotPrivate);
    }
    Ok(())
}

fn external_directory(root: &Path, directory: &Path) -> Result<PathBuf, StoreError> {
    if !directory.is_absolute() || !directory.is_dir() {
        return Err(StoreError::InvalidDirectory);
    }
    let directory = directory.canonicalize()?;
    if directory.starts_with(root.canonicalize()?) {
        return Err(StoreError::InvalidDirectory);
    }
    Ok(directory)
}

/// Read only for an explicit decision or wiki-drift CLI invocation. Missing
/// config preserves default behavior; invalid or unavailable targets error.
pub fn configured_directory(root: &Path) -> Result<Option<PathBuf>, StoreError> {
    let path = root.join(CONFIG_PATH);
    let meta = match fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    config_path(root)?;
    if !meta.is_file() || meta.len() > 64 * 1024 {
        return Err(StoreError::InvalidFile);
    }
    ensure_private(root)?;
    let config: Config = serde_json::from_slice(&fs::read(path)?)?;
    external_directory(root, &config.directory).map(Some)
}

/// Configure an existing external checkout directory without Git mutations.
/// Existing identical configuration is idempotent; other content is preserved.
pub fn configure(root: &Path, directory: &Path) -> Result<PathBuf, StoreError> {
    let directory = external_directory(root, directory)?;
    let path = config_path(root)?;
    ensure_private(root)?;
    if let Some(existing) = configured_directory(root)? {
        return if existing == directory {
            Ok(directory)
        } else {
            Err(StoreError::AlreadyConfigured)
        };
    }
    fs::create_dir_all(root.join(".phronesis"))?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    let config = Config {
        directory: directory.clone(),
    };
    file.write_all(&serde_json::to_vec_pretty(&config)?)?;
    file.write_all(b"\n")?;
    Ok(directory)
}

/// Explicit overrides are relative to the project root. They take precedence
/// over local configuration, allowing an intentional project-local ADR.
pub fn override_directory(
    root: &Path,
    explicit: Option<&Path>,
) -> Result<Option<PathBuf>, StoreError> {
    match explicit {
        Some(path) => Ok(Some(root.join(path))),
        None => configured_directory(root),
    }
}

pub fn creation_directory(root: &Path, explicit: Option<&Path>) -> Result<PathBuf, StoreError> {
    if let Some(directory) = override_directory(root, explicit)? {
        return Ok(directory);
    }
    let canonical = wiki::canonical_decisions_dir(root);
    Ok(if canonical.is_dir() {
        canonical
    } else {
        wiki::legacy_decisions_dir(root)
    })
}
