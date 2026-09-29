//! Commit detection from ground truth: HEAD before the shell call vs after.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;

pub fn is_shell_tool(tool_name: &str) -> bool {
    matches!(tool_name, "Bash" | "run_shell_command")
}

fn prefilter() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\bgit\b[^\n|;&]*\b(commit|cherry-pick|revert|merge|rebase)\b")
            .expect("static prefilter regex is a compile-time invariant")
    })
}
pub fn command_may_move_head(command: &str) -> bool {
    prefilter().is_match(command)
}

/// One shell word from the start of `rest`: a `'…'` or `"…"` quoted run, or
/// a bare run up to whitespace / `;` / `&` / `|`. `None` when the word is
/// empty or contains shell syntax this parser does not model (`$`, backtick,
/// backslash, `(`, `)`, `#`), because a guessed directory is worse than none.
fn shell_word(rest: &str) -> Option<String> {
    let rest = rest.trim_start();
    let first = rest.chars().next()?;
    let word = if first == '\'' || first == '"' {
        let end = rest[1..].find(first)?;
        &rest[1..end + 1]
    } else {
        let end = rest
            .find(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|'))
            .unwrap_or(rest.len());
        &rest[..end]
    };
    if word.is_empty() || word.contains(['$', '`', '\\', '(', ')', '#']) {
        return None;
    }
    Some(word.to_string())
}

/// The absolute directory the head-moving invocation in `command` acts in,
/// or `None`. Decided from the segment that matches the head-moving
/// prefilter: its own `-C <dir>` first; else a leading `cd <dir>` segment
/// that hands off to the rest of the command. Relative paths and unmodelled
/// shell syntax return `None`: the caller then probes the project root, so
/// commits are undercounted, never mis-attributed.
pub fn command_repo_dir(command: &str) -> Option<PathBuf> {
    if command.contains(['$', '`', '(', ')']) || command.contains("sh -c") {
        return None;
    }
    let segments = crate::outcomes::segment::command_heads(command);
    let mover = segments.iter().find(|s| prefilter().is_match(s))?;

    let dir = if let Some(idx) = mover.find(" -C ") {
        // Only the git command's own `-C` counts: a match inside an
        // `echo 'git -C /wt commit'` string is incidental, and the mover
        // starts with `git` only for a real invocation (`command_heads`
        // already stripped leading env assignments).
        if !mover.starts_with("git") {
            return None;
        }
        shell_word(&mover[idx + 4..])?
    } else {
        let first = segments.first()?;
        let rest = first.strip_prefix("cd ")?;
        if segments.len() < 2 {
            return None;
        }
        shell_word(rest)?
    };
    let path = PathBuf::from(dir);
    path.is_absolute().then_some(path)
}

/// `git rev-parse --git-common-dir` for `dir`, canonicalized. `None` outside
/// a repository or when git is unavailable.
fn git_common_dir(dir: &Path) -> Option<PathBuf> {
    let out = Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(dir)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let path = if Path::new(&raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        dir.join(raw)
    };
    std::fs::canonicalize(path).ok()
}

/// Where to probe HEAD for `command`: the absolute directory it names when
/// that directory exists and is a worktree of the same repository as
/// `project_root`; otherwise `project_root`. A sibling repository therefore
/// still reads as "HEAD did not move here" (spec §Non-goals).
pub fn probe_root_for(project_root: &Path, command: &str) -> PathBuf {
    let Some(candidate) = command_repo_dir(command) else {
        return project_root.to_path_buf();
    };
    if !candidate.is_dir() {
        return project_root.to_path_buf();
    }
    match (git_common_dir(project_root), git_common_dir(&candidate)) {
        (Some(a), Some(b)) if a == b => candidate,
        _ => project_root.to_path_buf(),
    }
}

/// How, or why not, a `commit` was detected for a call. `timeout` is stored on
/// the `inflight` entry (the pre-side `git rev-parse` lost its race, so nothing
/// can be compared); the other two are decided at post and recorded on the
/// event, so the record says which evidence it rests on (spec §"Success
/// signal: commit").
pub const DETECTION_TIMEOUT: &str = "timeout";
/// HEAD moved, but the host sent no exit code, so the movement is the only
/// evidence. A marker, not a skip: HEAD is the ground truth either way.
pub const DETECTION_NO_EXIT_CODE: &str = "no_exit_code";
/// No usable HEAD comparison (the probe failed or timed out), but the host
/// itself reported a commit sha for the call.
pub const DETECTION_HOST_REPORTED: &str = "host_reported";

/// Is `s` a full 40-hex object name? Only a full sha is accepted as a
/// host-reported commit: an abbreviation is not a stable identifier, and this
/// value is recorded as one.
pub fn is_full_sha(s: &str) -> bool {
    s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// True when the host's exit code does not veto detection: either it said the
/// command succeeded, or it said nothing at all.
pub fn exit_allows_detection(command_exit: Option<i32>) -> bool {
    command_exit.is_none_or(|c| c == 0)
}

/// Outcome of one `git rev-parse HEAD`. `Timeout` is separate from
/// `Unavailable` because only a timeout is worth marking: "not a repo" is a
/// permanent, uninteresting condition, while a 2 s timeout means the probe lost
/// a race with something and the commit may have been real.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeadProbe {
    Head(String),
    Timeout,
    Unavailable,
}

pub fn git_head_probe(root: &Path) -> HeadProbe {
    let Ok(mut child) = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()
    else {
        return HeadProbe::Unavailable;
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return HeadProbe::Unavailable;
                }
                let mut out = String::new();
                use std::io::Read;
                let Some(mut stdout) = child.stdout.take() else {
                    return HeadProbe::Unavailable;
                };
                if stdout.read_to_string(&mut out).is_err() {
                    return HeadProbe::Unavailable;
                }
                let sha = out.trim().to_string();
                // 40 for SHA-1, 64 under `--object-format=sha256`.
                return if matches!(sha.len(), 40 | 64) && sha.chars().all(|c| c.is_ascii_hexdigit())
                {
                    HeadProbe::Head(sha)
                } else {
                    HeadProbe::Unavailable
                };
            }
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20))
            }
            Ok(None) => {
                let _ = child.kill();
                return HeadProbe::Timeout;
            }
            Err(_) => {
                let _ = child.kill();
                return HeadProbe::Unavailable;
            }
        }
    }
}

pub fn git_head(root: &Path) -> Option<String> {
    match git_head_probe(root) {
        HeadProbe::Head(sha) => Some(sha),
        HeadProbe::Timeout | HeadProbe::Unavailable => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub sha: String,
    pub head_before: String,
}

/// HEAD movement is the ground truth; the exit code is only a veto.
/// `Some(nonzero)` suppresses the check, which is what makes `git commit &&
/// false` a non-commit. `None` — a host that reports no exit code at all, as
/// Claude Code's `Bash` `tool_response` does — does **not**: the comparison
/// still decides, and the caller records `DETECTION_NO_EXIT_CODE` on the event
/// so the weaker evidence is visible in the record.
pub fn detect_commit(
    root: &Path,
    head_before: Option<&str>,
    command: &str,
    command_exit: Option<i32>,
) -> Option<Commit> {
    let before = head_before?;
    if !exit_allows_detection(command_exit) || !command_may_move_head(command) {
        return None;
    }
    let after = git_head(root)?;
    (after != before).then(|| Commit {
        sha: after,
        head_before: before.to_string(),
    })
}
