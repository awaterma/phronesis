//! Agent lifecycle events: sub-agent start/stop, prompts, interrupts, turn
//! stops, commits, kalpa boundaries. One type, two on-disk projections
//! (journey journal record, action-log entry), five small correlation files.
//! See `docs/specs/SPEC-agent-lifecycle-events.md`.

pub mod event;
pub mod inflight;
pub mod kalpa_cli;
pub mod outcome;
pub mod record;
pub mod scrub;
pub mod state;
pub mod unit_cli;
pub mod unit_report;

pub use event::{Host, Kind, LifecycleEvent, Mode, PromptText, Stamped};

/// Refuse an explicit lifecycle write (`kalpa start`, `unit start`,
/// `submit_suggestion`) when `root` is not a governed project. Writing there
/// would create exactly the stray `.phronesis/{journey,log.jsonl}` shape that
/// stops project-root discovery, and report success for a boundary no hook
/// will ever record against (hooks write nothing in an ungoverned root).
pub fn require_governed(root: &std::path::Path, what: &str) -> anyhow::Result<()> {
    if crate::security::is_governed(root) {
        return Ok(());
    }
    anyhow::bail!(
        "{what}: {} is not a governed phronesis project (no .phronesis/rules.json \
         or .phronesis/loader.json here or in any parent); run `phr-mcp init` \
         in the project root first",
        root.display()
    )
}
