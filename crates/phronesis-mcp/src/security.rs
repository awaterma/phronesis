//! Security boundary: path validation, size limits, and input sanity checks.
//!
//! All untrusted inputs (MCP tool params, hook payloads, file paths) must pass
//! through these helpers before reaching engine state or filesystem I/O.

use std::io::Read;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// Maximum length of any single string input (rule ID, fact arg, predicate, etc.).
pub const MAX_STRING_LEN: usize = 64 * 1024;

/// Maximum number of arguments in a single condition or action.
pub const MAX_ARGS_PER_ITEM: usize = 256;

/// Maximum rules allowed in the network.
pub const MAX_RULES: usize = 10_000;

/// Maximum facts allowed in working memory.
pub const MAX_FACTS: usize = 100_000;

/// Maximum accumulated consequences. Older consequences are evicted FIFO.
pub const MAX_CONSEQUENCES: usize = 10_000;

/// Maximum bytes of stdin a hook will read before erroring out.
///
/// Sized to accommodate an Edit/Write payload — the JSON wraps the file content
/// or the new_string, so the payload can be larger than the file itself.
pub const MAX_PAYLOAD_BYTES: u64 = 10 * 1024 * 1024;

/// Default maximum bytes for any single file read from disk.
///
/// Rule of thumb: source files are typically <100KB; markdown docs <1MB. Files
/// larger than this are unlikely to benefit from rule-based validation and
/// risk DoS by inflating working-memory usage. Override at runtime by setting
/// `PHRONESIS_MAX_FILE_BYTES` (decimal bytes).
pub const MAX_FILE_BYTES_DEFAULT: u64 = 1024 * 1024;

/// Hard ceiling on the runtime override. No matter what `PHRONESIS_MAX_FILE_BYTES`
/// is set to, the cap will never exceed this value. Prevents a misconfigured
/// override from re-introducing the unbounded-read class of bug.
pub const MAX_FILE_BYTES_CEILING: u64 = 64 * 1024 * 1024;

/// Maximum bytes of file content stored as a single fact argument.
///
/// Even when a file is small enough to read fully, storing its entire content
/// as a fact arg in working memory is wasteful — the RETE engine doesn't index
/// on free-form text, and pattern checks operate on the borrowed string slice.
/// Above this size, hooks skip the `file_content` fact assertion and rely only
/// on pattern-matched facts.
pub const MAX_FACT_CONTENT_BYTES: usize = 256 * 1024;

/// Return the effective max file-read cap, honoring the `PHRONESIS_MAX_FILE_BYTES`
/// env var when set and parseable, otherwise the default. Always bounded by
/// `MAX_FILE_BYTES_CEILING`.
pub fn max_file_bytes() -> u64 {
    let raw = std::env::var("PHRONESIS_MAX_FILE_BYTES")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(MAX_FILE_BYTES_DEFAULT);
    raw.min(MAX_FILE_BYTES_CEILING)
}

#[derive(Debug, Error)]
pub enum SecurityError {
    #[error("path is empty")]
    EmptyPath,
    #[error("path contains '..' traversal: {0}")]
    PathTraversal(String),
    #[error("path is outside project root: {0}")]
    PathOutsideRoot(String),
    #[error("path not found: {0}")]
    PathNotFound(String),
    #[error("path has wrong extension (expected {expected}): {path}")]
    InvalidExtension { expected: String, path: String },
    #[error("string field '{field}' is too long: {actual} bytes (max {max})")]
    StringTooLong {
        field: &'static str,
        actual: usize,
        max: usize,
    },
    #[error("too many {kind}: {count} (max {max})")]
    LimitExceeded {
        kind: &'static str,
        count: usize,
        max: usize,
    },
    #[error("io error on {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

/// Return the project root directory.
///
/// Honors `PHRONESIS_PROJECT_ROOT` if set, otherwise falls back to the current
/// working directory. The returned path is not guaranteed to exist or be canonical.
pub fn project_root() -> PathBuf {
    std::env::var("PHRONESIS_PROJECT_ROOT")
        .ok()
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Resolve a user-supplied path against `project_root`, ensuring the canonical
/// result is contained within `project_root`.
///
/// Rejects:
/// - Empty paths
/// - Paths containing `..` components (defense in depth)
/// - Absolute or relative paths whose canonical form escapes the root (including
///   via symlinks, since `canonicalize` resolves them)
pub fn resolve_safe_path(user_path: &str, project_root: &Path) -> Result<PathBuf, SecurityError> {
    if user_path.is_empty() {
        return Err(SecurityError::EmptyPath);
    }

    if user_path
        .split(['/', std::path::MAIN_SEPARATOR])
        .any(|component| component == "..")
    {
        return Err(SecurityError::PathTraversal(user_path.to_string()));
    }

    let candidate = if Path::new(user_path).is_absolute() {
        PathBuf::from(user_path)
    } else {
        project_root.join(user_path)
    };

    let canonical = candidate.canonicalize().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => SecurityError::PathNotFound(user_path.to_string()),
        _ => SecurityError::Io {
            path: user_path.to_string(),
            source: e,
        },
    })?;

    let canonical_root = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());

    if !canonical.starts_with(&canonical_root) {
        return Err(SecurityError::PathOutsideRoot(
            canonical.display().to_string(),
        ));
    }

    Ok(canonical)
}

/// Require that a path ends with the given extension (without dot).
pub fn require_extension(path: &Path, expected: &str) -> Result<(), SecurityError> {
    let matches = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case(expected))
        .unwrap_or(false);
    if matches {
        Ok(())
    } else {
        Err(SecurityError::InvalidExtension {
            expected: expected.to_string(),
            path: path.display().to_string(),
        })
    }
}

/// Read a file from disk, capping the read at `max_file_bytes()` to discard
/// against resource exhaustion via large or unbounded files (e.g. `/dev/zero`,
/// FIFOs).
pub fn read_file_capped(path: &Path) -> Result<String, SecurityError> {
    let file = std::fs::File::open(path).map_err(|e| SecurityError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    let mut content = String::new();
    file.take(max_file_bytes())
        .read_to_string(&mut content)
        .map_err(|e| SecurityError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
    Ok(content)
}

/// Read stdin, capping the read at `MAX_PAYLOAD_BYTES`.
pub fn read_stdin_capped() -> Result<String, SecurityError> {
    let mut input = String::new();
    std::io::stdin()
        .take(MAX_PAYLOAD_BYTES)
        .read_to_string(&mut input)
        .map_err(|e| SecurityError::Io {
            path: "<stdin>".to_string(),
            source: e,
        })?;
    Ok(input)
}

pub fn validate_string(value: &str, field: &'static str) -> Result<(), SecurityError> {
    if value.len() > MAX_STRING_LEN {
        return Err(SecurityError::StringTooLong {
            field,
            actual: value.len(),
            max: MAX_STRING_LEN,
        });
    }
    Ok(())
}

pub fn validate_args(args: &[String], field: &'static str) -> Result<(), SecurityError> {
    if args.len() > MAX_ARGS_PER_ITEM {
        return Err(SecurityError::LimitExceeded {
            kind: field,
            count: args.len(),
            max: MAX_ARGS_PER_ITEM,
        });
    }
    for arg in args {
        validate_string(arg, field)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn resolve_rejects_empty_path() {
        let root = tempdir().unwrap();
        assert!(matches!(
            resolve_safe_path("", root.path()),
            Err(SecurityError::EmptyPath)
        ));
    }

    #[test]
    fn resolve_rejects_dot_dot_traversal() {
        let root = tempdir().unwrap();
        assert!(matches!(
            resolve_safe_path("../etc/passwd", root.path()),
            Err(SecurityError::PathTraversal(_))
        ));
        assert!(matches!(
            resolve_safe_path("subdir/../../etc/passwd", root.path()),
            Err(SecurityError::PathTraversal(_))
        ));
    }

    #[test]
    fn resolve_rejects_absolute_outside_root() {
        let root = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let outside_file = outside.path().join("secret.md");
        std::fs::write(&outside_file, "x").unwrap();

        let result = resolve_safe_path(outside_file.to_str().unwrap(), root.path());
        assert!(
            matches!(result, Err(SecurityError::PathOutsideRoot(_))),
            "got: {:?}",
            result
        );
    }

    #[test]
    fn resolve_accepts_relative_inside_root() {
        let root = tempdir().unwrap();
        let inside = root.path().join("notes.md");
        std::fs::write(&inside, "x").unwrap();

        let result = resolve_safe_path("notes.md", root.path()).expect("should resolve");
        assert!(result.ends_with("notes.md"));
    }

    #[test]
    fn resolve_accepts_nested_inside_root() {
        let root = tempdir().unwrap();
        let sub = root.path().join("docs");
        std::fs::create_dir_all(&sub).unwrap();
        let inside = sub.join("guide.md");
        std::fs::write(&inside, "x").unwrap();

        let result = resolve_safe_path("docs/guide.md", root.path()).expect("should resolve");
        assert!(result.ends_with("guide.md"));
    }

    #[test]
    fn resolve_rejects_nonexistent_path() {
        let root = tempdir().unwrap();
        assert!(matches!(
            resolve_safe_path("does-not-exist.md", root.path()),
            Err(SecurityError::PathNotFound(_))
        ));
    }

    #[test]
    fn resolve_via_symlink_outside_root_is_rejected() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let root = tempdir().unwrap();
            let outside = tempdir().unwrap();
            let target = outside.path().join("secret.md");
            std::fs::write(&target, "x").unwrap();

            let link = root.path().join("link.md");
            symlink(&target, &link).unwrap();

            let result = resolve_safe_path("link.md", root.path());
            assert!(
                matches!(result, Err(SecurityError::PathOutsideRoot(_))),
                "symlink-out should be rejected, got {:?}",
                result
            );
        }
    }

    #[test]
    fn require_extension_accepts_matching() {
        let p = Path::new("/tmp/foo.md");
        assert!(require_extension(p, "md").is_ok());
    }

    #[test]
    fn require_extension_rejects_wrong() {
        let p = Path::new("/tmp/foo.rs");
        assert!(matches!(
            require_extension(p, "md"),
            Err(SecurityError::InvalidExtension { .. })
        ));
    }

    #[test]
    fn read_file_capped_truncates_oversize_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("big.txt");
        let cap = max_file_bytes();
        let oversize = "a".repeat((cap + 10) as usize);
        std::fs::write(&path, oversize).unwrap();

        let content = read_file_capped(&path).unwrap();
        assert_eq!(content.len(), cap as usize);
    }

    // Single test combining all env-var cases. Env vars are process-global,
    // so running these scenarios in parallel races. Keep them sequential.
    //
    // SAFETY: edition 2024 marks std::env::{set_var, remove_var} as unsafe
    // because mutating the environment is unsound when other threads read
    // it concurrently. This test runs single-threaded inside `cargo test`
    // and touches an env var no other test reads, so the contract holds.
    #[test]
    fn max_file_bytes_env_override_behavior() {
        let prior = std::env::var("PHRONESIS_MAX_FILE_BYTES").ok();

        // Default when unset
        unsafe { std::env::remove_var("PHRONESIS_MAX_FILE_BYTES") };
        assert_eq!(max_file_bytes(), MAX_FILE_BYTES_DEFAULT);

        // Honors explicit override
        unsafe { std::env::set_var("PHRONESIS_MAX_FILE_BYTES", "2048") };
        assert_eq!(max_file_bytes(), 2048);

        // Ceiling caps a runaway value
        unsafe { std::env::set_var("PHRONESIS_MAX_FILE_BYTES", "999999999999") };
        assert_eq!(max_file_bytes(), MAX_FILE_BYTES_CEILING);

        // Garbage falls back to default
        unsafe { std::env::set_var("PHRONESIS_MAX_FILE_BYTES", "not-a-number") };
        assert_eq!(max_file_bytes(), MAX_FILE_BYTES_DEFAULT);

        // Restore
        match prior {
            Some(v) => unsafe { std::env::set_var("PHRONESIS_MAX_FILE_BYTES", v) },
            None => unsafe { std::env::remove_var("PHRONESIS_MAX_FILE_BYTES") },
        }
    }

    #[test]
    fn validate_string_rejects_oversize() {
        let big = "a".repeat(MAX_STRING_LEN + 1);
        assert!(matches!(
            validate_string(&big, "test"),
            Err(SecurityError::StringTooLong { .. })
        ));
    }

    #[test]
    fn validate_string_accepts_normal_size() {
        assert!(validate_string("normal value", "test").is_ok());
    }

    #[test]
    fn validate_args_rejects_too_many() {
        let args: Vec<String> = (0..MAX_ARGS_PER_ITEM + 1).map(|i| i.to_string()).collect();
        assert!(matches!(
            validate_args(&args, "test"),
            Err(SecurityError::LimitExceeded { .. })
        ));
    }
}
