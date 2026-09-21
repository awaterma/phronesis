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
    /// A commit sha the host itself reported for this call, when its post
    /// payload carries one (Claude Code: `tool_response.gitOperation.commit
    /// .sha`). Corroboration, and the fallback when the HEAD probe failed.
    pub host_sha: Option<&'a str>,
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
    if !outcome::command_may_move_head(call.command) {
        return;
    }

    // Only a full object name is accepted: the host's abbreviation is not a
    // stable identifier, and this is recorded as one.
    let host_sha = call.host_sha.filter(|s| outcome::is_full_sha(s));

    // HEAD movement is the ground truth. The exit code only vetoes: absent, it
    // costs the record a `detection` marker, not the record itself.
    let detected = outcome::detect_commit(
        root,
        entry.head_before.as_deref(),
        call.command,
        command_exit,
    );
    let (sha, head_before, detection) = match detected {
        Some(commit) => {
            let marker = command_exit
                .is_none()
                .then_some(outcome::DETECTION_NO_EXIT_CODE);
            (commit.sha, Some(commit.head_before), marker)
        }
        // No usable comparison. The pre-side probe failed or timed out, so the
        // host's own report is the only evidence there is — enough to record
        // the commit, marked as resting on it. A non-zero exit still vetoes.
        None if entry.head_before.is_none() && outcome::exit_allows_detection(command_exit) => {
            match host_sha {
                Some(sha) => (
                    sha.to_string(),
                    None,
                    Some(outcome::DETECTION_HOST_REPORTED),
                ),
                None => return skipped(call.tool, entry.detection.as_deref()),
            }
        }
        None => return skipped(call.tool, entry.detection.as_deref()),
    };

    let mut ev = base(Kind::Commit).with_extra("sha", sha);
    if let Some(before) = head_before {
        ev = ev.with_extra("head_before", before);
    }
    if let Some(reported) = host_sha {
        ev = ev.with_extra("host_sha", reported);
    }
    if let Some(id) = call.tool_use_id.filter(|s| !s.is_empty()) {
        ev = ev.with_extra("tool_use_id", id);
    }
    if let Some(band) = confidence_band(root) {
        ev = ev.with_extra("confidence_band", band);
    }
    // The post-side marker names the evidence the record actually rests on, so
    // it wins over the pre-side `timeout`; the latter survives only when the
    // post side had nothing to say.
    if let Some(marker) = detection.or(entry.detection.as_deref()) {
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

/// Name a miss on stderr rather than letting it be silent: the pre-side marker
/// says the probe never produced a baseline (spec §"Success signal: commit").
fn skipped(tool: &str, marker: Option<&str>) {
    if let Some(marker) = marker {
        eprintln!("phronesis: commit detection skipped for {tool}: {marker}");
    }
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
