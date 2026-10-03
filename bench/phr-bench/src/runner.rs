use crate::governance::{summarize, GovernanceError};
use crate::manifest::{Caps, TaskSpec};
use crate::prompt;
use crate::record::{Arm, RunExit, RunRecord};
use crate::telemetry::{parse_transcript, TranscriptStats};
use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Env var that overrides the claude binary the runner drives. Tests point it
/// at a fixture script; operators leave it unset for the real CLI.
pub const CLAUDE_PATH_ENV: &str = "PHR_BENCH_CLAUDE";

/// Router env the operator exports from bench/run-env.sh (SPIKE-FINDINGS);
/// every key that is present is injected into the child process env.
const ROUTER_ENV_KEYS: &[&str] = &[
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_MODEL",
    "ANTHROPIC_SMALL_FAST_MODEL",
];

/// How the headless run ended at the process level.
struct ChildOutcome {
    success: bool,
    timed_out: bool,
}

/// Pure exit classification, unit-tested directly.
pub fn classify_exit(timed_out: bool, turns_capped: bool, success: bool, stderr: &str) -> RunExit {
    if timed_out {
        return RunExit::CapTime;
    }
    if turns_capped {
        return RunExit::CapTurns;
    }
    if success {
        RunExit::Completed
    } else {
        RunExit::Error {
            reason: stderr.chars().take(500).collect(),
        }
    }
}

/// Stage everything in the clone and diff the index against the pre-run HEAD,
/// so the patch captures agent commits AND untracked files — the agent may
/// commit, and the pre-run HEAD is recorded before claude starts
/// (SPIKE-FINDINGS correction 2). Governance wiring written by
/// `phr-mcp init` before the run is excluded: it is bench infrastructure,
/// not agent output — the wiring dirs (`.phronesis`, `.claude`, `.codex`,
/// `.gemini`) plus the two root files init creates or appends to
/// (`.mcp.json`, `.gitignore`). rv8 Major finding: without the two file
/// excludes every treatment patch carried a one-sided .gitignore + .mcp
/// hunk, biasing diff_bytes against treatment. Trade-off: excluding
/// `.gitignore` also drops legitimate agent edits to it — accepted, and
/// rare in SWE-bench fixes.
pub fn extract_diff(clone_dir: &Path, pre_head: &str) -> Result<String> {
    let dir = clone_dir
        .to_str()
        .context("clone path contains invalid UTF-8")?;
    let add = Command::new("git")
        .args(["-C", dir, "add", "-A"])
        .output()
        .context("git add -A")?;
    if !add.status.success() {
        bail!(
            "git add -A failed in {}: {}",
            dir,
            String::from_utf8_lossy(&add.stderr).trim()
        );
    }
    let diff = Command::new("git")
        .args([
            "-C",
            dir,
            "diff",
            "--cached",
            pre_head,
            "--",
            ".",
            ":(exclude).phronesis",
            ":(exclude).claude",
            ":(exclude).codex",
            ":(exclude).gemini",
            ":(exclude).mcp.json",
            ":(exclude).gitignore",
        ])
        .output()
        .context("git diff --cached")?;
    if !diff.status.success() {
        bail!(
            "git diff --cached {} failed in {}: {}",
            pre_head,
            dir,
            String::from_utf8_lossy(&diff.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&diff.stdout).into_owned())
}

/// A treatment run whose log.jsonl is missing/empty/never-hooked is invalid —
/// never a silent, control-equivalent pass.
pub fn not_wired_record(instance_id: &str, arm: Arm) -> Result<RunRecord> {
    if arm != Arm::Treatment {
        bail!("not_wired only applies to treatment");
    }
    Ok(RunRecord {
        instance_id: instance_id.into(),
        arm,
        exit: RunExit::Error {
            reason: "governance_not_wired".into(),
        },
        resolved: None,
        turns: 0,
        tokens_in: None,
        tokens_out: None,
        wall_clock_secs: 0,
        diff_bytes: 0,
        audit: None,
        governance: None,
    })
}

/// Drive `claude -p` headless in the arm's clone under the caps, then extract
/// the patch, parse telemetry, summarize governance, and write `record.json`
/// into `run_dir`. Artifacts: `transcript.jsonl`, `patch.diff`,
/// `claude-stderr.log`, `record.json`.
pub fn run(
    task: &TaskSpec,
    arm: Arm,
    clone_dir: &Path,
    run_dir: &Path,
    caps: &Caps,
) -> Result<RunRecord> {
    std::fs::create_dir_all(run_dir)
        .with_context(|| format!("creating run dir {}", run_dir.display()))?;
    // Pre-run HEAD, recorded BEFORE the run: the diff base that captures agent
    // commits as well as uncommitted and untracked work.
    let pre_head = record_head(clone_dir)?;
    let rendered = prompt::render(task);
    let transcript_path = run_dir.join("transcript.jsonl");
    let stderr_path = run_dir.join("claude-stderr.log");
    let cap = Duration::from_secs(caps.max_wall_clock_secs);
    let started = Instant::now();
    let stdout =
        Stdio::from(std::fs::File::create(&transcript_path).context("creating transcript.jsonl")?);
    let stderr =
        Stdio::from(std::fs::File::create(&stderr_path).context("creating claude-stderr.log")?);
    let mut child = Command::new(claude_path())
        .arg("-p")
        .arg(&rendered.text)
        .args([
            "--output-format",
            "stream-json",
            "--verbose",
            "--dangerously-skip-permissions",
            "--max-turns",
            &caps.max_turns.to_string(),
        ])
        .current_dir(clone_dir)
        .envs(router_env())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .context("spawning claude -p")?;
    let outcome = wait_with_timeout(&mut child, cap)?;
    let elapsed = started.elapsed();
    // Diff FIRST (ordering constraint: before quality ever stages anything),
    // then telemetry, then governance.
    let diff = extract_diff(clone_dir, &pre_head)?;
    std::fs::write(run_dir.join("patch.diff"), &diff).context("writing patch.diff")?;
    let jsonl = std::fs::read_to_string(&transcript_path)
        .with_context(|| format!("reading {}", transcript_path.display()))?;
    // Unparseable transcript or zero assistant events: zeroed stats plus an
    // Error exit below — never a silent pass.
    let (stats, transcript_ok) = match parse_transcript(&jsonl) {
        Ok(stats) => (stats, true),
        Err(_) => (TranscriptStats::default(), false),
    };
    let governance = match arm {
        Arm::Control => None,
        Arm::Treatment => {
            // A missing log reads as empty, which summarize reports as NotWired.
            let log =
                std::fs::read_to_string(clone_dir.join(".phronesis/log.jsonl")).unwrap_or_default();
            match summarize(&log) {
                Ok(summary) => Some(summary),
                Err(GovernanceError::NotWired) => {
                    let rec = not_wired_record(&task.instance_id, arm)?;
                    write_record(run_dir, &rec)?;
                    return Ok(rec);
                }
                Err(e) => bail!(
                    "governance telemetry malformed for {}: {e}",
                    task.instance_id
                ),
            }
        }
    };
    let stderr_text = std::fs::read_to_string(&stderr_path).unwrap_or_default();
    let exit = if !transcript_ok {
        RunExit::Error {
            reason: "transcript_unparseable".into(),
        }
    } else {
        classify_exit(
            outcome.timed_out || elapsed > cap,
            stats.turns >= caps.max_turns,
            outcome.success,
            &stderr_text,
        )
    };
    let rec = RunRecord {
        instance_id: task.instance_id.clone(),
        arm,
        exit,
        resolved: None,
        turns: stats.turns,
        tokens_in: stats.tokens_in,
        tokens_out: stats.tokens_out,
        wall_clock_secs: elapsed.as_secs(),
        diff_bytes: diff.len() as u64,
        audit: None,
        governance,
    };
    write_record(run_dir, &rec)?;
    Ok(rec)
}

fn claude_path() -> String {
    std::env::var(CLAUDE_PATH_ENV).unwrap_or_else(|_| "claude".into())
}

fn router_env() -> HashMap<String, String> {
    ROUTER_ENV_KEYS
        .iter()
        .filter_map(|key| {
            std::env::var(key)
                .ok()
                .map(|value| ((*key).to_string(), value))
        })
        .collect()
}

fn record_head(clone_dir: &Path) -> Result<String> {
    let dir = clone_dir
        .to_str()
        .context("clone path contains invalid UTF-8")?;
    let out = Command::new("git")
        .args(["-C", dir, "rev-parse", "HEAD"])
        .output()
        .context("git rev-parse HEAD")?;
    if !out.status.success() {
        bail!(
            "git rev-parse HEAD failed in {}: {}",
            dir,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Poll the child, killing it at the wall-clock cap. A killed child is not
/// `success()` and is reported with `timed_out`.
fn wait_with_timeout(child: &mut Child, cap: Duration) -> Result<ChildOutcome> {
    let started = Instant::now();
    // Back off 50 ms -> 5 s between polls: fast children (and test fakes) are
    // reaped promptly while long runs settle to one poll every 5 s. The sleep
    // is always clamped to the remaining cap, so the kill lands on time.
    let mut poll = Duration::from_millis(50);
    loop {
        if let Some(status) = child.try_wait().context("polling claude child")? {
            return Ok(ChildOutcome {
                success: status.success(),
                timed_out: false,
            });
        }
        let elapsed = started.elapsed();
        if elapsed >= cap {
            child
                .kill()
                .context("killing claude child at the wall-clock cap")?;
            let status = child.wait().context("reaping killed claude child")?;
            return Ok(ChildOutcome {
                success: status.success(),
                timed_out: true,
            });
        }
        let remaining = cap - elapsed;
        std::thread::sleep(remaining.min(poll));
        poll = (poll * 2).min(Duration::from_secs(5));
    }
}

fn write_record(run_dir: &Path, rec: &RunRecord) -> Result<()> {
    let json = serde_json::to_vec_pretty(rec).context("serializing run record")?;
    std::fs::write(run_dir.join("record.json"), json).context("writing record.json")?;
    Ok(())
}
