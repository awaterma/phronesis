//! Bounded verifier process runs (SPEC-verification-artifact-generation.md
//! §S9 "CPU/time limits"): a wall-clock limit enforced for every tier, the
//! whole process group killed on expiry (a verifier spawns solver children —
//! z3 — that must not outlive it), and stdout/stderr drained concurrently so
//! a chatty verifier cannot deadlock on a full pipe buffer.

use std::io::Read;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How a bounded run ended.
#[derive(Debug)]
pub enum RunOutcome {
    /// The process exited and both pipes closed within the limit.
    Finished(Output),
    /// The limit expired: the process group was killed. Carries whatever
    /// output was drained before the kill.
    TimedOut { stdout: Vec<u8>, stderr: Vec<u8> },
}

/// After a kill, how long to wait for the pipes to close. A descendant that
/// left the process group can hold a pipe open forever; its output is
/// abandoned rather than waited on.
const DRAIN_GRACE: Duration = Duration::from_secs(2);

/// How often the exit status is polled while output drains.
const POLL: Duration = Duration::from_millis(20);

/// Run `cmd` with stdin closed and stdout/stderr captured, in a fresh process
/// group, for at most `timeout`. On expiry `on_timeout` runs first (the
/// devcontainer tier stops its container there — killing the CLI client does
/// not stop a container), then the whole group is killed and reaped.
pub fn run_with_timeout(
    mut cmd: Command,
    timeout: Duration,
    on_timeout: impl FnOnce(),
) -> std::io::Result<RunOutcome> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // pgid = the child's pid: every descendant that does not setsid
        // itself away is reachable with one killpg.
        cmd.process_group(0);
    }
    let deadline = Instant::now() + timeout;
    let mut child = cmd.spawn()?;

    // Each pipe drains on its own thread; the pipe's index says which.
    let (tx, rx) = mpsc::channel::<(usize, Vec<u8>)>();
    let pipes: [Option<Box<dyn Read + Send>>; 2] = [
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    ];
    for (index, pipe) in pipes.into_iter().enumerate() {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut pipe) = pipe {
                // A read error ends the drain with what was read so far.
                let _ = pipe.read_to_end(&mut buf);
            }
            let _ = tx.send((index, buf));
        });
    }
    drop(tx);

    let mut outputs: [Option<Vec<u8>>; 2] = [None, None];
    let mut status: Option<ExitStatus> = None;
    loop {
        if status.is_none() {
            status = child.try_wait()?;
        }
        if let Some(status) = status
            && outputs.iter().all(Option::is_some)
        {
            let [stdout, stderr] = outputs.map(Option::unwrap_or_default);
            return Ok(RunOutcome::Finished(Output {
                status,
                stdout,
                stderr,
            }));
        }
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        let wait = POLL.min(deadline - now);
        if outputs.iter().all(Option::is_some) {
            // Both pipes closed; only the exit status is outstanding.
            std::thread::sleep(wait);
        } else if let Ok((index, buf)) = rx.recv_timeout(wait) {
            outputs[index] = Some(buf);
        }
    }

    // Expired. Stop anything the host runs out of band, then kill the group
    // while the leader is still unreaped (so its pgid cannot be reused).
    on_timeout();
    kill_group(&mut child);
    let _ = child.wait();
    let grace = Instant::now() + DRAIN_GRACE;
    while outputs.iter().any(Option::is_none) {
        let now = Instant::now();
        if now >= grace {
            break;
        }
        match rx.recv_timeout(grace - now) {
            Ok((index, buf)) => outputs[index] = Some(buf),
            Err(_) => break,
        }
    }
    let [stdout, stderr] = outputs.map(Option::unwrap_or_default);
    Ok(RunOutcome::TimedOut { stdout, stderr })
}

/// SIGKILL the child's whole process group (unix), then the child itself —
/// the latter covers a non-unix host and a child that changed its group.
fn kill_group(child: &mut Child) {
    #[cfg(unix)]
    if let Ok(pgid) = libc::pid_t::try_from(child.id()) {
        // SAFETY: killpg takes plain integers and has no memory effects; the
        // group id is our own unreaped child's pid (set by `process_group(0)`),
        // so it cannot name an unrelated group. Errors (ESRCH: group already
        // gone) are expected and ignored.
        unsafe {
            libc::killpg(pgid, libc::SIGKILL);
        }
    }
    let _ = child.kill();
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn sh(script: &str) -> Command {
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", script]);
        cmd
    }

    fn alive(pid: libc::pid_t) -> bool {
        // SAFETY: signal 0 only probes for existence; no signal is sent.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    /// A verifier that hangs (with a grandchild — the z3 shape) is killed at
    /// the limit, reported as timed out within the grace budget, and leaves
    /// no process behind.
    #[test]
    fn a_hung_run_times_out_and_leaves_no_grandchild() {
        let scratch = tempfile::tempdir().unwrap();
        let pid_file = scratch.path().join("grandchild.pid");
        let script = format!("sleep 60 & echo $! > '{}'; wait", pid_file.display());
        let started = Instant::now();
        let outcome = run_with_timeout(sh(&script), Duration::from_secs(1), || {}).unwrap();
        let elapsed = started.elapsed();
        assert!(
            matches!(outcome, RunOutcome::TimedOut { .. }),
            "{outcome:?}"
        );
        assert!(elapsed < Duration::from_secs(3), "took {elapsed:?}");

        let pid: libc::pid_t = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        // The orphaned grandchild is reparented and reaped by init; allow it
        // a moment to disappear.
        let gone_by = Instant::now() + Duration::from_secs(2);
        while alive(pid) && Instant::now() < gone_by {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!alive(pid), "grandchild {pid} outlived the timeout");
    }

    /// `on_timeout` runs on expiry (the devcontainer tier's container stop)
    /// and never on a run that finishes.
    #[test]
    fn on_timeout_runs_only_on_expiry() {
        let mut fired = false;
        let outcome =
            run_with_timeout(sh("exit 0"), Duration::from_secs(5), || fired = true).unwrap();
        assert!(matches!(outcome, RunOutcome::Finished(_)));
        assert!(!fired);
        let outcome =
            run_with_timeout(sh("sleep 60"), Duration::from_millis(200), || fired = true).unwrap();
        assert!(matches!(outcome, RunOutcome::TimedOut { .. }));
        assert!(fired);
    }

    /// Output larger than a pipe buffer on both streams drains concurrently:
    /// no deadlock, every byte captured, exit status kept.
    #[test]
    fn large_output_on_both_pipes_does_not_deadlock() {
        let script = "i=0; while [ $i -lt 4000 ]; do \
                      echo 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx'; \
                      echo 'yyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyy' >&2; \
                      i=$((i+1)); done; exit 3";
        let outcome = run_with_timeout(sh(script), Duration::from_secs(30), || {}).unwrap();
        let RunOutcome::Finished(output) = outcome else {
            panic!("large output must finish: {outcome:?}");
        };
        assert_eq!(output.status.code(), Some(3));
        assert_eq!(output.stdout.len(), 4000 * 64);
        assert_eq!(output.stderr.len(), 4000 * 64);
    }
}
