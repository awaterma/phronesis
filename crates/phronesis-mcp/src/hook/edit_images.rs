//! Full pre-edit and post-edit file images for changed-region mapping.
//!
//! Coverage and property hydration diff the old file against the new one
//! (`coverage::region_map::changed_regions`), so both sides must be the
//! WHOLE file. A host's edit payload carries only a snippet — a Claude Code
//! `Edit` is typically one `old_string` line — so the images are derived:
//!
//! - **pre-check**: old = the file on disk; new = that content with the
//!   edit applied, following the host's own matching contract (below).
//! - **post-check**: new = the file on disk; old = the edit reverse-applied,
//!   accepted only if re-applying the edit to it reproduces the disk content
//!   exactly; failing that, the host-reported pre-image (Claude Code's
//!   `tool_response.originalFile`) when present.
//!
//! Post-check reconstructs rather than stashing the pre-image at pre-check:
//! pre-check exits before any stash when a project has no `pre` rules (gap
//! rules are often `post`), a blocked pre-check never reaches post, and not
//! every host sends a `tool_use_id` to key a stash on. Reconstruction is
//! stateless and self-verifying.
//!
//! When no image can be derived — the edit does not apply (the host rejects
//! it too), the reverse is ambiguous, or a post-check `Write` arrives without
//! `originalFile` — the fallback treats the whole current file as changed.
//! That over-reports regions; it never under-reports them, so a gap rule
//! cannot go quiet for lack of the old side.

use serde_json::Value;

/// The two sides handed to region mapping. `old: None` means "no prior
/// content": every region in `new` counts as changed.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct FileImages {
    pub(super) old: Option<String>,
    pub(super) new: String,
}

impl FileImages {
    /// The conservative fallback: the whole of `content` is changed.
    fn whole(content: String) -> Self {
        Self {
            old: None,
            new: content,
        }
    }
}

/// How many occurrences of the search text the host requires.
#[derive(Clone, Copy)]
enum Occurrences {
    /// Claude Code `Edit` without `replace_all`; Gemini `replace` with its
    /// `expected_replacements` (default 1). All of them are replaced.
    Exactly(usize),
    /// Claude Code `Edit` with `replace_all: true`.
    AtLeastOne,
}

/// One string substitution in host terms.
struct Substitution<'a> {
    old: &'a str,
    new: &'a str,
    occurrences: Occurrences,
}

fn str_field<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

/// The substitutions an edit tool performs, in order. `None` for a tool that
/// is not a string-substitution edit, or a malformed payload.
fn substitutions<'a>(tool_name: &str, input: &'a Value) -> Option<Vec<Substitution<'a>>> {
    let one = |e: &'a Value, occurrences| {
        Some(Substitution {
            old: str_field(e, "old_string")?,
            new: str_field(e, "new_string")?,
            occurrences,
        })
    };
    let claude = |e: &'a Value| {
        let all = e.get("replace_all").and_then(Value::as_bool) == Some(true);
        one(
            e,
            if all {
                Occurrences::AtLeastOne
            } else {
                Occurrences::Exactly(1)
            },
        )
    };
    match tool_name {
        "Edit" => Some(vec![claude(input)?]),
        "MultiEdit" => {
            let edits = input.get("edits")?.as_array()?;
            if edits.is_empty() {
                return None;
            }
            edits.iter().map(claude).collect()
        }
        "replace" => {
            let n = input
                .get("expected_replacements")
                .and_then(Value::as_u64)
                .unwrap_or(1);
            let n = usize::try_from(n).ok().filter(|n| *n >= 1)?;
            Some(vec![one(input, Occurrences::Exactly(n))?])
        }
        _ => None,
    }
}

/// Apply one substitution under the host's contract. An empty `old` creates
/// a file: it applies only to empty content.
fn apply_one(text: &str, s: &Substitution<'_>) -> Option<String> {
    if s.old.is_empty() {
        return text.is_empty().then(|| s.new.to_string());
    }
    let count = text.matches(s.old).count();
    let ok = match s.occurrences {
        Occurrences::Exactly(n) => count == n,
        Occurrences::AtLeastOne => count >= 1,
    };
    ok.then(|| text.replace(s.old, s.new))
}

fn apply_all(text: &str, subs: &[Substitution<'_>]) -> Option<String> {
    subs.iter()
        .try_fold(text.to_string(), |acc, s| apply_one(&acc, s))
}

/// Undo the substitutions from the post-edit text, last first. The candidate
/// is accepted only when applying the edit to it reproduces `after`.
fn reverse_all(after: &str, subs: &[Substitution<'_>]) -> Option<String> {
    let mut text = after.to_string();
    for s in subs.iter().rev() {
        text = if s.old.is_empty() {
            // File creation: the pre-image was empty.
            (text == s.new).then(String::new)?
        } else if s.new.is_empty() {
            // A deletion leaves nothing to locate the removed text by.
            return None;
        } else {
            let inverse = Substitution {
                old: s.new,
                new: s.old,
                occurrences: s.occurrences,
            };
            apply_one(&text, &inverse)?
        };
    }
    (apply_all(&text, subs)? == after).then_some(text)
}

/// Pre-check images. `disk` is the file's current content (`None` when it
/// does not exist or cannot be read); `snippet` is the payload's proposed
/// content, the last resort when there is no file to apply the edit to.
pub(super) fn pre_images(
    tool_name: &str,
    input: &Value,
    disk: Option<String>,
    snippet: &str,
) -> FileImages {
    if matches!(tool_name, "Write" | "write_file") {
        return FileImages {
            old: disk,
            new: snippet.to_string(),
        };
    }
    let applied = substitutions(tool_name, input)
        .and_then(|subs| apply_all(disk.as_deref().unwrap_or(""), &subs));
    match (applied, disk) {
        (Some(new), disk) => FileImages { old: disk, new },
        (None, Some(disk)) => FileImages::whole(disk),
        (None, None) => FileImages::whole(snippet.to_string()),
    }
}

/// Post-check pre-image. `disk` is the file after the edit; `output` is the
/// host's tool response. `None` means no pre-image could be derived: the
/// caller diffs against nothing, so every region in `disk` counts as changed.
pub(super) fn post_old_image(
    tool_name: &str,
    input: &Value,
    output: Option<&Value>,
    disk: &str,
) -> Option<String> {
    substitutions(tool_name, input)
        .and_then(|subs| reverse_all(disk, &subs))
        .or_else(|| {
            output
                .and_then(|o| o.get("originalFile"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const FILE: &str = "fn a() {\n    1\n}\n\nfn b() {\n    2\n}\n";

    fn edit(old: &str, new: &str) -> Value {
        json!({"file_path": "f.rs", "old_string": old, "new_string": new})
    }

    #[test]
    fn edit_applies_once_to_the_disk_content() {
        let img = pre_images(
            "Edit",
            &edit("    1", "    10"),
            Some(FILE.into()),
            "    10",
        );
        assert_eq!(img.old.as_deref(), Some(FILE));
        assert_eq!(img.new, FILE.replace("    1\n", "    10\n"));
    }

    #[test]
    fn edit_without_replace_all_requires_a_unique_match() {
        let img = pre_images("Edit", &edit("fn ", "pub fn "), Some(FILE.into()), "x");
        assert_eq!(img, FileImages::whole(FILE.into()));
    }

    #[test]
    fn edit_with_replace_all_replaces_every_match() {
        let mut input = edit("fn ", "pub fn ");
        input["replace_all"] = json!(true);
        let img = pre_images("Edit", &input, Some(FILE.into()), "x");
        assert_eq!(img.new, FILE.replace("fn ", "pub fn "));
    }

    #[test]
    fn unmatched_edit_falls_back_to_the_whole_file() {
        let img = pre_images("Edit", &edit("absent", "x"), Some(FILE.into()), "x");
        assert_eq!(img, FileImages::whole(FILE.into()));
        let img = pre_images("Edit", &edit("absent", "x"), None, "x");
        assert_eq!(img, FileImages::whole("x".into()));
    }

    #[test]
    fn empty_old_string_creates_a_missing_file() {
        let img = pre_images("Edit", &edit("", "fn a() {}\n"), None, "fn a() {}\n");
        assert_eq!(img.old, None);
        assert_eq!(img.new, "fn a() {}\n");
    }

    #[test]
    fn multiedit_applies_in_order_and_fails_as_a_whole() {
        let input = json!({"edits": [
            {"old_string": "    1", "new_string": "    3"},
            {"old_string": "    3", "new_string": "    4"},
        ]});
        let img = pre_images("MultiEdit", &input, Some(FILE.into()), "x");
        assert_eq!(img.new, FILE.replace("    1\n", "    4\n"));
        let bad = json!({"edits": [
            {"old_string": "    1", "new_string": "    3"},
            {"old_string": "absent", "new_string": "y"},
        ]});
        let img = pre_images("MultiEdit", &bad, Some(FILE.into()), "x");
        assert_eq!(img, FileImages::whole(FILE.into()));
    }

    #[test]
    fn gemini_replace_honors_expected_replacements() {
        let input =
            json!({"old_string": "fn ", "new_string": "pub fn ", "expected_replacements": 2});
        let img = pre_images("replace", &input, Some(FILE.into()), "x");
        assert_eq!(img.new, FILE.replace("fn ", "pub fn "));
        let img = pre_images("replace", &edit("fn ", "pub fn "), Some(FILE.into()), "x");
        assert_eq!(
            img,
            FileImages::whole(FILE.into()),
            "default is exactly one"
        );
    }

    #[test]
    fn write_diffs_the_disk_content_against_the_payload() {
        let img = pre_images("Write", &json!({}), Some(FILE.into()), "new");
        assert_eq!(img.old.as_deref(), Some(FILE));
        assert_eq!(img.new, "new");
    }

    #[test]
    fn post_reverse_applies_a_unique_edit() {
        let after = FILE.replace("    1\n", "    10\n");
        let old = post_old_image("Edit", &edit("    1", "    10"), None, &after);
        assert_eq!(old.as_deref(), Some(FILE));
    }

    #[test]
    fn post_reverse_applies_multiedit_last_first() {
        let input = json!({"edits": [
            {"old_string": "    1", "new_string": "    3"},
            {"old_string": "    2", "new_string": "    4"},
        ]});
        let after = FILE
            .replace("    1\n", "    3\n")
            .replace("    2\n", "    4\n");
        let old = post_old_image("MultiEdit", &input, None, &after);
        assert_eq!(old.as_deref(), Some(FILE));
    }

    #[test]
    fn post_rejects_a_reverse_that_does_not_round_trip() {
        // `new_string` already existed in the file before the edit, so the
        // post image holds it twice: no unique reverse, no pre-image.
        let after = "fn a() {}\nfn a() {}\n";
        let old = post_old_image("Edit", &edit("fn b() {}", "fn a() {}"), None, after);
        assert_eq!(old, None);
    }

    #[test]
    fn post_deletion_uses_the_hosts_original_file_or_falls_back() {
        let after = FILE.replace("    1\n", "");
        let input = edit("    1\n", "");
        assert_eq!(post_old_image("Edit", &input, None, &after), None);
        let output = json!({"originalFile": FILE});
        let old = post_old_image("Edit", &input, Some(&output), &after);
        assert_eq!(old.as_deref(), Some(FILE));
    }

    #[test]
    fn post_write_needs_the_hosts_original_file() {
        assert_eq!(post_old_image("Write", &json!({}), None, "new"), None);
        let output = json!({"originalFile": FILE});
        let old = post_old_image("Write", &json!({}), Some(&output), "new");
        assert_eq!(old.as_deref(), Some(FILE));
        // A created file reports `originalFile: null`: nothing was there.
        let output = json!({"originalFile": null});
        assert_eq!(
            post_old_image("Write", &json!({}), Some(&output), "new"),
            None
        );
    }
}
