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
    let mut segments: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '&' if chars.peek() == Some(&'&') => {
                chars.next();
                segments.push(std::mem::take(&mut current));
            }
            '|' => {
                if chars.peek() == Some(&'|') {
                    chars.next();
                }
                segments.push(std::mem::take(&mut current));
            }
            ';' | '\n' => segments.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    segments.push(current);
    segments
        .iter()
        .filter_map(|s| strip_leading_env(s))
        .map(str::to_string)
        .collect()
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
}
