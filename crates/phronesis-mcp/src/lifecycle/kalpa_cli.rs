//! `phr-mcp kalpa` — name the theme a run of sessions belongs to.

use std::path::Path;

use crate::lifecycle::event::{Host, Kind, LifecycleEvent};
use crate::lifecycle::record::record;
use crate::lifecycle::state::{self, Kalpa};

#[derive(clap::Subcommand, Debug)]
pub enum KalpaCmd {
    /// Open a kalpa (ends any open one first). Name: [a-z0-9][a-z0-9-]{0,63}.
    Start { name: String },
    /// Close the open kalpa.
    End,
    /// Show the open kalpa, or a named one.
    Show { name: Option<String> },
}

const STALE_AFTER_SECS: u64 = 30 * 24 * 3600;

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn age(secs: u64) -> String {
    match secs {
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

pub fn header_line(root: &Path, now: u64) -> Option<String> {
    let k = state::read_kalpa(root)?;
    let elapsed = now.saturating_sub(k.started_ts);
    let mut line = format!("kalpa: {} ({})", k.name, age(elapsed));
    if elapsed > STALE_AFTER_SECS {
        line.push_str(" (stale? run phr-mcp kalpa end)");
    }
    Some(line)
}

/// Records `kalpa_end` **while the kalpa is still open**, then clears it, so the
/// boundary record carries `kalpa: "<name>"` and the `kalpa:<name>` tag like
/// every other record in the kalpa (spec §"Naming the kalpa": "Every lifecycle
/// record written while a kalpa is open carries `kalpa`"). Clearing first would
/// leave the one record that closes the theme unattributable to it, which is
/// exactly the record `phr-mcp kalpa show` needs to find the boundary.
fn end_open(root: &Path) -> Option<String> {
    let k = state::read_kalpa(root)?;
    record(root, LifecycleEvent::new(Kind::KalpaEnd, Host::Cli));
    state::clear_kalpa(root);
    Some(k.name)
}

pub fn run(root: &Path, cmd: KalpaCmd) -> anyhow::Result<String> {
    match cmd {
        KalpaCmd::Start { name } => {
            if !state::valid_kalpa_name(&name) {
                anyhow::bail!("invalid kalpa name `{name}`: use [a-z0-9][a-z0-9-]{{0,63}}");
            }
            let ended = end_open(root);
            state::write_kalpa(
                root,
                &Kalpa {
                    name: name.clone(),
                    started_ts: now(),
                },
            );
            record(root, LifecycleEvent::new(Kind::KalpaStart, Host::Cli));
            Ok(match ended {
                Some(e) => format!("ended kalpa {e}\nstarted kalpa {name}"),
                None => format!("started kalpa {name}"),
            })
        }
        KalpaCmd::End => match end_open(root) {
            Some(n) => Ok(format!("ended kalpa {n}")),
            None => anyhow::bail!("no kalpa open"),
        },
        KalpaCmd::Show { name } => {
            let now = now();
            match (name, state::read_kalpa(root)) {
                (None, Some(k)) => Ok(report(root, &k.name, now)),
                (Some(n), _) => Ok(report(root, &n, now)),
                (None, None) => anyhow::bail!("no kalpa open"),
            }
        }
    }
}

/// The report block from SPEC-agent-lifecycle-events §Reporting. Reads the same
/// action-log entries `phr-mcp stats` reads and counts them with the same
/// function, so the two surfaces cannot disagree.
fn report(root: &Path, kalpa: &str, now: u64) -> String {
    use crate::action_log::{self, ReadOpts};
    use crate::stats::{LifecycleOpts, aggregate_lifecycle, render_lifecycle, retention_line};

    let entries = action_log::read_recent(
        &action_log::default_path(root),
        &ReadOpts {
            kind: Some("lifecycle".to_string()),
            ..ReadOpts::default()
        },
    )
    .unwrap_or_default();
    let stats = aggregate_lifecycle(
        &entries,
        &LifecycleOpts {
            since_secs: None,
            kalpa: Some(kalpa.to_string()),
            now_secs: now,
        },
    );

    // "when the `kalpa_start` entry has itself rotated off, the header prints
    // `start not retained` in place of the start date" (spec §Reporting). The
    // open `kalpa` file still knows `started_ts`, but a *closed* kalpa's start
    // is only knowable from the log — and `kalpa end` deletes the file, so the
    // date is gone with the rotation. Saying "start not retained" is honest;
    // printing the oldest retained entry as if it were the start is not.
    let start_retained = entries.iter().any(|e| {
        e.event == "kalpa_start" && e.data.get("kalpa").and_then(|v| v.as_str()) == Some(kalpa)
    });
    let head = match state::read_kalpa(root).filter(|k| k.name == kalpa) {
        Some(k) => {
            let started = chrono::DateTime::from_timestamp(k.started_ts as i64, 0)
                .map(|dt| {
                    dt.with_timezone(&chrono::Local)
                        .format("%Y-%m-%d")
                        .to_string()
                })
                .unwrap_or_else(|| k.started_ts.to_string());
            format!(
                "kalpa: {kalpa}      started {started} ({})      {}",
                age(now.saturating_sub(k.started_ts)),
                retention_line(stats.oldest_entry_ts)
            )
        }
        None if start_retained => {
            let started = entries
                .iter()
                .find(|e| {
                    e.event == "kalpa_start"
                        && e.data.get("kalpa").and_then(|v| v.as_str()) == Some(kalpa)
                })
                .map(|e| e.ts)
                .unwrap_or_default();
            let when = chrono::DateTime::from_timestamp(started as i64, 0)
                .map(|dt| {
                    dt.with_timezone(&chrono::Local)
                        .format("%Y-%m-%d")
                        .to_string()
                })
                .unwrap_or_else(|| started.to_string());
            format!(
                "kalpa: {kalpa} (closed)      started {when}      {}",
                retention_line(stats.oldest_entry_ts)
            )
        }
        None => format!(
            "kalpa: {kalpa} (closed)      start not retained      {}",
            retention_line(stats.oldest_entry_ts)
        ),
    };
    format!("{head}\n{}", render_lifecycle(&stats))
}
