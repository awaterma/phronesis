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
/// Honors `PHRONESIS_PROJECT_ROOT` as an explicit override (returned verbatim,
/// no discovery). Otherwise walks up from the current working directory to find
/// the governing root via [`resolve_project_root`]. The returned path is not
/// guaranteed to exist or be canonical.
pub fn project_root() -> PathBuf {
    if let Some(override_root) = std::env::var("PHRONESIS_PROJECT_ROOT")
        .ok()
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
    {
        return override_root;
    }
    let start = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    resolve_project_root(&start)
}

/// Walk up from `start` to find the governing project root — the nearest
/// ancestor (or its main checkout) that actually carries `.phronesis` state.
///
/// Discovery precedence at each ancestor, nearest first:
/// - an ancestor carrying `.phronesis/` governs — a worktree that
///   copy-initialized its own `.phronesis` is governed by its own state, not the
///   main checkout's;
/// - otherwise, when the ancestor is a git *worktree* (its `.git` is a file
///   pointer) and lacks its own `.phronesis`, the main checkout's `.phronesis`
///   is probed via the `gitdir:` pointer and governs when present;
/// - a git root without `.phronesis` (and whose main checkout has none) does
///   not govern — the walk continues, so a repo with no phronesis state anywhere
///   falls through to `start`, reproducing the pre-discovery fail-open behavior.
///
/// Exposed separately from [`project_root`] so the worktree-aware discovery can
/// be exercised without changing the process working directory.
pub fn resolve_project_root(start: &Path) -> PathBuf {
    let mut current = Some(start.to_path_buf());
    while let Some(dir) = current.take() {
        if has_phronesis(&dir) {
            return dir;
        }
        if let Some(main_root) = main_checkout_root(&dir) {
            if has_phronesis(&main_root) {
                return main_root;
            }
        }
        current = dir.parent().map(|p| Path::new(p).to_path_buf());
    }
    start.to_path_buf()
}

/// True when `root` carries a `.phronesis` directory (the phronesis state root).
fn has_phronesis(root: &Path) -> bool {
    root.join(".phronesis").is_dir()
}

/// If `root` is a git worktree (its `.git` is a file pointer), return the main
/// checkout's root derived from the `gitdir:` line. Returns `None` when `.git`
/// is a directory (a normal repo) or the pointer is absent/malformed — the
/// caller then treats `root` as a non-governing ancestor and keeps walking up.
///
/// The pointer format is `gitdir: <main>/.git/worktrees/<name>`, so the main
/// checkout root is three components up from the value (name, worktrees, .git).
fn main_checkout_root(root: &Path) -> Option<PathBuf> {
    let git = root.join(".git");
    if !git.is_file() {
        return None;
    }
    let contents = std::fs::read_to_string(&git).ok()?;
    let value = contents.lines().find_map(|line| {
        line.strip_prefix("gitdir:")
            .map(str::trim)
            .map(|v| v.to_string())
    })?;
    let main_root = Path::new(&value).parent()?.parent()?.parent()?;
    Some(main_root.to_path_buf())
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

    #[test]
    fn max_file_bytes_env_override_behavior() {
        const EXPECTED_CAP: &str = "PHRONESIS_TEST_EXPECTED_MAX_FILE_BYTES";
        if let Ok(expected) = std::env::var(EXPECTED_CAP) {
            assert_eq!(max_file_bytes(), expected.parse::<u64>().unwrap());
            return;
        }

        // Other tests read this process-global setting, including twice in
        // read_file_capped_truncates_oversize_file. Configure each child before
        // it starts instead of mutating the parallel test runner's environment.
        let executable = std::env::current_exe().unwrap();
        for (value, expected) in [
            (None, MAX_FILE_BYTES_DEFAULT),
            (Some("2048"), 2048),
            (Some("999999999999"), MAX_FILE_BYTES_CEILING),
            (Some("not-a-number"), MAX_FILE_BYTES_DEFAULT),
            (Some("0"), 0),
            (Some("-1"), MAX_FILE_BYTES_DEFAULT),
            (Some("18446744073709551616"), MAX_FILE_BYTES_DEFAULT),
        ] {
            let mut command = std::process::Command::new(&executable);
            command
                .args([
                    "--exact",
                    "security::tests::max_file_bytes_env_override_behavior",
                    "--nocapture",
                ])
                .env(EXPECTED_CAP, expected.to_string())
                .env_remove("PHRONESIS_MAX_FILE_BYTES");
            if let Some(value) = value {
                command.env("PHRONESIS_MAX_FILE_BYTES", value);
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success()
                    && String::from_utf8_lossy(&output.stdout).contains("1 passed;"),
                "override {value:?} failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
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
