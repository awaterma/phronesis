//! Commit detection from ground truth: HEAD before the shell call vs after.

use std::path::Path;
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
        Regex::new(r"\bgit\b[^\n|;&]*\b(commit|cherry-pick|revert|merge|rebase)\b").unwrap()
    })
}
pub fn command_may_move_head(command: &str) -> bool {
    prefilter().is_match(command)
}

/// Why detection was disabled for a call. Stored on the `inflight` entry so the
/// miss is auditable rather than silent (spec §"Success signal: commit").
pub const DETECTION_TIMEOUT: &str = "timeout";
pub const DETECTION_NO_EXIT_CODE: &str = "no_exit_code";

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

/// `command_exit != Some(0)` suppresses the check, which is what makes
/// `git commit && false` a non-commit — and what makes a host that sends no
/// exit code record `DETECTION_NO_EXIT_CODE` on the entry instead of guessing.
pub fn detect_commit(
    root: &Path,
    head_before: Option<&str>,
    command: &str,
    command_exit: Option<i32>,
) -> Option<Commit> {
    let before = head_before?;
    if command_exit != Some(0) || !command_may_move_head(command) {
        return None;
    }
    let after = git_head(root)?;
    (after != before).then(|| Commit {
        sha: after,
        head_before: before.to_string(),
    })
}
