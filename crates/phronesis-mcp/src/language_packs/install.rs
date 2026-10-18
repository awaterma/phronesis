//! Validating and installing a pack directory into `.phronesis/packs/<id>/`.
//!
//! `validate_directory` runs the full pipeline (manifest, asset safety,
//! ownership, compilation, fixtures) without writing anything. `install`
//! reuses it, then atomically publishes only declared assets plus
//! `installed.json` digest metadata. Identical reinstall is a no-op; a
//! different digest under an existing id is refused (upgrade is deferred to
//! a later release). Pack files are authored configuration: `phr-mcp clean`
//! never removes `.phronesis/packs/`.

use std::fs;
use std::io::Write as _;
use std::path::Path;

use fs2::FileExt;
use serde::Serialize;

use super::assets::{PackAssets, digest_assets, read_assets};
use super::error::PackError;
use super::extract::compile_pack;
use super::fixtures::{FixtureResult, run_fixtures};
use super::limits;
use super::manifest::{check_ownership, validate_manifest};
use super::records::PackDiagnostic;
use super::registry::{
    INSTALLED_FILE, INSTALLED_SCHEMA_VERSION, InstalledMeta, NATIVE_EXTENSIONS, PACKS_DIR,
    manifests_for_install, read_installed_pack_assets,
};

#[derive(Debug, Clone, Serialize)]
pub struct ValidationReport {
    pub id: Option<String>,
    pub digest: Option<String>,
    pub ok: bool,
    pub diagnostics: Vec<PackDiagnostic>,
    pub fixtures: Vec<FixtureResult>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InstallReport {
    pub id: String,
    pub digest: String,
    pub action: InstallAction,
    pub dry_run: bool,
    pub location: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallAction {
    Installed,
    Unchanged,
}

/// `validate_directory`'s full result, plus (only when `report.ok`) the
/// exact validated [`PackAssets`] read from disk. `install` publishes these
/// bytes directly instead of re-reading the source directory a second time,
/// so what gets validated (manifest, ownership, compilation, fixtures) is
/// byte-for-byte what gets written — no window for the source to change
/// between validation and publication.
struct Validated {
    report: ValidationReport,
    assets: Option<PackAssets>,
}

fn validate_directory_with_assets(dir: &Path) -> Result<Validated, PackError> {
    let assets = read_assets(dir)?;
    let manifest = assets.manifest.clone();
    let mut diagnostics = Vec::new();

    if let Err(err) = validate_manifest(&manifest) {
        diagnostics.push(err.to_diagnostic());
    }
    if let Err(err) = check_ownership([&manifest], NATIVE_EXTENSIONS) {
        diagnostics.push(err.to_diagnostic());
    }

    let digest = digest_assets(&assets);
    if !diagnostics.is_empty() {
        return Ok(Validated {
            report: ValidationReport {
                id: Some(manifest.id),
                digest: Some(digest),
                ok: false,
                diagnostics,
                fixtures: Vec::new(),
            },
            assets: None,
        });
    }

    let pack = match compile_pack(assets.clone()) {
        Ok(pack) => pack,
        Err(err) => {
            return Ok(Validated {
                report: ValidationReport {
                    id: Some(manifest.id),
                    digest: Some(digest),
                    ok: false,
                    diagnostics: vec![err.to_diagnostic()],
                    fixtures: Vec::new(),
                },
                assets: None,
            });
        }
    };

    if let Err(err) = super::integration::validate_pack_resources(&pack) {
        return Ok(Validated {
            report: ValidationReport {
                id: Some(manifest.id),
                digest: Some(digest),
                ok: false,
                diagnostics: vec![err.to_diagnostic()],
                fixtures: Vec::new(),
            },
            assets: None,
        });
    }

    let fixtures = run_fixtures(&pack)?;
    let fixtures_ok = fixtures.iter().all(|f| f.ok);
    if !fixtures_ok {
        diagnostics.extend(fixtures.iter().filter(|f| !f.ok).map(|f| PackDiagnostic {
            pack: Some(manifest.id.clone()),
            path: Some(f.path.clone()),
            stage: "fixture".to_string(),
            message: f.message.clone().unwrap_or_default(),
        }));
    }

    Ok(Validated {
        report: ValidationReport {
            id: Some(manifest.id),
            digest: Some(digest),
            ok: fixtures_ok,
            diagnostics,
            fixtures,
        },
        assets: fixtures_ok.then_some(assets),
    })
}

/// Validate a pack source directory: manifest shape, asset safety,
/// ownership against native extensions, compilation, and every declared
/// fixture. Writes nothing; safe to call repeatedly on an author's own
/// working tree.
pub fn validate_directory(dir: &Path) -> Result<ValidationReport, PackError> {
    Ok(validate_directory_with_assets(dir)?.report)
}

/// Refuse a path whose target (or any ancestor under `root`, from
/// `root` down to and including `rel`) already exists as a symlink. Called
/// before any directory is created, so a symlinked `.phronesis`,
/// `.phronesis/packs`, or `.phronesis/packs/<id>` cannot redirect an install
/// outside the project — in a dry run this is a pure read, and in a real
/// install it runs again under the lock before any write.
fn ensure_no_symlink_components(root: &Path, rel: &Path) -> Result<(), PackError> {
    let mut current = root.to_path_buf();
    for component in rel.components() {
        current.push(component);
        if let Ok(meta) = fs::symlink_metadata(&current)
            && meta.file_type().is_symlink()
        {
            return Err(PackError::UnsafePath {
                pack: None,
                path: current.display().to_string(),
                reason: "is a symlink; refusing to install through it".to_string(),
            });
        }
    }
    Ok(())
}

/// Check whether `target` is an intact existing installation at `digest`.
/// `Ok(Some(report))` means no-op reinstall; `Ok(None)` means `target` does
/// not exist yet; `Err` covers both a different digest under the same id
/// (upgrade is deferred; remove first) and a corrupted/tampered existing
/// install (schema mismatch, digest mismatch, or an undeclared file) —
/// repair is explicit removal and reinstall, never a silent overwrite.
fn check_existing(
    target: &Path,
    id: &str,
    digest: &str,
    dry_run: bool,
) -> Result<Option<InstallReport>, PackError> {
    if !target.exists() {
        return Ok(None);
    }
    match read_installed_pack_assets(target) {
        Ok(existing_assets) => {
            let existing_digest = digest_assets(&existing_assets);
            if existing_digest == digest {
                return Ok(Some(InstallReport {
                    id: id.to_string(),
                    digest: digest.to_string(),
                    action: InstallAction::Unchanged,
                    dry_run,
                    location: target.display().to_string(),
                }));
            }
            Err(PackError::Duplicate {
                pack: Some(id.to_string()),
                field: "id".to_string(),
                value: format!(
                    "{id} is already installed with a different digest at {}; remove it before reinstalling",
                    target.display()
                ),
            })
        }
        Err(err) => Err(PackError::Manifest {
            pack: Some(id.to_string()),
            path: target.display().to_string(),
            field: "installed_pack".to_string(),
            message: format!(
                "the existing installation at {} is corrupted or was modified after install ({err}); \
                 remove it manually before reinstalling",
                target.display()
            ),
        }),
    }
}

/// Validate `dir`, then atomically publish it as `.phronesis/packs/<id>/`
/// under `root`. Staging happens outside the installed-directory
/// enumeration and is locked against concurrent installs; only validated
/// bytes are renamed into place. A dry run performs every check above
/// (including the symlink and existing-install checks) but creates no
/// directory, lock file, or pack content — it is a pure read.
pub fn install(root: &Path, dir: &Path, dry_run: bool) -> Result<InstallReport, PackError> {
    let validated = validate_directory_with_assets(dir)?;
    if !validated.report.ok {
        let first = validated
            .report
            .diagnostics
            .first()
            .map(|d| d.message.clone())
            .unwrap_or_else(|| "pack failed validation".to_string());
        return Err(PackError::Manifest {
            pack: validated.report.id,
            path: dir.display().to_string(),
            field: "install".to_string(),
            message: first,
        });
    }
    let id = validated
        .report
        .id
        .expect("a validated report names its pack id");
    let digest = validated
        .report
        .digest
        .expect("a validated report has a digest");
    let assets = validated
        .assets
        .expect("report.ok implies validated assets");

    let packs_rel = Path::new(PACKS_DIR);
    ensure_no_symlink_components(root, packs_rel)?;
    ensure_no_symlink_components(root, &packs_rel.join(&id))?;

    let packs_dir = root.join(PACKS_DIR);
    let target = packs_dir.join(&id);

    // Validate bounded registry ownership before returning, including for a
    // dry run or idempotent reinstall; both must see corrupt entries.
    let (mut manifests, installed_others) = manifests_for_install(root, &id)?;
    manifests.push(assets.manifest.clone());
    check_ownership(manifests.iter(), NATIVE_EXTENSIONS)?;
    if let Some(report) = check_existing(&target, &id, &digest, dry_run)? {
        return Ok(report);
    }
    if installed_others >= limits::MAX_INSTALLED_PACKS {
        return Err(PackError::Limit {
            pack: Some(id.clone()),
            path: Some(PACKS_DIR.into()),
            what: "installed packs".into(),
            max: limits::MAX_INSTALLED_PACKS,
        });
    }

    if dry_run {
        return Ok(InstallReport {
            id,
            digest,
            action: InstallAction::Installed,
            dry_run: true,
            location: target.display().to_string(),
        });
    }

    fs::create_dir_all(&packs_dir).map_err(|e| io_err(&id, &packs_dir, e))?;
    // Recheck path components after creation. This path-based check does not
    // make directory creation descriptor-confined against a concurrent
    // replacement of an ancestor.
    ensure_no_symlink_components(root, packs_rel)?;
    ensure_no_symlink_components(root, &packs_rel.join(&id))?;

    let lock_path = packs_dir.join(".install.lock");
    let lock_file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|e| io_err(&id, &lock_path, e))?;
    lock_file
        .lock_exclusive()
        .map_err(|e| io_err(&id, &lock_path, e))?;

    // Check extension ownership against every other installed/built-in pack
    // under the same lock, so two concurrent installs cannot both claim the
    // same extension by each only checking against native extensions.
    let (mut manifests, installed_others) = manifests_for_install(root, &id)?;
    manifests.push(assets.manifest.clone());
    check_ownership(manifests.iter(), NATIVE_EXTENSIONS)?;
    if let Some(report) = check_existing(&target, &id, &digest, dry_run)? {
        return Ok(report);
    }
    if installed_others >= limits::MAX_INSTALLED_PACKS {
        return Err(PackError::Limit {
            pack: Some(id.clone()),
            path: Some(PACKS_DIR.into()),
            what: "installed packs".into(),
            max: limits::MAX_INSTALLED_PACKS,
        });
    }

    let staging = packs_dir.join(format!(".staging-{id}-{}", std::process::id()));
    if staging.exists() {
        fs::remove_dir_all(&staging).map_err(|e| io_err(&id, &staging, e))?;
    }
    fs::create_dir_all(&staging).map_err(|e| io_err(&id, &staging, e))?;
    for (rel, bytes) in &assets.files {
        let dest = staging.join(rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| io_err(&id, parent, e))?;
        }
        fs::write(&dest, bytes).map_err(|e| io_err(&id, &dest, e))?;
    }
    let meta = InstalledMeta {
        schema_version: INSTALLED_SCHEMA_VERSION,
        adapter_api: assets.manifest.adapter_api,
        id: id.clone(),
        version: assets.manifest.version.clone(),
        digest: digest.clone(),
    };
    let meta_path = staging.join(INSTALLED_FILE);
    let mut file = fs::File::create(&meta_path).map_err(|e| io_err(&id, &meta_path, e))?;
    let meta_json = serde_json::to_string_pretty(&meta).expect("InstalledMeta serializes");
    file.write_all(meta_json.as_bytes())
        .map_err(|e| io_err(&id, &meta_path, e))?;
    drop(file);

    if target.exists() {
        let _ = fs::remove_dir_all(&staging);
        return Err(PackError::Duplicate {
            pack: Some(id.clone()),
            field: "id".to_string(),
            value: format!("{} already exists", target.display()),
        });
    }
    fs::rename(&staging, &target).map_err(|e| io_err(&id, &target, e))?;

    Ok(InstallReport {
        id,
        digest,
        action: InstallAction::Installed,
        dry_run: false,
        location: target.display().to_string(),
    })
}

fn io_err(pack: &str, path: &Path, source: std::io::Error) -> PackError {
    PackError::Io {
        pack: Some(pack.to_string()),
        path: path.display().to_string(),
        source,
    }
}
