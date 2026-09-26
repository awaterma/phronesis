//! Direct tests of the security boundary helpers.
//! Hook-level path traversal is exercised in tests/hook_integration.rs.

use phronesis_mcp::security::{
    MAX_ARGS_PER_ITEM, MAX_STRING_LEN, SecurityError, max_file_bytes, read_file_capped,
    read_stdin_capped, require_extension, resolve_safe_path, validate_args, validate_string,
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
    assert!(matches!(result, Err(SecurityError::EmptyPath)));
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
    assert!(require_extension(Path::new("/tmp/guide.md"), "md").is_ok());
}

#[test]
fn require_extension_rejects_non_md() {
    let result = require_extension(Path::new("/tmp/file.rs"), "md");
    assert!(matches!(
        result,
        Err(SecurityError::InvalidExtension { .. })
    ));
}

#[test]
fn require_extension_is_case_insensitive() {
    assert!(require_extension(Path::new("/tmp/GUIDE.MD"), "md").is_ok());
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

// Worktree-aware discovery (swarm WTb2 salvage): a git worktree without its
// own .phronesis is governed by the main checkout's state.
#[test]
fn resolve_project_root_finds_main_checkout_from_a_worktree() {
    let main = tempfile::tempdir().expect("main tmp");
    std::fs::create_dir_all(main.path().join(".phronesis")).expect("mkdir .phronesis");
    std::fs::write(main.path().join(".phronesis/rules.json"), "{\"rules\":[]}").expect("rules");
    let init = |args: &[&str], dir: &std::path::Path| {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?} failed");
    };
    // .phronesis is gitignored in real repos - the worktree must be born
    // ungoverned for the main-checkout fallback to apply.
    std::fs::write(main.path().join(".gitignore"), ".phronesis/\n").expect("gitignore");
    init(&["init", "-q"], main.path());
    init(&["add", "."], main.path());
    init(&["commit", "-q", "-m", "fixture"], main.path());
    let wt = tempfile::tempdir().expect("wt tmp");
    init(
        &[
            "worktree",
            "add",
            "-q",
            wt.path().join("wt").to_str().expect("path"),
            "-b",
            "wt-test",
        ],
        main.path(),
    );

    // From inside the worktree, the governing root is the MAIN checkout.
    let resolved = phronesis_mcp::security::resolve_project_root(&wt.path().join("wt"));
    let expected = std::fs::canonicalize(main.path()).expect("canonicalize main");
    assert_eq!(
        resolved, expected,
        "worktree without own .phronesis must resolve to the main checkout: {resolved:?}"
    );

    // A worktree that copy-initialized its own .phronesis governs itself.
    std::fs::create_dir_all(wt.path().join("wt/.phronesis")).expect("mkdir wt .phronesis");
    std::fs::write(wt.path().join("wt/.phronesis/rules.json"), "{\"rules\":[]}").expect("rules");
    let resolved = phronesis_mcp::security::resolve_project_root(&wt.path().join("wt"));
    assert_eq!(
        resolved,
        wt.path().join("wt"),
        "a governed worktree must keep its own state: {resolved:?}"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Worktree discovery matrix: {absolute, relative gitdir} × {worktree root,
// worktree/src, worktree/src/deep}. A relative `gitdir:` (written by
// `git worktree add --relative-paths`, git >= 2.48) is relative to the
// directory holding the `.git` file — never to the process cwd.
// ─────────────────────────────────────────────────────────────────────────

fn git(args: &[&str], dir: &Path) -> std::process::Output {
    std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .expect("spawn git")
}

fn git_ok(args: &[&str], dir: &Path) {
    let out = git(args, dir);
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A governed main checkout at `<tmp>/main` (with a blocking pre rule) and an
/// ungoverned worktree at `<tmp>/wt` with `src/deep` inside it. Returns `None`
/// when `relative` is requested but the installed git lacks `--relative-paths`.
fn worktree_fixture(relative: bool) -> Option<(tempfile::TempDir, std::path::PathBuf)> {
    let tmp = tempdir().expect("tmp");
    let main = tmp.path().join("main");
    std::fs::create_dir_all(main.join(".phronesis")).expect("mkdir .phronesis");
    std::fs::write(
        main.join(".phronesis/rules.json"),
        r#"{"rules":[{"id":"forbid-marker","phase":"pre","priority":1,
            "when":[{"new_content_contains":"FORBIDDEN_MARKER"}],
            "then":{"block":"forbidden marker"}}]}"#,
    )
    .expect("rules");
    std::fs::write(main.join(".gitignore"), ".phronesis/\n").expect("gitignore");
    std::fs::create_dir_all(main.join("src/deep")).expect("mkdir src");
    std::fs::write(main.join("src/deep/lib.rs"), "// fixture\n").expect("src file");
    git_ok(&["init", "-q"], &main);
    git_ok(&["add", "."], &main);
    git_ok(&["commit", "-q", "-m", "fixture"], &main);

    let wt = tmp.path().join("wt");
    let wt_str = wt.to_str().expect("utf8 path");
    let mut args = vec!["worktree", "add", "-q"];
    if relative {
        args.push("--relative-paths");
    }
    args.extend([wt_str, "-b", "wt-test"]);
    let out = git(&args, &main);
    if !out.status.success() {
        if relative {
            eprintln!(
                "skipping relative-gitdir case: `git worktree add --relative-paths` unsupported: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            return None;
        }
        panic!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let pointer = std::fs::read_to_string(wt.join(".git")).expect("worktree .git file");
    let gitdir = pointer
        .lines()
        .find_map(|l| l.strip_prefix("gitdir:"))
        .map(str::trim)
        .expect("gitdir line");
    assert_eq!(
        !Path::new(gitdir).is_absolute(),
        relative,
        "fixture gitdir shape mismatch: {gitdir}"
    );
    Some((tmp, wt))
}

fn assert_matrix_row(relative: bool) {
    let Some((tmp, wt)) = worktree_fixture(relative) else {
        return;
    };
    let expected = std::fs::canonicalize(tmp.path().join("main")).expect("canonicalize main");
    for start in [wt.clone(), wt.join("src"), wt.join("src/deep")] {
        let resolved = phronesis_mcp::security::resolve_project_root(&start);
        assert_eq!(
            resolved, expected,
            "relative={relative}: start {start:?} must resolve to the main checkout"
        );
    }
}

#[test]
fn resolve_project_root_absolute_gitdir_worktree_at_every_depth() {
    assert_matrix_row(false);
}

#[test]
fn resolve_project_root_relative_gitdir_worktree_at_every_depth() {
    assert_matrix_row(true);
}

// A submodule-style `.git` file with a relative gitdir, nested inside a
// governed checkout, still resolves to that checkout from any depth.
#[test]
fn resolve_project_root_submodule_style_relative_gitdir_file() {
    let tmp = tempdir().expect("tmp");
    let main = tmp.path().join("main");
    std::fs::create_dir_all(main.join(".phronesis")).expect("mkdir .phronesis");
    std::fs::create_dir_all(main.join(".git/modules/sub")).expect("mkdir modules");
    let sub = main.join("vendor/sub");
    std::fs::create_dir_all(sub.join("src")).expect("mkdir sub");
    std::fs::write(sub.join(".git"), "gitdir: ../../.git/modules/sub\n").expect("sub .git");
    let expected = std::fs::canonicalize(&main).expect("canonicalize main");
    for start in [sub.clone(), sub.join("src")] {
        let resolved = phronesis_mcp::security::resolve_project_root(&start);
        assert_eq!(
            std::fs::canonicalize(&resolved).expect("canonicalize resolved"),
            expected,
            "submodule-style start {start:?} must resolve to the enclosing checkout"
        );
    }
}

/// Drive the real `phr-mcp pre-check` binary from inside the worktree: the
/// main checkout's blocking rule must fire (exit 2) at every depth.
fn assert_pre_check_blocks_at_every_depth(relative: bool) {
    use std::io::Write;
    let Some((_tmp, wt)) = worktree_fixture(relative) else {
        return;
    };
    let payload = r#"{"tool_name":"Edit","tool_input":{"file_path":"src/deep/lib.rs","old_string":"// fixture","new_string":"// FORBIDDEN_MARKER"}}"#;
    for cwd in [wt.clone(), wt.join("src"), wt.join("src/deep")] {
        let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
            .arg("pre-check")
            .current_dir(&cwd)
            .env_remove("PHRONESIS_PROJECT_ROOT")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn phr-mcp");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(payload.as_bytes())
            .expect("write payload");
        let out = child.wait_with_output().expect("wait phr-mcp");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(
            out.status.code(),
            Some(2),
            "relative={relative}: pre-check from {cwd:?} must be governed and block: {stderr}"
        );
        assert!(stderr.contains("forbidden marker"), "{stderr}");
    }
}

#[test]
fn pre_check_blocks_in_absolute_gitdir_worktree_at_every_depth() {
    assert_pre_check_blocks_at_every_depth(false);
}

#[test]
fn pre_check_blocks_in_relative_gitdir_worktree_at_every_depth() {
    assert_pre_check_blocks_at_every_depth(true);
}
