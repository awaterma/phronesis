//! Parse Codex `apply_patch` payloads.
//!
//! Accepts the patch format Codex sends via `PreToolUse`/`PostToolUse`:
//! `*** Begin Patch`, `*** Update File:` (optionally followed by
//! `*** Move to:`), `*** Add File:`, `*** Delete File:`.
//!
//! This is a lightweight parser, not a general patch engine.

use super::PatchFile;

/// Parse a Codex `apply_patch` payload into per-file entries.
///
/// Recognised block types:
/// - `*** Begin Patch` / `*** End Patch` (optional wrapper)
/// - `*** Update File: <path>` — existing file being modified
/// - `*** Add File: <path>` — new file being created
/// - `*** Delete File: <path>` — file being removed (`deleted: true`)
/// - `*** Move to: <path>` — the preceding Update renames its file; the
///   destination is a touched file too (its own entry), and receives the
///   hunks that follow (the content lands at the new path)
///
/// Returns an empty vec when the input doesn't match any recognised blocks.
pub fn parse_patch(input: &str) -> Vec<PatchFile> {
    const MARKERS: [&str; 4] = [
        "*** Update File:",
        "*** Add File:",
        "*** Delete File:",
        "*** Move to:",
    ];

    let mut files: Vec<PatchFile> = Vec::new();
    for line in input.lines() {
        let trimmed = line.trim_end();
        let stripped = trimmed.trim_start();
        if let Some(marker) = MARKERS.iter().find(|m| stripped.starts_with(*m)) {
            files.push(PatchFile {
                path: stripped.trim_start_matches(marker).trim().to_string(),
                added: String::new(),
                deleted: *marker == "*** Delete File:",
            });
        } else if let Some(current) = files.last_mut() {
            // Hunk lines added by the patch carry the content rules must
            // see (the file may not exist on disk yet for Add File).
            if let Some(add) = trimmed.strip_prefix('+') {
                current.added.push_str(add);
                current.added.push('\n');
            }
        }
    }
    if !files.is_empty() {
        return files;
    }

    Vec::new()
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_update_file_block() {
        let input = "*** Update File: src/main.rs\n*** Begin Hunk\n@@ -1,3 +1,4 @@\n";
        let files = parse_patch(input);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "src/main.rs");
    }

    #[test]
    fn parse_add_file_block() {
        let input = "*** Add File: src/new.rs\n*** Begin Hunk\n@@ -0,0 +1,5 @@\n";
        let files = parse_patch(input);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "src/new.rs");
    }

    #[test]
    fn parse_delete_file_block() {
        let input = "*** Delete File: src/old.rs\n";
        let files = parse_patch(input);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "src/old.rs");
    }

    #[test]
    fn move_to_destination_is_a_touched_file() {
        let input = "*** Update File: a.rs\n*** Move to: src/b.rs\n@@\n-x\n+y\n";
        let files = parse_patch(input);
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["a.rs", "src/b.rs"]);
        assert_eq!(files[1].added, "y\n");
        assert!(!files[0].deleted && !files[1].deleted);
        assert!(parse_patch("*** Delete File: x\n")[0].deleted);
    }

    #[test]
    fn parse_multi_file_patch() {
        let input = "\
*** Update File: src/a.rs\n@@ -1,3 +1,4 @@\n@@ -1,3 +1,4 @@\n*** Add File: src/b.rs\n@@ -0,0 +1,2 @@\n";
        let files = parse_patch(input);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "src/a.rs");
        assert_eq!(files[1].path, "src/b.rs");
    }

    #[test]
    fn parse_begin_end_patch_wrapper() {
        let input = "\
*** Begin Patch
*** Update File: src/main.rs
@@ -1,3 +1,4 @@
*** End Patch
";
        let files = parse_patch(input);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "src/main.rs");
    }

    #[test]
    fn parse_empty_input_returns_empty() {
        let files = parse_patch("");
        assert!(files.is_empty());
    }

    #[test]
    fn parse_non_patch_text_is_malformed() {
        let files = parse_patch("some random text");
        assert!(files.is_empty());
    }

    /// A rename writes its destination: `*** Move to:` must surface the new
    /// path (with the hunk's added lines) so path rules see it.
    #[test]
    fn parse_move_to_reports_the_destination() {
        let input = "\
*** Begin Patch
*** Update File: src/x.json
*** Move to: .phronesis/verification-allowlist.json
@@
+{\"entries\": []}
*** End Patch
";
        let files = parse_patch(input);
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        let dest = files
            .iter()
            .find(|f| f.path == ".phronesis/verification-allowlist.json")
            .unwrap_or_else(|| panic!("move destination missing: {paths:?}"));
        assert!(dest.added.contains("entries"), "{}", dest.added);
    }

    #[test]
    fn parse_path_with_whitespace_strips() {
        let input = "*** Update File:   src/foo.rs   \n";
        let files = parse_patch(input);
        assert_eq!(files[0].path, "src/foo.rs");
    }
}
