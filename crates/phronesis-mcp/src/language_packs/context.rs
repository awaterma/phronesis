//! Host-owned discovery of nearest manifest-declared context files.
use super::{ContextFile, FileContext, PackError, ValidatedPack, limits, relative_path_problem};
use std::{fs, path::Path};

/// Each declared name resolves independently to the nearest regular file.
pub fn file_context(
    root: &Path,
    rel: &str,
    pack: &ValidatedPack,
) -> Result<FileContext, PackError> {
    let id = &pack.assets.manifest.id;
    if relative_path_problem(rel).is_some() {
        return Err(PackError::UnsafePath {
            pack: Some(id.clone()),
            path: rel.into(),
            reason: "source path must be normalized and relative".into(),
        });
    }
    let parent = Path::new(rel).parent().unwrap_or_else(|| Path::new(""));
    let mut context_files = Vec::new();
    for name in &pack.assets.manifest.context_files {
        for dir in parent.ancestors() {
            let candidate_rel = dir.join(name);
            let candidate = root.join(&candidate_rel);
            match fs::symlink_metadata(&candidate) {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(source) => {
                    return Err(PackError::Io {
                        pack: Some(id.clone()),
                        path: candidate_rel.display().to_string(),
                        source,
                    });
                }
            }
            let bytes = super::assets::read_bounded(
                root,
                Some(id),
                &candidate_rel.to_string_lossy(),
                limits::MAX_FILE_BYTES,
            )?;
            let content = String::from_utf8(bytes).map_err(|e| PackError::Record {
                pack: Some(id.clone()),
                path: Some(candidate_rel.display().to_string()),
                message: format!("context file is not UTF-8: {e}"),
            })?;
            context_files.push(ContextFile {
                name: name.clone(),
                file_rel: candidate_rel.to_string_lossy().replace('\\', "/"),
                directory_rel: if dir.as_os_str().is_empty() {
                    ".".into()
                } else {
                    dir.to_string_lossy().replace('\\', "/")
                },
                content,
            });
            break;
        }
    }
    Ok(FileContext {
        file_rel: rel.into(),
        context_files,
    })
}
