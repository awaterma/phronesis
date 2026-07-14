//! Anonymize captured hook payloads before committing them as fixtures.
//!
//! Two distinct mechanisms with different guarantees:
//!
//! - **Deterministic anonymization** ([`Scrubber`]) rewrites exactly the
//!   enumerated identity classes: project-root paths (to
//!   `/home/dev/project`), other `$HOME`-rooted paths (to indexed
//!   `/home/dev/external/pN` placeholders), session ids and transcript
//!   paths under recognized key variants, and username path components /
//!   free-text tokens. Project-internal content passes through
//!   byte-for-byte.
//! - **Residual-risk detection** ([`detect_residual_risks`]) runs
//!   conservative, bounded pattern checks over the anonymized output for
//!   common leak classes: credential-bearing URLs, private-key headers,
//!   token/secret assignments, secret-suggesting environment keys,
//!   absolute paths outside the canonical placeholder roots, and email
//!   addresses. Findings are classified [`Severity::Error`] or
//!   [`Severity::Warning`]; diagnostics truncate the matched text so a
//!   suspected secret is never echoed in full.
//!
//! scrub-payload performs deterministic anonymization and detects several
//! common leak classes. It is not a proof that arbitrary source or command
//! content contains no secrets. Review scrubbed fixtures before committing
//! them.
//!
//! See `docs/superpowers/specs/2026-07-06-payload-contract-corpus-design.md`
//! and `docs/superpowers/specs/2026-07-12-evidence-integrity-hardening.md`.

use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ScrubError {
    #[error("scrubbed output still contains the {what} in: {context}")]
    Residual { what: &'static str, context: String },
    #[error("invalid {which}: {reason}")]
    InvalidRoot { which: &'static str, reason: String },
    #[error("residual-risk detector failed to initialize: {0}")]
    DetectorInit(String),
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
    /// Validated construction (evidence-integrity spec, Task 1). Rejects
    /// roots that would make substring scrubbing unsafe: empty or
    /// whitespace-only values, relative paths, and the filesystem root `/`.
    /// Trailing separators are normalized away without changing which
    /// directory the root names. A project root outside the home directory
    /// is explicitly supported: project-root replacement runs before home
    /// replacement, so the two never conflict.
    pub fn new(home: &str, project_root: &str) -> Result<Self, ScrubError> {
        let home = validate_root("home directory", home)?;
        let project_root = validate_root("project root", project_root)?;
        let user = std::path::Path::new(&home)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(Self {
            home,
            user,
            project_root,
            external: Vec::new(),
        })
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
        let out = s.replace(&self.project_root, "/home/dev/project");
        // 2. Any remaining $HOME-rooted path → indexed external placeholder.
        let mut out = self.scrub_external_paths(out);
        // 3. Bare username anywhere else (long enough to be unambiguous).
        if self.user.len() >= MIN_BARE_USERNAME_LEN {
            out = out.replace(&self.user, "dev");
        }
        out
    }

    fn scrub_external_paths(&mut self, mut out: String) -> String {
        // The cursor only moves forward — each iteration resumes past the
        // text it just inserted — and canonical placeholder paths are skipped
        // outright, so replacement terminates and is a fixpoint even when
        // `home` is a prefix of the placeholder root (e.g. `/home/dev`).
        let mut search_from = 0;
        while let Some(start) = self.home_match_start(&out, search_from) {
            if PLACEHOLDER_PREFIXES
                .iter()
                .any(|p| out[start..].starts_with(p))
            {
                search_from = start + self.home.len();
                continue;
            }
            let end = external_path_end(&out, start);
            let path = out[start..end].to_string();
            let replacement = self.external_replacement(path);
            let replacement_len = replacement.len();
            out.replace_range(start..end, &replacement);
            search_from = start + replacement_len;
        }
        out
    }

    fn home_match_start(&self, out: &str, search_from: usize) -> Option<usize> {
        out[search_from..]
            .find(&self.home)
            .map(|relative| search_from + relative)
    }

    fn external_replacement(&mut self, path: String) -> String {
        let index = match self.external.iter().position(|known| known == &path) {
            Some(index) => index,
            None => {
                self.external.push(path);
                self.external.len() - 1
            }
        };
        format!("/home/dev/external/p{index}")
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

fn external_path_end(out: &str, start: usize) -> usize {
    out[start..]
        .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ':' | ','))
        .map(|offset| start + offset)
        .unwrap_or(out.len())
}

/// Validate and normalize one scrub root (evidence-integrity spec, Task 1).
///
/// Rejections:
/// - empty / whitespace-only — an empty needle turns substring replacement
///   into pathological or non-terminating behavior;
/// - relative paths — ambiguous: the scrubbed meaning would depend on an
///   unstated current directory;
/// - the filesystem root `/` — it would rewrite every absolute path in the
///   payload.
///
/// Normalization: surrounding whitespace and trailing `/` separators are
/// trimmed; neither changes the filesystem identity of the root.
fn validate_root(which: &'static str, value: &str) -> Result<String, ScrubError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ScrubError::InvalidRoot {
            which,
            reason: "must not be empty or whitespace-only".to_string(),
        });
    }
    if !std::path::Path::new(trimmed).is_absolute() {
        return Err(ScrubError::InvalidRoot {
            which,
            reason: format!("must be an absolute path (got the relative path {trimmed:?})"),
        });
    }
    let normalized = trimmed.trim_end_matches('/');
    if normalized.is_empty() {
        return Err(ScrubError::InvalidRoot {
            which,
            reason: "the filesystem root `/` cannot be used as a scrub root".to_string(),
        });
    }
    Ok(normalized.to_string())
}

// ─────────────────────────────────────────────────────────────────────
// Residual-risk detection (evidence-integrity spec, Task 2)
//
// Runs over the compact-rendered JSON of an ALREADY-SCRUBBED record.
// Conservative and bounded by design: this detects several common leak
// classes; it is not a proof that arbitrary content contains no secrets.
// ─────────────────────────────────────────────────────────────────────

/// Classification of a residual-risk finding, per the spec's failure
/// policy. `Error` findings abort the run (nonzero exit; under `--write`
/// nothing is written, not even the backup). `Warning` findings go to
/// stderr and the run still exits 0, keeping scrubbing idempotent — a
/// human adjudicates them before committing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// One residual-risk finding. `hint` is a truncated excerpt of the matched
/// text — never the full suspected secret.
#[derive(Debug)]
pub struct Finding {
    pub severity: Severity,
    pub what: &'static str,
    pub hint: String,
}

struct Detector {
    what: &'static str,
    severity: Severity,
    /// When true, a match containing no ASCII digit is downgraded to
    /// `Warning` ("possible identity token" in the spec's failure policy):
    /// prose like `password: mandatory` must not hard-fail, while real
    /// keys and tokens virtually always contain digits.
    digitless_downgrade: bool,
    re: Regex,
}

/// Detection runs on the rendered JSON, where inner string quotes appear
/// as `\"` — hence the `["'\\]{0,4}` bridges around assignment operators.
fn build_detectors() -> Result<Vec<Detector>, ScrubError> {
    let specs: &[(&'static str, Severity, bool, &str)] = &[
        (
            "private-key header",
            Severity::Error,
            false,
            r#"-----BEGIN [A-Z0-9 ]{0,40}PRIVATE KEY-----"#,
        ),
        (
            "credential-bearing URL",
            Severity::Error,
            false,
            r#"[a-zA-Z][a-zA-Z0-9+.\-]{0,15}://[^/\s:@"',]{0,64}:[^@\s"',]{1,256}@"#,
        ),
        (
            "token/secret assignment",
            Severity::Error,
            true,
            r#"(?i)\b(?:api[_-]?key|access[_-]?token|auth[_-]?token|refresh[_-]?token|client[_-]?secret|secret[_-]?key|private[_-]?key|password|passwd)["'\\]{0,4}\s*[:=]\s*["'\\]{0,4}[A-Za-z0-9+/_.\-]{8,}"#,
        ),
        (
            "bearer token",
            Severity::Error,
            true,
            r#"(?i)\bbearer(?:\s+|:\s*)[A-Za-z0-9\-._~+/]{16,}"#,
        ),
        (
            "secret-suggesting environment key",
            Severity::Error,
            true,
            r#"\b[A-Z][A-Z0-9_]{2,40}(?:TOKEN|SECRET|PASSWORD|APIKEY|API_KEY|PRIVATE_KEY|CREDENTIALS)["'\\]{0,4}\s*[:=]\s*["'\\]{0,4}\S{4,}"#,
        ),
        (
            "email address",
            Severity::Warning,
            false,
            r#"\b[A-Za-z0-9._%+\-]{1,64}@[A-Za-z0-9.\-]{1,128}\.[A-Za-z]{2,24}\b"#,
        ),
    ];
    let mut out = Vec::with_capacity(specs.len());
    for &(what, severity, digitless_downgrade, pattern) in specs {
        let re =
            Regex::new(pattern).map_err(|e| ScrubError::DetectorInit(format!("{what}: {e}")))?;
        out.push(Detector {
            what,
            severity,
            digitless_downgrade,
            re,
        });
    }
    Ok(out)
}

fn detectors() -> Result<&'static [Detector], ScrubError> {
    static CELL: OnceLock<Vec<Detector>> = OnceLock::new();
    if let Some(built) = CELL.get() {
        return Ok(built.as_slice());
    }
    let built = build_detectors()?;
    Ok(CELL.get_or_init(|| built).as_slice())
}

/// A bounded absolute path: `/seg/seg[...]`, two or more segments.
const ABS_PATH_PATTERN: &str = r#"/[A-Za-z0-9._+\-]{1,64}(?:/[A-Za-z0-9._+\-]{1,64}){1,32}"#;

fn absolute_path_regex() -> Result<&'static Regex, ScrubError> {
    static CELL: OnceLock<Regex> = OnceLock::new();
    if let Some(re) = CELL.get() {
        return Ok(re);
    }
    let re = Regex::new(ABS_PATH_PATTERN)
        .map_err(|e| ScrubError::DetectorInit(format!("absolute-path pattern: {e}")))?;
    Ok(CELL.get_or_init(|| re))
}

/// Absolute-path prefixes that carry no host identity: the canonical
/// scrub placeholders (must stay in sync with `PLACEHOLDER_PREFIXES`)
/// plus identity-neutral system roots. Everything else absolute is a
/// residual-risk error — `/private/tmp/...`, `/Users/...`, `/var/...`
/// and friends can all carry usernames or session-specific state.
const ALLOWED_ABSOLUTE_PREFIXES: &[&str] = &[
    "/home/dev/project",
    "/home/dev/external/",
    "/home/dev/.claude/",
    "/usr/",
    "/bin/",
    "/sbin/",
    "/etc/",
    "/opt/",
    "/lib/",
    "/lib64/",
    "/dev/",
    "/proc/",
    "/sys/",
    "/run/",
    "/System/",
    "/Library/",
];

fn is_allowed_absolute(path: &str) -> bool {
    ALLOWED_ABSOLUTE_PREFIXES.iter().any(|prefix| {
        let p = prefix.trim_end_matches('/');
        path == p || (path.starts_with(p) && path.as_bytes().get(p.len()) == Some(&b'/'))
    })
}

/// Is the text before a `/...` match a path boundary? A preceding `/`,
/// `.` (relative-path context like `./x` or `../x`), or word character
/// means the match is the tail of a URL, a relative path, or a date —
/// not an absolute path. Exception: in rendered JSON a newline/tab is
/// the two-character escape `\n` / `\t` / `\r`, so a path right after
/// one IS at a line boundary.
fn is_path_boundary(before: &str) -> bool {
    let Some(prev) = before.chars().next_back() else {
        return true;
    };
    if prev == '/' || prev == '.' {
        return false;
    }
    if prev.is_alphanumeric() {
        return matches!(prev, 'n' | 't' | 'r')
            && before.len() >= 2
            && before.as_bytes()[before.len() - 2] == b'\\';
    }
    true
}

fn collect_disallowed_absolute_paths(
    rendered: &str,
    findings: &mut Vec<Finding>,
) -> Result<(), ScrubError> {
    let re = absolute_path_regex()?;
    for m in re.find_iter(rendered) {
        if !is_path_boundary(&rendered[..m.start()]) {
            continue;
        }
        if !is_allowed_absolute(m.as_str()) {
            findings.push(Finding {
                severity: Severity::Error,
                what: "absolute path outside the project placeholder roots",
                hint: redacted_hint(m.as_str()),
            });
        }
    }
    Ok(())
}

/// Residual-risk detection over an already-scrubbed record. Returns every
/// finding; the caller decides how to surface them ([`Severity`] documents
/// the CLI policy).
pub fn detect_residual_risks(v: &Value) -> Result<Vec<Finding>, ScrubError> {
    let rendered = v.to_string();
    let mut findings = Vec::new();
    for d in detectors()? {
        for m in d.re.find_iter(&rendered) {
            let severity =
                if d.digitless_downgrade && !m.as_str().bytes().any(|b| b.is_ascii_digit()) {
                    Severity::Warning
                } else {
                    d.severity
                };
            findings.push(Finding {
                severity,
                what: d.what,
                hint: redacted_hint(m.as_str()),
            });
        }
    }
    collect_disallowed_absolute_paths(&rendered, &mut findings)?;
    Ok(findings)
}

/// First 12 characters of the match plus a redacted-length note — a
/// suspected secret is never echoed in full.
fn redacted_hint(matched: &str) -> String {
    const SHOWN: usize = 12;
    let head: String = matched.chars().take(SHOWN).collect();
    let total = matched.chars().count();
    if total > SHOWN {
        format!("{head}…[{} more chars redacted]", total - SHOWN)
    } else {
        head
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
        Scrubber::new("/Users/alicejones", "/Users/alicejones/Git/myproject").expect("valid roots")
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
        let mut s = Scrubber::new("/home/dev", "/tmp/someproject").expect("valid roots");
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
        let mut s = Scrubber::new("/home/dev", "/home/dev/myproject").expect("valid roots");
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
        let mut s = Scrubber::new("/home/al", "/home/al/proj").expect("valid roots");
        let mut v = json!({"command": "cargo align --all"});
        s.scrub_value(&mut v);
        assert_eq!(v["command"], "cargo align --all");
        // ...and verify/warnings must not fire on the short name either.
        assert!(s.verify(&v).is_ok());
        assert!(s.warnings(&v).is_empty());
    }

    #[test]
    fn empty_and_whitespace_roots_are_rejected() {
        assert!(Scrubber::new("", "/Users/a/proj").is_err());
        assert!(Scrubber::new("/Users/a", "").is_err());
        assert!(Scrubber::new("   ", "/Users/a/proj").is_err());
        assert!(Scrubber::new("/Users/a", "\t\n").is_err());
    }

    #[test]
    fn filesystem_root_is_rejected_as_either_root() {
        assert!(Scrubber::new("/", "/Users/a/proj").is_err());
        assert!(Scrubber::new("/Users/a", "/").is_err());
        assert!(Scrubber::new("/Users/a", "///").is_err());
    }

    #[test]
    fn relative_roots_are_rejected() {
        assert!(Scrubber::new("Users/alicejones", "/Users/alicejones/p").is_err());
        assert!(Scrubber::new("/Users/alicejones", "Git/myproject").is_err());
        assert!(Scrubber::new("/Users/alicejones", "./proj").is_err());
    }

    #[test]
    fn trailing_separators_are_normalized() {
        let mut s = Scrubber::new("/Users/alicejones/", "/Users/alicejones/Git/myproject///")
            .expect("trailing separators are valid");
        let mut v = json!({"file_path": "/Users/alicejones/Git/myproject/src/lib.rs"});
        s.scrub_value(&mut v);
        assert_eq!(v["file_path"], "/home/dev/project/src/lib.rs");
    }

    #[test]
    fn adversarial_repeated_home_prefix_terminates() {
        // Non-termination pin: a giant blob of back-to-back home prefixes
        // must scrub in one bounded pass and leave no residual.
        let mut s = scrubber();
        let mut v = json!({ "blob": "/Users/alicejones".repeat(200) });
        s.scrub_value(&mut v);
        assert!(!v.to_string().contains("alicejones"));
    }

    // ── Residual-risk detection (spec Task 2) ──

    fn error_count(findings: &[Finding]) -> usize {
        findings
            .iter()
            .filter(|f| f.severity == Severity::Error)
            .count()
    }

    #[test]
    fn detector_regexes_compile() {
        // Loader-level pin: a mis-escaped pattern fails HERE, not at first
        // CLI use. Also pins the detector count so silent drops are caught.
        let built = build_detectors().expect("every detector pattern compiles");
        assert_eq!(built.len(), 6, "expected exactly 6 pattern detectors");
        assert!(
            absolute_path_regex().is_ok(),
            "absolute-path pattern compiles"
        );
    }

    #[test]
    fn detects_api_key_assignment_in_shell_command() {
        let v = json!({"command": "export API_KEY=SUPERSECRETVALUE123456"});
        let findings = detect_residual_risks(&v).expect("detectors run");
        assert!(
            findings
                .iter()
                .any(|f| f.severity == Severity::Error && f.what == "token/secret assignment"),
            "got {findings:?}"
        );
    }

    #[test]
    fn detects_bearer_token() {
        let v = json!({"headers": "Authorization: Bearer abc123def456ghi789jkl"});
        let findings = detect_residual_risks(&v).expect("detectors run");
        assert!(
            findings
                .iter()
                .any(|f| f.severity == Severity::Error && f.what == "bearer token"),
            "got {findings:?}"
        );
    }

    #[test]
    fn detects_colon_separated_bearer_token() {
        let v = json!({"headers": "Authorization: Bearer:abc123def456ghi789jkl"});
        let findings = detect_residual_risks(&v).expect("detectors run");
        assert!(
            findings
                .iter()
                .any(|f| f.severity == Severity::Error && f.what == "bearer token"),
            "got {findings:?}"
        );
    }

    #[test]
    fn detects_credential_bearing_url() {
        let v = json!({"command": "git clone https://alice:hunter2pass@github.com/x/y.git"});
        let findings = detect_residual_risks(&v).expect("detectors run");
        assert!(
            findings
                .iter()
                .any(|f| f.severity == Severity::Error && f.what == "credential-bearing URL"),
            "got {findings:?}"
        );
    }

    #[test]
    fn detects_credential_bearing_url_with_empty_username() {
        let v = json!({"command": "git clone https://:hunter2pass@github.com/x/y.git"});
        let findings = detect_residual_risks(&v).expect("detectors run");
        assert!(
            findings
                .iter()
                .any(|f| f.severity == Severity::Error && f.what == "credential-bearing URL"),
            "got {findings:?}"
        );
    }

    #[test]
    fn detects_pem_private_key_header() {
        let v = json!({"new_string": "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEAfake"});
        let findings = detect_residual_risks(&v).expect("detectors run");
        assert!(
            findings
                .iter()
                .any(|f| f.severity == Severity::Error && f.what == "private-key header"),
            "got {findings:?}"
        );
    }

    #[test]
    fn detects_secret_env_key_with_value() {
        let v = json!({"command": "GITHUB_TOKEN=ghp_abc123xyz789 gh api /user"});
        let findings = detect_residual_risks(&v).expect("detectors run");
        assert!(
            findings
                .iter()
                .any(|f| f.severity == Severity::Error
                    && f.what == "secret-suggesting environment key"),
            "got {findings:?}"
        );
    }

    #[test]
    fn env_key_with_placeholder_value_downgrades_to_warning() {
        // Docs prose: a secret-suggesting key with a digit-less placeholder
        // value is a "possible identity token" warning, not an error.
        let v = json!({"doc": "set GITHUB_TOKEN=<yourtoken> before running"});
        let findings = detect_residual_risks(&v).expect("detectors run");
        assert_eq!(error_count(&findings), 0, "got {findings:?}");
        assert!(
            findings.iter().any(|f| f.severity == Severity::Warning),
            "got {findings:?}"
        );
    }

    #[test]
    fn flags_absolute_path_outside_home() {
        let v = json!({"command": "cat /private/tmp/cap/payloads.jsonl"});
        let findings = detect_residual_risks(&v).expect("detectors run");
        assert!(
            findings.iter().any(|f| f.severity == Severity::Error
                && f.what == "absolute path outside the project placeholder roots"),
            "got {findings:?}"
        );
        // Same path right after a newline inside a multiline command.
        let v2 = json!({"command": "echo hi\ncat /private/tmp/cap/payloads.jsonl"});
        let findings2 = detect_residual_risks(&v2).expect("detectors run");
        assert!(error_count(&findings2) >= 1, "got {findings2:?}");
    }

    #[test]
    fn allows_placeholder_and_system_paths() {
        let v = json!({
            "cwd": "/home/dev/project",
            "file_path": "/home/dev/project/src/lib.rs",
            "transcript_path": "/home/dev/.claude/transcript.jsonl",
            "external": "/home/dev/external/p0",
            "command": "/usr/bin/env python3 /home/dev/project/x.py"
        });
        let findings = detect_residual_risks(&v).expect("detectors run");
        assert!(findings.is_empty(), "got {findings:?}");
    }

    #[test]
    fn allows_relative_paths_and_benign_password_words() {
        let v = json!({
            "file_path": "src/lib.rs",
            "content": "the password field is required"
        });
        let findings = detect_residual_risks(&v).expect("detectors run");
        assert!(findings.is_empty(), "got {findings:?}");
    }

    #[test]
    fn password_prose_with_colon_is_warning_not_error() {
        let v = json!({"content": "password: mandatory"});
        let findings = detect_residual_risks(&v).expect("detectors run");
        assert_eq!(error_count(&findings), 0, "got {findings:?}");
        assert!(
            findings.iter().any(|f| f.severity == Severity::Warning),
            "got {findings:?}"
        );
    }

    #[test]
    fn email_address_is_warning_not_error() {
        let v = json!({"command": "git log --author=someone@example.com"});
        let findings = detect_residual_risks(&v).expect("detectors run");
        assert_eq!(error_count(&findings), 0, "got {findings:?}");
        assert!(
            findings
                .iter()
                .any(|f| f.severity == Severity::Warning && f.what == "email address"),
            "got {findings:?}"
        );
    }

    #[test]
    fn diagnostics_never_echo_full_secret_values() {
        let v = json!({"command": "export API_KEY=SUPERSECRETVALUE123456"});
        let findings = detect_residual_risks(&v).expect("detectors run");
        assert!(!findings.is_empty());
        for f in &findings {
            assert!(
                !f.hint.contains("SECRETVALUE123456"),
                "hint must truncate the secret: {}",
                f.hint
            );
        }
    }

    #[test]
    fn scrubbed_output_of_a_normal_capture_has_no_findings() {
        // Idempotence guard: everything the scrubber itself writes must be
        // invisible to the detectors, or clean fixtures could never pass.
        let mut s = scrubber();
        let mut v = json!({
            "session_id": "550e8400-e29b-41d4-a716-446655440000",
            "transcript_path": "/Users/alicejones/.claude/projects/x/y.jsonl",
            "cwd": "/Users/alicejones/Git/myproject",
            "tool_input": {
                "file_path": "/Users/alicejones/Git/myproject/src/lib.rs",
                "command": "cargo test --workspace"
            }
        });
        s.scrub_value(&mut v);
        let findings = detect_residual_risks(&v).expect("detectors run");
        assert!(
            findings.is_empty(),
            "clean scrubbed output must have no findings: {findings:?}"
        );
    }

    // ── Finding 1: relative `./x` and `../x` paths must not be flagged ──

    #[test]
    fn relative_dot_slash_path_is_not_flagged_as_absolute() {
        // Scrubbed command with `./scripts/build.sh` — the preceding `.`
        // followed by `/` is relative-path context, not an absolute path.
        let v = json!({"command": "./scripts/build.sh"});
        let findings = detect_residual_risks(&v).expect("detectors run");
        let abs_findings: Vec<&Finding> = findings
            .iter()
            .filter(|f| {
                f.what == "absolute path outside the project placeholder roots"
            })
            .collect();
        assert!(
            abs_findings.is_empty(),
            "`./scripts/build.sh` must not be flagged as absolute; got {abs_findings:?}"
        );
    }

    #[test]
    fn relative_dotdot_path_is_not_flagged_as_absolute() {
        let v = json!({"command": "../other/proj/file.rs"});
        let findings = detect_residual_risks(&v).expect("detectors run");
        let abs_findings: Vec<&Finding> = findings
            .iter()
            .filter(|f| {
                f.what == "absolute path outside the project placeholder roots"
            })
            .collect();
        assert!(
            abs_findings.is_empty(),
            "`../other/proj/file.rs` must not be flagged as absolute; got {abs_findings:?}"
        );
    }

    #[test]
    fn genuine_absolute_path_still_flagged_as_error() {
        // Regression: a real user path like `/Users/alice/leak.txt`
        // preceded by `"` in rendered JSON MUST still be flagged.
        let v = json!({"data": "/Users/alice/leak.txt"});
        let findings = detect_residual_risks(&v).expect("detectors run");
        let abs_findings: Vec<&Finding> = findings
            .iter()
            .filter(|f| {
                f.severity == Severity::Error
                    && f.what == "absolute path outside the project placeholder roots"
            })
            .collect();
        assert!(
            !abs_findings.is_empty(),
            "`/Users/alice/leak.txt` MUST be flagged as an Error"
        );
    }

    #[test]
    fn dot_before_absolute_typo_is_relative_path_context() {
        // `foo./Users/alice/leak` — a concatenated typo. The `.` immediately
        // preceding the absolute path is treated as relative-path context
        // (consistent with the `./x` behavior). A missed leak here is
        // acceptable; this test pins that we don't silently change behavior
        // without notice.
        let v = json!({"data": "foo./Users/alice/leak"});
        let findings = detect_residual_risks(&v).expect("detectors run");
        let abs_findings: Vec<&Finding> = findings
            .iter()
            .filter(|f| {
                f.what == "absolute path outside the project placeholder roots"
            })
            .collect();
        // Acceptable: the `.` makes it relative-path-shaped context.
        assert!(
            abs_findings.is_empty(),
            "concatenated typo with `.` is relative-path-shaped; got {abs_findings:?}"
        );
    }
}
