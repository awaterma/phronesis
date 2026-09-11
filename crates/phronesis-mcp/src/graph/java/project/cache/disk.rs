//! Best-effort cache for separate hook processes; never graph authority.

use super::Files;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

// Increment when declaration extraction or its grammar changes.
const FORMAT: u32 = 3;
const MAX_BYTES: u64 = 64 * 1024 * 1024;
const NAME: &str = "java-declarations.json";

#[derive(serde::Serialize, serde::Deserialize)]
struct Stored {
    format: u32,
    engine_version: String,
    root: PathBuf,
    files: Files,
}

fn directory(root: &Path) -> Option<PathBuf> {
    let root = root.canonicalize().ok()?;
    let path = root.join(".phronesis");
    // Do not create configuration merely because discovery was requested,
    // or follow a project-controlled cache directory outside the repository.
    (path.canonicalize().ok()? == path && path.is_dir()).then_some(path)
}

pub(super) fn load(root: &Path) -> Option<Files> {
    let path = directory(root)?.join(NAME);
    let metadata = std::fs::symlink_metadata(&path).ok()?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_BYTES {
        return None;
    }
    let stored: Stored = serde_json::from_slice(&bytes).ok()?;
    (stored.format == FORMAT
        && stored.engine_version == env!("CARGO_PKG_VERSION")
        && stored.root == root.canonicalize().ok()?)
    .then_some(stored.files)
}

pub(super) fn save(root: &Path, files: &Files) {
    if let Err(error) = write(root, files) {
        tracing::debug!("Java declaration cache write skipped: {error}");
    }
}

fn write(root: &Path, files: &Files) -> std::io::Result<()> {
    let Some(directory) = directory(root) else {
        return Ok(());
    };
    let stored = Stored {
        format: FORMAT,
        engine_version: env!("CARGO_PKG_VERSION").into(),
        root: root.canonicalize()?,
        files: files.clone(),
    };
    let bytes = serde_json::to_vec(&stored)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Ok(());
    }
    let mut temporary = tempfile::NamedTempFile::new_in(&directory)?;
    temporary.write_all(&bytes)?;
    temporary.persist(directory.join(NAME))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::java::project::cache::parse_inputs;
    use std::collections::BTreeMap;

    fn repository() -> tempfile::TempDir {
        let root = tempfile::tempdir().expect("repository");
        std::fs::create_dir(root.path().join(".phronesis")).expect("cache directory");
        root
    }

    #[test]
    fn persisted_declarations_reuse_matching_bytes_and_drop_changed_or_deleted_entries() {
        let root = repository();
        let mut inputs = BTreeMap::from([(
            "A.java".into(),
            "package a; class A { void run() { new B().go(); } }".into(),
        )]);
        let (cold, _) = parse_inputs(&inputs, None);
        write(root.path(), &cold).expect("persist");
        let loaded = load(root.path()).expect("load from disk");
        assert_eq!(cold, loaded);
        let (_, warm) = parse_inputs(&inputs, Some(&loaded));
        assert!(std::sync::Arc::ptr_eq(&loaded["A.java"].1, &warm["A.java"]));
        inputs.insert("A.java".into(), "package b; class B {}".into());
        let (changed, sources) = parse_inputs(&inputs, Some(&loaded));
        assert_eq!(sources["A.java"].package, "b");
        assert!(!std::sync::Arc::ptr_eq(
            &loaded["A.java"].1,
            &sources["A.java"]
        ));
        let (empty, _) = parse_inputs(&BTreeMap::new(), Some(&changed));
        write(root.path(), &empty).expect("persist deletion");
        assert!(load(root.path()).expect("empty disk cache").is_empty());
    }

    #[test]
    fn corrupt_incompatible_oversized_and_other_root_caches_are_misses() {
        let root = repository();
        let path = root.path().join(".phronesis").join(NAME);
        for body in [b"broken".as_slice(), b"{}"] {
            std::fs::write(&path, body).expect("invalid cache");
            assert!(load(root.path()).is_none());
        }
        write(root.path(), &Files::new()).expect("valid cache");
        let valid = std::fs::read(&path).expect("bytes");
        for field in ["format", "engine_version", "root"] {
            let mut value: serde_json::Value = serde_json::from_slice(&valid).expect("json");
            value[field] = if field == "format" {
                serde_json::json!(FORMAT + 1)
            } else {
                serde_json::json!("incompatible")
            };
            std::fs::write(&path, serde_json::to_vec(&value).expect("json")).expect("cache");
            assert!(load(root.path()).is_none(), "{field}");
        }
        let file = std::fs::File::create(path).expect("oversized cache");
        file.set_len(MAX_BYTES + 1).expect("sparse size");
        assert!(load(root.path()).is_none());
    }

    #[test]
    fn discovery_cache_does_not_create_project_configuration() {
        let root = tempfile::tempdir().expect("repository");
        write(root.path(), &Files::new()).expect("optional cache");
        assert!(!root.path().join(".phronesis").exists());
    }

    #[cfg(unix)]
    #[test]
    fn cache_directory_and_file_symlinks_are_not_read_through() {
        let root = repository();
        let outside = repository();
        write(outside.path(), &Files::new()).expect("outside cache");
        let path = root.path().join(".phronesis").join(NAME);
        std::os::unix::fs::symlink(outside.path().join(".phronesis").join(NAME), &path)
            .expect("file symlink");
        assert!(load(root.path()).is_none());
        std::fs::remove_file(path).expect("remove link");
        std::fs::remove_dir(root.path().join(".phronesis")).expect("remove directory");
        std::os::unix::fs::symlink(
            outside.path().join(".phronesis"),
            root.path().join(".phronesis"),
        )
        .expect("directory symlink");
        assert!(load(root.path()).is_none());
        write(root.path(), &Files::new()).expect("skip symlink directory");
    }
}
