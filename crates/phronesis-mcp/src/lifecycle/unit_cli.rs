//! `phr-mcp unit` — name the work item an agent is building, and close it.
//!
//! The work item *is* the existing `outcomes::subject` work unit (spec
//! §"Work items and governed throughput": "The work item is the existing work
//! unit"). This module adds the explicit bracket around it — a caller-chosen
//! id, a spec pointer, and the two lifecycle records that let a report find
//! the boundary — without introducing a second notion of "unit".

use std::path::Path;

use anyhow::{Context, bail};

use crate::action_log::{self, ReadOpts};
use crate::lifecycle::event::{Host, Kind, LifecycleEvent};
use crate::lifecycle::record::record;
use crate::outcomes::subject;
use crate::security;

#[derive(clap::Subcommand, Debug)]
pub enum UnitCmd {
    /// Open a work item (ends any open one first).
    Start {
        /// The work-item id. Omitted, a fresh `unit-<nanos>` id is minted.
        id: Option<String>,
        /// Repo-relative path to the spec this item is built to. Must exist.
        #[arg(long, value_name = "PATH")]
        spec: Option<String>,
    },
    /// Close the open work item.
    End,
}

/// Validate `--spec`: repo-relative, existing, inside the project root, no
/// `..`. `resolve_safe_path` enforces the last three (it canonicalizes, so
/// symlinks out of the tree are caught too); the absolute-path rejection is
/// ours, because the value we *record* must stay repo-relative — an absolute
/// path would leak a machine layout into the action log and would not resolve
/// on another checkout.
fn validate_spec(root: &Path, spec: &str) -> anyhow::Result<String> {
    if Path::new(spec).is_absolute() {
        bail!("--spec must be repo-relative, not `{spec}`");
    }
    security::resolve_safe_path(spec, root).with_context(|| format!("--spec `{spec}`"))?;
    Ok(spec.to_string())
}

/// Was `unit_id` opened by an explicit `unit start`? Read from the action log
/// (and its rotated predecessor) rather than tracked in a file: the spec says
/// "No new file", and the `unit_start` record is already the durable evidence.
/// A unit whose `unit_start` has rotated off reads as implicit, which is the
/// safe direction — it undercounts explicit units rather than claiming one.
fn was_started_explicitly(root: &Path, unit_id: &str) -> bool {
    action_log::read_recent(
        &action_log::default_path(root),
        &ReadOpts {
            kind: Some("lifecycle".to_string()),
            event: Some("unit_start".to_string()),
            ..ReadOpts::default()
        },
    )
    .unwrap_or_default()
    .iter()
    .any(|e| e.data.get("unit_id").and_then(|v| v.as_str()) == Some(unit_id))
}

/// Record `unit_end` for the open unit and clear the subject. Records **before**
/// clearing, so `record` stamps the closing record with the subject it closes —
/// the record a report joins on. Returns the id that was closed.
fn end_open(root: &Path) -> anyhow::Result<Option<String>> {
    let Some(id) = subject::current(root) else {
        return Ok(None);
    };
    let mut ev = LifecycleEvent::new(Kind::UnitEnd, Host::Cli).with_extra("unit_id", id.clone());
    if !was_started_explicitly(root, &id) {
        ev = ev.with_extra("implicit", true);
    }
    record(root, ev);
    subject::clear(root)?;
    Ok(Some(id))
}

pub fn run(root: &Path, cmd: UnitCmd) -> anyhow::Result<String> {
    match cmd {
        UnitCmd::Start { id, spec } => {
            // Validate first: a rejected spec must leave the open unit exactly
            // as it was, so a typo costs nothing.
            let spec = spec.map(|s| validate_spec(root, &s)).transpose()?;
            let ended = end_open(root)?;
            let id = match id {
                Some(id) => {
                    subject::set(root, &id)?;
                    id
                }
                None => subject::open(root)?,
            };
            let mut ev =
                LifecycleEvent::new(Kind::UnitStart, Host::Cli).with_extra("unit_id", id.clone());
            if let Some(s) = &spec {
                ev = ev.with_extra("spec", s.clone());
            }
            record(root, ev);
            let mut out = String::new();
            if let Some(e) = ended {
                out.push_str(&format!("ended work unit {e}\n"));
            }
            out.push_str(&format!("started work unit {id}"));
            if let Some(s) = &spec {
                out.push_str(&format!("   spec: {s}"));
            }
            Ok(out)
        }
        UnitCmd::End => match end_open(root)? {
            Some(id) => Ok(format!("ended work unit {id}")),
            None => bail!("no work unit open"),
        },
    }
}
