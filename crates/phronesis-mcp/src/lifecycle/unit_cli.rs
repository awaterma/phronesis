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
use crate::outcomes::{bugs, subject};
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
        /// Name the item from `.phronesis/bugs.json`: the unit becomes
        /// `bug-<ID>` and carries the registry's test name (and its spec, if
        /// it has one). An unknown id is an error.
        #[arg(long = "bug", value_name = "ID", conflicts_with = "id")]
        bug_id: Option<String>,
    },
    /// Close the open work item.
    End,
    /// Report on a work item: its spec, rules, evidence, interventions, commits.
    Show {
        /// The work item to report on. Omitted, the open one.
        id: Option<String>,
        /// Emit the report as one JSON object.
        #[arg(long)]
        json: bool,
    },
}

/// What to name a work item, from any of the spec's three naming sources.
///
/// One struct rather than three `start_*` functions because the sources differ only
/// in where the name comes from: once resolved they take the identical path
/// (end the open unit, set the subject, record one `unit_start`), and a second
/// path is exactly what §"Where the name comes from" forbids.
#[derive(Debug, Default)]
pub struct StartRequest {
    /// Caller-chosen id. `None` with no `bug_id` mints `unit-<nanos>`.
    pub id: Option<String>,
    /// Repo-relative spec pointer; validated before the subject moves.
    pub spec: Option<String>,
    /// Look the name up in `.phronesis/bugs.json` instead: `bug-<bug_id>`.
    pub bug_id: Option<String>,
}

/// What a start actually did — the caller renders it (CLI text, MCP JSON).
#[derive(Debug)]
pub struct Started {
    pub unit_id: String,
    /// The spec finally recorded: `--spec` if given, else the registry's.
    pub spec: Option<String>,
    /// The unit this start closed first, when one was open.
    pub ended: Option<String>,
}

/// Open a work item. The one code path behind `phr-mcp unit start` and the
/// `submit_suggestion` MCP tool (spec §"Where the name comes from": "through
/// `lifecycle::unit_cli::start`, so there is one code path").
///
/// Everything that can fail — the spec pointer, the bug lookup — is checked
/// **before** the open subject moves, so a rejected start leaves the workspace
/// exactly as it was. `Host::Cli` is stamped on the record from both callers:
/// the MCP server is the same `phr-mcp` process, and `Host` names the host
/// that produced the event, not the transport that asked for it.
pub fn start(root: &Path, req: StartRequest) -> anyhow::Result<Started> {
    let mut spec = req.spec.map(|s| validate_spec(root, &s)).transpose()?;
    let mut test = None;
    let mut chosen = req.id;
    if let Some(bug_id) = &req.bug_id {
        let bugs = bugs::load(root);
        let Some(bug) = bugs.iter().find(|b| &b.bug_id == bug_id) else {
            // `load` is fail-open, so a missing or malformed registry lands
            // here too — which is right: either way the id is not known.
            bail!("unknown bug id `{bug_id}` (not in .phronesis/bugs.json)");
        };
        chosen = Some(format!("bug-{bug_id}"));
        test = Some(bug.test.clone());
        if spec.is_none()
            && let Some(s) = &bug.spec
        {
            spec = Some(validate_spec(root, s)?);
        }
    }
    let ended = end_open(root)?;
    let unit_id = match chosen {
        Some(id) => {
            subject::set(root, &id)?;
            id
        }
        None => subject::open(root)?,
    };
    let mut ev =
        LifecycleEvent::new(Kind::UnitStart, Host::Cli).with_extra("unit_id", unit_id.clone());
    if let Some(s) = &spec {
        ev = ev.with_extra("spec", s.clone());
    }
    if let Some(t) = &test {
        ev = ev.with_extra("test", t.clone());
    }
    if let Some(b) = &req.bug_id {
        ev = ev.with_extra("bug_id", b.clone());
    }
    record(root, ev);
    Ok(Started {
        unit_id,
        spec,
        ended,
    })
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
        UnitCmd::Start { id, spec, bug_id } => {
            let started = start(root, StartRequest { id, spec, bug_id })?;
            let mut out = String::new();
            if let Some(e) = started.ended {
                out.push_str(&format!("ended work unit {e}\n"));
            }
            out.push_str(&format!("started work unit {}", started.unit_id));
            if let Some(s) = &started.spec {
                out.push_str(&format!("   spec: {s}"));
            }
            Ok(out)
        }
        UnitCmd::End => match end_open(root)? {
            Some(id) => Ok(format!("ended work unit {id}")),
            None => bail!("no work unit open"),
        },
        UnitCmd::Show { id, json } => {
            let id = match id.or_else(|| subject::current(root)) {
                Some(id) => id,
                None => bail!("no work unit open"),
            };
            let report = crate::lifecycle::unit_report::build(root, &id);
            Ok(if json {
                crate::lifecycle::unit_report::render_json(&report)
            } else {
                crate::lifecycle::unit_report::render(&report)
                    .trim_end()
                    .to_string()
            })
        }
    }
}
