//! Fixture verification: compare an adapter's extraction against a pinned
//! expected `Extraction` for each manifest-declared `(path, expected)` pair.
//! Fixtures are inputs and expected outputs — never executable commands, and
//! nothing here spawns a process or touches the filesystem outside the
//! pack's own declared assets.

use std::path::Path;

use super::error::PackError;
use super::extract::{ValidatedPack, extract};
use super::records::{ContextFile, Extraction, FileContext};

/// One fixture's verification outcome.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FixtureResult {
    pub path: String,
    pub expected: String,
    pub ok: bool,
    pub message: Option<String>,
}

/// Build the `FileContext` for a fixture source file: the nearest
/// manifest-declared context file is searched among the pack's own asset
/// tree, walking from the file's parent directory up to the pack root. This
/// mirrors the project-root search `language_packs::context` performs later,
/// scoped to the pack's own fixtures so authoring needs no real project.
fn fixture_context(pack: &ValidatedPack, file_rel: &str) -> FileContext {
    let manifest = &pack.assets.manifest;
    let mut context_files = Vec::new();
    'search: for name in &manifest.context_files {
        let mut dir = Path::new(file_rel).parent();
        loop {
            let dir_rel = dir
                .map(|d| d.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            let candidate = if dir_rel.is_empty() {
                name.clone()
            } else {
                format!("{dir_rel}/{name}")
            };
            if let Some(bytes) = pack.assets.files.get(&candidate)
                && let Ok(content) = String::from_utf8(bytes.clone())
            {
                context_files.push(ContextFile {
                    name: name.clone(),
                    file_rel: candidate,
                    directory_rel: dir_rel,
                    content,
                });
                continue 'search;
            }
            match dir {
                Some(d) if !d.as_os_str().is_empty() => dir = d.parent(),
                _ => break,
            }
        }
    }
    FileContext {
        file_rel: file_rel.to_string(),
        context_files,
    }
}

fn record_error(pack: &str, path: &str, message: String) -> PackError {
    PackError::Record {
        pack: Some(pack.to_string()),
        path: Some(path.to_string()),
        message,
    }
}

/// Run every fixture declared in `pack`'s manifest, returning one result per
/// fixture. Never executes anything beyond the bounded adapter path that
/// `extract` already runs under its own limits.
pub fn run_fixtures(pack: &ValidatedPack) -> Result<Vec<FixtureResult>, PackError> {
    let manifest = &pack.assets.manifest;
    let pack_id = manifest.id.clone();
    let mut results = Vec::with_capacity(manifest.fixtures.len());

    for fixture in &manifest.fixtures {
        let source_bytes = pack.assets.files.get(&fixture.path).ok_or_else(|| {
            record_error(
                &pack_id,
                &fixture.path,
                "declared fixture source is missing from pack assets".to_string(),
            )
        })?;
        let source = String::from_utf8(source_bytes.clone()).map_err(|_| {
            record_error(
                &pack_id,
                &fixture.path,
                "fixture source is not valid UTF-8".to_string(),
            )
        })?;
        let expected_bytes = pack.assets.files.get(&fixture.expected).ok_or_else(|| {
            record_error(
                &pack_id,
                &fixture.expected,
                "declared fixture expectation is missing from pack assets".to_string(),
            )
        })?;
        let expected: Extraction = serde_json::from_slice(expected_bytes).map_err(|e| {
            record_error(
                &pack_id,
                &fixture.expected,
                format!("expected fixture output is not a valid Extraction: {e}"),
            )
        })?;

        let context = fixture_context(pack, &fixture.path);
        let result = match extract(pack, &context, &source) {
            Ok(actual) if actual == expected => FixtureResult {
                path: fixture.path.clone(),
                expected: fixture.expected.clone(),
                ok: true,
                message: None,
            },
            Ok(actual) => FixtureResult {
                path: fixture.path.clone(),
                expected: fixture.expected.clone(),
                ok: false,
                message: Some(format!(
                    "extraction did not match expected fixture: expected {expected:?}, got {actual:?}"
                )),
            },
            Err(err) => FixtureResult {
                path: fixture.path.clone(),
                expected: fixture.expected.clone(),
                ok: false,
                message: Some(err.to_string()),
            },
        };
        results.push(result);
    }

    Ok(results)
}
