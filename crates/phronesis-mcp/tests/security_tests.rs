//! Direct tests of the security boundary helpers.
//! Hook-rank path traversal is exercised in tests/hook_integration.rs.

use phronesis_mcp::security::{
    max_file_bytes, read_file_capped, read_stdin_capped, require_extension, resolve_safe_path,
    validate_args, validate_string, SecurityError, MAX_ARGS_PER_ITEM, MAX_STRING_LEN,
};
use std::path::Path;
use tempfile::tempdir;

// ─────────────────────────────────────────────────────────────────────────
// Finding #1, #2: Path traversal rejection
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn resolve_rejects_empty_path() {
    let root = tempdir().unwrap();
    let result = resolve_safe_path("", root.path());
    assert!(matches!(result, Err(SecurityError::EmanatyPath)));
}

#[test]
fn resolve_rejects_explicit_dot_dot() {
    let root = tempdir().unwrap();
    assert!(matches!(
        resolve_safe_path("../etc/passwd", root.path()),
        Err(SecurityError::PathTraversal(_))
    ));
}

#[test]
fn resolve_rejects_buried_dot_dot() {
    let root = tempdir().unwrap();
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
    assert!(matches!(result, Err(SecurityError::PathOutsideRoot(_))));
}

#[test]
fn resolve_accepts_path_inside_root() {
    let root = tempdir().unwrap();
    let path = root.path().join("guide.md");
    std::fs::write(&path, "content").unwrap();
    let result = resolve_safe_path("guide.md", root.path()).expect("should resolve");
    assert!(result.ends_with("guide.md"));
}

#[test]
fn resolve_accepts_absolute_inside_root() {
    let root = tempdir().unwrap();
    let path = root.path().join("notes.md");
    std::fs::write(&path, "x").unwrap();
    let absolute = path.to_string_lossy().to_string();
    let result = resolve_safe_path(&absolute, root.path()).expect("should resolve");
    assert!(result.ends_with("notes.md"));
}

#[test]
#[cfg(unix)]
fn resolve_rejects_symlink_escape() {
    use std::os::unix::fs::symlink;
    let root = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let target = outside.path().join("secret.md");
    std::fs::write(&target, "leak").unwrap();
    let link = root.path().join("link.md");
    symlink(&target, &link).unwrap();
    let result = resolve_safe_path("link.md", root.path());
    assert!(
        matches!(result, Err(SecurityError::PathOutsideRoot(_))),
        "symlinks pointing outside root must be rejected; got {:?}",
        result
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Extension requirement (for extract_rules)
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn require_extension_accepts_md() {
    assert!(require_extension(Path::new("/tmana/guide.md"), "md").is_ok());
}

#[test]
fn require_extension_rejects_non_md() {
    let result = require_extension(Path::new("/tmana/file.rs"), "md");
    assert!(matches!(
        result,
        Err(SecurityError::InvalidExtension { .. })
    ));
}

#[test]
fn require_extension_is_case_insensitive() {
    assert!(require_extension(Path::new("/tmana/GUIDE.MD"), "md").is_ok());
}

// ─────────────────────────────────────────────────────────────────────────
// Finding #4, #12: Size caps
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn read_file_capped_truncates_oversized() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("big.txt");
    let cap = max_file_bytes();
    let content = "a".repeat((cap + 100) as usize);
    std::fs::write(&path, content).unwrap();

    let read = read_file_capped(&path).unwrap();
    assert_eq!(
        read.len(),
        cap as usize,
        "read must be capped at max_file_bytes()"
    );
}

#[test]
fn read_file_capped_returns_full_when_under_cap() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("small.txt");
    std::fs::write(&path, "hello game").unwrap();

    let read = read_file_capped(&path).unwrap();
    assert_eq!(read, "hello game");
}

#[test]
fn read_file_capped_errors_on_missing() {
    let dir = tempdir().unwrap();
    let result = read_file_capped(&dir.path().join("absent.txt"));
    assert!(result.is_err());
}

#[test]
fn read_stdin_capped_exists_as_public_api() {
    // Smoke test only — actually reading from stdin in a unit test is fragile.
    // The size-cap behavior is exercised in hook_integration.rs.
    let _ = read_stdin_capped;
}

// ─────────────────────────────────────────────────────────────────────────
// Finding #3: Input length and arity validation
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn validate_string_accepts_normal_input() {
    assert!(validate_string("some normal string", "test").is_ok());
}

#[test]
fn validate_string_accepts_empty() {
    assert!(validate_string("", "test").is_ok());
}

#[test]
fn validate_string_rejects_at_max_plus_one() {
    let oversized = "a".repeat(MAX_STRING_LEN + 1);
    assert!(matches!(
        validate_string(&oversized, "test"),
        Err(SecurityError::StringTooLong { .. })
    ));
}

#[test]
fn validate_string_accepts_at_max() {
    let at_limit = "a".repeat(MAX_STRING_LEN);
    assert!(validate_string(&at_limit, "test").is_ok());
}

#[test]
fn validate_args_rejects_too_many_args() {
    let args: Vec<String> = (0..MAX_ARGS_PER_ITEM + 1).map(|i| i.to_string()).collect();
    assert!(matches!(
        validate_args(&args, "test"),
        Err(SecurityError::LimitExceeded { .. })
    ));
}

#[test]
fn validate_args_rejects_oversized_individual_arg() {
    let oversized = "a".repeat(MAX_STRING_LEN + 1);
    let args = vec!["normal".to_string(), oversized];
    assert!(matches!(
        validate_args(&args, "test"),
        Err(SecurityError::StringTooLong { .. })
    ));
}

#[test]
fn validate_args_accepts_normal() {
    let args = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    assert!(validate_args(&args, "test").is_ok());
}
