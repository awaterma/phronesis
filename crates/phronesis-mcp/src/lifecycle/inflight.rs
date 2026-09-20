//! The in-flight correlation entry and commit detection, shared by every host
//! adapter. Claude/Gemini (`hook::lifecycle_wiring`) and Codex
//! (`codex_hook`) differ only in how they read a tool call out of their own
//! payload shape, so they hand that reading in as a [`Call`] and a base-event
//! builder, and the ordering — HEAD probe before the tool runs, pop plus
//! detection after — lives here once.

use std::path::Path;

use serde_json::Value;

use crate::lifecycle::outcome;
use crate::lifecycle::record::record;
use crate::lifecycle::state;
use crate::lifecycle::{Kind, LifecycleEvent};
use crate::outcomes;

/// One host's tool call, already read out of its payload.
pub(crate) struct Call<'a> {
    pub tool: &'a str,
    pub tool_use_id: Option<&'a str>,
    pub tool_input: &'a Value,
    /// The shell command text, or empty for a non-shell tool.
    pub command: &'a str,
    /// The sub-agent that made the call, when the payload named one.
    pub agent_id: Option<&'a str>,
}

impl Call<'_> {
    fn key(&self) -> String {
        state::inflight_key_for(self.tool_use_id, self.tool, self.tool_input)
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Push the in-flight entry and return its key. Runs immediately after the
/// payload is read, before the allowlist and before rules load, so a tool
/// Phronesis does not govern still makes the next prompt a `correction`.
pub(crate) fn push(root: &Path, call: &Call<'_>) -> String {
    let key = call.key();
    // HEAD is read only for a shell tool whose command passes the text
    // pre-filter, so a shell call that cannot be a commit spawns no git process
    // at all (spec §"Success signal: commit" step 1). This is the one git call
    // on the pre path.
    let (head_before, detection) =
        if outcome::is_shell_tool(call.tool) && outcome::command_may_move_head(call.command) {
            match outcome::git_head_probe(root) {
                outcome::HeadProbe::Head(sha) => (Some(sha), None),
                // A timeout is a miss worth auditing: detection is disabled for
                // this call either way, but only a timeout means the commit may
                // have been real.
                outcome::HeadProbe::Timeout => (None, Some(outcome::DETECTION_TIMEOUT.to_string())),
                // Not a repo, or git unavailable: nothing to detect, nothing to
                // audit.
                outcome::HeadProbe::Unavailable => (None, None),
            }
        } else {
            (None, None)
        };
    state::push_inflight(
        root,
        state::Inflight {
            key: key.clone(),
            tool: call.tool.to_string(),
            ts: now_secs(),
            agent_id: call.agent_id.filter(|s| !s.is_empty()).map(str::to_string),
            head_before,
            detection,
        },
    );
    key
}

/// Drop the entry this call pushed, because the tool never ran. A block is not
/// an interrupt, so the entry must not survive to fake one.
pub(crate) fn drop_entry(root: &Path, key: &str) {
    state::pop_inflight(root, key);
}

/// Pop the entry this call pushed and, for a shell call that may have moved
/// HEAD, record a `commit`. The pop is unconditional and ignores the TTL: the
/// 900 s window is a classification rule, not a retention rule, and a long
/// build must still get its commit detected.
///
/// `base` builds the host's own empty event — Codex stamps session, turn and
/// agent onto it; Claude/Gemini pick the host from the tool name.
pub(crate) fn pop_and_detect(
    root: &Path,
    call: &Call<'_>,
    command_exit: Option<i32>,
    base: impl FnOnce(Kind) -> LifecycleEvent,
) {
    let entry = state::pop_inflight(root, &call.key());
    if !outcome::is_shell_tool(call.tool) {
        return;
    }
    let Some(entry) = entry else { return };

    // Why detection was skipped, when it was, named on stderr so the miss is
    // auditable rather than silent. `detection` on the entry came from the pre
    // side (a timed-out `git rev-parse`); `no_exit_code` is decided here,
    // because a host that sends no exit code cannot be given the benefit of the
    // doubt — that is what makes `git commit && false` a non-commit.
    if outcome::command_may_move_head(call.command) {
        let tool = call.tool;
        if let Some(marker) = entry.detection.as_deref() {
            eprintln!("phronesis: commit detection skipped for {tool}: {marker}");
        } else if command_exit.is_none() {
            eprintln!(
                "phronesis: commit detection skipped for {tool}: {}",
                outcome::DETECTION_NO_EXIT_CODE
            );
        }
    }

    let Some(commit) = outcome::detect_commit(
        root,
        entry.head_before.as_deref(),
        call.command,
        command_exit,
    ) else {
        return;
    };
    let mut ev = base(Kind::Commit)
        .with_extra("sha", commit.sha)
        .with_extra("head_before", commit.head_before);
    if let Some(id) = call.tool_use_id.filter(|s| !s.is_empty()) {
        ev = ev.with_extra("tool_use_id", id);
    }
    if let Some(band) = confidence_band(root) {
        ev = ev.with_extra("confidence_band", band);
    }
    if let Some(marker) = entry.detection.as_deref() {
        ev = ev.with_extra("detection", marker);
    }
    // The pre-side entry is the only witness of which sub-agent ran the call
    // when the post payload omits `agent_id`; a payload that named one has
    // already put it on the base event, and that one wins.
    if ev.agent_id.is_none()
        && let Some(agent) = entry.agent_id
    {
        ev = ev.with_agent(agent, None);
    }
    record(root, ev);
}

/// The band at commit time, when confidence scoring is enabled and a work
/// unit is open. Absent otherwise — no band is better than a fabricated one.
pub(crate) fn confidence_band(root: &Path) -> Option<&'static str> {
    if !outcomes::enabled(root) {
        return None;
    }
    Some(match outcomes::report(root, None)?.band {
        outcomes::Band::Low => "low",
        outcomes::Band::Medium => "medium",
        outcomes::Band::High => "high",
    })
}
