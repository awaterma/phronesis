//! Parse Codex `apply_patch` payloads.
//!
//! Accepts the patch format Codex sends via `PreToolUse`/`PostToolUse`:
//! `*** Begin Patch`, `*** Update File:`, `*** Add File:`, `*** Delete File:`.
//!
//! This is a lightweight parser, not a general patch engine.

use super::PatchFile;

/// Parse a Codex `apply_patch` payload into per-file entries.
///
/// Recognised block types:
/// - `*** Begin Patch` / `*** End Patch` (optional wrapper)
/// - `*** Update File: <path>` — existing file being modified
/// - `*** Add File: <path>` — new file being created
/// - `*** Delete File: <path>` — file being removed
///
/// Returns an empty vec when the input doesn't match any recognised blocks.
pub fn parse_patch(input: &str) -> Vec<PatchFile> {
    let mut files = Vec::new();
    let mut in_patch = false;
    let mut current_path: Option<String> = None;
    let mut current_type: Option<PatchType> = None;

    for line in input.lines() {
        let trimmed = line.trim();

        if trimmed == "*** Begin Patch" {
            in_patch = true;
            continue;
        }
        if trimmed == "*** End Patch" {
            in_patch = false;
            continue;
        }

        // Outside a patch block, treat the whole text as a single command.
        if !in_patch {
            if !trimmed.is_empty() {
                files.push(PatchFile {
                    path: trimmed.to_string(),
                });
            }
            continue;
        }

        // Inside a patch block, look for file markers.
        if trimmed.starts_with("*** Update File:") {
            current_path = trimmed
                .trim_start_matches("*** Update File:")
                .trim()
                .to_string()
                .into();
            current_type = Some(PatchType::Update);
        } else if trimmed.starts_with("*** Add File:") {
            current_path = trimmed
                .trim_start_matches("*** Add File:")
                .trim()
                .to_string()
                .into();
            current_type = Some(PatchType::Add);
        } else if trimmed.starts_with("*** Delete File:") {
            current_path = trimmed
                .trim_start_matches("*** Delete File:")
                .trim()
                .to_string()
                .into();
            current_type = Some(PatchType::Delete);
        } else if trimmed == "*** Begin Hunk" || trimmed.starts_with("@@") {
            // Start of diff content — we don't parse hunks, just track files.
        }
    }

    files
}

#[derive(Clone, Copy)]
enum PatchType {
    Update,
    Add,
    Delete,
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
    fn parse_non_patch_text_returns_single_entry() {
        let files = parse_patch("some random text");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "some random text");
    }

    #[test]
    fn parse_path_with_whitespace_strips() {
        let input = "*** Update File:   src/foo.rs   \n";
        let files = parse_patch(input);
        assert_eq!(files[0].path, "src/foo.rs");
    }
}
