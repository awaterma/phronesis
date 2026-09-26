//! Codex hook adapter.
//!
//! Reads a Codex hook JSON payload from stdin, dispatches to the existing
//! pre/post rule-evaluation pipeline, and writes a Codex-specific JSON
//! response to stdout. Context events (SessionStart, UserPromptSubmit,
//! PreCompact, PostCompact, SubagentStart) reuse the context builders.
//! Stop events enforce the configured confidence gate when a work unit is open.
//!
//! **Supported events**:
//! `PreToolUse`, `PostToolUse`, `SessionStart`, `UserPromptSubmit`,
//! `PreCompact`, `PostCompact`, `SubagentStart`, `SubagentStop`, and `Stop`.
//!
//! **Supported tools**: `Bash` (command text), `apply_patch` (patch parsing).
//! MCP calls and other tools are allowed without comment.

mod codex_patch;
mod renderer;

use std::path::Path;
use std::process;

use phr::Fact;
use serde::Deserialize;

use crate::action_log;
use crate::context;
use crate::journey;
use crate::lifecycle::event::{Host, Kind, LifecycleEvent, Mode};
use crate::lifecycle::inflight;
use crate::lifecycle::record::record;
use crate::lifecycle::scrub;
use crate::lifecycle::state;
use crate::outcomes;
use crate::security;

// ---------------------------------------------------------------------------
// Payload shapes
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
struct CodexPayload {
    #[serde(default)]
    hook_event_name: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    turn_id: Option<String>,
    #[serde(default)]
    tool_use_id: Option<String>,
    #[serde(default)]
    tool_name: Option<String>,
    #[serde(default)]
    tool_input: Option<serde_json::Value>,
    #[serde(default)]
    tool_response: Option<serde_json::Value>,
    /// Sub-agent identity on `SubagentStart` / `SubagentStop`.
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    agent_type: Option<String>,
    /// Main-session transcript; carried by `Interrupt` and most events.
    #[serde(default)]
    #[allow(dead_code)] // deserialized so the capture tee can redact it; never read
    transcript_path: Option<String>,
    /// `SubagentStop` only.
    #[serde(default)]
    #[allow(dead_code)] // deserialized so the capture tee can redact it; never read
    agent_transcript_path: Option<String>,
    /// `SubagentStop` only. Redacted before capture; never journaled.
    #[serde(default)]
    #[allow(dead_code)] // deserialized so the capture tee can redact it; never read
    last_assistant_message: Option<String>,
    /// True when the host re-runs a stop hook after a block. `Stop` and
    /// `SubagentStop`.
    #[serde(default)]
    stop_hook_active: Option<bool>,
    /// `UserPromptSubmit` only, verbatim. Scrubbed before it reaches the log.
    #[serde(default)]
    prompt: Option<String>,
    /// `SessionStart` only: `startup` | `resume` | `clear` | `compact` | `fork`.
    /// Task 7 widens the registration matcher to `""`, so `compact` and `fork`
    /// reach the handler for the first time and the gate is what keeps them
    /// from orphaning an open sub-agent.
    #[serde(default)]
    source: Option<String>,
}

struct PatchFile {
    path: String,
    /// Lines the patch adds (`+` hunk lines). Used as `new_content` when the
    /// file isn't readable on disk yet (Add File), and preferred otherwise so
    /// rules see the incoming content, not just the current file state.
    added: String,
    /// `*** Delete File:` — the patch removes this file.
    deleted: bool,
}

#[derive(Debug)]
struct CodexDecision {
    block_messages: Vec<String>,
    warn_messages: Vec<String>,
    additional_context: String,
    /// Paths touched by the tool call; journaled once post-hook wiring lands.
    #[expect(
        dead_code,
        reason = "populated by handlers; read once post-hook journaling lands"
    )]
    files: Vec<String>,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub async fn run(event: &str) -> ! {
    let root = security::project_root();
    let parsed = parse_payload(event);
    let fallback;
    let (event, result) = match parsed.as_ref() {
        Ok(payload) => {
            let event = payload.hook_event_name.as_deref().unwrap_or(event);
            // An ungoverned root has no rules and records nothing: every
            // lifecycle write would create a stray `<cwd>/.phronesis/journey/`
            // that later stops the project-root walk.
            if security::is_governed(&root) {
                (event, dispatch(payload, event, &root).await)
            } else {
                (event, empty_decision())
            }
        }
        Err(error) => {
            fallback = invalid_payload_decision(event, error);
            // A denied PreToolUse is a block like any other and must be
            // explained in the log — but only under a governed root, which
            // already has a `.phronesis/` to write into.
            if !fallback.block_messages.is_empty() && security::is_governed(&root) {
                let payload = CodexPayload::default();
                log_unverdicted_block(&ToolCall::from_payload(&payload, ""), &fallback);
            }
            (event, fallback)
        }
    };

    let response = renderer::render_codex_response(event, &result);
    println!("{}", response);
    // Codex consumes structured decisions from stdout. Returning exit 0 keeps
    // the JSON authoritative; exit 2 is reserved for handlers that cannot
    // produce a valid response and must block via stderr.
    process::exit(0);
}

fn parse_payload(event: &str) -> anyhow::Result<CodexPayload> {
    let raw = security::read_stdin_capped()?;
    // Tees to PHRONESIS_CAPTURE_DIR when set, after redacting free-text
    // fields. Best-effort: never changes the response or the exit code.
    crate::hook::capture_raw_payload(event, &raw);
    Ok(serde_json::from_str(&raw)?)
}

fn invalid_payload_decision(event: &str, error: &anyhow::Error) -> CodexDecision {
    let message = format!("invalid Codex hook payload: {error}");
    if matches!(event, "PreToolUse" | "pre-tool-use") {
        CodexDecision {
            block_messages: vec![message],
            warn_messages: Vec::new(),
            additional_context: String::new(),
            files: Vec::new(),
        }
    } else if matches!(event, "PostToolUse" | "post-tool-use") {
        CodexDecision {
            block_messages: Vec::new(),
            warn_messages: vec![message],
            additional_context: String::new(),
            files: Vec::new(),
        }
    } else {
        empty_decision()
    }
}

async fn dispatch(payload: &CodexPayload, event: &str, root: &Path) -> CodexDecision {
    match event {
        "PreToolUse" | "pre-tool-use" => handle_pre(payload, root).await,
        "PostToolUse" | "post-tool-use" => handle_post(payload, root).await,
        "SessionStart" | "session-start" => {
            // Source-gated. The shared `session` file is the single source of
            // session identity for every host (spec §Correlation state), and a
            // session-begin source (`startup` / `resume` / `clear`, or an absent
            // source) overwrites it and truncates agents/inflight, closing any
            // turn left open by a crashed or aborted previous session.
            //
            // `compact` and `fork` continue the current session, so its open
            // sub-agents and in-flight tools are real. Task 7 widens the
            // registration matcher from `"startup|resume|clear"` to `""`, which
            // is safe ONLY because of this gate: without it, a mid-session
            // compaction would orphan every open sub-agent and discard every
            // in-flight tool, on a path that did not fire at all before.
            if state::is_session_begin(payload.source.as_deref()) {
                if let Some(sid) = payload.session_id.as_deref().filter(|s| !s.is_empty()) {
                    state::set_session(root, sid);
                }
                state::reset_for_session_start(root);
            }
            make_ctx_decision(root, ContextKind::SessionStart).await
        }
        "UserPromptSubmit" | "user-prompt-submit" => {
            record_prompt(payload, root);
            make_ctx_decision(root, ContextKind::InteractionContext).await
        }
        "PreCompact" | "pre-compact" => make_compact_decision(root, true),
        "PostCompact" | "post-compact" => make_ctx_decision(root, ContextKind::PostCompact).await,
        "SubagentStart" | "subagent-start" => {
            // Record first so the Stamped seq is available for the synthesized
            // id fallback (spec §Correlation state). Codex always supplies
            // agent_id today; the fallback keeps `agents` poppable if it stops.
            let stamped = record(root, lifecycle_event(payload, Kind::SubagentStart));
            let agent_id = payload.agent_id.clone().unwrap_or_else(|| match &stamped {
                Some(s) => format!("{}:{}", s.sid, s.seq),
                None => format!("codex:{}", unix_secs_now()),
            });
            state::push_agent(
                root,
                state::OpenAgent {
                    agent_id,
                    // Sanitized on the way in, so `agents` can never hand a
                    // hostile type back to a later stop's backfill.
                    agent_type: payload
                        .agent_type
                        .as_deref()
                        .and_then(crate::lifecycle::event::sanitize_agent_type),
                    ts: stamped.as_ref().map_or_else(unix_secs_now, |s| s.ts),
                    seq: stamped.as_ref().map_or(0, |s| s.seq),
                },
            );
            make_ctx_decision(root, ContextKind::SubagentStart).await
        }
        // NOTE the shape of both arms: the decision is made FIRST, and the
        // record is written only when it does not block. A blocked stop means
        // the host continues the same turn, so it is not a stop — recording one
        // would close a turn that is still running and the next steer would
        // classify `fresh`, losing the intervention (spec §"Host adapters /
        // Codex CLI", and the same rule as on Claude).
        //
        // `stop_hook_active: true` skips the gate entirely, as the docs require,
        // so a blocking gate cannot loop; the re-fire is the invocation that
        // records.
        "SubagentStop" | "subagent-stop" => {
            let decision = completion_decision_for_stop(root, payload);
            if !blocks(&decision) {
                let opened = state::pop_agent(root, payload.agent_id.as_deref());
                let now = unix_secs_now();
                let mut event = lifecycle_event(payload, Kind::SubagentStop)
                    .with_extra("matched_start", opened.is_some())
                    .with_extra(
                        "stop_hook_active",
                        payload.stop_hook_active.unwrap_or(false),
                    );
                if let Some(open) = &opened {
                    event = event.with_extra("duration_secs", now.saturating_sub(open.ts));
                }
                // Backfill identity the stop payload omitted. Through
                // `with_agent`, never by field assignment: that is the one
                // place `agent_type` is sanitized, and a direct write would
                // put an unfiltered string into a `lifecycle:agent:*` tag.
                let agent_id = payload
                    .agent_id
                    .clone()
                    .filter(|s| !s.is_empty())
                    .or_else(|| opened.as_ref().map(|o| o.agent_id.clone()));
                if let Some(id) = agent_id {
                    let agent_type = payload
                        .agent_type
                        .clone()
                        .filter(|s| !s.is_empty())
                        .or_else(|| opened.as_ref().and_then(|o| o.agent_type.clone()));
                    event = event.with_agent(id, agent_type);
                }
                record(root, event);
            }
            // **`SubagentStop` never closes `turn`.** A sub-agent finishing does
            // not end the human's turn; only the main-agent `Stop` does. The two
            // arms used to share a body, which is exactly how this gets lost.
            decision
        }
        "Stop" | "stop" => {
            let decision = completion_decision_for_stop(root, payload);
            if !blocks(&decision) {
                // Close before recording so a concurrent prompt hook cannot read
                // the turn as still open.
                state::close_turn(root, "stop");
                record(
                    root,
                    lifecycle_event(payload, Kind::Stop).with_extra(
                        "stop_hook_active",
                        payload.stop_hook_active.unwrap_or(false),
                    ),
                );
            }
            decision
        }
        // Codex fires Interrupt on abort, before TurnAborted, with the
        // transcript flushed. Stop does not fire, so this is the only end of
        // an aborted turn. Its schema permits `systemMessage` only.
        "Interrupt" | "interrupt" => {
            record(
                root,
                lifecycle_event(payload, Kind::Interrupt).with_extra("inferred_from", "hook"),
            );
            // `{open: false, last_event: "interrupt"}`. The next prompt reads
            // `last_event` in classification step 1 and comes out `correction`
            // with no second interrupt record — the single
            // interrupt-already-recorded path, on every host.
            state::close_turn(root, "interrupt");
            // The aborted tools' PostToolUse never fires; a lingering entry
            // would fake an interrupt for 900 s.
            state::clear_inflight(root);
            empty_decision()
        }
        // A session ending with a turn still open ended without a Stop:
        // record the stop so the turn is closed in the journal too. `session` is
        // left in place — the next session-begin SessionStart overwrites it, and
        // truncating would let any stray hook between sessions mint a throwaway
        // sid (spec §"Host adapters / Codex CLI").
        "SessionEnd" | "session-end" => {
            // Only an open turn is closed here: an already-closed turn may
            // carry `last_event: "interrupt"`, which the next prompt's
            // classification depends on.
            if state::read_turn(root).open {
                record(root, lifecycle_event(payload, Kind::Stop));
                state::close_turn(root, "stop");
            }
            empty_decision()
        }
        _ => empty_decision(),
    }
}

fn empty_decision() -> CodexDecision {
    CodexDecision {
        block_messages: Vec::new(),
        warn_messages: Vec::new(),
        additional_context: String::new(),
        files: Vec::new(),
    }
}

/// A `LifecycleEvent` pre-filled from the identity fields any Codex payload
/// may carry. Callers add the kind-specific mode, prompt, and extras.
fn lifecycle_event(payload: &CodexPayload, kind: Kind) -> LifecycleEvent {
    let mut event = LifecycleEvent::new(kind, Host::Codex);
    if let Some(sid) = payload.session_id.as_deref().filter(|s| !s.is_empty()) {
        event = event.with_session(sid);
    }
    if let Some(tid) = payload.turn_id.as_deref().filter(|s| !s.is_empty()) {
        event = event.with_turn(tid);
    }
    if let Some(aid) = payload.agent_id.as_deref().filter(|s| !s.is_empty()) {
        event = event.with_agent(aid, payload.agent_type.clone());
    }
    event
}

fn unix_secs_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The completion decision for a stop event. `stop_hook_active: true` skips the
/// gate entirely and responds `{}`, as the host docs require: without it a
/// blocking gate re-fires forever.
fn completion_decision_for_stop(root: &Path, payload: &CodexPayload) -> CodexDecision {
    if payload.stop_hook_active.unwrap_or(false) {
        return empty_decision();
    }
    make_completion_decision(root)
}

/// Did the decision block? `render_completion` emits the block shape exactly
/// when `block_messages` is non-empty, so this is the same predicate the
/// response speaks — structural, not a substring test.
fn blocks(decision: &CodexDecision) -> bool {
    !decision.block_messages.is_empty()
}

/// Classify the prompt, record any inferred interrupt, record the prompt with
/// its scrubbed text, and open the turn. Spec §Classification.
fn record_prompt(payload: &CodexPayload, root: &Path) {
    let now = unix_secs_now();
    // Read before `classify_prompt`, which may rewrite the file.
    let turn = state::read_turn(root);
    let context = state::PromptContext {
        host: Host::Codex,
        now,
        agent_id: payload.agent_id.as_deref(),
        turn_id: payload.turn_id.as_deref(),
        // Codex needs no transcript scan: its Interrupt hook is ground truth,
        // and step 1 reads the result from `turn.last_event`.
        transcript_path: None,
    };
    let classification = state::classify_prompt(root, &context);

    // Codex fires UserPromptSubmit with the *running* turn's id for a message
    // queued mid-turn, which is spec step 3's second sufficient signal for
    // `mid_turn`. The `inflight` branch is off on Codex, so an open turn with no
    // interrupt evidence already classifies `mid_turn` and the two signals
    // agree; the assertion is here rather than an override, so a future
    // divergence is loud instead of silently papered over.
    debug_assert!(
        !(turn.open
            && payload.turn_id.is_some()
            && turn.turn_id.as_deref() == payload.turn_id.as_deref()
            && classification.mode != Mode::MidTurn
            && classification.interrupt.is_none()),
        "a message queued in the running turn classified as {:?}",
        classification.mode
    );

    // `Some(source)` means this handler must write the record. `None` with mode
    // `Correction` means the `Interrupt` arm already wrote it, and a second
    // record would double-count the friction. On Codex today only the second
    // case occurs.
    if let Some(source) = classification.interrupt {
        record(
            root,
            lifecycle_event(payload, Kind::Interrupt).with_extra("inferred_from", source.as_str()),
        );
    }

    let mut event = lifecycle_event(payload, Kind::Prompt).with_mode(classification.mode);
    if let Some(text) = payload.prompt.as_deref().filter(|t| !t.is_empty()) {
        event = event.with_prompt(scrub::scrub_prompt(root, text));
    }
    record(root, event);

    // A prompt carrying an `agent_id` never writes `turn` — a sub-agent's prompt
    // must not move the parent's turn state (spec §Correlation state). Codex
    // does not deliver prompts inside sub-agents today; the guard costs one line
    // and means this adapter does not have to be revisited if it starts.
    if payload
        .agent_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .is_none()
    {
        state::open_turn(root, payload.turn_id.as_deref(), now);
    }
}

fn make_completion_decision(root: &Path) -> CodexDecision {
    if !outcomes::enabled(root) {
        return empty_decision();
    }
    let Some(report) = outcomes::report(root, None) else {
        return empty_decision();
    };
    let message = match report.band {
        outcomes::Band::Low => format!(
            "Low confidence for {} — resolve failing or missing grounded signals before completing.",
            report.subject
        ),
        outcomes::Band::Medium => format!(
            "Medium confidence for {} — one grounded signal is still missing.",
            report.subject
        ),
        outcomes::Band::High => return empty_decision(),
    };
    let (block_messages, warn_messages) = match report.band {
        outcomes::Band::Low => (vec![message], Vec::new()),
        outcomes::Band::Medium => (Vec::new(), vec![message]),
        outcomes::Band::High => unreachable!("high confidence returned above"),
    };
    CodexDecision {
        block_messages,
        warn_messages,
        additional_context: String::new(),
        files: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Tool-call context and verdicts
// ---------------------------------------------------------------------------

/// One tool invocation as the pre/post handlers see it.
#[derive(Clone, Copy)]
struct ToolCall<'a> {
    payload: &'a CodexPayload,
    tool_name: &'a str,
    file_path: &'a str,
}

impl<'a> ToolCall<'a> {
    fn from_payload(payload: &'a CodexPayload, file_path: &'a str) -> Self {
        Self {
            payload,
            tool_name: payload.tool_name.as_deref().unwrap_or(""),
            file_path,
        }
    }

    fn supported(&self) -> bool {
        matches!(self.tool_name, "Bash" | "apply_patch")
    }

    fn with_file(self, file_path: &'a str) -> Self {
        Self { file_path, ..self }
    }
}

fn block_decision(message: String) -> CodexDecision {
    CodexDecision {
        block_messages: vec![message],
        warn_messages: Vec::new(),
        additional_context: String::new(),
        files: Vec::new(),
    }
}

fn warn_decision(message: String) -> CodexDecision {
    CodexDecision {
        block_messages: Vec::new(),
        warn_messages: vec![message],
        additional_context: String::new(),
        files: Vec::new(),
    }
}

/// Rule outcome of firing one network.
#[derive(Debug)]
struct Verdict {
    logged: Vec<crate::hook_logged::LoggedConsequence>,
    block_msgs: Vec<String>,
    warn_msgs: Vec<String>,
}

impl Verdict {
    /// Rules reading a drifted graph reason from evidence we cannot vouch
    /// for, so the harness declines to act on their verdicts: they warn
    /// rather than block, exactly as the Claude pre-hook does.
    fn demote_stale(&mut self, stale_graph_rules: &std::collections::BTreeSet<phr::RuleId>) {
        if stale_graph_rules.is_empty() {
            return;
        }
        crate::hook_logged::demote_violations_from(&mut self.logged, stale_graph_rules);
        let (v, w) = crate::hook_logged::split_messages_by_action_type(&self.logged);
        self.block_msgs = v;
        self.warn_msgs = w;
    }

    fn extend(&mut self, other: Verdict) {
        self.logged.extend(other.logged);
        self.block_msgs.extend(other.block_msgs);
        self.warn_msgs.extend(other.warn_msgs);
    }

    /// Report a pre-hook verdict on stderr, log it, and turn it into a decision.
    fn finish_pre(self, call: &ToolCall<'_>, files: Vec<String>) -> CodexDecision {
        for v in &self.block_msgs {
            eprintln!("phronesis: BLOCKED — {}", v);
        }
        for w in &self.warn_msgs {
            eprintln!("phronesis: WARNING — {}", w);
        }
        let exit = if !self.block_msgs.is_empty() {
            2
        } else if !self.warn_msgs.is_empty() {
            1
        } else {
            0
        };
        // Block messages no rule consequence accounts for came from the hook
        // itself (a pattern check that errored), so they are logged as
        // fail-closed reasons alongside the rule verdicts.
        let mut blocked_by = hook::BlockReason::from_violations(&self.logged);
        blocked_by.extend(
            self.block_msgs
                .iter()
                .filter(|m| !self.logged.iter().any(|c| &c.message == *m))
                .map(|m| hook::BlockReason::fail_closed(None, m.clone())),
        );
        log_event("pre", call, exit, &self.logged, &blocked_by);
        CodexDecision {
            block_messages: self.block_msgs,
            warn_messages: self.warn_msgs,
            additional_context: String::new(),
            files,
        }
    }

    /// Report a post-hook verdict (everything is advisory after the fact),
    /// log it, journal the edit, and turn it into a decision.
    async fn finish_post(self, call: &ToolCall<'_>) -> CodexDecision {
        let flagged = !self.block_msgs.is_empty() || !self.warn_msgs.is_empty();
        for v in self.block_msgs.iter().chain(&self.warn_msgs) {
            eprintln!("phronesis: WARNING — {}", v);
        }
        log_event("post", call, i32::from(flagged), &self.logged, &[]);
        journal_supported_post(call.payload, call.file_path).await;
        if !flagged {
            return empty_decision();
        }
        CodexDecision {
            block_messages: self.block_msgs,
            warn_messages: self.warn_msgs,
            additional_context: String::new(),
            files: Vec::new(),
        }
    }
}

/// Fire the network and collect its logged consequences.
///
/// RETE consequences reach the agenda only after an explicit update, as
/// the Claude hook does before firing. Without it any verdict derived
/// purely from fact matching — every structural graph rule — is computed
/// and then dropped.
async fn fire_verdict(network: &phr::ReteNetwork, root: &Path) -> Result<Verdict, String> {
    network
        .update_agenda()
        .await
        .map_err(|e| format!("agenda update failed: {}", e))?;
    let consequences = network
        .fire_all_consequences()
        .map_err(|e| format!("rule execution failed: {}", e))?;
    crate::capsule::capture_for_hook(root, &consequences);
    crate::hook::report_unbound_at_fire(&consequences);
    let (logged, block_msgs, warn_msgs) =
        hook::collect_logged(&consequences, &security::project_root());
    Ok(Verdict {
        logged,
        block_msgs,
        warn_msgs,
    })
}

// ---------------------------------------------------------------------------
// PreToolUse
// ---------------------------------------------------------------------------

async fn handle_pre(payload: &CodexPayload, root: &Path) -> CodexDecision {
    // `init` routes Codex's tool phases here rather than to `pre-check` /
    // `post-check`, so the `inflight` correlation entry and commit detection
    // are this adapter's own (spec §"Host adapters / Codex CLI"). The push
    // happens before the supported-tool allowlist, so a tool Phronesis does not
    // govern still makes the next prompt a `correction`.
    let key = pre_push_inflight(root, payload);
    let decision = match evaluate_pre(payload, root).await {
        Ok(decision) => decision,
        Err(blocked) => {
            let file_path = extract_file_path(payload.tool_input.as_ref());
            log_unverdicted_block(&ToolCall::from_payload(payload, &file_path), &blocked);
            blocked
        }
    };
    if !decision.block_messages.is_empty() {
        // The tool never ran: a block is not an interrupt, so the entry must
        // not survive to fake one for the next 900 s.
        inflight::drop_entry(root, &key);
    }
    decision
}

/// Read this payload as a shared [`inflight::Call`].
fn inflight_call<'a>(
    payload: &'a CodexPayload,
    tool: &'a str,
    command: &'a str,
) -> inflight::Call<'a> {
    inflight::Call {
        tool,
        tool_use_id: payload.tool_use_id.as_deref(),
        tool_input: payload
            .tool_input
            .as_ref()
            .unwrap_or(&serde_json::Value::Null),
        command,
        agent_id: payload.agent_id.as_deref(),
        // Codex reports no commit of its own; HEAD is the only witness here.
        host_sha: None,
    }
}

/// Push the in-flight entry and return its key.
fn pre_push_inflight(root: &Path, payload: &CodexPayload) -> String {
    let tool = payload.tool_name.clone().unwrap_or_default();
    let command = extract_bash_command(payload);
    inflight::push(root, &inflight_call(payload, &tool, &command))
}

/// Pop the entry this call pushed and, for a shell call that may have moved
/// HEAD, record a `commit`.
fn post_pop_and_detect(root: &Path, payload: &CodexPayload) {
    let tool = payload.tool_name.clone().unwrap_or_default();
    let command = extract_bash_command(payload);
    inflight::pop_and_detect(
        root,
        &inflight_call(payload, &tool, &command),
        extract_command_exit(payload),
        |kind| lifecycle_event(payload, kind),
    );
}

/// Evaluate a PreToolUse call. `Ok` is a decision already logged (or one that
/// needs no entry); `Err` is a fail-closed block the hook imposed without a
/// rule verdict, which the caller logs.
async fn evaluate_pre(payload: &CodexPayload, root: &Path) -> Result<CodexDecision, CodexDecision> {
    let file_path = extract_file_path(payload.tool_input.as_ref());
    let call = ToolCall::from_payload(payload, &file_path);

    if !call.supported() {
        return Ok(empty_decision());
    }

    if call.tool_name == "apply_patch" {
        return handle_pre_patch(payload).await;
    }

    // --- Bash ---
    let loaded = match load_rules("pre") {
        Ok(Some(r)) => r,
        Ok(None) => return Ok(empty_decision()),
        Err(e) => return Err(block_decision(format!("rules error: {}", e))),
    };

    let (network, stale_graph_rules) =
        match build_pre_network(&loaded.rules, &loaded.override_facts, &file_path, root).await {
            Ok(built) => built,
            Err(decision) => return Err(decision),
        };
    let command = extract_bash_command(payload);

    // Assert content fact
    if let Err(error) = assert_new_content(&network, "new_content", &command).await {
        return Err(block_decision(format!(
            "failed to assert content fact: {error}"
        )));
    }

    // Content pattern checks
    let violations = check_bash_patterns(&network, &loaded.rules, &file_path, &command).await;

    // Cargo workspace scanner (sync-safe)
    crate::hook::assert_cargo_workspace_facts(&network, &command).await;
    if let Err(error) = crate::predicate_provider::assert_facts(
        &network,
        root,
        &codex_provider_event(&call, "pre", &command),
    )
    .await
    {
        return Err(block_decision(error.to_string()));
    }

    let mut verdict = match fire_verdict(&network, root).await {
        Ok(verdict) => verdict,
        Err(message) => return Err(block_decision(message)),
    };
    verdict.demote_stale(&stale_graph_rules);
    verdict.block_msgs.extend(violations);
    Ok(verdict.finish_pre(&call, Vec::new()))
}

/// Content-pattern and bash-command-pattern violations for a command.
///
/// Both checks only assert facts for rules to match against later; an `Err`
/// here means fact assertion itself failed, which `hook/pre.rs` treats as a
/// block for either pattern family ("pattern check failed" /
/// "command pattern check failed") rather than a mere warning — matched here.
async fn check_bash_patterns(
    network: &phr::ReteNetwork,
    rules: &[phr::Rule],
    file_path: &str,
    command: &str,
) -> Vec<String> {
    let cp = crate::hook_facts::collect_content_patterns(rules);
    let bcp = crate::hook_facts::collect_bash_command_patterns(rules);
    let mut violations: Vec<String> =
        crate::hook_facts::check_content_patterns(network, file_path, command, &cp)
            .await
            .err()
            .map(|e| format!("pattern check failed: {}", e))
            .into_iter()
            .collect();
    if let Err(e) = crate::hook_facts::check_bash_command_patterns(network, command, &bcp).await {
        violations.push(format!("command pattern check failed: {}", e));
    }
    violations
}

/// The patch text of an `apply_patch` call.
///
/// The current Codex contract supplies both Bash and apply_patch input in
/// `tool_input.command`.
fn extract_patch_text(payload: &CodexPayload) -> &str {
    payload
        .tool_input
        .as_ref()
        .and_then(|t| t.get("command"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("")
}

/// Block on traversal. NotFound is fine — Add File targets don't exist yet;
/// only genuine escape attempts (.. segments, outside root) block.
fn reject_unsafe_patch_paths(files: &[PatchFile], root: &Path) -> Option<CodexDecision> {
    for pf in files {
        if Path::new(&pf.path).is_absolute()
            || pf
                .path
                .split(['/', std::path::MAIN_SEPARATOR])
                .any(|part| part == "..")
        {
            return Some(block_decision(format!(
                "unsafe patch path blocked: {}",
                pf.path
            )));
        }
        if let Err(
            security::SecurityError::PathTraversal(_) | security::SecurityError::PathOutsideRoot(_),
        ) = security::resolve_safe_path(&pf.path, root)
        {
            return Some(block_decision(format!(
                "path traversal blocked: {}",
                pf.path
            )));
        }
    }
    None
}

async fn handle_pre_patch(payload: &CodexPayload) -> Result<CodexDecision, CodexDecision> {
    let root = security::project_root();
    let files = codex_patch::parse_patch(extract_patch_text(payload));
    if files.is_empty() {
        return Err(block_decision(
            "malformed apply_patch input: no file blocks".to_string(),
        ));
    }
    if let Some(decision) = reject_unsafe_patch_paths(&files, &root) {
        return Err(decision);
    }
    let paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();

    let loaded = match load_rules("pre") {
        Ok(Some(r)) => r,
        Ok(None) => {
            return Ok(CodexDecision {
                block_messages: Vec::new(),
                warn_messages: Vec::new(),
                additional_context: String::new(),
                files: paths,
            });
        }
        // A patch that only touches the file whose load error blocks every
        // hook is the repair; blocking it would leave no way to fix the file.
        // Deleting the rules file is not a repair: it turns governance off.
        Err(e)
            if files.iter().all(|f| {
                !f.deleted && crate::rule_layers::is_repair_target(&root, &e.failing_file, &f.path)
            }) =>
        {
            return Err(warn_decision(format!(
                "allowing this patch because it only edits the rules file that failed to load; every other tool call stays blocked until it loads. rules error: {e}"
            )));
        }
        Err(e) => return Err(block_decision(format!("rules error: {}", e))),
    };
    let call = ToolCall {
        payload,
        tool_name: "apply_patch",
        file_path: "",
    };

    // Providers get one batch-level view before the per-file views.
    let mut verdict = evaluate_patch_batch(&call, &loaded, &paths, &root).await?;

    // Evaluate each file with its own network, mirroring the single-file
    // contract of the Claude hook: file_path facts (and their per-segment
    // file_path_matches derivatives) are scoped to one file at a time.
    for pf in &files {
        match evaluate_patch_file(&call, &loaded, pf, &root).await {
            Ok(Some(file_verdict)) => verdict.extend(file_verdict),
            Ok(None) => {}
            Err(decision) => return Err(decision),
        }
    }

    Ok(verdict.finish_pre(&call, paths))
}

/// Batch-level provider view of an `apply_patch` call.
///
/// Keeping `file_path` empty and `files` populated makes the batch and
/// per-file contexts unambiguous and prevents existing per-file providers
/// from double-firing.
async fn evaluate_patch_batch(
    call: &ToolCall<'_>,
    loaded: &LoadedRules,
    paths: &[String],
    root: &Path,
) -> Result<Verdict, CodexDecision> {
    let (batch_network, _) =
        build_pre_network(&loaded.rules, &loaded.override_facts, "", root).await?;
    let mut batch_event = codex_provider_event(call, "pre", "");
    batch_event.files = paths.to_vec();
    crate::predicate_provider::assert_facts(&batch_network, root, &batch_event)
        .await
        .map_err(|error| block_decision(error.to_string()))?;
    fire_verdict(&batch_network, root)
        .await
        .map_err(block_decision)
}

/// Evaluate one patched file. `Ok(None)` means the file was skipped because
/// it has no added lines and cannot be read safely from disk.
async fn evaluate_patch_file(
    call: &ToolCall<'_>,
    loaded: &LoadedRules,
    pf: &PatchFile,
    root: &Path,
) -> Result<Option<Verdict>, CodexDecision> {
    // Prefer the patch's own added lines (the file may not exist on disk
    // yet for Add File), falling back to the current on-disk content. A
    // missing file is fine — Add File targets legitimately don't exist yet —
    // but any other read error (permissions, a dangling symlink, I/O) must
    // fail closed rather than silently evaluating content rules against
    // empty text, which would let real content slip past every rule.
    let content = if pf.added.is_empty() {
        match security::resolve_safe_path(&pf.path, root) {
            Ok(safe) => match tokio::fs::read_to_string(&safe).await {
                Ok(text) => text,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(e) => {
                    return Err(block_decision(format!("failed to read {}: {}", pf.path, e)));
                }
            },
            Err(_) => return Ok(None),
        }
    } else {
        pf.added.clone()
    };

    let (network, stale_graph_rules) =
        build_pre_network(&loaded.rules, &loaded.override_facts, &pf.path, root).await?;
    if !content.is_empty() {
        assert_patch_content(&network, &loaded.rules, &pf.path, &content).await?;
    }
    crate::predicate_provider::assert_facts(
        &network,
        root,
        &codex_provider_event(&call.with_file(&pf.path), "pre", &content),
    )
    .await
    .map_err(|error| block_decision(error.to_string()))?;

    let mut verdict = fire_verdict(&network, root).await.map_err(block_decision)?;
    verdict.demote_stale(&stale_graph_rules);
    Ok(Some(verdict))
}

/// Assert a patched file's content facts: the raw content, the derived
/// `new_content_contains` pattern facts rules actually match on (mirroring
/// the Claude pre-hook's pattern scan), and language-pack facts.
async fn assert_patch_content(
    network: &phr::ReteNetwork,
    rules: &[phr::Rule],
    path: &str,
    content: &str,
) -> Result<(), CodexDecision> {
    let fact_id = crate::fact_id::fact_id("new_content", &[path]);
    assert_new_content(network, &fact_id, content)
        .await
        .map_err(|error| block_decision(format!("failed to assert content fact: {error}")))?;
    let patterns = crate::hook_facts::collect_content_patterns(rules);
    crate::hook_facts::check_content_patterns(network, path, content, &patterns)
        .await
        .map_err(|e| block_decision(format!("pattern check failed: {}", e)))?;
    crate::hook_facts::assert_language_pack_facts(network, path, content)
        .await
        .map_err(|error| block_decision(format!("language-pack fact assertion failed: {error}")))
}

// ---------------------------------------------------------------------------
// PostToolUse
// ---------------------------------------------------------------------------

async fn handle_post(payload: &CodexPayload, root: &Path) -> CodexDecision {
    // Before the allowlist, mirroring the push in `handle_pre`: every entry
    // that phase created is popped here, governed tool or not.
    post_pop_and_detect(root, payload);

    let file_path = extract_file_path(payload.tool_input.as_ref());
    let call = ToolCall::from_payload(payload, &file_path);

    if !call.supported() {
        return empty_decision();
    }

    record_post_edits(&call, root);

    let loaded = match load_rules("post") {
        Ok(Some(r)) => r,
        Ok(None) => {
            log_event("post", &call, 0, &[], &[]);
            journal_supported_post(payload, &file_path).await;
            return empty_decision();
        }
        Err(e) => {
            journal_supported_post(payload, &file_path).await;
            return warn_decision(format!("rules error: {}", e));
        }
    };

    let network =
        match build_post_network(&loaded.rules, &loaded.override_facts, &file_path, root).await {
            Ok(network) => network,
            Err(message) => {
                // Advisory, as `post-check` warns: the tool already ran.
                eprintln!("phronesis: WARNING — {message}");
                journal_supported_post(payload, &file_path).await;
                return warn_decision(message);
            }
        };

    let command = if call.tool_name == "Bash" {
        extract_bash_command(payload)
    } else {
        String::new()
    };
    if let Err(error) = check_post_bash_command(&call, &network, &loaded.rules, &command).await {
        let message = error.to_string();
        eprintln!("phronesis: WARNING — {message}");
        journal_supported_post(payload, &file_path).await;
        return warn_decision(message);
    }
    if let Err(error) = crate::predicate_provider::assert_facts(
        &network,
        root,
        &post_provider_event(&call, &command),
    )
    .await
    {
        journal_supported_post(payload, &file_path).await;
        return warn_decision(error.to_string());
    }

    match fire_verdict(&network, root).await {
        Ok(verdict) => verdict.finish_post(&call).await,
        Err(message) => {
            journal_supported_post(payload, &file_path).await;
            warn_decision(message)
        }
    }
}

/// Structural sensor, matching the Claude post-hook. Without it a Codex
/// project's graph goes stale on every edit and its structural rules stay
/// demoted to warnings until someone runs `graph rebuild` by hand.
/// `apply_patch` writes several files at once, so each is recorded.
fn record_post_edits(call: &ToolCall<'_>, root: &Path) {
    if call.tool_name == "apply_patch" {
        for file in codex_patch::parse_patch(&extract_bash_command(call.payload)) {
            crate::graph::sync::record_from_disk(root, &file.path);
        }
    } else if !call.file_path.is_empty() {
        crate::graph::sync::record_from_disk(root, call.file_path);
    }
}

/// Command pattern check on an already-executed Bash command.
///
/// The action already ran, so `hook/post.rs` treats a fact-assertion failure
/// here as advisory (warn, exit 1), not silence — matched here.
async fn check_post_bash_command(
    call: &ToolCall<'_>,
    network: &phr::ReteNetwork,
    rules: &[phr::Rule],
    command: &str,
) -> anyhow::Result<()> {
    if call.tool_name != "Bash" {
        return Ok(());
    }
    let bcp = crate::hook_facts::collect_bash_command_patterns(rules);
    crate::hook_facts::check_bash_command_patterns(network, command, &bcp)
        .await
        .map_err(|e| anyhow::anyhow!("command pattern check failed: {}", e))
}

/// Post-phase provider event; `apply_patch` calls list every patched file.
fn post_provider_event(
    call: &ToolCall<'_>,
    command: &str,
) -> crate::predicate_provider::ProviderEvent {
    let mut provider_event = codex_provider_event(call, "post", command);
    if call.tool_name == "apply_patch" {
        provider_event.files = codex_patch::parse_patch(&extract_bash_command(call.payload))
            .into_iter()
            .map(|file| file.path)
            .collect();
    }
    provider_event
}

// ---------------------------------------------------------------------------
// Context helpers
// ---------------------------------------------------------------------------

/// Which Codex hook is asking for context.
///
/// `SessionStart`, `PostCompact`, and `SubagentStart` all render the charter,
/// but they are recorded under distinct observation labels: only the first two
/// are part of the durability contract, and lumping subagent starts in with
/// them would inflate the session-restoration numbers.
#[derive(Clone, Copy)]
enum ContextKind {
    SessionStart,
    PostCompact,
    SubagentStart,
    InteractionContext,
}

impl ContextKind {
    fn event(self) -> context::ContextEvent {
        match self {
            Self::InteractionContext => context::ContextEvent::Interaction,
            Self::PostCompact => context::ContextEvent::PostCompact,
            Self::SessionStart | Self::SubagentStart => context::ContextEvent::Session,
        }
    }

    fn metric_event(self) -> &'static str {
        match self {
            Self::SubagentStart => "subagent_context",
            other => other.event().metric_event(),
        }
    }
}

async fn make_ctx_decision(root: &Path, kind: ContextKind) -> CodexDecision {
    CodexDecision {
        block_messages: Vec::new(),
        warn_messages: Vec::new(),
        additional_context: build_context_body(root, kind).await,
        files: Vec::new(),
    }
}

fn make_compact_decision(root: &Path, _pre: bool) -> CodexDecision {
    let durable = context::read_durable_directives(root);
    CodexDecision {
        block_messages: Vec::new(),
        warn_messages: Vec::new(),
        additional_context: durable,
        files: Vec::new(),
    }
}

async fn build_context_body(root: &Path, kind: ContextKind) -> String {
    context::run_body_with_load_notice(root, kind.event(), 5, kind.metric_event()).await
}

// ---------------------------------------------------------------------------
// Network builders (async)
// ---------------------------------------------------------------------------

/// Build the pre-check network, returning it alongside the rules whose
/// verdicts must be demoted because the code graph they read has drifted.
///
/// Hydration is not optional decoration: both shipped structural rules open
/// on `edited_file`, and that fact exists only inside `hydrate`. Without this
/// the Codex host maintained a graph on every save that nothing ever read,
/// and the pack produced zero warnings while appearing installed.
async fn build_pre_network(
    rules: &[phr::Rule],
    override_facts: &[phr::Fact],
    file_path: &str,
    root: &Path,
) -> Result<(phr::ReteNetwork, std::collections::BTreeSet<phr::RuleId>), CodexDecision> {
    let net = crate::hook::build_rule_network(crate::hook::RuleNetworkInput {
        rules,
        override_facts,
        file_path,
        tool_name: "Bash",
        phase: "pre",
        project_root: root,
    })
    .await
    .map_err(|e| {
        // Fail closed, as `pre-check` does: a rule set that cannot be loaded
        // or evaluated (an undefined journey selector, say) must not let the
        // tool call through.
        let reason = e.to_string();
        eprintln!("phronesis: BLOCKED — {reason}");
        block_decision(reason)
    })?;
    crate::hook::assert_pack_marker_facts(&net, root).await;
    crate::hook::assert_confidence_signals(&net).await;
    let stale_graph_rules = assert_structural_facts(&net, root, rules, file_path).await;
    Ok((net, stale_graph_rules))
}

/// Hydrate the structural graph into `net`, returning the rules whose
/// verdicts must be demoted to warnings because the graph (or the symbols a
/// rule names) cannot be vouched for.
async fn assert_structural_facts(
    net: &phr::ReteNetwork,
    root: &Path,
    rules: &[phr::Rule],
    file_path: &str,
) -> std::collections::BTreeSet<phr::RuleId> {
    let mut stale_graph_rules = std::collections::BTreeSet::new();
    let edited = (!file_path.is_empty()).then_some(file_path);
    let hydration = crate::graph::hydrate::hydrate(root, rules, edited);
    if !hydration.fresh && !hydration.facts.is_empty() {
        let cause = if hydration.outdated {
            "was built by an older phronesis and names entities differently".to_string()
        } else {
            format!(
                "is stale ({} file(s) changed outside the hook)",
                hydration.drifted.len()
            )
        };
        eprintln!(
            "phronesis: NOTE — structural graph {cause}; structural rules will warn, not block. Run `phr-mcp graph rebuild`."
        );
        stale_graph_rules = hydration.graph_rules;
    }
    for (rule_id, symbols) in crate::graph::bindings::stale_rules(root, hydration.verified_fresh) {
        eprintln!(
            "phronesis: NOTE — rule `{rule_id}` names `{}`, which the code graph no longer defines; this rule will warn, not block. Review or retire it.",
            symbols.join("`, `")
        );
        stale_graph_rules.insert(rule_id);
    }
    for fact in hydration.facts {
        if net.assert_fact(fact).await.is_err() {
            break;
        }
    }
    stale_graph_rules
}

async fn build_post_network(
    rules: &[phr::Rule],
    override_facts: &[phr::Fact],
    file_path: &str,
    root: &Path,
) -> Result<phr::ReteNetwork, String> {
    let net = crate::hook::build_rule_network(crate::hook::RuleNetworkInput {
        rules,
        override_facts,
        file_path,
        tool_name: "Bash",
        phase: "post",
        project_root: root,
    })
    .await
    .map_err(|e| e.to_string())?;
    crate::hook::assert_pack_marker_facts(&net, root).await;
    Ok(net)
}

// ---------------------------------------------------------------------------
// Rules loading
// ---------------------------------------------------------------------------

struct LoadedRules {
    rules: Vec<phr::Rule>,
    override_facts: Vec<phr::Fact>,
}

/// The rules on disk did not load; `failing_file` is the file to edit.
struct RulesLoadError {
    message: String,
    failing_file: std::path::PathBuf,
}

impl std::fmt::Display for RulesLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

fn load_rules(phase: &str) -> Result<Option<LoadedRules>, RulesLoadError> {
    let root = security::project_root();
    let project_path = crate::rules_file::default_path(&root);
    let loader_path = crate::rule_layers::config_path(&root);
    if !project_path.exists() && !loader_path.exists() {
        return Ok(None);
    }
    let resolved = crate::rule_layers::resolve(&root).map_err(|e| RulesLoadError {
        message: e.to_string(),
        failing_file: e.failing_file(&root),
    })?;
    let override_facts = crate::rule_layers::override_facts(&resolved.overrides);
    let rules: Vec<phr::Rule> = resolved
        .rules
        .into_iter()
        .filter(|r| r.phase == phase)
        .map(|r| crate::rules_file::rule_from_disk(&r).0)
        .collect();
    if rules.is_empty() {
        Ok(None)
    } else {
        Ok(Some(LoadedRules {
            rules,
            override_facts,
        }))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn extract_file_path(tool_input: Option<&serde_json::Value>) -> String {
    tool_input
        .and_then(|t| t.get("file_path"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

fn extract_bash_command(payload: &CodexPayload) -> String {
    payload
        .tool_input
        .as_ref()
        .and_then(|t| t.get("command"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string()
}

fn codex_provider_event(
    call: &ToolCall<'_>,
    phase: &str,
    content: &str,
) -> crate::predicate_provider::ProviderEvent {
    crate::predicate_provider::ProviderEvent {
        phase: phase.to_string(),
        tool_name: call.tool_name.to_string(),
        file_path: call.file_path.to_string(),
        file_rel: crate::graph::hydrate::repo_relative(
            &crate::security::project_root(),
            call.file_path,
        )
        .unwrap_or_default(),
        files: Vec::new(),
        old_content: String::new(),
        new_content: content.to_string(),
        command: if call.tool_name == "Bash" {
            content.to_string()
        } else {
            String::new()
        },
        output: extract_tool_output_text(call.payload),
    }
}

async fn assert_new_content(
    network: &phr::ReteNetwork,
    fact_id: &str,
    content: &str,
) -> Result<(), phr::ReteError> {
    network
        .assert_fact(Fact {
            id: fact_id.to_string(),
            predicate: "new_content".to_string(),
            args: vec![content.to_string()],
            timestamp: 0,
            source: Some("hook".to_string()),
        })
        .await
}

/// Log a pre-phase block the adapter imposed without a rule verdict (a load
/// error, a fact-assertion failure, an unsafe patch path). Every deny is
/// logged with its reason, so the audit trail can always explain it.
fn log_unverdicted_block(call: &ToolCall<'_>, decision: &CodexDecision) {
    let reasons: Vec<hook::BlockReason> = decision
        .block_messages
        .iter()
        .map(|m| hook::BlockReason::fail_closed(None, m.clone()))
        .collect();
    log_event("pre", call, 2, &[], &reasons);
}

fn log_event(
    phase: &str,
    call: &ToolCall<'_>,
    exit: i32,
    logged: &[crate::hook_logged::LoggedConsequence],
    blocked_by: &[hook::BlockReason],
) {
    let ToolCall {
        payload,
        tool_name,
        file_path,
    } = *call;
    let root = security::project_root();
    let path = action_log::default_path(&root);
    let mut entry = action_log::LogEntry::new("hook", "codex_hook")
        .with("phase", phase.to_string())
        .with("tool", tool_name.to_string())
        .with("exit", exit)
        .with("host", "codex".to_string());
    // The open work unit, exactly as `hook/mod.rs::log_hook_event` stamps it on
    // `pre_check` / `post_check`. Without it a Codex session's rule evaluations
    // join to no work item and `unit show` reports zero for a governed unit.
    if let Some(subject) = crate::outcomes::subject::current(&root) {
        entry = entry.with("subject", subject);
    }
    let affected_files = if tool_name == "apply_patch" {
        codex_patch::parse_patch(&extract_bash_command(payload))
            .into_iter()
            .map(|file| file.path)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    if affected_files.len() > 1 {
        entry = entry.with(
            "files",
            serde_json::Value::Array(
                affected_files
                    .into_iter()
                    .map(serde_json::Value::String)
                    .collect(),
            ),
        );
    } else {
        entry = entry.with(
            "file",
            affected_files
                .first()
                .cloned()
                .unwrap_or_else(|| file_path.to_string()),
        );
    }
    if let Some(ref sid) = payload.session_id {
        entry = entry.with("session_id", sid.clone());
    }
    if let Some(ref tid) = payload.turn_id {
        entry = entry.with("turn_id", tid.clone());
    }
    if let Some(ref tuid) = payload.tool_use_id {
        entry = entry.with("tool_use_id", tuid.clone());
    }
    if !logged.is_empty() {
        let val = serde_json::to_value(logged).unwrap_or(serde_json::Value::Null);
        entry = entry.with("consequences", val);
    }
    if !blocked_by.is_empty() {
        let val = serde_json::to_value(blocked_by).unwrap_or(serde_json::Value::Null);
        entry = entry.with("blocked_by", val);
    }
    let _ = action_log::append(&path, &entry);
}

async fn journal_supported_post(payload: &CodexPayload, file_path: &str) {
    if payload.tool_name.as_deref() == Some("apply_patch") {
        let patch = extract_bash_command(payload);
        let files = codex_patch::parse_patch(&patch);
        for file in files {
            journal_post(payload, &file.path).await;
        }
    } else {
        journal_post(payload, file_path).await;
    }
}

async fn journal_post(payload: &CodexPayload, file_path: &str) {
    if std::env::var("PHRONESIS_NO_JOURNEY").is_ok() {
        return;
    }
    let root = security::project_root();
    let tool = payload.tool_name.as_deref().unwrap_or("");
    let cfg = journey::load_config(&root).unwrap_or_default();
    let tag_result = match journey::tagger::fire(&cfg, &file_path_facts(file_path)).await {
        Ok(result) => result,
        Err(error) => {
            eprintln!("phronesis: journey tagger warning: {error}");
            Default::default()
        }
    };
    let (outcome_tags, subject, command_exit) = extract_post_outcomes(payload, &root, tool);
    let record = journey::journal::JournalRecord {
        // Tool records and lifecycle records share one schema version.
        v: journey::journal::JOURNAL_V,
        ts: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        // Session identity comes from the shared `session` file, as on every
        // other host; `payload.session_id` can disagree after a fork or resume
        // (spec §Premise, "two session-id sources can disagree").
        sid: journey::current_sid(&root),
        seq: crate::hook::seq::next_seq(&root),
        tool: tool.to_string(),
        path: file_path.to_string(),
        ext: std::path::Path::new(file_path)
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string()),
        module: journey::tagger::resolve_module(&cfg, file_path),
        tags: tag_result.tags.into_iter().chain(outcome_tags).collect(),
        subject,
        command_exit,
        kind: None,
        mode: None,
        host: None,
        turn: None,
        agent: None,
        agent_type: None,
        kalpa: None,
    };
    let _ = journey::journal::append(&root, &record);
}

/// The `file_path` fact plus one `file_path_matches` fact per path segment.
fn file_path_facts(file_path: &str) -> Vec<Fact> {
    let mut facts: Vec<Fact> = Vec::new();
    facts.push(Fact {
        id: "file_path".to_string(),
        predicate: "file_path".to_string(),
        args: vec![file_path.to_string()],
        timestamp: 0,
        source: Some("hook".to_string()),
    });
    for part in file_path.split('/') {
        if !part.is_empty() {
            facts.push(Fact {
                id: crate::fact_id::fact_id("file_path_matches", &[part]),
                predicate: "file_path_matches".to_string(),
                args: vec![part.to_string()],
                timestamp: 0,
                source: Some("hook".to_string()),
            });
        }
    }
    facts
}

/// Outcome tags, subject, and command exit derived from the tool response.
fn extract_post_outcomes(
    payload: &CodexPayload,
    root: &Path,
    tool: &str,
) -> (Vec<String>, Option<String>, Option<i32>) {
    let command = extract_bash_command(payload);
    let output = extract_tool_output_text(payload);
    let command_exit = extract_command_exit(payload);
    let (outcome_tags, subject) =
        outcomes::adapter::extract_from(outcomes::adapter::ExtractFromInput {
            project_root: root,
            tool_name: tool,
            command: Some(&command),
            output: &output,
            command_exit,
        });
    (outcome_tags, subject, command_exit)
}

fn extract_command_exit(payload: &CodexPayload) -> Option<i32> {
    let response = payload.tool_response.as_ref()?;
    ["exit_code", "exitCode", "code", "status"]
        .iter()
        .find_map(|key| response.get(key).and_then(serde_json::Value::as_i64))
        .and_then(|value| i32::try_from(value).ok())
}

fn extract_tool_output_text(payload: &CodexPayload) -> String {
    match &payload.tool_response {
        None => String::new(),
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(v) => {
            let mut parts = Vec::new();
            for key in ["stdout", "stderr", "output", "result"] {
                if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
                    parts.push(s.to_string());
                }
            }
            if parts.is_empty() {
                v.to_string()
            } else {
                parts.join("\n")
            }
        }
    }
}

// Re-exports
mod hook {
    pub(crate) use crate::hook::{BlockReason, collect_logged};
}

#[cfg(test)]
mod tests {
    use super::{
        CodexPayload, LoadedRules, PatchFile, ToolCall, assert_new_content, check_bash_patterns,
        check_post_bash_command, evaluate_patch_file, fire_verdict,
    };

    #[tokio::test]
    async fn new_content_assertion_surfaces_engine_errors() {
        let network = phr::ReteNetwork::new();
        assert_new_content(&network, "incoming", "first")
            .await
            .expect("first assertion succeeds");
        let error = assert_new_content(&network, "incoming", "second")
            .await
            .expect_err("duplicate fact id must be propagated");
        assert!(error.to_string().contains("incoming"));
    }

    #[test]
    fn tool_call_defaults_missing_tool_name_and_input_without_panicking() {
        let payload: CodexPayload =
            serde_json::from_value(serde_json::json!({})).expect("payload defaults");
        let call = ToolCall::from_payload(&payload, "");
        assert_eq!(call.tool_name, "");
        assert_eq!(call.file_path, "");
        assert!(!call.supported());
        assert!(payload.tool_input.is_none());
    }

    #[test]
    fn tool_call_preserves_supported_name_and_resolved_path() {
        let payload: CodexPayload = serde_json::from_value(serde_json::json!({
            "tool_name": "apply_patch",
            "tool_input": {"command": "*** Begin Patch\n*** End Patch"}
        }))
        .expect("payload");
        let call = ToolCall::from_payload(&payload, "src/lib.rs");
        assert_eq!(call.tool_name, "apply_patch");
        assert_eq!(call.file_path, "src/lib.rs");
        assert!(call.supported());
    }

    // -----------------------------------------------------------------
    // Residual fail-opens (T15): `fire_verdict`, `check_bash_patterns`,
    // `check_post_bash_command`, and `evaluate_patch_file` must match
    // `hook/pre.rs` and `hook/post.rs` semantics rather than swallowing
    // or downgrading errors.
    // -----------------------------------------------------------------

    /// A `ScriptEval` that always errors out of evaluation entirely (rather
    /// than returning a normal `Err`, which the network treats as a merely
    /// blocked condition), used to poison one of `ReteNetwork`'s internal
    /// locks from outside the engine crate — the smallest available seam,
    /// since every other reachable path inside `update_agenda` only ever
    /// produces `ReteError::LockPoisoned` this way.
    #[derive(Debug)]
    struct AlwaysAbortScriptEval;

    impl phr::ScriptEval for AlwaysAbortScriptEval {
        fn evaluate(
            &self,
            _script: &str,
            _facts: &[phr::Fact],
            _bindings: &std::collections::HashMap<String, String>,
        ) -> std::result::Result<bool, String> {
            unreachable!("AlwaysAbortScriptEval always aborts evaluation")
        }
    }

    fn pure_script_rule(id: &str) -> phr::Rule {
        phr::Rule {
            id: id.to_string(),
            priority: 1,
            conditions: vec![phr::Condition {
                predicate: "__script__".to_string(),
                args: Vec::new(),
                script: Some("true".to_string()),
            }],
            actions: Vec::new(),
        }
    }

    /// Before the fix, `fire_verdict` did `let _ = network.update_agenda().await;`
    /// and pressed on to `fire_all_consequences` — which never touches the
    /// `fired_activations` lock, so it reports `Ok` regardless — meaning an
    /// `update_agenda` failure was invisible to every caller. `hook/pre.rs`
    /// treats the same failure as "agenda update failed" and blocks. This
    /// poisons `fired_activations` via an aborting script evaluator (the
    /// smallest reachable seam: `update_agenda` holds that lock across every
    /// `evaluate_script_conditions` call) and asserts `fire_verdict` now
    /// surfaces it as `Err` instead of silently returning `Ok`.
    #[tokio::test]
    async fn fire_verdict_surfaces_agenda_update_failure_instead_of_ignoring_it() {
        let network = std::sync::Arc::new(phr::ReteNetwork::with_script_evaluator(Box::new(
            AlwaysAbortScriptEval,
        )));
        network
            .add_rule(pure_script_rule("aborts-on-eval"))
            .await
            .expect("rule loads");

        // Poison the lock out of band first: the task's abort during
        // evaluation unwinds through `update_agenda`, dropping its
        // `fired_activations` guard while unwinding.
        let poison_net = std::sync::Arc::clone(&network);
        let join = tokio::spawn(async move { poison_net.update_agenda().await });
        assert!(
            join.await.is_err(),
            "the aborting script evaluator must abort update_agenda's task"
        );

        let root = std::path::Path::new("/nonexistent-root-for-test");
        let result = fire_verdict(&network, root).await;
        let message = result.expect_err(
            "fire_verdict must propagate an update_agenda failure as Err, matching hook/pre.rs",
        );
        assert!(
            message.contains("agenda update failed"),
            "message should name the failed step like hook/pre.rs does: {message}"
        );
    }

    /// A rule naming a `bash_command_matches` pattern whose derived fact id
    /// is already held by different content. `hook_facts::check_bash_command_patterns`
    /// then returns `Err`, which `hook/pre.rs` blocks on ("command pattern
    /// check failed"). Before the fix, Codex's `check_bash_patterns` routed
    /// this into `warnings` instead of `violations`.
    fn seed_conflicting_bash_pattern_fact(pattern: &str) -> phr::Fact {
        let fact_id = format!(
            "bash_command_matches_{}",
            pattern
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect::<String>()
        );
        phr::Fact {
            id: fact_id,
            predicate: "bash_command_matches".to_string(),
            args: vec!["different-content".to_string()],
            timestamp: 0,
            source: Some("test-seed".to_string()),
        }
    }

    fn bash_pattern_rule(id: &str, pattern: &str) -> phr::Rule {
        phr::Rule {
            id: id.to_string(),
            priority: 1,
            conditions: vec![phr::Condition {
                predicate: "bash_command_matches".to_string(),
                args: vec![pattern.to_string()],
                script: None,
            }],
            actions: Vec::new(),
        }
    }

    #[tokio::test]
    async fn check_bash_patterns_blocks_on_command_pattern_fact_assertion_failure() {
        let network = phr::ReteNetwork::new();
        network
            .assert_fact(seed_conflicting_bash_pattern_fact("danger"))
            .await
            .expect("seed fact");
        let rules = vec![bash_pattern_rule("bash-pattern-rule", "danger")];

        let violations = check_bash_patterns(&network, &rules, "", "danger-zone").await;

        assert!(
            violations
                .iter()
                .any(|v| v.contains("command pattern check failed")),
            "a fact-assertion failure in the command-pattern check must block \
             like hook/pre.rs, not disappear into a warning: {violations:?}"
        );
    }

    #[tokio::test]
    async fn check_post_bash_command_warns_instead_of_discarding_the_failure() {
        let network = phr::ReteNetwork::new();
        network
            .assert_fact(seed_conflicting_bash_pattern_fact("danger"))
            .await
            .expect("seed fact");
        let rules = vec![bash_pattern_rule("bash-pattern-rule", "danger")];

        let payload: CodexPayload = serde_json::from_value(serde_json::json!({
            "tool_name": "Bash",
            "tool_input": {"command": "danger-zone"}
        }))
        .expect("payload");
        let call = ToolCall::from_payload(&payload, "");

        let error = check_post_bash_command(&call, &network, &rules, "danger-zone")
            .await
            .expect_err(
                "a fact-assertion failure must surface as Err, matching hook/post.rs's \
                 warn behavior, not be silently discarded",
            );
        assert!(error.to_string().contains("command pattern check failed"));
    }

    /// `evaluate_patch_file` must fail closed on a genuine read error (e.g. no
    /// read permission) for a file the patch claims already exists, rather
    /// than treating the error as empty content — which would let real
    /// content slip past every content rule. A genuinely missing file (Add
    /// File in the patch) must still be treated as empty, so this only
    /// covers the non-`NotFound` branch.
    #[cfg(unix)]
    #[tokio::test]
    async fn evaluate_patch_file_fails_closed_on_unreadable_existing_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let rel_path = "src/unreadable.rs";
        let abs_path = root.join(rel_path);
        std::fs::create_dir_all(abs_path.parent().unwrap()).expect("mkdir");
        std::fs::write(&abs_path, "fn existing() {}\n").expect("write file");
        std::fs::set_permissions(&abs_path, std::fs::Permissions::from_mode(0o000))
            .expect("chmod 000");
        if std::fs::read(&abs_path).is_ok() {
            // Running as root: the mode bits do not apply, so there is nothing
            // to test here.
            return;
        }

        let loaded = LoadedRules {
            rules: Vec::new(),
            override_facts: Vec::new(),
        };
        let payload: CodexPayload = serde_json::from_value(serde_json::json!({
            "tool_name": "apply_patch",
            "tool_input": {"command": "*** Begin Patch\n*** End Patch"}
        }))
        .expect("payload");
        let call = ToolCall {
            payload: &payload,
            tool_name: "apply_patch",
            file_path: "",
        };
        // No added lines: `evaluate_patch_file` must fall back to reading
        // disk content, and that read must fail (permission denied) rather
        // than silently becoming empty.
        let pf = PatchFile {
            path: rel_path.to_string(),
            added: String::new(),
            deleted: false,
        };

        let result = evaluate_patch_file(&call, &loaded, &pf, root).await;

        // Restore permissions so the tempdir can be cleaned up regardless of
        // the assertion outcome below.
        let _ = std::fs::set_permissions(&abs_path, std::fs::Permissions::from_mode(0o644));

        let decision = result.expect_err(
            "an unreadable existing file must fail closed (deny), not be treated as empty content",
        );
        assert!(
            !decision.block_messages.is_empty(),
            "expected a block message, got {decision:?}"
        );
        assert!(
            decision.block_messages[0].contains(rel_path),
            "block message should name the unreadable file: {decision:?}"
        );
    }
}
