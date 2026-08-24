//! The `[ownership.rust]` section of `.phronesis/graph.toml`.
//!
//! There is no TOML parser in this crate — `.phronesis/graph.toml` is read by
//! a hand-rolled line scanner — and decision D15 forbids adding one for a
//! section holding a bool, a string, two string arrays, and an integer. This
//! module is that scanner, made **section-aware** so a key can never be
//! attributed to the wrong table.
//!
//! Supported grammar, and deliberately nothing else:
//!
//! ```toml
//! [ownership.rust]
//! enabled = true                  # bool
//! provider = "ast"                # quoted string ("ast" | "rust-analyzer")
//! include = ["src/**/*.rs"]       # array of quoted strings, possibly
//! exclude = ["target/**"]         #   spread over several lines
//! max_sites_per_file = 2000       # integer
//! ```
//!
//! Missing file, missing section, and `enabled = false` all mean **disabled**,
//! and disabled must produce zero ownership edges.

use std::path::Path;

/// Where the section lives, relative to the project root.
pub const CONFIG_REL_PATH: &str = ".phronesis/graph.toml";

/// The table header this module owns.
const SECTION_HEADER: &str = "[ownership.rust]";

/// Site budget applied when the section does not set one (spec §9).
pub const DEFAULT_MAX_SITES_PER_FILE: usize = 2000;

/// Which evidence provider the project asked for.
///
/// `RustAnalyzer` *includes* AST extraction and additionally requests compiler
/// enrichment; it never replaces the AST provider (spec §9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnershipProvider {
    Ast,
    RustAnalyzer,
}

impl OwnershipProvider {
    /// The configuration spelling, which is also the value written into
    /// `ownership_analysis_status` reasons and CLI output.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ast => "ast",
            Self::RustAnalyzer => "rust-analyzer",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "ast" => Some(Self::Ast),
            "rust-analyzer" => Some(Self::RustAnalyzer),
            _ => None,
        }
    }
}

/// A misconfigured `[ownership.rust]` section.
///
/// A typo is reported rather than guessed at: silently falling back to a
/// default would leave the project believing it had asked for something it did
/// not get, which is the exact failure mode this whole feature exists to
/// avoid. Callers that must not fail (the save hook) use
/// [`load_or_disabled`], which logs and disables.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OwnershipConfigError {
    #[error(
        "unknown ownership provider {provider:?} in {CONFIG_REL_PATH} [ownership.rust]; expected \"ast\" or \"rust-analyzer\""
    )]
    UnknownProvider { provider: String },
    #[error("invalid value {value:?} for [ownership.rust] {key} in {CONFIG_REL_PATH}")]
    InvalidValue { key: String, value: String },
}

/// Parsed `[ownership.rust]` settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnershipConfig {
    /// False unless the section exists and says `enabled = true`.
    pub enabled: bool,
    pub provider: OwnershipProvider,
    /// Repo-relative glob patterns. Empty means "no include filter".
    pub include: Vec<String>,
    /// Repo-relative glob patterns. Exclude beats include.
    pub exclude: Vec<String>,
    pub max_sites_per_file: usize,
}

impl Default for OwnershipConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: OwnershipProvider::Ast,
            include: Vec::new(),
            exclude: Vec::new(),
            max_sites_per_file: DEFAULT_MAX_SITES_PER_FILE,
        }
    }
}

impl OwnershipConfig {
    /// The disabled configuration: what a project without the section gets.
    pub fn disabled() -> Self {
        Self::default()
    }

    /// Whether a repo-relative path is in scope for ownership extraction.
    ///
    /// Per D16 this **filters** the output of `sync::tracked_files`; it never
    /// walks the filesystem itself. An independent walk would index files the
    /// freshness check can never match, producing drift that nothing heals.
    pub fn matches(&self, path: &str) -> bool {
        if self.exclude.iter().any(|pattern| glob_match(pattern, path)) {
            return false;
        }
        self.include.is_empty() || self.include.iter().any(|pattern| glob_match(pattern, path))
    }
}

/// Read `<root>/.phronesis/graph.toml`.
///
/// An unreadable or absent file is not an error — it is the ordinary state of
/// every project that has not opted in.
pub fn load(root: &Path) -> Result<OwnershipConfig, OwnershipConfigError> {
    let Ok(content) = std::fs::read_to_string(root.join(CONFIG_REL_PATH)) else {
        return Ok(OwnershipConfig::disabled());
    };
    parse(&content)
}

/// [`load`], with a misconfiguration downgraded to "disabled" plus a warning.
///
/// The save hook cannot fail the user's edit over a typo in an optional
/// enrichment section, but it must not quietly pretend the typo was the
/// configuration that was asked for either.
pub fn load_or_disabled(root: &Path) -> OwnershipConfig {
    match load(root) {
        Ok(config) => config,
        Err(error) => {
            tracing::warn!("ownership enrichment disabled: {error}");
            OwnershipConfig::disabled()
        }
    }
}

/// Parse the whole file, keeping only the `[ownership.rust]` table.
pub fn parse(content: &str) -> Result<OwnershipConfig, OwnershipConfigError> {
    let mut config = OwnershipConfig::disabled();
    let mut scan = Scan::default();
    for raw in content.lines() {
        let line = raw.trim();
        if let Some((key, mut buffer)) = scan.pending.take() {
            buffer.push(' ');
            // Comments are stripped per line: a `#` carried into the
            // accumulated buffer would truncate the array at that point.
            buffer.push_str(strip_comment(line));
            if closes_array(&buffer) {
                apply(&mut config, &key, &buffer)?;
            } else {
                scan.pending = Some((key, buffer));
            }
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            scan.in_section = line == SECTION_HEADER;
            continue;
        }
        if !scan.in_section {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (
            key.trim().to_string(),
            strip_comment(value.trim()).to_string(),
        );
        if value.starts_with('[') && !closes_array(&value) {
            scan.pending = Some((key, value));
            continue;
        }
        apply(&mut config, &key, &value)?;
    }
    if let Some((key, value)) = scan.pending {
        // An unterminated array is a malformed value, not an empty list.
        return Err(OwnershipConfigError::InvalidValue { key, value });
    }
    Ok(config)
}

/// Line-scanner state for [`parse`].
#[derive(Default)]
struct Scan {
    /// True while inside the `[ownership.rust]` table.
    in_section: bool,
    /// An array value may span lines; `pending` holds the key and the text
    /// accumulated so far until its closing bracket arrives.
    pending: Option<(String, String)>,
}

fn apply(config: &mut OwnershipConfig, key: &str, value: &str) -> Result<(), OwnershipConfigError> {
    let invalid = || OwnershipConfigError::InvalidValue {
        key: key.to_string(),
        value: value.to_string(),
    };
    match key {
        "enabled" => config.enabled = parse_bool(value).ok_or_else(invalid)?,
        "provider" => {
            let provider = quoted_value(value).ok_or_else(invalid)?;
            config.provider = OwnershipProvider::parse(&provider)
                .ok_or(OwnershipConfigError::UnknownProvider { provider })?;
        }
        "include" => config.include = quoted_strings(value),
        "exclude" => config.exclude = quoted_strings(value),
        "max_sites_per_file" => {
            config.max_sites_per_file = parse_usize(value).ok_or_else(invalid)?
        }
        // Unknown keys are ignored so a newer file stays readable by an older
        // binary; the closed relation set, not this scanner, is the contract.
        _ => {}
    }
    Ok(())
}

/// True when `text` contains a `]` outside a quoted string.
fn closes_array(text: &str) -> bool {
    let mut quote: Option<char> = None;
    for character in text.chars() {
        if let Some(open) = quote {
            if character == open {
                quote = None;
            }
        } else if character == '"' || character == '\'' {
            quote = Some(character);
        } else if character == ']' {
            return true;
        } else if character == '#' {
            return false;
        }
    }
    false
}

/// Drop a trailing `# comment` that begins outside a quoted string.
fn strip_comment(value: &str) -> &str {
    let mut quote: Option<char> = None;
    for (index, character) in value.char_indices() {
        if let Some(open) = quote {
            if character == open {
                quote = None;
            }
        } else if character == '"' || character == '\'' {
            quote = Some(character);
        } else if character == '#' {
            return value[..index].trim_end();
        }
    }
    value.trim_end()
}

fn parse_bool(value: &str) -> Option<bool> {
    match strip_comment(value).trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn parse_usize(value: &str) -> Option<usize> {
    strip_comment(value).trim().replace('_', "").parse().ok()
}

/// The contents of a single quoted scalar, rejecting anything trailing it.
///
/// Modelled on `data_contracts::quoted_value`, which this file deliberately
/// mirrors so both scanners fail the same way on the same malformed line.
fn quoted_value(value: &str) -> Option<String> {
    let value = strip_comment(value).trim();
    let quote = value
        .chars()
        .next()
        .filter(|quote| matches!(quote, '"' | '\''))?;
    let rest = value.strip_prefix(quote)?;
    let end = rest.find(quote)?;
    if !rest[end + quote.len_utf8()..].trim().is_empty() {
        return None;
    }
    Some(rest[..end].to_string())
}

/// Every quoted string in an array literal, in order.
fn quoted_strings(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    // `Some((open, current))` while inside a quoted string: the opening
    // quote character and the text accumulated so far.
    let mut quoted: Option<(char, String)> = None;
    for character in value.chars() {
        if let Some((open, current)) = quoted.as_mut() {
            if character == *open {
                out.push(std::mem::take(current));
                quoted = None;
            } else {
                current.push(character);
            }
        } else if character == '"' || character == '\'' {
            quoted = Some((character, String::new()));
        } else if character == '#' {
            break;
        }
    }
    out
}

/// Glob match with `**` support, over repo-relative paths.
///
/// `graph::query::glob_matches` is single-level: its `*` crosses `/`, so
/// `src/*.rs` would match `src/a/b.rs` and `src/**/*.rs` would degrade to
/// nonsense. Decision D16 requires a `**`-capable matcher here.
///
/// Semantics, matching the common `.gitignore`/`globset` reading:
///
/// - `?` — exactly one character other than `/`.
/// - `*` — any run of characters not containing `/`.
/// - `**` — any run of characters, `/` included.
/// - `**/` — **zero or more** whole directories, so `src/**/*.rs` matches
///   `src/lib.rs` as well as `src/graph/ownership/config.rs`. The
///   zero-directory case is why this is not simply
///   `journey::tagger::glob_match`, which requires at least one.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    glob_match_bytes(pattern.as_bytes(), path.as_bytes())
}

fn glob_match_bytes(pattern: &[u8], path: &[u8]) -> bool {
    let mut p = 0usize;
    let mut s = 0usize;
    while p < pattern.len() {
        if pattern[p] == b'*' {
            if pattern.get(p + 1) == Some(&b'*') {
                let rest = &pattern[p + 2..];
                if rest.is_empty() {
                    return true;
                }
                if rest[0] == b'/' {
                    // `**/` spans whole directories, including none at all.
                    let tail = &rest[1..];
                    if glob_match_bytes(tail, &path[s..]) {
                        return true;
                    }
                    return path[s..]
                        .iter()
                        .enumerate()
                        .filter(|(_, byte)| **byte == b'/')
                        .any(|(offset, _)| glob_match_bytes(tail, &path[s + offset + 1..]));
                }
                return (s..=path.len()).any(|i| glob_match_bytes(rest, &path[i..]));
            }
            let rest = &pattern[p + 1..];
            for i in s..=path.len() {
                // A single `*` is anchored to one path segment.
                if i > s && path[i - 1] == b'/' {
                    break;
                }
                if glob_match_bytes(rest, &path[i..]) {
                    return true;
                }
            }
            return false;
        }
        if s >= path.len() {
            return false;
        }
        if pattern[p] == b'?' {
            if path[s] == b'/' {
                return false;
            }
        } else if pattern[p] != path[s] {
            return false;
        }
        p += 1;
        s += 1;
    }
    s == path.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENABLED: &str = r#"
[ownership.rust]
enabled = true
provider = "ast"
include = ["src/**/*.rs"]
exclude = ["target/**", "vendor/**"]
max_sites_per_file = 500
"#;

    // Pins the whole documented grammar in one pass: a wrong reading of any
    // single value type would leave the feature enabled with silently wrong
    // bounds.
    #[test]
    fn the_documented_section_parses_every_value_type() {
        let config = parse(ENABLED).expect("section parses");
        assert!(config.enabled, "enabled = true must enable");
        assert_eq!(config.provider, OwnershipProvider::Ast, "provider");
        assert_eq!(config.include, vec!["src/**/*.rs".to_string()], "include");
        assert_eq!(
            config.exclude,
            vec!["target/**".to_string(), "vendor/**".to_string()],
            "exclude"
        );
        assert_eq!(config.max_sites_per_file, 500, "max_sites_per_file");
    }

    // Pins D15's three disabled paths. Each one is a state a real project is
    // in, and treating any of them as enabled would emit ownership edges the
    // project never asked for.
    #[test]
    fn a_missing_file_missing_section_or_false_flag_all_mean_disabled() {
        let temp = tempfile::tempdir().expect("tempdir");
        assert!(
            !load(temp.path())
                .expect("absent file is not an error")
                .enabled,
            "a missing graph.toml means disabled"
        );
        assert!(
            !parse("[[generated_artifacts]]\nproducer = \"x\"\n")
                .expect("parses")
                .enabled,
            "a file without the section means disabled"
        );
        assert!(
            !parse("[ownership.rust]\nenabled = false\nprovider = \"ast\"\n")
                .expect("parses")
                .enabled,
            "enabled = false means disabled"
        );
    }

    // Pins that the scanner is section-aware. Before this module existed the
    // only graph.toml reader absorbed every later key into the last block it
    // recognised; the same bug here would let a generated-artifact key set
    // `enabled`.
    #[test]
    fn keys_outside_the_ownership_section_are_never_absorbed() {
        let config = parse(
            "[ownership.rust]\nenabled = true\n\n[[generated_artifacts]]\nprovider = \"rust-analyzer\"\nmax_sites_per_file = 1\n",
        )
        .expect("parses");
        assert!(config.enabled, "the ownership key still applies");
        assert_eq!(
            config.provider,
            OwnershipProvider::Ast,
            "a provider key in another table must not apply"
        );
        assert_eq!(
            config.max_sites_per_file, DEFAULT_MAX_SITES_PER_FILE,
            "a cap key in another table must not apply"
        );
    }

    // Pins the documented failure mode for a typo: an error, never a silent
    // fallback that leaves the project believing it asked for something else.
    #[test]
    fn an_unknown_provider_is_an_error_and_disables_rather_than_guessing() {
        let error = parse("[ownership.rust]\nenabled = true\nprovider = \"rust_analyzer\"\n")
            .expect_err("unknown provider must not parse");
        assert_eq!(
            error,
            OwnershipConfigError::UnknownProvider {
                provider: "rust_analyzer".to_string()
            },
            "the offending value is named in the error"
        );
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(temp.path().join(".phronesis")).expect("mkdir");
        std::fs::write(
            temp.path().join(CONFIG_REL_PATH),
            "[ownership.rust]\nenabled = true\nprovider = \"rust_analyzer\"\n",
        )
        .expect("write");
        assert!(
            !load_or_disabled(temp.path()).enabled,
            "a misconfigured section disables rather than half-enabling"
        );
    }

    // Pins that a non-boolean flag is reported instead of being read as
    // "false" — the difference between "opted out" and "typed it wrong".
    #[test]
    fn a_malformed_scalar_is_reported_rather_than_defaulted() {
        assert_eq!(
            parse("[ownership.rust]\nenabled = yes\n").expect_err("must not parse"),
            OwnershipConfigError::InvalidValue {
                key: "enabled".to_string(),
                value: "yes".to_string()
            },
            "enabled = yes is malformed"
        );
        assert!(
            parse("[ownership.rust]\nmax_sites_per_file = many\n").is_err(),
            "a non-integer cap is malformed"
        );
    }

    // Pins that comments and multi-line arrays — both ordinary in a
    // hand-edited TOML file — do not corrupt the parsed lists.
    #[test]
    fn inline_comments_and_multi_line_arrays_parse() {
        let config = parse(
            "# top\n[ownership.rust]\nenabled = true # on\ninclude = [\n  \"src/**/*.rs\", # sources\n  \"build.rs\",\n]\nmax_sites_per_file = 2_000\n",
        )
        .expect("parses");
        assert!(config.enabled, "trailing comment must not break the bool");
        assert_eq!(
            config.include,
            vec!["src/**/*.rs".to_string(), "build.rs".to_string()],
            "multi-line array entries survive"
        );
        assert_eq!(config.max_sites_per_file, 2000, "underscored integer");
    }

    // Pins the `**` semantics D16 requires, and specifically the
    // zero-directory case: `src/**/*.rs` that missed `src/lib.rs` would drop
    // every crate root from the enrichment without any diagnostic.
    #[test]
    fn double_star_globs_match_across_and_without_directories() {
        assert!(
            glob_match("src/**/*.rs", "src/lib.rs"),
            "src/**/*.rs must match a file directly in src"
        );
        assert!(
            glob_match("src/**/*.rs", "src/graph/ownership/config.rs"),
            "src/**/*.rs must match a nested file"
        );
        assert!(
            !glob_match("src/**/*.rs", "tests/graph.rs"),
            "src/**/*.rs must not escape src"
        );
        assert!(
            !glob_match("src/**/*.rs", "src/graph/query.py"),
            "the extension still binds"
        );
        assert!(
            glob_match("target/**", "target/debug/build/x.rs"),
            "a trailing ** matches any depth"
        );
        assert!(
            !glob_match("src/*.rs", "src/graph/query.rs"),
            "a single * must not cross a directory separator"
        );
        assert!(
            glob_match("src/?.rs", "src/a.rs"),
            "? matches one character"
        );
        assert!(
            !glob_match("src/?.rs", "src//.rs"),
            "? must not match a separator"
        );
        assert!(glob_match("build.rs", "build.rs"), "literals match exactly");
    }

    // Pins that exclude beats include and that an empty include list is "no
    // filter" rather than "nothing" — the latter would silently disable the
    // feature for any project that set only `exclude`.
    #[test]
    fn include_and_exclude_compose_with_exclude_winning() {
        let config = parse(ENABLED).expect("parses");
        assert!(config.matches("src/graph/ownership/mod.rs"), "included");
        assert!(!config.matches("tests/graph.rs"), "outside include");
        assert!(
            !config.matches("target/debug/src/x.rs"),
            "excluded even though it is not under src"
        );
        let only_exclude =
            parse("[ownership.rust]\nenabled = true\nexclude = [\"target/**\"]\n").expect("parses");
        assert!(
            only_exclude.matches("src/lib.rs"),
            "an empty include list must not exclude everything"
        );
        assert!(
            !only_exclude.matches("target/x.rs"),
            "exclude still applies"
        );
    }

    // Pins that operand-style values never carry the byte `Edge::fact_id`
    // joins arguments with; a pattern containing it would corrupt every fact
    // id built from it.
    #[test]
    fn parsed_patterns_never_contain_the_fact_id_separator() {
        let config = parse(ENABLED).expect("parses");
        for pattern in config.include.iter().chain(config.exclude.iter()) {
            assert!(
                !pattern.contains('\u{1f}'),
                "pattern {pattern:?} must not contain the fact-id separator"
            );
        }
    }
}
