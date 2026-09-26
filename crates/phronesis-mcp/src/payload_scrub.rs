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
//! ## Where a `$HOME`-rooted path ends
//!
//! Path extent is ambiguous in free text once names may contain spaces, so
//! the rewrite follows one rule per context and resolves any remaining
//! ambiguity toward over-scrubbing (losing a neighbouring word) rather than
//! leaking a path component:
//!
//! 1. **Path-keyed values** (`file_path`, `cwd`, `*_dir`, `*_root`, …; see
//!    `is_path_key`): the string *is* a path, so the path runs to the end of
//!    the line.
//! 2. **Quoted**: a path opened by `"` or `'` (including the `\"` of embedded
//!    JSON) runs to the next quote of the same kind on the line.
//! 3. **Bare free text**: the path runs to whitespace or `"` `'` `:` `,`, with
//!    shell-escaped spaces (`My\ Plans`) kept inside it. It then extends across
//!    following space-separated words while one of the next
//!    `MAX_CONTINUATION_WORDS` words still contains a `/` (`My Secret
//!    Plans/q3.txt`); a word starting a new path, flag, or shell operator
//!    stops the extension. A bare path whose *last* segment contains a space
//!    cannot be told apart from prose and is the one documented residual.
//!
//! ## Escaped separators
//!
//! Captured tool output often holds JSON with escaped slashes (`\/Users\/x`),
//! sometimes escaped again (`\\\/`). Rather than unescaping the content and
//! re-escaping it (which must guess which slashes were escaped and breaks
//! byte-for-byte passthrough), the matchers accept a separator at any
//! escaping depth — zero or more backslashes then `/` — and every placeholder
//! is written back with the separator spelling of the path it replaces, so
//! embedded JSON stays valid. Verification and residual-risk detection run
//! over a separator-normalized rendering, so an escaped residual fails the
//! run exactly as a plain one does.
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

/// How many space-separated words a bare path may look ahead for a word that
/// continues it with a `/` (extent rule 3 in the module docs). Bounds the
/// over-scrubbing of prose that merely follows a path.
const MAX_CONTINUATION_WORDS: usize = 6;

/// A quoted path longer than this is not treated as quote-delimited; the bare
/// rule applies instead. Bounds how much text an unbalanced quote can swallow.
const MAX_QUOTED_PATH_LEN: usize = 4096;

pub struct Scrubber {
    home: String,
    user: String,
    /// `home` / the project root with every `/` accepting any escaping depth.
    home_re: Regex,
    project_re: Regex,
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
        let home_re = root_regex("home directory", &home)?;
        let project_re = root_regex("project root", &project_root)?;
        Ok(Self {
            home,
            user,
            home_re,
            project_re,
            external: Vec::new(),
        })
    }

    /// Recursively rewrite every string in `v` per the scrub rules. Identity
    /// keys (`session_id`, `prompt_id`, `turn_id`, `agent_id`) and
    /// `transcript_path` get fixed placeholder values, whatever their spelling
    /// — a host that sends `promptId` must not evade scrubbing either.
    pub fn scrub_value(&mut self, v: &mut Value) {
        self.scrub_value_in(v, false);
    }

    /// `path_keyed`: the value sits under a path-naming key (extent rule 1).
    /// Arrays inherit their key, so `"dirs": [...]` elements are paths too.
    fn scrub_value_in(&mut self, v: &mut Value, path_keyed: bool) {
        match v {
            Value::String(s) => *s = self.scrub_str(s, path_keyed),
            Value::Array(items) => {
                for item in items {
                    self.scrub_value_in(item, path_keyed);
                }
            }
            Value::Object(map) => {
                for (key, val) in map.iter_mut() {
                    if let Some(placeholder) = identifier_placeholder(key) {
                        *val = Value::String(placeholder.to_string());
                    } else if is_transcript_key(key) {
                        *val = Value::String("/home/dev/.claude/transcript.jsonl".to_string());
                    } else {
                        self.scrub_value_in(val, is_path_key(key));
                    }
                }
            }
            _ => {}
        }
    }

    pub(crate) fn scrub_str(&mut self, s: &str, path_keyed: bool) -> String {
        // 1. Project-root prefix → canonical fixture root.
        let out = self.scrub_project_root(s);
        // 2. Any remaining $HOME-rooted path → indexed external placeholder.
        let mut out = self.scrub_external_paths(out, path_keyed);
        // 3. Bare username anywhere else (long enough to be unambiguous).
        if self.user.len() >= MIN_BARE_USERNAME_LEN {
            out = out.replace(&self.user, "dev");
        }
        out
    }

    /// Rewrite the project root to `/home/dev/project`, in whatever separator
    /// spelling it appears. Only a whole root counts: `/…/project2` is a
    /// sibling directory, not the project, and falls through to the `$HOME`
    /// rule. Canonical placeholders are left alone (fixpoint).
    fn scrub_project_root(&self, s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut last = 0;
        for m in self.project_re.find_iter(s) {
            if !is_root_boundary(&s[m.end()..]) || starts_with_placeholder(&s[m.start()..]) {
                continue;
            }
            out.push_str(&s[last..m.start()]);
            out.push_str(&respell("/home/dev/project", separator_of(m.as_str())));
            last = m.end();
        }
        out.push_str(&s[last..]);
        out
    }

    fn scrub_external_paths(&mut self, mut out: String, path_keyed: bool) -> String {
        // The cursor only moves forward — each iteration resumes past the
        // text it just inserted — and canonical placeholder paths are skipped
        // outright, so replacement terminates and is a fixpoint even when
        // `home` is a prefix of the placeholder root (e.g. `/home/dev`).
        let mut search_from = 0;
        while let Some((start, home_end)) = self
            .home_re
            .find_at(&out, search_from)
            .map(|m| (m.start(), m.end()))
        {
            if starts_with_placeholder(&out[start..]) {
                search_from = home_end;
                continue;
            }
            let end = if path_keyed {
                keyed_path_end(&out, home_end)
            } else {
                external_path_end(&out, start, home_end)
            };
            let separator = separator_of(&out[start..home_end]).to_string();
            // One placeholder per path whatever its spelling: `\/a\/b` and
            // `/a/b` name the same file.
            let placeholder = self.external_replacement(normalize_separators(&out[start..end]));
            let replacement = respell(&placeholder, &separator);
            out.replace_range(start..end, &replacement);
            search_from = start + replacement.len();
        }
        out
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
    ///
    /// Runs over a separator-normalized rendering, so a JSON-escaped residual
    /// (`\\/Users\\/…` in the rendered text) is caught like a plain one.
    pub fn verify(&self, v: &Value) -> Result<(), ScrubError> {
        let rendered = normalize_separators(&v.to_string());
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

/// End of a path-keyed value's path (extent rule 1): the end of the line.
fn keyed_path_end(s: &str, home_end: usize) -> usize {
    s[home_end..]
        .find(['\n', '\r'])
        .map_or(s.len(), |offset| home_end + offset)
}

/// End of a `$HOME`-rooted path in free text (extent rules 2 and 3 in the
/// module docs). `start..home_end` is the matched home prefix.
fn external_path_end(s: &str, start: usize, home_end: usize) -> usize {
    if let Some(end) = quoted_path_end(s, start, home_end) {
        return end;
    }
    let mut end = token_end(s, home_end);
    while let Some(next) = continuation_end(s, end) {
        end = next;
    }
    end
}

/// Extent rule 2: a path opened by a quote runs to the next quote of the same
/// kind on the same line. The backslash run right before that quote escapes
/// the quote itself (embedded JSON's `\"`), so it stays outside the path.
fn quoted_path_end(s: &str, start: usize, home_end: usize) -> Option<usize> {
    let quote = s[..start]
        .chars()
        .next_back()
        .filter(|c| matches!(c, '"' | '\''))?;
    let rest = &s[home_end..];
    let body = &rest[..rest.find(quote)?];
    if body.len() > MAX_QUOTED_PATH_LEN || body.contains(['\n', '\r']) {
        return None;
    }
    Some(home_end + body.trim_end_matches('\\').len())
}

/// End of one whitespace-free path token starting at `from`: whitespace or
/// `"` `'` `:` `,` ends it. A backslash run belongs to the token only as an
/// escaped separator (`\/`, `\\\/`) or a shell-escaped space (`\ `); any other
/// backslash run (the `\"` closing embedded JSON, a `\n` escape) ends it.
fn token_end(s: &str, from: usize) -> usize {
    let mut chars = s[from..].char_indices().peekable();
    while let Some((offset, c)) = chars.next() {
        if c == '\\' {
            while chars.next_if(|&(_, n)| n == '\\').is_some() {}
            if chars.next_if(|&(_, n)| n == '/' || n == ' ').is_none() {
                return from + offset;
            }
        } else if c.is_whitespace() || matches!(c, '"' | '\'' | ':' | ',') {
            return from + offset;
        }
    }
    s.len()
}

/// Extent rule 3's continuation step: when the path at `..end` is followed by
/// space-separated words and one of the next [`MAX_CONTINUATION_WORDS`]
/// contains a `/`, the path continues through that word (`My Secret
/// Plans/q3.txt`). A word that starts a new path, flag, variable, or shell
/// operator — or a non-space separator such as a tab or newline — stops it.
fn continuation_end(s: &str, end: usize) -> Option<usize> {
    let mut pos = end;
    for _ in 0..MAX_CONTINUATION_WORDS {
        let rest = &s[pos..];
        let gap = rest.len() - rest.trim_start_matches(' ').len();
        if gap == 0 {
            return None;
        }
        let word_start = pos + gap;
        let first = s[word_start..].chars().next()?;
        if matches!(
            first,
            '/' | '\\' | '-' | '~' | '$' | '|' | '&' | ';' | '<' | '>' | '(' | ')' | '`' | '#'
        ) {
            return None;
        }
        let word_end = token_end(s, word_start);
        if word_end == word_start {
            return None;
        }
        if s[word_start..word_end].contains('/') {
            return Some(word_end);
        }
        pos = word_end;
    }
    None
}

/// Separator at any JSON-escaping depth: zero or more backslashes, then `/`.
const ESCAPABLE_SEPARATOR: &str = r"\\*/";

/// A matcher for `root` whose every `/` also matches its escaped spellings,
/// so `\/Users\/zed` is found as readily as `/Users/zed`.
fn root_regex(which: &'static str, root: &str) -> Result<Regex, ScrubError> {
    let pattern: String = root
        .split('/')
        .skip(1)
        .map(|segment| format!("{ESCAPABLE_SEPARATOR}{}", regex::escape(segment)))
        .collect();
    Regex::new(&pattern).map_err(|e| ScrubError::InvalidRoot {
        which,
        reason: format!("cannot build a matcher: {e}"),
    })
}

/// The separator spelling a matched root opens with (`/`, `\/`, `\\\/`, …).
fn separator_of(matched: &str) -> &str {
    matched.find('/').map_or("/", |i| &matched[..=i])
}

/// `placeholder` written with `separator` in place of every `/`, so an
/// escaped path is replaced by an equally escaped placeholder.
fn respell(placeholder: &str, separator: &str) -> String {
    if separator == "/" {
        placeholder.to_string()
    } else {
        placeholder.replace('/', separator)
    }
}

/// Collapse every escaped separator (`\/`, `\\\/`, …) to a plain `/`. Other
/// backslashes pass through unchanged.
fn normalize_separators(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut backslashes = 0usize;
    for c in s.chars() {
        if c == '\\' {
            backslashes += 1;
            continue;
        }
        if c != '/' {
            out.extend(std::iter::repeat_n('\\', backslashes));
        }
        backslashes = 0;
        out.push(c);
    }
    out.extend(std::iter::repeat_n('\\', backslashes));
    out
}

/// Does `s` open with one of the scrubber's own placeholder roots, in any
/// separator spelling? `/home/dev/project` counts only as a whole component,
/// so a real `/home/dev/projectX` is still scrubbed.
fn starts_with_placeholder(s: &str) -> bool {
    let mut window_end = s.len().min(64);
    while !s.is_char_boundary(window_end) {
        window_end -= 1;
    }
    let window = normalize_separators(&s[..window_end]);
    PLACEHOLDER_PREFIXES.iter().any(|p| {
        window.starts_with(p) && (p.ends_with('/') || is_root_boundary(&window[p.len()..]))
    })
}

/// Is `rest` (the text right after a matched root) a component boundary?
/// `/root2` or `/root.bak` names a different directory; `/root/`, `/root"`,
/// `/root.` at the end of a sentence, and `\/` / `\"` escapes do not.
fn is_root_boundary(rest: &str) -> bool {
    let mut chars = rest.chars();
    match chars.next() {
        None | Some('/' | '\\') => true,
        Some('.') => !chars.next().is_some_and(is_name_char),
        Some(c) => !is_name_char(c),
    }
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '+' | '@' | '~' | '.')
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

/// Identity-neutral system roots that may legitimately appear in scrubbed
/// output. Placeholder roots are NOT listed here — they come from
/// PLACEHOLDER_PREFIXES so the two can never drift.
const ALLOWED_SYSTEM_PREFIXES: &[&str] = &[
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
    PLACEHOLDER_PREFIXES
        .iter()
        .chain(ALLOWED_SYSTEM_PREFIXES)
        .any(|p| {
            let p = p.trim_end_matches('/');
            path == p || (path.starts_with(p) && path.as_bytes().get(p.len()) == Some(&b'/'))
        })
}

/// Is the text before a `/...` match a path boundary? A preceding `/`,
/// `.` (relative-path context like `./x` or `../x`), `~` (home-relative
/// like `~/x`), or word character means the match is the tail of a URL,
/// a relative path, or a date — not an absolute path. Exception: in
/// rendered JSON a newline/tab is the two-character escape `\n` / `\t` /
/// `\r`, so a path right after one IS at a line boundary.
fn is_path_boundary(before: &str) -> bool {
    // The third slash in `file:///path` starts a local absolute path, so a
    // file URL is a boundary even though the preceding character is `/`.
    // Other URL path tails (`https://host/Users/...`) stay excluded by the
    // generic preceding-slash rule below.
    if before.ends_with("file://") {
        return true;
    }
    let Some(prev) = before.chars().next_back() else {
        return true;
    };
    if prev == '/' || prev == '.' || prev == '~' {
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
/// the CLI policy). The absolute-path check runs over a separator-normalized
/// rendering, so a JSON-escaped path (`\\/Users\\/x`) is judged exactly like
/// its plain spelling.
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
    collect_disallowed_absolute_paths(&normalize_separators(&rendered), &mut findings)?;
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

/// The fixed placeholder for an identity-bearing key, compared case- and
/// separator-insensitively (`session_id`, `sessionId`, `SessionID` all match)
/// — finding #3: a CLI sending `sessionId` must not evade scrubbing.
///
/// `prompt_id`, `turn_id` and `agent_id` join `session_id` because hosts send
/// raw UUIDs in them: Claude Code's `prompt_id` is a per-turn UUID present on
/// most events. One placeholder per class, not per value, so a fixture cannot
/// re-link two records through an id that survived.
fn identifier_placeholder(key: &str) -> Option<&'static str> {
    Some(match normalize_key(key).as_str() {
        "sessionid" => "sess-00000000",
        "promptid" => "prompt-00000000",
        "turnid" => "turn-00000000",
        "agentid" => "agent-00000000",
        _ => return None,
    })
}

fn is_transcript_key(key: &str) -> bool {
    normalize_key(key) == "transcriptpath"
}

/// Keys whose string values are paths in their entirety (extent rule 1):
/// `cwd` and anything ending in `path`, `dir`, `directory` or `root`, in any
/// spelling and optionally plural (`file_path`, `workingDirectory`, `dirs`).
fn is_path_key(key: &str) -> bool {
    let normalized = normalize_key(key);
    let key = normalized.strip_suffix('s').unwrap_or(&normalized);
    key == "cwd"
        || ["path", "dir", "directory", "root"]
            .iter()
            .any(|s| key.ends_with(s))
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

    // ── Finding 6: DRY PLACEHOLDER_PREFIXES vs ALLOWED_ABSOLUTE_PREFIXES ──

    #[test]
    fn every_placeholder_prefix_is_allowed_absolute() {
        // Each PLACEHOLDER_PREFIXES entry must be accepted by
        // is_allowed_absolute, otherwise the scrubber's own output
        // would be flagged as an Error — a DRY break.
        for prefix in &PLACEHOLDER_PREFIXES {
            let path = format!("{prefix}/x");
            assert!(
                is_allowed_absolute(&path),
                "is_allowed_absolute({path}) must accept placeholder {prefix}"
            );
        }
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
            .filter(|f| f.what == "absolute path outside the project placeholder roots")
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
            .filter(|f| f.what == "absolute path outside the project placeholder roots")
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
    fn file_url_absolute_path_is_flagged_as_error() {
        let v = json!({"data": "file:///Users/alice/leak.txt"});
        let findings = detect_residual_risks(&v).expect("detectors run");
        assert!(
            findings.iter().any(|f| {
                f.severity == Severity::Error
                    && f.what == "absolute path outside the project placeholder roots"
            }),
            "a local absolute path in a file URL must be flagged; got {findings:?}"
        );
    }

    #[test]
    fn http_url_path_tail_is_not_flagged_as_absolute() {
        let v = json!({"data": "https://example.com/Users/alice/page.html"});
        let findings = detect_residual_risks(&v).expect("detectors run");
        assert!(
            findings
                .iter()
                .all(|f| f.what != "absolute path outside the project placeholder roots"),
            "an HTTP URL path tail is not a local absolute path; got {findings:?}"
        );
    }

    /// The same contrast over a handful of path shapes, so a future change to
    /// the boundary rule cannot fix one scheme by breaking the other.
    #[test]
    fn file_url_paths_are_detected_without_flagging_http_url_tails() {
        for path in [
            "/Users/alice/x",
            "/home/bob/.ssh/id",
            "/work/repo-2/file.txt",
            "/a/b",
        ] {
            assert!(
                !is_allowed_absolute(path),
                "{path} must not be an allowed placeholder root for this test"
            );
            let file_findings = detect_residual_risks(&json!({"data": format!("file://{path}")}))
                .expect("detectors run");
            assert!(
                file_findings.iter().any(|f| {
                    f.severity == Severity::Error
                        && f.what == "absolute path outside the project placeholder roots"
                }),
                "file://{path} not flagged: {file_findings:?}"
            );
            let http_findings =
                detect_residual_risks(&json!({"data": format!("https://example.com{path}")}))
                    .expect("detectors run");
            assert!(
                http_findings
                    .iter()
                    .all(|f| f.what != "absolute path outside the project placeholder roots"),
                "https://example.com{path} wrongly flagged: {http_findings:?}"
            );
        }
    }

    #[test]
    fn tilde_prefixed_path_is_not_flagged_as_absolute() {
        // `~/Documents/notes.txt` — the `~` marks a home-relative path, the
        // same class as `./x`: the `/...` tail is not an absolute path.
        let v = json!({"command": "cat ~/Documents/notes.txt"});
        let findings = detect_residual_risks(&v).expect("detectors run");
        let abs_findings: Vec<&Finding> = findings
            .iter()
            .filter(|f| f.what == "absolute path outside the project placeholder roots")
            .collect();
        assert!(
            abs_findings.is_empty(),
            "`~/Documents/notes.txt` must not be flagged as an absolute path; got {abs_findings:?}"
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
            .filter(|f| f.what == "absolute path outside the project placeholder roots")
            .collect();
        // Acceptable: the `.` makes it relative-path-shaped context.
        assert!(
            abs_findings.is_empty(),
            "concatenated typo with `.` is relative-path-shaped; got {abs_findings:?}"
        );
    }

    // ── Path extent and escaped separators (scrub-leaks fix) ──

    /// Mirror of the CLI's `scrub_one` exit decision: `true` when the run
    /// would exit 0 for this record.
    fn cli_would_pass(s: &Scrubber, v: &Value) -> bool {
        s.verify(v).is_ok()
            && detect_residual_risks(v)
                .expect("detectors run")
                .iter()
                .all(|f| f.severity != Severity::Error)
    }

    fn zed() -> Scrubber {
        Scrubber::new("/Users/zed", "/Users/zed/project").expect("valid roots")
    }

    #[test]
    fn path_keyed_value_with_spaces_is_rewritten_whole() {
        let mut s = zed();
        let mut v = json!({"file_path": "/Users/zed/My Secret Plans/q3.txt"});
        s.scrub_value(&mut v);
        assert_eq!(v["file_path"], "/home/dev/external/p0");
        assert!(cli_would_pass(&s, &v));
    }

    #[test]
    fn free_text_path_with_spaces_extends_while_a_later_word_continues_it() {
        let mut s = zed();
        let mut v = json!({"note": "moved /Users/zed/My Secret Plans/q3.txt yesterday"});
        s.scrub_value(&mut v);
        assert_eq!(v["note"], "moved /home/dev/external/p0 yesterday");
        assert!(!v.to_string().contains("Secret"), "{v}");
    }

    #[test]
    fn quoted_path_with_spaces_extends_to_the_closing_quote() {
        let mut s = zed();
        let mut v = json!({
            "command": "cat \"/Users/zed/My Secret Plans/q 3.txt\" | wc -l",
            "other": "open '/Users/zed/Tax Docs 2025' now"
        });
        s.scrub_value(&mut v);
        assert_eq!(v["command"], "cat \"/home/dev/external/p0\" | wc -l");
        assert_eq!(v["other"], "open '/home/dev/external/p1' now");
    }

    #[test]
    fn shell_escaped_spaces_stay_inside_the_path() {
        let mut s = zed();
        let mut v = json!({"command": "ls /Users/zed/My\\ Secret\\ Plans && pwd"});
        s.scrub_value(&mut v);
        assert_eq!(v["command"], "ls /home/dev/external/p0 && pwd");
    }

    #[test]
    fn separate_paths_and_flags_are_not_swallowed() {
        let mut s = zed();
        let mut v = json!({"command": "cp /Users/zed/a.txt /Users/zed/b.txt -v"});
        s.scrub_value(&mut v);
        assert_eq!(
            v["command"],
            "cp /home/dev/external/p0 /home/dev/external/p1 -v"
        );
    }

    #[test]
    fn json_escaped_slashes_are_scrubbed_and_keep_their_escaping() {
        let mut s = Scrubber::new("/Users/al", "/Users/al/proj").expect("valid roots");
        let mut v = json!({"stdout": r"\/Users\/al\/clients\/acme\/key.txt"});
        s.scrub_value(&mut v);
        assert_eq!(v["stdout"], r"\/home\/dev\/external\/p0");
        let out = v.to_string();
        for leaked in ["clients", "acme", "key.txt"] {
            assert!(!out.contains(leaked), "{leaked} survived: {out}");
        }
        assert!(cli_would_pass(&s, &v));
    }

    #[test]
    fn json_escaped_path_inside_embedded_json_keeps_the_document_valid() {
        let mut s = zed();
        let mut v = json!({
            "stdout": r#"{"p":"\/Users\/zed\/My Plans\/k.txt","q":"\/Users\/zed\/project\/src\/a.rs"}"#
        });
        s.scrub_value(&mut v);
        let inner = v["stdout"].as_str().expect("string");
        assert_eq!(
            inner,
            r#"{"p":"\/home\/dev\/external\/p0","q":"\/home\/dev\/project\/src\/a.rs"}"#
        );
        let parsed: Value = serde_json::from_str(inner).expect("embedded JSON still parses");
        assert_eq!(parsed["p"], "/home/dev/external/p0");
    }

    #[test]
    fn escaped_and_plain_spellings_share_one_placeholder() {
        let mut s = zed();
        let mut v = json!({"a": "/Users/zed/x/y.txt", "b": r"\/Users\/zed\/x\/y.txt"});
        s.scrub_value(&mut v);
        assert_eq!(v["a"], "/home/dev/external/p0");
        assert_eq!(v["b"], r"\/home\/dev\/external\/p0");
    }

    #[test]
    fn escaped_placeholders_are_a_fixpoint() {
        let mut s = Scrubber::new("/home/dev", "/tmp/someproject").expect("valid roots");
        let mut v = json!({"stdout": r"\/home\/dev\/external\/p0 and \/home\/dev\/project\/x"});
        let before = v.clone();
        s.scrub_value(&mut v);
        assert_eq!(v, before);
    }

    #[test]
    fn verify_flags_json_escaped_home_path() {
        let s = Scrubber::new("/Users/al", "/Users/al/proj").expect("valid roots");
        let v = json!({"stdout": r"\/Users\/al\/clients\/acme\/key.txt"});
        assert!(s.verify(&v).is_err(), "escaped residual must fail verify");
        let doubled = json!({"stdout": r"\\\/Users\\\/al\\\/k"});
        assert!(s.verify(&doubled).is_err());
    }

    #[test]
    fn residual_detector_flags_json_escaped_absolute_path() {
        let v = json!({"stdout": r"\/Users\/someone\/clients\/key.txt"});
        let findings = detect_residual_risks(&v).expect("detectors run");
        assert!(
            findings.iter().any(|f| f.severity == Severity::Error
                && f.what == "absolute path outside the project placeholder roots"),
            "got {findings:?}"
        );
        // An escaped URL tail is still a URL tail, not a local path.
        let url = json!({"stdout": r"https:\/\/example.com\/a\/b"});
        let findings = detect_residual_risks(&url).expect("detectors run");
        assert!(
            findings
                .iter()
                .all(|f| f.what != "absolute path outside the project placeholder roots"),
            "got {findings:?}"
        );
        // Escaped placeholder roots are allowed like the plain ones.
        let ok = json!({"stdout": r"\/home\/dev\/external\/p0 \/home\/dev\/project\/a"});
        assert!(
            detect_residual_risks(&ok)
                .expect("detectors run")
                .is_empty()
        );
    }

    #[test]
    fn project_root_prefix_collision_is_not_treated_as_the_project() {
        let mut s = zed();
        let mut v = json!({
            "a": "/Users/zed/project/src/lib.rs",
            "b": "/Users/zed/project2/src/lib.rs",
            "c": "/Users/zed/project"
        });
        s.scrub_value(&mut v);
        assert_eq!(v["a"], "/home/dev/project/src/lib.rs");
        assert_eq!(v["c"], "/home/dev/project");
        assert_eq!(v["b"], "/home/dev/external/p0");
        assert!(cli_would_pass(&s, &v), "{v}");
    }

    /// Property / differential test: generated `$HOME`-rooted paths with
    /// spaces, unicode and escaped separators, embedded in every context the
    /// extent rule defines. Whenever the record would exit 0, no generated
    /// path component may survive in any spelling — and every defined
    /// context must in fact exit 0 (the rewrite, not the failure path, is
    /// what keeps it clean).
    #[test]
    fn generated_home_paths_never_survive_a_passing_scrub() {
        use rand::rngs::StdRng;
        use rand::{Rng, SeedableRng};

        const WORDS: &[&str] = &[
            "Qplans",
            "Xreport",
            "naïve",
            "Überblick",
            "プラン",
            "résumé",
            "Kvault",
            "Wq3",
            "Mb-7",
            "notes_v2",
            "Ωmega",
            "Jxx.tar",
            "🗂Box",
        ];
        fn segment(rng: &mut StdRng, allow_spaces: bool) -> String {
            let n = if allow_spaces {
                rng.gen_range(1..=3)
            } else {
                1
            };
            (0..n)
                .map(|_| WORDS[rng.gen_range(0..WORDS.len())])
                .collect::<Vec<_>>()
                .join(" ")
        }

        let mut rng = StdRng::seed_from_u64(0x5c0b_1eaf);
        for case in 0..400 {
            let depth = rng.gen_range(1..=4);
            let mut segs: Vec<String> = (0..depth).map(|_| segment(&mut rng, true)).collect();
            // Bare free text needs the last segment space-free (see the
            // documented extent rule); other contexts take anything.
            let last_plain = segment(&mut rng, false);
            let path = format!("/Users/zed/{}", segs.join("/"));
            segs.push(last_plain.clone());
            let bare_path = format!("/Users/zed/{}", segs.join("/"));
            let escaped = path.replace('/', r"\/");
            let double_escaped = path.replace('/', r"\\\/");
            let bare_escaped = bare_path.replace('/', r"\/");
            let records = [
                json!({"file_path": path}),
                json!({"cwd": escaped}),
                json!({"command": format!("cat \"{path}\" | wc -l")}),
                json!({"command": format!("open '{path}' now")}),
                json!({"stdout": format!(r#"{{"p":"{escaped}"}}"#)}),
                json!({"stdout": format!(r#"{{\"p\":\"{double_escaped}\"}}"#)}),
                json!({"note": format!("edited {bare_path} then stopped")}),
                json!({"note": format!("edited {bare_escaped} then stopped")}),
            ];
            let mut s = zed();
            for mut v in records {
                let original = v.to_string();
                s.scrub_value(&mut v);
                let out = v.to_string();
                assert!(
                    cli_would_pass(&s, &v),
                    "case {case}: defined context must exit 0: {original} -> {out}"
                );
                for seg in &segs {
                    for word in seg.split(' ') {
                        assert!(
                            !out.contains(word),
                            "case {case}: component {word:?} survived: {original} -> {out}"
                        );
                    }
                }
            }
        }
    }
}
