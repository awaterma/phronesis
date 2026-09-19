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
        KalpaCmd::Show { name } => match (name, state::read_kalpa(root)) {
            (None, Some(_)) => Ok(header_line(root, now()).unwrap_or_default()),
            (Some(n), Some(k)) if k.name == n => Ok(header_line(root, now()).unwrap_or_default()),
            (Some(n), _) => Ok(format!("kalpa: {n} (closed)")),
            (None, None) => anyhow::bail!("no kalpa open"),
        },
    }
}
