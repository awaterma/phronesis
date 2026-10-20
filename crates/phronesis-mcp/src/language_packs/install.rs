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
use std::path::{Path, PathBuf};

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

    let _lock = lock_pack_store(&packs_dir, &id)?;

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

/// Open and exclusively lock `.phronesis/packs/.install.lock`, the lock every
/// pack-store write holds. The lock is opened without following a symlink
/// in its final component (and without blocking on a FIFO), and must be a
/// regular file, so a planted link cannot make an install create or open a
/// file outside the project.
fn lock_pack_store(packs_dir: &Path, id: &str) -> Result<fs::File, PackError> {
    let lock_path = packs_dir.join(".install.lock");
    let unsafe_lock = |reason: &str| PackError::UnsafePath {
        pack: Some(id.to_string()),
        path: lock_path.display().to_string(),
        reason: reason.to_string(),
    };
    if let Ok(meta) = fs::symlink_metadata(&lock_path)
        && !meta.file_type().is_file()
    {
        return Err(unsafe_lock("is not a regular file; refusing to open it"));
    }
    let mut options = fs::OpenOptions::new();
    options.create(true).write(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let lock_file = options.open(&lock_path).map_err(|e| {
        if e.raw_os_error() == Some(libc::ELOOP) {
            unsafe_lock("is a symlink; refusing to follow it")
        } else {
            io_err(id, &lock_path, e)
        }
    })?;
    let is_file = lock_file
        .metadata()
        .map_err(|e| io_err(id, &lock_path, e))?
        .is_file();
    if !is_file {
        return Err(unsafe_lock("is not a regular file; refusing to open it"));
    }
    lock_file
        .lock_exclusive()
        .map_err(|e| io_err(id, &lock_path, e))?;
    Ok(lock_file)
}

/// Remove the installed pack `id`: the repair for a broken install (an
/// installed pack cannot be edited back into shape, since its recorded
/// digest rejects changed bytes) and the first step of an upgrade.
///
/// Only a real directory directly under `.phronesis/packs/` named by a pack
/// identifier is removed, under the pack-store lock and never through a
/// symlink. The directory is first renamed to a hidden name, which
/// enumeration skips, so an interrupted removal never leaves a half-deleted
/// pack that loads. A built-in pack cannot be removed. Returns the removed
/// directory.
pub fn remove(root: &Path, id: &str) -> Result<PathBuf, PackError> {
    let refused = |message: String| PackError::Manifest {
        pack: Some(id.to_string()),
        path: PACKS_DIR.to_string(),
        field: "remove".to_string(),
        message,
    };
    if !super::is_identifier(id) {
        return Err(refused(format!("`{id}` is not a pack identifier")));
    }
    let packs_rel = Path::new(PACKS_DIR);
    let target_rel = packs_rel.join(id);
    ensure_no_symlink_components(root, &target_rel)?;
    let packs_dir = root.join(PACKS_DIR);
    let target = packs_dir.join(id);
    match fs::symlink_metadata(&target) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => {
            return Err(PackError::UnsafePath {
                pack: Some(id.to_string()),
                path: target.display().to_string(),
                reason: "is not a directory; refusing to remove it".to_string(),
            });
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let builtin = super::builtin::builtin_pack_sources()
                .iter()
                .any(|pack| pack.manifest.id == id);
            return Err(refused(if builtin {
                format!("`{id}` is a built-in pack; it ships in the binary and cannot be removed")
            } else {
                format!("no pack `{id}` is installed in {}", packs_dir.display())
            }));
        }
        Err(e) => return Err(io_err(id, &target, e)),
    }

    let doomed = format!(".removing-{id}-{}", std::process::id());
    #[cfg(unix)]
    {
        // Every step below is relative to one descriptor for the pack store,
        // opened beneath the project root without following a symlink, so
        // replacing `.phronesis` or `.phronesis/packs` after the checks above
        // cannot redirect the lock, the rename, or the deletion.
        let store = anchored::open_dir_beneath(root, &[".phronesis", "packs"])
            .map_err(|e| io_err(id, &packs_dir, e))?;
        let _lock = anchored::lock(&store, id, &packs_dir)?;
        anchored::remove_pack(&store, id, &doomed).map_err(|e| io_err(id, &target, e))?;
    }
    #[cfg(not(unix))]
    {
        // Path-based: an ancestor replaced between these steps is not
        // detected on this platform.
        let _lock = lock_pack_store(&packs_dir, id)?;
        ensure_no_symlink_components(root, &target_rel)?;
        let doomed = packs_dir.join(doomed);
        if fs::symlink_metadata(&doomed).is_ok() {
            fs::remove_dir_all(&doomed).map_err(|e| io_err(id, &doomed, e))?;
        }
        fs::rename(&target, &doomed).map_err(|e| io_err(id, &target, e))?;
        fs::remove_dir_all(&doomed).map_err(|e| io_err(id, &doomed, e))?;
    }
    Ok(target)
}

/// Descriptor-anchored pack-store operations for `remove` (Unix).
#[cfg(unix)]
mod anchored {
    use std::ffi::CString;
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    use fs2::FileExt;

    use super::{PackError, io_err};

    fn c(name: &str) -> io::Result<CString> {
        CString::new(name).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))
    }

    fn check(ret: libc::c_int) -> io::Result<()> {
        if ret == -1 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn open_dir_at(dir: libc::c_int, name: &CString) -> io::Result<OwnedFd> {
        let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        // SAFETY: `name` is a valid NUL-terminated string and `dir` is an open
        // directory descriptor borrowed from the caller.
        let fd = unsafe { libc::openat(dir, name.as_ptr(), flags) };
        if fd == -1 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `fd` was just returned by a successful `openat`; we own it.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    /// Open `root/<components...>` as a directory, never following a symlink
    /// in any component, including `root`'s own children.
    pub(super) fn open_dir_beneath(root: &Path, components: &[&str]) -> io::Result<OwnedFd> {
        let root_c = CString::new(root.as_os_str().as_bytes())
            .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC;
        // SAFETY: `root_c` is a valid NUL-terminated path.
        let fd = unsafe { libc::open(root_c.as_ptr(), flags) };
        if fd == -1 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `fd` was just returned by a successful `open`; we own it.
        let mut dir = unsafe { OwnedFd::from_raw_fd(fd) };
        for component in components {
            dir = open_dir_at(dir.as_raw_fd(), &c(component)?)?;
        }
        Ok(dir)
    }

    /// The store lock, opened relative to the store descriptor without
    /// following a symlink (or blocking on a FIFO), and required to be a
    /// regular file.
    pub(super) fn lock(
        store: &OwnedFd,
        id: &str,
        packs_dir: &Path,
    ) -> Result<std::fs::File, PackError> {
        let lock_path = packs_dir.join(".install.lock");
        let name = c(".install.lock").map_err(|e| io_err(id, &lock_path, e))?;
        let flags =
            libc::O_CREAT | libc::O_WRONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC;
        // SAFETY: `name` is a valid NUL-terminated string and `store` is an
        // open directory descriptor owned by the caller.
        let fd = unsafe {
            libc::openat(
                store.as_raw_fd(),
                name.as_ptr(),
                flags,
                0o644 as libc::c_uint,
            )
        };
        if fd == -1 {
            let e = io::Error::last_os_error();
            if e.raw_os_error() == Some(libc::ELOOP) {
                return Err(PackError::UnsafePath {
                    pack: Some(id.to_string()),
                    path: lock_path.display().to_string(),
                    reason: "is a symlink; refusing to follow it".to_string(),
                });
            }
            return Err(io_err(id, &lock_path, e));
        }
        // SAFETY: `fd` was just returned by a successful `openat`; we own it.
        let file = std::fs::File::from(unsafe { OwnedFd::from_raw_fd(fd) });
        let regular = file
            .metadata()
            .map_err(|e| io_err(id, &lock_path, e))?
            .is_file();
        if !regular {
            return Err(PackError::UnsafePath {
                pack: Some(id.to_string()),
                path: lock_path.display().to_string(),
                reason: "is not a regular file; refusing to open it".to_string(),
            });
        }
        file.lock_exclusive()
            .map_err(|e| io_err(id, &lock_path, e))?;
        Ok(file)
    }

    fn is_dir_at(dir: &OwnedFd, name: &CString) -> io::Result<bool> {
        // SAFETY: zeroed is a valid bit pattern for `stat`, which `fstatat`
        // fills in before it is read.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: valid descriptor, NUL-terminated name, and a writable stat.
        check(unsafe {
            libc::fstatat(
                dir.as_raw_fd(),
                name.as_ptr(),
                &mut st,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        })?;
        Ok((st.st_mode & libc::S_IFMT) == libc::S_IFDIR)
    }

    /// Rename `store/id` to the hidden `store/doomed` (enumeration skips
    /// dot-names, so an interrupted removal never leaves a loadable half
    /// pack), then delete it, all relative to `store`.
    pub(super) fn remove_pack(store: &OwnedFd, id: &str, doomed: &str) -> io::Result<()> {
        let id_c = c(id)?;
        let doomed_c = c(doomed)?;
        if !is_dir_at(store, &id_c)? {
            return Err(io::Error::other("installed pack is not a directory"));
        }
        if is_dir_at(store, &doomed_c).is_ok() {
            remove_tree_at(store, &doomed_c)?;
        }
        // SAFETY: both names are NUL-terminated and relative to `store`.
        check(unsafe {
            libc::renameat(
                store.as_raw_fd(),
                id_c.as_ptr(),
                store.as_raw_fd(),
                doomed_c.as_ptr(),
            )
        })?;
        remove_tree_at(store, &doomed_c)
    }

    /// Recursively delete the directory `parent/name`, never following a
    /// symlink: links are unlinked as entries, not traversed.
    fn remove_tree_at(parent: &OwnedFd, name: &CString) -> io::Result<()> {
        let dir = open_dir_at(parent.as_raw_fd(), name)?;
        for entry in entries(&dir)? {
            if is_dir_at(&dir, &entry)? {
                remove_tree_at(&dir, &entry)?;
            } else {
                // SAFETY: valid descriptor and NUL-terminated entry name.
                check(unsafe { libc::unlinkat(dir.as_raw_fd(), entry.as_ptr(), 0) })?;
            }
        }
        // SAFETY: valid descriptor and NUL-terminated name.
        check(unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) })
    }

    /// The names in `dir`, excluding `.` and `..`.
    fn entries(dir: &OwnedFd) -> io::Result<Vec<CString>> {
        // SAFETY: `dup` of an open descriptor; the copy is handed to
        // `fdopendir`, which owns it from then on and `closedir` releases it.
        let copy = unsafe { libc::dup(dir.as_raw_fd()) };
        if copy == -1 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `copy` is an open directory descriptor we own.
        let stream = unsafe { libc::fdopendir(copy) };
        if stream.is_null() {
            let e = io::Error::last_os_error();
            // SAFETY: `fdopendir` failed, so `copy` is still ours to close.
            unsafe { libc::close(copy) };
            return Err(e);
        }
        let mut names = Vec::new();
        loop {
            // SAFETY: `stream` is a valid, open DIR*.
            let entry = unsafe { libc::readdir(stream) };
            if entry.is_null() {
                break;
            }
            // SAFETY: `readdir` returned a valid dirent whose `d_name` is
            // NUL-terminated.
            let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) };
            let bytes = name.to_bytes();
            if bytes != b"." && bytes != b".." {
                names.push(name.to_owned());
            }
        }
        // SAFETY: `stream` is a valid DIR* that is closed exactly once.
        unsafe { libc::closedir(stream) };
        Ok(names)
    }
}

fn io_err(pack: &str, path: &Path, source: std::io::Error) -> PackError {
    PackError::Io {
        pack: Some(pack.to_string()),
        path: path.display().to_string(),
        source,
    }
}

#[cfg(all(test, unix))]
mod anchored_tests {
    use super::anchored;

    /// Second-pass review finding 6: replacing `.phronesis/packs` after the
    /// store descriptor is opened must not redirect the removal. The swap is
    /// done deterministically between opening and removing.
    #[test]
    fn removal_stays_in_the_store_it_opened_when_an_ancestor_is_swapped() {
        let root = tempfile::tempdir().unwrap();
        let store = root.path().join(".phronesis/packs");
        std::fs::create_dir_all(store.join("demo/queries")).unwrap();
        std::fs::write(store.join("demo/pack.json"), b"{}").unwrap();
        std::fs::write(store.join("demo/queries/q.scm"), b"q").unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(outside.path().join("demo")).unwrap();
        std::fs::write(outside.path().join("demo/keep.txt"), b"keep").unwrap();

        let fd = anchored::open_dir_beneath(root.path(), &[".phronesis", "packs"]).unwrap();
        // Swap: the real store moves aside, a symlink to `outside` takes its name.
        let moved = root.path().join(".phronesis/packs-moved");
        std::fs::rename(&store, &moved).unwrap();
        std::os::unix::fs::symlink(outside.path(), &store).unwrap();

        anchored::remove_pack(&fd, "demo", ".removing-demo-test").unwrap();
        assert!(
            outside.path().join("demo/keep.txt").exists(),
            "followed the swapped ancestor"
        );
        assert!(
            !moved.join("demo").exists(),
            "the opened store's pack was not removed"
        );
        assert!(!moved.join(".removing-demo-test").exists());
    }

    #[test]
    fn a_symlinked_store_is_never_opened() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".phronesis")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join(".phronesis/packs")).unwrap();
        assert!(anchored::open_dir_beneath(root.path(), &[".phronesis", "packs"]).is_err());
    }
}
