//! Claude Code lifecycle hook adapter.
//!
//! Reads one Claude Code hook payload from stdin, records the lifecycle event
//! it represents, and prints exactly one JSON object. Tool phases never reach
//! the body of this module: they delegate to the existing `pre-check` /
//! `post-check` runners before stdin is touched, keeping their exit-code
//! contract intact.
//!
//! Gemini CLI's `BeforeAgent` / `AfterAgent` (registered by the Gemini init
//! writer) are accepted here too and mapped onto the Claude vocabulary; the
//! host is inferred from the incoming event name.
//!
//! See `docs/specs/SPEC-agent-lifecycle-events.md` §"Host adapters".

use std::path::{Path, PathBuf};
use std::process;

use serde::Deserialize;

use crate::context;
use crate::hook;
use crate::journey;
use crate::lifecycle::record::record;
use crate::lifecycle::state;
use crate::lifecycle::{Host, Kind, LifecycleEvent};
use crate::outcomes;
use crate::security;

/// Every field is optional: Claude Code, Gemini CLI and our own fixtures each
/// send a different subset, and a missing field must never fail a hook.
#[derive(Debug, Default, Deserialize)]
pub struct ClaudePayload {
    #[serde(default)]
    pub hook_event_name: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    /// `SessionStart` only: `startup` | `resume` | `clear` | `compact` | `fork`.
    /// The source gate turns on this one field (spec §Correlation state).
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub prompt_id: Option<String>,
    #[serde(default)]
    pub transcript_path: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub agent_type: Option<String>,
    #[serde(default)]
    pub agent_transcript_path: Option<String>,
    #[serde(default)]
    pub stop_hook_active: bool,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub tool_input: Option<serde_json::Value>,
    #[serde(default)]
    pub tool_response: Option<serde_json::Value>,
    /// Gemini `AfterAgent` only. Read for nothing and **never persisted**; it is
    /// declared so `serde` sees the whole payload and so the capture redaction
    /// has a name to redact. Dropped at this boundary.
    #[serde(default)]
    pub prompt_response: Option<String>,
    /// Codex `SubagentStop` shape, accepted here for the same reason and
    /// dropped at the same boundary.
    #[serde(default)]
    pub last_assistant_message: Option<String>,
}

const EMPTY: &str = "{}";

/// Map a host's event name onto the Claude vocabulary this module dispatches
/// on. Gemini's `BeforeAgent` is a prompt and `AfterAgent` is a turn stop
/// (spec §"Host adapters / Gemini CLI").
pub(crate) fn canonical_event(event: &str) -> &str {
    match event {
        "BeforeAgent" => "UserPromptSubmit",
        "AfterAgent" => "Stop",
        "BeforeTool" => "PreToolUse",
        "AfterTool" => "PostToolUse",
        other => other,
    }
}

/// Which host sent this. Only names unique to Gemini identify it; the shared
/// names (`SessionStart`, `SessionEnd`) default to Claude, which is what the
/// Claude init writer registers.
pub(crate) fn host_for(event: &str) -> Host {
    match event {
        "BeforeAgent" | "AfterAgent" | "BeforeTool" | "AfterTool" => Host::Gemini,
        _ => Host::Claude,
    }
}

pub(crate) fn unix_secs_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub async fn run(event: &str) -> ! {
    // Tool phases delegate before stdin is read: the runners read and parse it
    // themselves and own their exit codes (pre 0/1/2, post 0/1). Both are
    // `async fn … -> anyhow::Result<()>` and exit the process internally on the
    // allow and block paths; the `Err` return is the remaining path, and
    // `main.rs` maps it to exit 1 (`Command::PreCheck => hook::run_pre_check().await`,
    // `main.rs:570-571`). `let _ = …; process::exit(0)` would rewrite that 1
    // into a 0, so the error is reproduced here instead of discarded.
    match canonical_event(event) {
        "PreToolUse" => match hook::run_pre_check().await {
            Ok(()) => process::exit(0),
            Err(e) => {
                eprintln!("phronesis: {e}");
                process::exit(1);
            }
        },
        "PostToolUse" => match hook::run_post_check().await {
            Ok(()) => process::exit(0),
            Err(e) => {
                eprintln!("phronesis: {e}");
                process::exit(1);
            }
        },
        _ => {}
    }

    let root = security::project_root();
    let raw = match security::read_stdin_capped() {
        Ok(raw) => raw,
        Err(e) => {
            eprintln!("phronesis: claude-hook {event}: stdin read failed: {e}");
            println!("{EMPTY}");
            process::exit(0);
        }
    };
    // Tees to PHRONESIS_CAPTURE_DIR; `redact_for_capture` (Plan 1) strips
    // `prompt` and `last_assistant_message` inside it, so no prompt text ever
    // reaches `payloads.jsonl`.
    hook::capture_raw_payload(event, &raw);

    let payload: ClaudePayload = match serde_json::from_str(&raw) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("phronesis: claude-hook {event}: payload parse failed: {e}");
            println!("{EMPTY}");
            process::exit(0);
        }
    };

    // The payload's own name wins: a settings file may name the event
    // differently from what the host actually fired.
    let named = payload
        .hook_event_name
        .clone()
        .unwrap_or_else(|| event.to_string());
    let host = host_for(&named);
    // Gemini-specific event names identify Gemini unambiguously; stamp it so a
    // later SessionEnd (shared name) can inherit the host. SessionStart and
    // SessionEnd are shared with Claude, so when host_for defaults to Claude,
    // check whether a prior Gemini event stamped the host file.
    let host = if host == Host::Gemini {
        state::set_host(&root, "gemini");
        host
    } else if matches!(named.as_str(), "SessionStart" | "SessionEnd") {
        match state::read_host(&root).as_deref() {
            Some("gemini") => Host::Gemini,
            _ => host,
        }
    } else {
        host
    };
    let response = dispatch(&root, canonical_event(&named), host, &payload).await;
    println!("{response}");
    process::exit(0);
}

async fn dispatch(root: &Path, event: &str, host: Host, p: &ClaudePayload) -> String {
    match event {
        "UserPromptSubmit" => handle_prompt(root, host, p).await,
        "SessionStart" => handle_session_start(root, p).await,
        "SessionEnd" => {
            handle_session_end(root, host, p);
            EMPTY.to_string()
        }
        "SubagentStart" => {
            handle_subagent_start(root, host, p);
            EMPTY.to_string()
        }
        // The response is decided FIRST, and the handler runs only when it does
        // not block. A blocked stop means Claude continues the same turn, so it
        // is not a stop: it records nothing and leaves `turn` open (spec §"A
        // blocked stop is not a stop"). Recording first and then discovering the
        // block would close a turn that is still running, and the next steer
        // would classify `fresh`.
        "SubagentStop" => {
            let response = completion_response(root, host, p);
            if !blocks(&response) {
                handle_subagent_stop(root, host, p);
            }
            response
        }
        "Stop" => {
            let response = completion_response(root, host, p);
            if !blocks(&response) {
                handle_stop(root, host, p);
            }
            response
        }
        _ => EMPTY.to_string(),
    }
}

/// `handle_*` renders may legitimately be empty; the Claude and Gemini hook
/// protocols both require parseable JSON on stdout, so empty becomes `{}`.
fn context_or_empty(rendered: String) -> String {
    if rendered.trim().is_empty() {
        EMPTY.to_string()
    } else {
        rendered
    }
}

/// `{"decision":"block","reason":…}` when the confidence gate blocks, else
/// `{}`. `stop_hook_active` short-circuits without evaluating the gate, as the
/// Claude docs require, so a blocking gate cannot loop.
///
/// The gate is **Claude- and Codex-only**. Gemini has no documented `decision`
/// semantics for `AfterAgent`, so a Gemini-mapped stop records its event and
/// prints `{}`; blocking a host whose response schema we have not established
/// would be guessing with the user's turn (spec §"Host adapters / Gemini CLI").
fn completion_response(root: &Path, host: Host, p: &ClaudePayload) -> String {
    if host == Host::Gemini || p.stop_hook_active {
        return EMPTY.to_string();
    }
    match gate_block_reason(root) {
        Some(reason) => serde_json::json!({"decision": "block", "reason": reason}).to_string(),
        None => EMPTY.to_string(),
    }
}

/// Mirrors `codex_hook::make_completion_decision`: low confidence blocks,
/// medium warns on stderr, high is silent.
fn gate_block_reason(root: &Path) -> Option<String> {
    if !outcomes::enabled(root) {
        return None;
    }
    let report = outcomes::report(root, None)?;
    match report.band {
        outcomes::Band::Low => Some(format!(
            "Low confidence for {} — resolve failing or missing grounded signals before completing.",
            report.subject
        )),
        outcomes::Band::Medium => {
            eprintln!(
                "phronesis: Medium confidence for {} — one grounded signal is still missing.",
                report.subject
            );
            None
        }
        outcomes::Band::High => None,
    }
}

/// Did the completion response block? One definition, so the two arms above
/// cannot drift, and structural rather than a substring test on the JSON text.
fn blocks(response: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(response)
        .ok()
        .and_then(|v| {
            v.get("decision")
                .and_then(|d| d.as_str())
                .map(str::to_string)
        })
        .as_deref()
        == Some("block")
}

fn nonempty(v: &Option<String>) -> Option<&str> {
    v.as_deref().filter(|s| !s.is_empty())
}

/// Stamp session and turn ids when the host supplied them.
fn with_session_turn(mut ev: LifecycleEvent, p: &ClaudePayload) -> LifecycleEvent {
    if let Some(s) = nonempty(&p.session_id) {
        ev = ev.with_session(s);
    }
    if let Some(t) = nonempty(&p.prompt_id) {
        ev = ev.with_turn(t);
    }
    ev
}

/// The `{sid}:{seq}` fallback for hosts that supply no agent id (Gemini, and
/// Claude internal forks with empty fields). See spec §"Correlation state".
pub(crate) fn synth_agent_id(root: &Path) -> String {
    format!(
        "{}:{}",
        journey::current_sid(root),
        crate::hook::seq::next_seq(root)
    )
}

async fn handle_prompt(root: &Path, host: Host, p: &ClaudePayload) -> String {
    let now = unix_secs_now();
    let transcript = p.transcript_path.as_deref().map(PathBuf::from);
    // Spec §"Host adapters / Claude Code": "UserPromptSubmit → render
    // interaction context, then record prompt." Rendering first means the
    // context the human sees describes the state their prompt arrived into,
    // not one that already counts their own prompt.
    let rendered = context_or_empty(
        context::run_interaction_context_configured(root, 5, context::DEFAULT_MAX_BYTES).await,
    );
    let agent_id = nonempty(&p.agent_id);
    let classification = state::classify_prompt(
        root,
        &state::PromptContext {
            host,
            now,
            agent_id,
            turn_id: nonempty(&p.prompt_id),
            transcript_path: transcript.as_deref(),
        },
    );

    // `Some(source)` means this handler must write the record;
    // `None` with mode `Correction` means it already exists (Codex's Interrupt
    // hook, or an earlier inferred branch), and a second record would
    // double-count the friction.
    if let Some(source) = classification.interrupt {
        let ev = with_session_turn(
            LifecycleEvent::new(Kind::Interrupt, host).with_extra("inferred_from", source.as_str()),
            p,
        );
        record(root, ev);
        state::close_turn(root, "interrupt");
    }

    let mut ev = LifecycleEvent::new(Kind::Prompt, host).with_mode(classification.mode);
    ev = with_session_turn(ev, p);
    if let Some(a) = agent_id {
        ev = ev.with_agent(a, p.agent_type.clone());
    }
    if let Some(text) = p.prompt.as_deref() {
        ev = ev.with_prompt(crate::lifecycle::scrub::scrub_prompt(root, text));
    }
    record(root, ev);

    // A prompt carrying an `agent_id` never writes `turn`: a sub-agent's prompt
    // must not move the parent's turn state or its `last_prompt_ts`, which the
    // transcript-marker comparison keys on (spec §Correlation state).
    if agent_id.is_none() {
        state::open_turn(root, nonempty(&p.prompt_id), now);
    }

    rendered
}

async fn handle_session_start(root: &Path, p: &ClaudePayload) -> String {
    // Source-gated. `startup` / `resume` / `clear` begin a session: adopt the
    // host's id and truncate correlation state. `compact` / `fork` continue one,
    // so its open sub-agents and in-flight tools are real and are left alone —
    // only the context render runs.
    if state::is_session_begin(p.source.as_deref()) {
        // The host's id wins over the create-on-miss id `current_sid` would mint.
        if let Some(sid) = nonempty(&p.session_id) {
            state::set_session(root, sid);
        }
        state::reset_for_session_start(root);
    }
    context_or_empty(
        context::run_session_context_configured(root, context::DEFAULT_MAX_BYTES).await,
    )
}

/// Quitting out of an aborted turn must not be recorded as a completed turn, so
/// SessionEnd runs the same interrupt detection step 2 of the classification
/// runs and records `interrupt` when there is evidence, `stop` otherwise.
///
/// `session` is left in place: truncating it would let any stray hook between
/// sessions mint a throwaway sid, and the next session-begin overwrites anyway.
fn handle_session_end(root: &Path, host: Host, p: &ClaudePayload) {
    if state::read_turn(root).open {
        let transcript = p.transcript_path.as_deref().map(PathBuf::from);
        // SessionEnd is a clean session exit, not a new prompt arriving while a
        // turn is running, so the Gemini "open turn = abort" heuristic must not
        // fire; `detect_interrupt_at_session_end` leaves it out while the real
        // host still gates the transcript and inflight checks.
        let source = state::detect_interrupt_at_session_end(
            root,
            &state::PromptContext {
                host,
                now: unix_secs_now(),
                agent_id: None,
                turn_id: nonempty(&p.prompt_id),
                transcript_path: transcript.as_deref(),
            },
        );
        let (kind, last_event) = match source {
            Some(_) => (Kind::Interrupt, "interrupt"),
            None => (Kind::Stop, "stop"),
        };
        let mut ev = with_session_turn(LifecycleEvent::new(kind, host), p);
        if let Some(src) = source {
            ev = ev.with_extra("inferred_from", src.as_str());
        }
        record(root, ev);
        state::close_turn(root, last_event);
    } else {
        state::close_turn(root, "stop");
    }
}

/// Reached only when the response does not block.
fn handle_stop(root: &Path, host: Host, p: &ClaudePayload) {
    record(
        root,
        with_session_turn(LifecycleEvent::new(Kind::Stop, host), p)
            .with_extra("stop_hook_active", p.stop_hook_active),
    );
    // Spec §Correlation state: if the write that would close the turn fails, the
    // classifier treats the turn as closed — the conservative answer — so the
    // failure is reported rather than silently leaving `open: true` on disk.
    if !state::close_turn(root, "stop") {
        eprintln!("phronesis: claude-hook Stop: could not close the turn; next prompt reads fresh");
    }
}

fn handle_subagent_start(root: &Path, host: Host, p: &ClaudePayload) {
    let agent_id = nonempty(&p.agent_id)
        .map(str::to_string)
        .unwrap_or_else(|| synth_agent_id(root));
    let agent_type = p.agent_type.clone();
    let ev = with_session_turn(
        LifecycleEvent::new(Kind::SubagentStart, host)
            .with_agent(agent_id.clone(), agent_type.clone()),
        p,
    );
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

/// Reached only when the response does not block, so a blocked `SubagentStop`
/// leaves the `agents` entry in place for the real stop to pop.
///
/// **It never touches `turn`.** Only the main-agent `Stop` closes a turn; a
/// sub-agent finishing does not end the human's turn.
fn handle_subagent_stop(root: &Path, host: Host, p: &ClaudePayload) {
    // `agents`, not the journal, is authoritative for pairing.
    let open = state::pop_agent(root, nonempty(&p.agent_id));
    let now = unix_secs_now();
    let mut ev = LifecycleEvent::new(Kind::SubagentStop, host)
        .with_extra("matched_start", open.is_some())
        .with_extra("stop_hook_active", p.stop_hook_active);
    if let Some(o) = &open {
        ev = ev.with_extra("duration_secs", now.saturating_sub(o.ts));
    }
    let agent_id = nonempty(&p.agent_id)
        .map(str::to_string)
        .or_else(|| open.as_ref().map(|o| o.agent_id.clone()));
    if let Some(id) = agent_id {
        let agent_type = p
            .agent_type
            .clone()
            .filter(|s| !s.is_empty())
            .or_else(|| open.as_ref().and_then(|o| o.agent_type.clone()));
        ev = ev.with_agent(id, agent_type);
    }
    record(root, with_session_turn(ev, p));
}
