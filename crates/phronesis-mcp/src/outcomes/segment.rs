//! Shell-command segmentation for toolchain recognition (evidence-integrity
//! spec, Task 5).
//!
//! Toolchain `matches` regexes used to run against the whole command line, so
//! incidental text (`echo cargo test`, `touch cargo-test.log`) could be
//! recognized as a real invocation. This module splits a command line into
//! candidate *command segments* so head-anchored patterns see each command
//! position:
//!
//! - Split on the separators `&&`, `||`, `;`, `|`, and newlines.
//! - Strip leading environment assignments (`NAME=value …`) and a leading
//!   `env` word (plus its own `NAME=value` arguments) from each segment.
//! - Drop empty segments and comment segments (first non-space char `#`).
//!
//! This is a **segmenter, not a shell parser** — the spec forbids building a
//! full parser. Known limitations, accepted by design:
//!
//! - Quotes are not interpreted: a separator inside quotes still splits
//!   (`echo "a && b"` yields a bogus trailing segment). Wrong segments simply
//!   fail the match — recognition errs toward *not* recognizing.
//! - `env` flags (`env -i …`) are not stripped; such commands go
//!   unrecognized rather than misrecognized.
//! - A single `&` (background) is not a separator, so redirections such as
//!   `2>&1` survive intact.
//! - Command substitution, subshells, and backslash escapes are not parsed.

/// Split `command` into normalized candidate command heads. Each returned
/// string starts where a command word would appear; empty and comment
/// segments are dropped.
pub fn command_heads(command: &str) -> Vec<String> {
    command_segments(command)
        .iter()
        .filter_map(|s| strip_leading_env(s))
        .map(str::to_string)
        .collect()
}

/// Split into raw shell command segments using the same separators as
/// `command_heads`, retaining redirections and pipeline neighbors.
pub fn command_segments(command: &str) -> Vec<String> {
    let mut split = CommandSplit::default();
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '&' if chars.peek() == Some(&'&') => {
                chars.next();
                split.finish_segment();
            }
            '|' => {
                if chars.peek() == Some(&'|') {
                    chars.next();
                }
                split.finish_segment();
            }
            ';' | '\n' => split.finish_segment(),
            _ => split.current.push(c),
        }
    }
    split.finish_segment();
    split.segments
}

#[derive(Default)]
struct CommandSplit {
    segments: Vec<String>,
    current: String,
}

impl CommandSplit {
    fn finish_segment(&mut self) {
        self.segments.push(std::mem::take(&mut self.current));
    }
}

/// Strip leading `NAME=value` assignments and a leading `env` word (with its
/// own assignment arguments) from a trimmed segment. Returns `None` for
/// empty or comment segments, or when nothing but assignments remain.
fn strip_leading_env(segment: &str) -> Option<&str> {
    let mut rest = segment.trim();
    if rest.is_empty() || rest.starts_with('#') {
        return None;
    }
    loop {
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let token = &rest[..end];
        if token == "env" || is_env_assignment(token) {
            rest = rest[end..].trim_start();
            if rest.is_empty() {
                return None;
            }
        } else {
            return Some(rest);
        }
    }
}

/// `NAME=value` where NAME is a valid shell variable identifier
/// (`[A-Za-z_][A-Za-z0-9_]*`).
fn is_env_assignment(token: &str) -> bool {
    let Some((name, _)) = token.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Read one shell word, refusing expansion and escaping syntax we do not model.
pub(crate) fn shell_word(rest: &str) -> Option<String> {
    shell_word_span(rest).map(|(word, _)| word)
}

fn shell_word_span(rest: &str) -> Option<(String, usize)> {
    let trimmed = rest.trim_start();
    let leading = rest.len() - trimmed.len();
    let quote = trimmed
        .as_bytes()
        .first()
        .copied()
        .filter(|b| *b == b'\'' || *b == b'"');
    let (word, used) = if let Some(q) = quote {
        let end = trimmed.as_bytes()[1..].iter().position(|b| *b == q)? + 1;
        (&trimmed[1..end], end + 1)
    } else {
        let end = trimmed
            .find(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '<' | '>'))
            .unwrap_or(trimmed.len());
        (&trimmed[..end], end)
    };
    if word.is_empty() || word.contains(['$', '`', '\\', '(', ')', '#']) {
        return None;
    }
    Some((word.to_string(), leading + used))
}

/// Return the last plain stdout redirect target in this command segment.
pub fn stdout_redirect_target(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut i = 0;
    let mut quote = None;
    let mut target = None;
    while i < bytes.len() {
        if let Some(q) = quote {
            if bytes[i] == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        if bytes[i] == b'\'' || bytes[i] == b'"' {
            quote = Some(bytes[i]);
            i += 1;
            continue;
        }
        if bytes[i] != b'>' {
            i += 1;
            continue;
        }
        let operator = i;
        let fd_stdout = if operator > 0 && bytes[operator - 1].is_ascii_digit() {
            bytes[operator - 1] == b'1'
        } else {
            true
        };
        let mut j = i + 1;
        if j < bytes.len() && bytes[j] == b'>' {
            j += 1;
        }
        // Descriptor duplication, including >&2, 2>&1, and &>&1.
        if j < bytes.len() && bytes[j] == b'&' {
            i = j + 1;
            continue;
        }
        if fd_stdout {
            match shell_word_span(&segment[j..]) {
                Some((word, _)) => target = Some(word),
                None => return None,
            }
        }
        i = j;
    }
    target
}

/// Return the first file argument to a tee segment.
pub fn tee_target(segment: &str) -> Option<String> {
    let trimmed = segment.trim_start();
    let rest = trimmed.strip_prefix("tee")?;
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let mut rest = rest;
    let mut end_options = false;
    while let Some(word) = shell_word(rest) {
        let Some((_, consumed)) = shell_word_span(rest) else {
            break;
        };
        rest = &rest[consumed..];
        if word == "--" {
            end_options = true;
            continue;
        }
        if !end_options && word.starts_with('-') {
            continue;
        }
        return Some(word);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_command_is_one_head() {
        assert_eq!(command_heads("cargo test"), vec!["cargo test"]);
    }

    #[test]
    fn splits_on_all_separators() {
        assert_eq!(
            command_heads("cd repo && cargo test || echo failed; ls | wc -l\npwd"),
            vec!["cd repo", "cargo test", "echo failed", "ls", "wc -l", "pwd"]
        );
    }

    #[test]
    fn strips_leading_env_assignments() {
        assert_eq!(
            command_heads("FOO=1 BAR=two cargo test"),
            vec!["cargo test"]
        );
    }

    #[test]
    fn strips_leading_env_word_and_its_assignments() {
        assert_eq!(command_heads("env FOO=1 cargo test"), vec!["cargo test"]);
    }

    #[test]
    fn comment_segment_is_dropped() {
        assert!(command_heads("# cargo test").is_empty());
    }

    #[test]
    fn trailing_comment_does_not_hide_the_command_head() {
        assert_eq!(
            command_heads("cargo test # quick check"),
            vec!["cargo test # quick check"]
        );
    }

    #[test]
    fn redirection_two_gt_amp_one_is_not_split() {
        // A single `&` is not a separator — `2>&1` must survive intact.
        assert_eq!(
            command_heads("cargo test --workspace 2>&1"),
            vec!["cargo test --workspace 2>&1"]
        );
    }

    #[test]
    fn assignment_only_or_env_only_segment_yields_no_head() {
        assert!(command_heads("FOO=1").is_empty());
        assert!(command_heads("env").is_empty());
    }

    #[test]
    fn non_identifier_equals_token_is_a_command_head() {
        // `=weird` is not a valid assignment — don't strip it.
        assert_eq!(command_heads("=weird arg"), vec!["=weird arg"]);
    }

    #[test]
    fn empty_and_dangling_separator_segments_are_dropped() {
        assert!(command_heads("").is_empty());
        assert!(command_heads("   ").is_empty());
        assert_eq!(command_heads("cargo test &&"), vec!["cargo test"]);
    }

    #[test]
    fn stdout_redirect_target_is_quote_aware_and_skips_fd_dups() {
        let t = stdout_redirect_target;
        assert_eq!(t("cargo test --workspace"), None);
        assert_eq!(
            t("cargo test --workspace > /tmp/t.log 2>&1"),
            Some("/tmp/t.log".into())
        );
        assert_eq!(t("cargo test >> logs/run.txt"), Some("logs/run.txt".into()));
        assert_eq!(
            t("cargo test &> 'out dir/all.log'"),
            Some("out dir/all.log".into())
        );
        assert_eq!(
            t("cargo test >\"out dir/all.log\""),
            Some("out dir/all.log".into())
        );
        assert_eq!(t("cargo test >run.log 2>&1"), Some("run.log".into()));
        assert_eq!(t("cargo test 1>run.log"), Some("run.log".into()));
        assert_eq!(
            t("cargo test > a.log > b.log"),
            Some("b.log".into()),
            "the last wins"
        );
        assert_eq!(t("cargo test 2>&1"), None);
        assert_eq!(t("echo x >&2"), None);
        assert_eq!(t("echo x 1>&2"), None);
        assert_eq!(t("echo x &>&1"), None);
        assert_eq!(
            t("echo x > $OUT"),
            None,
            "unmodelled shell syntax is not a target"
        );
        assert_eq!(t("echo x > out\\ dir/log"), None);
    }

    #[test]
    fn tee_target_reads_the_first_file_argument() {
        assert_eq!(tee_target("tee run.log"), Some("run.log".into()));
        assert_eq!(tee_target("tee -a run.log"), Some("run.log".into()));
        assert_eq!(tee_target("tee -- -dashed.log"), Some("-dashed.log".into()));
        assert_eq!(tee_target("tee 'my run.log'"), Some("my run.log".into()));
        assert_eq!(tee_target("cargo test"), None);
        assert_eq!(tee_target("tee"), None);
    }
}
