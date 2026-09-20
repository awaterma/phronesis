//! Lifecycle side effects of the tool hooks: the `inflight` correlation
//! entry, commit detection from HEAD movement, and Gemini's `invoke_agent`
//! sub-agent derivation. Kept out of `pre.rs`/`post.rs` so those stay about
//! rule evaluation. Everything here is best-effort and never changes an exit
//! code (spec §"Where the writes happen").

use std::path::Path;

use serde_json::Value;

use crate::claude_hook::{synth_agent_id, unix_secs_now};
use crate::lifecycle::inflight;
use crate::lifecycle::record::record;
use crate::lifecycle::state;
use crate::lifecycle::{Host, Kind, LifecycleEvent};

use super::HookPayload;

/// The synthetic path an `invoke_agent` tool record carries — never the
/// sub-agent prompt, which would put content in the journal.
pub(crate) const INVOKE_AGENT_PATH: &str = "<invoke_agent>";

fn tool_of(payload: &HookPayload) -> String {
    payload.tool_name.clone().unwrap_or_default()
}

/// Read this payload as a shared [`inflight::Call`].
fn inflight_call<'a>(
    payload: &'a HookPayload,
    tool: &'a str,
    command: &'a str,
) -> inflight::Call<'a> {
    inflight::Call {
        tool,
        tool_use_id: payload.tool_use_id.as_deref(),
        tool_input: payload.tool_input.as_ref().unwrap_or(&Value::Null),
        command,
        agent_id: payload.agent_id.as_deref(),
    }
}

/// Push the in-flight entry. Runs immediately after `read_payload`, before the
/// allowlist and before rules load, so a tool Phronesis does not govern still
/// makes the next prompt a `correction`.
pub(super) fn pre_push_inflight(root: &Path, payload: &HookPayload) -> String {
    let tool = tool_of(payload);
    let command = super::extract_new_content(payload, &tool).unwrap_or_default();
    inflight::push(root, &inflight_call(payload, &tool, &command))
}

/// Undo everything this pre-check pushed, because the tool never ran. The
/// `inflight` entry goes because a block is not an interrupt. The `agents`
/// entry goes because a sub-agent that never ran must not leave a dangling
/// entry for the next real stop to pop LIFO (spec §"Where the writes happen")
/// — but the `subagent_start` is already durable by then, recorded before the
/// decision, so popping alone would leave a start that never stops. The
/// compensating `subagent_stop` closes the pair at zero duration and says why.
pub(super) fn undo_blocked_pre(root: &Path, key: &str, tool_name: &str) {
    inflight::drop_entry(root, key);
    if tool_name == "invoke_agent"
        && let Some(open) = state::pop_agent(root, None)
    {
        record(
            root,
            LifecycleEvent::new(Kind::SubagentStop, Host::Gemini)
                .with_agent(open.agent_id, open.agent_type)
                .with_extra("matched_start", true)
                .with_extra("duration_secs", 0)
                .with_extra("blocked", true),
        );
    }
}

/// Pop the entry this call pushed and, for a shell call that may have moved
/// HEAD, record a `commit`.
pub(super) fn post_pop_and_detect(root: &Path, payload: &HookPayload, tool_name: &str) {
    let command = super::extract_new_content(payload, tool_name).unwrap_or_default();
    inflight::pop_and_detect(
        root,
        &inflight_call(payload, tool_name, &command),
        super::journey_record::payload_command_exit(payload),
        |kind| LifecycleEvent::new(kind, host_for_tool(tool_name)),
    );
}

/// `Bash` is Claude's shell tool; `run_shell_command` is Gemini's.
fn host_for_tool(tool_name: &str) -> Host {
    if tool_name == "Bash" {
        Host::Claude
    } else {
        Host::Gemini
    }
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
