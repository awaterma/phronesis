//! Anonymize captured hook payloads before committing them as fixtures.
//!
//! Scrubs exactly the class of data that reaches outside the project —
//! `$HOME` paths, the OS username, session ids, transcript paths — and
//! leaves project-internal content byte-for-byte intact. See
//! `docs/superpowers/specs/2026-07-06-payload-contract-corpus-design.md`.

use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ScrubError {
    #[error("scrubbed output still contains the {what} in: {context}")]
    Residual { what: &'static str, context: String },
}

/// Minimum username length for bare-substring replacement. Shorter names
/// would corrupt ordinary text ("al" inside "align"); path-prefix rules
/// still cover them because paths embed the full `$HOME` prefix.
const MIN_BARE_USERNAME_LEN: usize = 3;

/// Canonical placeholder roots the scrubber itself writes. `scrub_str` must
/// never rewrite text under these prefixes: when the configured home dir is a
/// prefix of the placeholder root (e.g. home = `/home/dev`), a naive
/// find-and-replace loop matches its own freshly inserted replacement forever
/// (non-termination + unbounded `external` growth), and re-scrubbing an
/// already-scrubbed fixture would mangle the canonical paths.
const PLACEHOLDER_PREFIXES: [&str; 3] = [
    "/home/dev/project",
    "/home/dev/external/",
    "/home/dev/.claude/",
];

pub struct Scrubber {
    home: String,
    user: String,
    project_root: String,
    /// Unique external paths seen so far; index = placeholder number, so
    /// the same path always maps to the same `/home/dev/external/pN`.
    external: Vec<String>,
}

impl Scrubber {
    pub fn new(home: &str, project_root: &str) -> Self {
        let user = std::path::Path::new(home)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self {
            home: home.trim_end_matches('/').to_string(),
            user,
            project_root: project_root.trim_end_matches('/').to_string(),
            external: Vec::new(),
        }
    }

    /// Recursively rewrite every string in `v` per the scrub rules. Keys
    /// named `session_id` / `transcript_path` get fixed placeholder values.
    pub fn scrub_value(&mut self, v: &mut Value) {
        match v {
            Value::String(s) => *s = self.scrub_str(s),
            Value::Array(items) => {
                for item in items {
                    self.scrub_value(item);
                }
            }
            Value::Object(map) => {
                for (key, val) in map.iter_mut() {
                    if is_session_key(key) {
                        *val = Value::String("sess-00000000".to_string());
                    } else if is_transcript_key(key) {
                        *val = Value::String("/home/dev/.claude/transcript.jsonl".to_string());
                    } else {
                        self.scrub_value(val);
                    }
                }
            }
            _ => {}
        }
    }

    fn scrub_str(&mut self, s: &str) -> String {
        // 1. Project-root prefix → canonical fixture root.
        let mut out = s.replace(&self.project_root, "/home/dev/project");
        // 2. Any remaining $HOME-rooted path → indexed external placeholder.
        // The cursor only moves forward — each iteration resumes past the
        // text it just inserted — and canonical placeholder paths are skipped
        // outright, so replacement terminates and is a fixpoint even when
        // `home` is a prefix of the placeholder root (e.g. `/home/dev`).
        let mut search_from = 0;
        while let Some(rel) = out[search_from..].find(&self.home) {
            let start = search_from + rel;
            if PLACEHOLDER_PREFIXES
                .iter()
                .any(|p| out[start..].starts_with(p))
            {
                search_from = start + self.home.len();
                continue;
            }
            let end = out[start..]
                .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ':' | ','))
                .map(|off| start + off)
                .unwrap_or(out.len());
            let path = out[start..end].to_string();
            let n = match self.external.iter().position(|p| p == &path) {
                Some(i) => i,
                None => {
                    self.external.push(path);
                    self.external.len() - 1
                }
            };
            let replacement = format!("/home/dev/external/p{n}");
            let replacement_len = replacement.len();
            out.replace_range(start..end, &replacement);
            search_from = start + replacement_len;
        }
        // 3. Bare username anywhere else (long enough to be unambiguous).
        if self.user.len() >= MIN_BARE_USERNAME_LEN {
            out = out.replace(&self.user, "dev");
        }
        out
    }

    /// Post-scrub verification. Split by residual shape (adversarial-review
    /// finding #1): a surviving `$HOME` path, or the username *as a path
    /// component*, is an unambiguous leak → `Err`. The bare username as a
    /// free-text token is NOT a hard failure — it is returned by
    /// [`warnings`](Self::warnings) for a human to adjudicate — so a
    /// legitimately-scrubbed fixture whose content happens to contain the
    /// username as a word stays idempotent and exits 0.
    pub fn verify(&self, v: &Value) -> Result<(), ScrubError> {
        let rendered = v.to_string();
        if rendered.contains(&self.home) {
            return Err(ScrubError::Residual {
                what: "home directory",
                context: excerpt(&rendered, &self.home),
            });
        }
        // Username *as a path component* (`/…/<user>/…`) is still a leak.
        if self.user.len() >= MIN_BARE_USERNAME_LEN {
            let as_path = format!("/{}/", self.user);
            let trailing = format!("/{}", self.user);
            if rendered.contains(&as_path) || rendered.ends_with(&trailing) {
                return Err(ScrubError::Residual {
                    what: "username in a path",
                    context: excerpt(&rendered, &self.user),
                });
            }
        }
        Ok(())
    }

    /// Non-fatal residuals for the human reviewer: the bare username appearing
    /// as a free-text token. Empty when nothing needs a look.
    pub fn warnings(&self, v: &Value) -> Vec<String> {
        let rendered = v.to_string();
        if self.user.len() >= MIN_BARE_USERNAME_LEN && rendered.contains(&self.user) {
            return vec![format!(
                "username {:?} appears as a free-text token (not a path) — review: {}",
                self.user,
                excerpt(&rendered, &self.user)
            )];
        }
        Vec::new()
    }
}

/// True for session-id-style keys, compared case- and separator-insensitively
/// (`session_id`, `sessionId`, `SessionID` all match) — finding #3: a CLI
/// sending `sessionId` must not evade scrubbing.
fn is_session_key(key: &str) -> bool {
    normalize_key(key) == "sessionid"
}

fn is_transcript_key(key: &str) -> bool {
    normalize_key(key) == "transcriptpath"
}

/// Lowercase and strip `_`/`-` so key-name variants collapse to one form.
fn normalize_key(key: &str) -> String {
    key.chars()
        .filter(|c| *c != '_' && *c != '-')
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// A short window around the first occurrence of `needle`, for error messages.
fn excerpt(haystack: &str, needle: &str) -> String {
    let Some(pos) = haystack.find(needle) else {
        return String::new();
    };
    let start = pos.saturating_sub(20);
    let end = (pos + needle.len() + 20).min(haystack.len());
    haystack[start..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scrubber() -> Scrubber {
        Scrubber::new("/Users/alicejones", "/Users/alicejones/Git/myproject")
    }

    #[test]
    fn project_root_paths_become_home_dev_project() {
        let mut v =
            json!({"tool_input": {"file_path": "/Users/alicejones/Git/myproject/src/lib.rs"}});
        scrubber().scrub_value(&mut v);
        assert_eq!(v["tool_input"]["file_path"], "/home/dev/project/src/lib.rs");
    }

    #[test]
    fn external_home_paths_get_indexed_placeholders() {
        let mut v = json!({
            "a": "/Users/alicejones/Git/otherrepo/src/main.rs",
            "b": "/Users/alicejones/Git/otherrepo/src/main.rs",
            "c": "/Users/alicejones/.cargo/bin/tool"
        });
        let mut s = scrubber();
        s.scrub_value(&mut v);
        // Same external path → same placeholder; different path → different index.
        assert_eq!(v["a"], v["b"]);
        assert_ne!(v["a"], v["c"]);
        let a = v["a"].as_str().expect("string");
        assert!(a.starts_with("/home/dev/external/p"), "got {a}");
        // The sibling repo name must be gone entirely.
        assert!(!v.to_string().contains("otherrepo"));
    }

    #[test]
    fn username_is_replaced_everywhere() {
        let mut v = json!({"command": "echo hello alicejones"});
        scrubber().scrub_value(&mut v);
        assert_eq!(v["command"], "echo hello dev");
    }

    #[test]
    fn session_id_and_transcript_path_keys_get_fixed_placeholders() {
        let mut v = json!({
            "session_id": "550e8400-e29b-41d4-a716-446655440000",
            "transcript_path": "/Users/alicejones/.claude/projects/x/y.jsonl",
            "nested": {"session_id": "another-id"}
        });
        scrubber().scrub_value(&mut v);
        assert_eq!(v["session_id"], "sess-00000000");
        assert_eq!(v["transcript_path"], "/home/dev/.claude/transcript.jsonl");
        assert_eq!(v["nested"]["session_id"], "sess-00000000");
    }

    #[test]
    fn case_and_separator_variant_id_keys_are_scrubbed() {
        // Finding #3: a CLI sending camelCase or no-separator keys must not leak.
        let mut v = json!({
            "sessionId": "abc",
            "SessionID": "def",
            "transcriptPath": "/Users/alicejones/.claude/t.jsonl"
        });
        scrubber().scrub_value(&mut v);
        assert_eq!(v["sessionId"], "sess-00000000");
        assert_eq!(v["SessionID"], "sess-00000000");
        assert_eq!(v["transcriptPath"], "/home/dev/.claude/transcript.jsonl");
    }

    #[test]
    fn scrub_is_idempotent() {
        let mut v = json!({
            "file_path": "/Users/alicejones/Git/myproject/src/a.rs",
            "session_id": "x"
        });
        let mut s = scrubber();
        s.scrub_value(&mut v);
        let once = v.clone();
        s.scrub_value(&mut v);
        assert_eq!(v, once);
    }

    #[test]
    fn project_internal_content_is_untouched() {
        let mut v = json!({"tool_input": {"new_string": "fn main() { let x = 1; }", "file_path": "src/lib.rs"}});
        let before = v.clone();
        scrubber().scrub_value(&mut v);
        assert_eq!(
            v, before,
            "relative paths and code content must pass through verbatim"
        );
    }

    #[test]
    fn verify_flags_residual_home_path() {
        let s = scrubber();
        let v = json!({"sneaky": "path is /Users/alicejones/secret"});
        assert!(s.verify(&v).is_err());
        let clean = json!({"ok": "/home/dev/project/src/lib.rs"});
        assert!(s.verify(&clean).is_ok());
    }

    #[test]
    fn verify_flags_username_as_path_component_but_not_as_free_token() {
        // Finding #1 resolution: username in a path is a hard leak; username
        // as a free-text word is a warning, not a verify failure — otherwise
        // a correctly-scrubbed fixture whose content mentions the word can
        // never pass, breaking idempotence.
        let s = scrubber();
        // Path component → Err.
        let leak = json!({"p": "/opt/alicejones/thing"});
        assert!(s.verify(&leak).is_err());
        // Free-text token → Ok from verify, but surfaced by warnings().
        let token = json!({"command": "echo alicejones was here"});
        assert!(s.verify(&token).is_ok());
        assert_eq!(s.warnings(&token).len(), 1);
        // Nothing to say about clean content.
        let clean = json!({"command": "cargo build"});
        assert!(s.warnings(&clean).is_empty());
    }

    #[test]
    fn home_dev_home_on_already_scrubbed_content_terminates_and_is_fixpoint() {
        // C2 regression: when the configured home dir is a prefix of the
        // placeholder root (home = "/home/dev"), the old find-loop matched
        // its own freshly inserted replacement forever. Re-scrubbing
        // already-scrubbed content must terminate AND leave it unchanged.
        let mut s = Scrubber::new("/home/dev", "/tmp/someproject");
        let mut v = json!({
            "cwd": "/home/dev/project",
            "tool_input": {"file_path": "/home/dev/project/src/lib.rs"},
            "external": "/home/dev/external/p0",
            "transcript_path": "/home/dev/.claude/transcript.jsonl"
        });
        let before = v.clone();
        s.scrub_value(&mut v);
        assert_eq!(
            v, before,
            "already-scrubbed content must be a fixpoint under home=/home/dev"
        );
        // And a second pass stays put too.
        s.scrub_value(&mut v);
        assert_eq!(v, before);
    }

    #[test]
    fn home_dev_home_still_scrubs_non_placeholder_paths() {
        // The C2 guard must not stop legitimate scrubbing for a user whose
        // home really is /home/dev: paths outside the canonical placeholder
        // roots still get anonymized, and the loop still terminates even
        // though every inserted replacement contains the home prefix.
        let mut s = Scrubber::new("/home/dev", "/home/dev/myproject");
        let mut v = json!({
            "a": "/home/dev/otherrepo/src/main.rs",
            "b": "/home/dev/myproject/src/lib.rs"
        });
        s.scrub_value(&mut v);
        assert_eq!(v["b"], "/home/dev/project/src/lib.rs");
        let a = v["a"].as_str().expect("string");
        assert!(a.starts_with("/home/dev/external/p"), "got {a}");
        assert!(!v.to_string().contains("otherrepo"));
    }

    #[test]
    fn short_usernames_are_not_blindly_replaced() {
        // A 1-2 char username would shred ordinary text; the scrubber must
        // refuse to substring-replace it and rely on path rules only.
        let mut s = Scrubber::new("/home/al", "/home/al/proj");
        let mut v = json!({"command": "cargo align --all"});
        s.scrub_value(&mut v);
        assert_eq!(v["command"], "cargo align --all");
        // ...and verify/warnings must not fire on the short name either.
        assert!(s.verify(&v).is_ok());
        assert!(s.warnings(&v).is_empty());
    }
}
