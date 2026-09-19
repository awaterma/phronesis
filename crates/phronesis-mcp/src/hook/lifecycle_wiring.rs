//! Lifecycle side effects of the tool hooks: the `inflight` correlation
//! entry, commit detection from HEAD movement, and Gemini's `invoke_agent`
//! sub-agent derivation. Kept out of `pre.rs`/`post.rs` so those stay about
//! rule evaluation. Everything here is best-effort and never changes an exit
//! code (spec §"Where the writes happen").

use std::path::Path;

use serde_json::Value;

use crate::claude_hook::{synth_agent_id, unix_secs_now};
use crate::lifecycle::record::record;
use crate::lifecycle::state;
use crate::lifecycle::{Host, Kind, LifecycleEvent, outcome};
use crate::outcomes;

use super::HookPayload;

/// The synthetic path an `invoke_agent` tool record carries — never the
/// sub-agent prompt, which would put content in the journal.
pub(crate) const INVOKE_AGENT_PATH: &str = "<invoke_agent>";

fn tool_of(payload: &HookPayload) -> String {
    payload.tool_name.clone().unwrap_or_default()
}

fn input_of(payload: &HookPayload) -> Value {
    payload.tool_input.clone().unwrap_or(Value::Null)
}

/// Push the in-flight entry. Runs immediately after `read_payload`, before the
/// allowlist and before rules load, so a tool Phronesis does not govern still
/// makes the next prompt a `correction`.
pub(super) fn pre_push_inflight(root: &Path, payload: &HookPayload) -> String {
    let tool = tool_of(payload);
    let input = input_of(payload);
    let key = state::inflight_key_for(payload.tool_use_id.as_deref(), &tool, &input);
    // HEAD is read only for a shell tool whose command passes the text
    // pre-filter, so a shell call that cannot be a commit spawns no git process
    // at all (spec §"Success signal: commit" step 1). This is the one git call
    // on the pre path.
    let command = super::extract_new_content(payload, &tool).unwrap_or_default();
    let (head_before, detection) =
        if outcome::is_shell_tool(&tool) && outcome::command_may_move_head(&command) {
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
            tool,
            ts: unix_secs_now(),
            agent_id: payload.agent_id.clone().filter(|s| !s.is_empty()),
            head_before,
            detection,
        },
    );
    key
}

/// Undo everything this pre-check pushed, because the tool never ran. The
/// `inflight` entry goes because a block is not an interrupt; the `agents` entry
/// goes because a sub-agent that never ran must not leave a dangling entry for
/// the next real stop to pop LIFO (spec §"Where the writes happen").
pub(super) fn undo_blocked_pre(root: &Path, key: &str, tool_name: &str) {
    state::pop_inflight(root, key);
    if tool_name == "invoke_agent" {
        state::pop_agent(root, None);
    }
}

/// Pop the entry this call pushed and, for a shell call that may have moved
/// HEAD, record a `commit`.
pub(super) fn post_pop_and_detect(root: &Path, payload: &HookPayload, tool_name: &str) {
    let key = state::inflight_key_for(
        payload.tool_use_id.as_deref(),
        tool_name,
        &input_of(payload),
    );
    let entry = state::pop_inflight(root, &key);
    if !outcome::is_shell_tool(tool_name) {
        return;
    }
    let Some(entry) = entry else { return };
    let command = super::extract_new_content(payload, tool_name).unwrap_or_default();
    let exit = super::journey_record::payload_command_exit(payload);

    // Why detection was skipped, when it was, named on stderr so the miss is
    // auditable rather than silent. `detection` on the entry came from the pre
    // side (a timed-out `git rev-parse`); `no_exit_code` is decided here,
    // because a host that sends no exit code cannot be given the benefit of the
    // doubt — that is what makes `git commit && false` a non-commit.
    if outcome::command_may_move_head(&command) {
        if let Some(marker) = entry.detection.as_deref() {
            eprintln!("phronesis: commit detection skipped for {tool_name}: {marker}");
        } else if exit.is_none() {
            eprintln!(
                "phronesis: commit detection skipped for {tool_name}: {}",
                outcome::DETECTION_NO_EXIT_CODE
            );
        }
    }

    let Some(commit) = outcome::detect_commit(root, entry.head_before.as_deref(), &command, exit)
    else {
        return;
    };
    let mut ev = LifecycleEvent::new(Kind::Commit, host_for_tool(tool_name))
        .with_extra("sha", commit.sha)
        .with_extra("head_before", commit.head_before);
    if let Some(id) = payload.tool_use_id.as_deref().filter(|s| !s.is_empty()) {
        ev = ev.with_extra("tool_use_id", id);
    }
    if let Some(band) = confidence_band(root) {
        ev = ev.with_extra("confidence_band", band);
    }
    if let Some(marker) = entry.detection.as_deref() {
        ev = ev.with_extra("detection", marker);
    }
    if let Some(agent) = entry.agent_id {
        ev = ev.with_agent(agent, None);
    }
    record(root, ev);
}

/// `Bash` is Claude's shell tool; `run_shell_command` is Gemini's.
fn host_for_tool(tool_name: &str) -> Host {
    if tool_name == "Bash" {
        Host::Claude
    } else {
        Host::Gemini
    }
}

/// The band at commit time, when confidence scoring is enabled and a work
/// unit is open. Absent otherwise — no band is better than a fabricated one.
fn confidence_band(root: &Path) -> Option<&'static str> {
    if !outcomes::enabled(root) {
        return None;
    }
    Some(match outcomes::report(root, None)?.band {
        outcomes::Band::Low => "low",
        outcomes::Band::Medium => "medium",
        outcomes::Band::High => "high",
    })
}

/// Gemini has no sub-agent event: it invokes one through the `invoke_agent`
/// tool, so the pair is derived from `BeforeTool`/`AfterTool` (spec §"Host
/// adapters / Gemini CLI"). The id is synthesized and the stop pops LIFO.
pub(super) fn gemini_subagent_start(root: &Path, payload: &HookPayload) {
    // Raw here; `LifecycleEvent::with_agent` and `OpenAgent` both receive it
    // through `sanitize_agent_type`, so the hostile-name case is handled once,
    // in Plan 1, rather than at each of the two call sites.
    let agent_type = payload
        .tool_input
        .as_ref()
        .and_then(|v| v.get("agent_name"))
        .and_then(Value::as_str)
        .and_then(crate::lifecycle::event::sanitize_agent_type);
    let agent_id = synth_agent_id(root);
    let ev = LifecycleEvent::new(Kind::SubagentStart, Host::Gemini)
        .with_agent(agent_id.clone(), agent_type.clone());
    let stamped = record(root, ev);
    state::push_agent(
        root,
        state::OpenAgent {
            agent_id,
            agent_type,
            ts: stamped.as_ref().map(|s| s.ts).unwrap_or_else(unix_secs_now),
            seq: stamped.as_ref().map(|s| s.seq).unwrap_or(0),
        },
    );
}

pub(super) fn gemini_subagent_stop(root: &Path) {
    let open = state::pop_agent(root, None);
    let now = unix_secs_now();
    let mut ev = LifecycleEvent::new(Kind::SubagentStop, Host::Gemini)
        .with_extra("matched_start", open.is_some());
    if let Some(o) = &open {
        ev = ev
            .with_agent(o.agent_id.clone(), o.agent_type.clone())
            .with_extra("duration_secs", now.saturating_sub(o.ts));
    }
    record(root, ev);
}
