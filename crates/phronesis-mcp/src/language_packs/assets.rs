//! Bounded, symlink-refusing reads of a pack's declared assets, and the
//! canonical content digest over exactly those bytes.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;
#[cfg(not(unix))]
use std::path::PathBuf;

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{PackError, PackManifest, is_identifier, limits, validate_manifest};

pub const MANIFEST_FILE: &str = "pack.json";

/// A manifest plus the exact bytes of `pack.json` and every resource it
/// declares, keyed by pack-relative path. Undeclared files are never read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackAssets {
    pub manifest: PackManifest,
    pub files: BTreeMap<String, Vec<u8>>,
}

/// Read and validate the pack in `dir`. The manifest is validated — paths
/// included — before any resource is opened, so a traversal or symlinked
/// path is refused without reading the bytes it points at.
pub fn read_assets(dir: &Path) -> Result<PackAssets, PackError> {
    let root = fs::symlink_metadata(dir).map_err(|source| PackError::Io {
        pack: None,
        path: dir.display().to_string(),
        source,
    })?;
    if root.file_type().is_symlink() || !root.is_dir() {
        return Err(PackError::UnsafePath {
            pack: None,
            path: dir.display().to_string(),
            reason: "the pack directory must be a real directory, not a symlink".into(),
        });
    }

    let manifest_bytes = read_bounded(dir, None, MANIFEST_FILE, limits::MAX_MANIFEST_BYTES)?;
    let manifest = parse_manifest(&manifest_bytes)?;
    validate_manifest(&manifest)?;
    let pack = Some(manifest.id.as_str());

    let mut total = manifest_bytes.len();
    let mut files = BTreeMap::new();
    files.insert(MANIFEST_FILE.to_string(), manifest_bytes);
    for (field, rel) in manifest.resource_paths() {
        let cap = if field.starts_with("queries") {
            limits::MAX_QUERY_BYTES
        } else if field == "adapter" {
            limits::MAX_SCRIPT_BYTES
        } else {
            limits::MAX_FILE_BYTES
        };
        let bytes = read_bounded(dir, pack, rel, cap)?;
        total += bytes.len();
        if total > limits::MAX_TOTAL_ASSET_BYTES {
            return Err(PackError::Limit {
                pack: pack.map(str::to_string),
                path: None,
                what: "total asset bytes".into(),
                max: limits::MAX_TOTAL_ASSET_BYTES,
            });
        }
        files.insert(rel.to_string(), bytes);
    }
    Ok(PackAssets { manifest, files })
}

fn parse_manifest(bytes: &[u8]) -> Result<PackManifest, PackError> {
    let fail = |pack: Option<String>, e: serde_json::Error| PackError::Manifest {
        pack,
        path: MANIFEST_FILE.into(),
        field: "manifest".into(),
        message: e.to_string(),
    };
    let value: Value = serde_json::from_slice(bytes).map_err(|e| fail(None, e))?;
    // Attribute schema errors to the pack when its id is at least well formed.
    let pack = value
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| is_identifier(id))
        .map(str::to_string);
    serde_json::from_value(value).map_err(|e| fail(pack, e))
}

/// Read `rel` under `root`, refusing symlinked or non-directory components,
/// non-regular files, and anything over `cap` bytes.
pub(crate) fn read_bounded(
    root: &Path,
    pack: Option<&str>,
    rel: &str,
    cap: usize,
) -> Result<Vec<u8>, PackError> {
    let unsafe_path = |reason: String| PackError::UnsafePath {
        pack: pack.map(str::to_string),
        path: rel.to_string(),
        reason,
    };
    let io = |source| PackError::Io {
        pack: pack.map(str::to_string),
        path: rel.to_string(),
        source,
    };
    let too_big = || PackError::Limit {
        pack: pack.map(str::to_string),
        path: Some(rel.to_string()),
        what: "file bytes".into(),
        max: cap,
    };

    let file = open_beneath(root, rel).map_err(|e| match e {
        OpenError::Symlink(component) => {
            unsafe_path(format!("component `{component}` is a symlink"))
        }
        OpenError::NotDirectory(component) => {
            unsafe_path(format!("component `{component}` is not a directory"))
        }
        OpenError::NotRegular => unsafe_path("is not a regular file".into()),
        OpenError::Io(source) => io(source),
    })?;
    let meta = file.metadata().map_err(io)?;
    if !meta.is_file() {
        return Err(unsafe_path("is not a regular file".into()));
    }
    if meta.len() > cap as u64 {
        return Err(too_big());
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take(cap as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(io)?;
    if bytes.len() > cap {
        return Err(too_big());
    }
    Ok(bytes)
}

enum OpenError {
    Symlink(String),
    NotDirectory(String),
    NotRegular,
    Io(std::io::Error),
}

/// Open `rel` beneath `root` with descriptor-relative traversal: the root is
/// opened `O_DIRECTORY|O_NOFOLLOW`, each intermediate component is opened
/// with `openat(.., O_DIRECTORY|O_NOFOLLOW)` relative to its parent's
/// descriptor, and the final component with `openat(.., O_NOFOLLOW|
/// O_NONBLOCK)`. A component swapped for a symlink at any point is refused
/// (`ELOOP`), and a directory renamed away after it was opened is still the
/// one read from, so no path re-resolution can escape the pack.
///
/// Limits: the ancestors of `root` itself are resolved by path (the caller
/// chose `root`); a hard link inside the pack to an outside file is a regular
/// file and is read.
#[cfg(unix)]
fn open_beneath(root: &Path, rel: &str) -> Result<File, OpenError> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;

    /// Name a refused open. Platforms disagree on the errno for a symlink
    /// under `O_NOFOLLOW` (Linux `ELOOP`; macOS `ENOTDIR` with
    /// `O_DIRECTORY`), so `is_link` is an `lstat` of the refused name. It
    /// only chooses the message — the open has already been refused.
    fn errno_for(component: &str, err: std::io::Error, last: bool, is_link: bool) -> OpenError {
        match err.raw_os_error() {
            Some(libc::ELOOP | libc::ENOTDIR) if is_link => {
                OpenError::Symlink(component.to_string())
            }
            Some(libc::ELOOP) => OpenError::Symlink(component.to_string()),
            Some(libc::ENOTDIR) if !last => OpenError::NotDirectory(component.to_string()),
            _ => OpenError::Io(err),
        }
    }
    fn is_symlink_at(dir: &OwnedFd, name: &CString) -> bool {
        // SAFETY: an all-zero `stat` is a valid value for `fstatat` to overwrite.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: `dir` is an open descriptor, `name` is NUL-terminated, and
        // `st` is a writable `stat` that outlives the call.
        let rc = unsafe {
            libc::fstatat(
                dir.as_raw_fd(),
                name.as_ptr(),
                &mut st,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        rc == 0 && (st.st_mode & libc::S_IFMT) == libc::S_IFLNK
    }
    fn c_string(bytes: &[u8]) -> Result<CString, OpenError> {
        CString::new(bytes).map_err(|_| {
            OpenError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "path contains NUL",
            ))
        })
    }

    let dir_flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    let root_c = c_string(root.as_os_str().as_bytes())?;
    // SAFETY: `root_c` is a valid NUL-terminated string for the call's duration.
    let fd = unsafe { libc::open(root_c.as_ptr(), dir_flags) };
    if fd < 0 {
        let err = std::io::Error::last_os_error();
        let is_link = fs::symlink_metadata(root).is_ok_and(|m| m.file_type().is_symlink());
        return Err(errno_for(&root.display().to_string(), err, false, is_link));
    }
    // SAFETY: `fd` was just returned by a successful `open` and is owned here.
    let mut dir = unsafe { OwnedFd::from_raw_fd(fd) };

    let components: Vec<&str> = rel.split('/').collect();
    for (i, component) in components.iter().enumerate() {
        let last = i + 1 == components.len();
        #[cfg(test)]
        race_hook::fire(component, last);
        let name = c_string(component.as_bytes())?;
        let flags = if last {
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC
        } else {
            dir_flags
        };
        // SAFETY: `dir` is an open directory descriptor and `name` is a valid
        // NUL-terminated single path component.
        let fd = unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            let err = std::io::Error::last_os_error();
            let is_link = is_symlink_at(&dir, &name);
            return Err(errno_for(component, err, last, is_link));
        }
        // SAFETY: `fd` was just returned by a successful `openat` and is owned here.
        let next = unsafe { OwnedFd::from_raw_fd(fd) };
        if last {
            return Ok(File::from(next));
        }
        dir = next;
    }
    Err(OpenError::NotRegular)
}

/// Non-Unix fallback: `lstat` each component, then open by path. This is
/// **not** race safe — a component swapped for a symlink or junction between
/// the check and the open is followed. Only the static layout is checked.
#[cfg(not(unix))]
fn open_beneath(root: &Path, rel: &str) -> Result<File, OpenError> {
    let components: Vec<&str> = rel.split('/').collect();
    let mut path = PathBuf::from(root);
    for (i, component) in components.iter().enumerate() {
        path.push(component);
        let meta = fs::symlink_metadata(&path).map_err(OpenError::Io)?;
        let last = i + 1 == components.len();
        if meta.file_type().is_symlink() {
            return Err(OpenError::Symlink(component.to_string()));
        }
        if !last && !meta.is_dir() {
            return Err(OpenError::NotDirectory(component.to_string()));
        }
        if last && !meta.is_file() {
            return Err(OpenError::NotRegular);
        }
    }
    fs::OpenOptions::new()
        .read(true)
        .open(&path)
        .map_err(OpenError::Io)
}

/// `sha256:<hex>` over a domain tag, the file count, and each file in path
/// order as length-prefixed name and length-prefixed exact bytes.
pub fn digest_assets(assets: &PackAssets) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"phronesis-language-pack\0v1\0");
    hasher.update((assets.files.len() as u64).to_be_bytes());
    for (name, bytes) in &assets.files {
        hasher.update((name.len() as u64).to_be_bytes());
        hasher.update(name.as_bytes());
        hasher.update((bytes.len() as u64).to_be_bytes());
        hasher.update(bytes);
    }
    let hex: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("sha256:{hex}")
}

/// Test-only seam fired before each component is opened, so a test can swap
/// the tree at the exact moment a path-based reader would be vulnerable.
#[cfg(test)]
mod race_hook {
    use std::cell::RefCell;

    type Hook = Box<dyn FnMut(&str, bool)>;
    thread_local!(static HOOK: RefCell<Option<Hook>> = RefCell::new(None));

    pub(super) fn set(hook: impl FnMut(&str, bool) + 'static) {
        HOOK.with(|h| *h.borrow_mut() = Some(Box::new(hook)));
    }

    pub(super) fn clear() {
        HOOK.with(|h| *h.borrow_mut() = None);
    }

    pub(super) fn fire(component: &str, last: bool) {
        HOOK.with(|h| {
            if let Some(hook) = h.borrow_mut().as_mut() {
                hook(component, last);
            }
        });
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;

    const SECRET: &[u8] = b"OUTSIDE-SECRET-BYTES";
    const INSIDE: &[u8] = b"(function_definition) @def\n";

    /// A pack with one nested resource, `queries/decls.scm`, and an outside
    /// directory holding a same-named file with secret bytes.
    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let pack = tmp.path().join("pack");
        let outside = tmp.path().join("outside");
        fs::create_dir_all(pack.join("queries")).expect("mkdir queries");
        fs::create_dir_all(&outside).expect("mkdir outside");
        fs::write(pack.join("queries/decls.scm"), INSIDE).expect("write query");
        fs::write(outside.join("decls.scm"), SECRET).expect("write secret");
        let manifest = serde_json::json!({
            "schema_version": 1, "adapter_api": 1, "id": "race_pack",
            "version": "0.1.0", "language": "demo", "extensions": ["demo"],
            "grammar": "python", "capabilities": ["structural_facts"],
            "queries": [{"id": "decls", "path": "queries/decls.scm"}]
        });
        fs::write(pack.join(MANIFEST_FILE), manifest.to_string()).expect("write manifest");
        (tmp, pack, outside)
    }

    /// Replace `pack/queries` with a symlink to `outside`, keeping the real
    /// directory alive under another name.
    fn swap(pack: &Path, outside: &Path) {
        fs::rename(pack.join("queries"), pack.join("queries.orig")).expect("rename");
        symlink(outside, pack.join("queries")).expect("symlink");
    }

    #[test]
    fn intermediate_swapped_before_open_is_refused() {
        let (_tmp, pack, outside) = fixture();
        let (p, o) = (pack.clone(), outside.clone());
        race_hook::set(move |component, last| {
            if component == "queries" && !last {
                swap(&p, &o);
            }
        });
        let result = read_assets(&pack);
        race_hook::clear();
        let err = result.expect_err("a symlinked intermediate must be refused");
        let text = err.to_string();
        assert!(matches!(err, PackError::UnsafePath { .. }), "{text}");
        assert!(text.contains("symlink"), "{text}");
        assert!(!text.contains("OUTSIDE-SECRET"), "{text}");
    }

    /// The window the old lstat-then-open-by-path reader left open: every
    /// component checks clean, then the intermediate directory becomes a
    /// symlink just before the final open. Descriptor-relative traversal
    /// still reads from the directory it already opened.
    #[test]
    fn intermediate_swapped_after_check_cannot_redirect_read() {
        let (_tmp, pack, outside) = fixture();
        let (p, o) = (pack.clone(), outside.clone());
        let mut swapped = false;
        race_hook::set(move |component, last| {
            if component == "decls.scm" && last && !swapped {
                swap(&p, &o);
                swapped = true;
            }
        });
        let result = read_assets(&pack);
        race_hook::clear();
        let assets = result.expect("read from the already-opened directory");
        let bytes = &assets.files["queries/decls.scm"];
        assert_eq!(bytes.as_slice(), INSIDE);
        assert!(
            fs::symlink_metadata(pack.join("queries"))
                .expect("lstat")
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn final_component_swapped_before_open_is_refused() {
        let (_tmp, pack, outside) = fixture();
        let p = pack.clone();
        race_hook::set(move |component, last| {
            if component == "decls.scm" && last {
                let file = p.join("queries/decls.scm");
                fs::remove_file(&file).expect("remove");
                symlink(outside.join("decls.scm"), &file).expect("symlink");
            }
        });
        let result = read_assets(&pack);
        race_hook::clear();
        let text = result
            .expect_err("a symlinked file must be refused")
            .to_string();
        assert!(text.contains("symlink"), "{text}");
        assert!(!text.contains("OUTSIDE-SECRET"), "{text}");
    }
}
