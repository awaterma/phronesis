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
use crate::outcomes;
use crate::security;

// ---------------------------------------------------------------------------
// Payload shapes
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
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
}

struct PatchFile {
    path: String,
    /// Lines the patch adds (`+` hunk lines). Used as `new_content` when the
    /// file isn't readable on disk yet (Add File), and preferred otherwise so
    /// rules see the incoming content, not just the current file state.
    added: String,
}

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
    let parsed = parse_payload();
    let fallback;
    let (event, result) = match parsed.as_ref() {
        Ok(payload) => {
            let event = payload.hook_event_name.as_deref().unwrap_or(event);
            (event, dispatch(payload, event, &root).await)
        }
        Err(error) => {
            fallback = invalid_payload_decision(event, error);
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

fn parse_payload() -> anyhow::Result<CodexPayload> {
    let raw = security::read_stdin_capped()?;
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
            make_ctx_decision(root, ContextKind::SessionStart).await
        }
        "UserPromptSubmit" | "user-prompt-submit" => {
            make_ctx_decision(root, ContextKind::InteractionContext).await
        }
        "PreCompact" | "pre-compact" => make_compact_decision(root, true),
        "PostCompact" | "post-compact" => make_ctx_decision(root, ContextKind::PostCompact).await,
        "SubagentStart" | "subagent-start" => {
            make_ctx_decision(root, ContextKind::SubagentStart).await
        }
        "SubagentStop" | "subagent-stop" | "Stop" | "stop" => make_completion_decision(root),
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
        log_event("pre", call, exit, &self.logged);
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
        log_event("post", call, i32::from(flagged), &self.logged);
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
    let _ = network.update_agenda().await;
    let consequences = network
        .fire_all_consequences()
        .map_err(|e| format!("rule execution failed: {}", e))?;
    crate::capsule::capture_for_hook(root, &consequences);
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
    let file_path = extract_file_path(payload.tool_input.as_ref());
    let call = ToolCall::from_payload(payload, &file_path);

    if !call.supported() {
        return empty_decision();
    }

    if call.tool_name == "apply_patch" {
        return handle_pre_patch(payload).await;
    }

    // --- Bash ---
    let loaded = match load_rules("pre") {
        Ok(Some(r)) => r,
        Ok(None) => return empty_decision(),
        Err(e) => return block_decision(format!("rules error: {}", e)),
    };

    let (network, stale_graph_rules) =
        build_pre_network(&loaded.rules, &loaded.override_facts, &file_path, root).await;
    let command = extract_bash_command(payload);

    // Assert content fact
    if let Err(error) = assert_new_content(&network, "new_content", &command).await {
        return block_decision(format!("failed to assert content fact: {error}"));
    }

    // Content pattern checks
    let (violations, warnings) =
        check_bash_patterns(&network, &loaded.rules, &file_path, &command).await;

    // Cargo workspace scanner (sync-safe)
    crate::hook::assert_cargo_workspace_facts(&network, &command).await;
    if let Err(error) = crate::predicate_provider::assert_facts(
        &network,
        root,
        &codex_provider_event(&call, "pre", &command),
    )
    .await
    {
        return block_decision(error.to_string());
    }

    let mut verdict = match fire_verdict(&network, root).await {
        Ok(verdict) => verdict,
        Err(message) => return block_decision(message),
    };
    verdict.demote_stale(&stale_graph_rules);
    verdict.block_msgs.extend(violations);
    verdict.warn_msgs.extend(warnings);
    verdict.finish_pre(&call, Vec::new())
}

/// Content-pattern violations and bash-command-pattern warnings for a command.
async fn check_bash_patterns(
    network: &phr::ReteNetwork,
    rules: &[phr::Rule],
    file_path: &str,
    command: &str,
) -> (Vec<String>, Vec<String>) {
    let cp = crate::hook_facts::collect_content_patterns(rules);
    let bcp = crate::hook_facts::collect_bash_command_patterns(rules);
    let violations = crate::hook_facts::check_content_patterns(network, file_path, command, &cp)
        .await
        .err()
        .map(|e| e.to_string())
        .into_iter()
        .collect();
    let warnings = crate::hook_facts::check_bash_command_patterns(network, command, &bcp)
        .await
        .err()
        .map(|e| e.to_string())
        .into_iter()
        .collect();
    (violations, warnings)
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

async fn handle_pre_patch(payload: &CodexPayload) -> CodexDecision {
    let root = security::project_root();
    let files = codex_patch::parse_patch(extract_patch_text(payload));
    if files.is_empty() {
        return block_decision("malformed apply_patch input: no file blocks".to_string());
    }
    if let Some(decision) = reject_unsafe_patch_paths(&files, &root) {
        return decision;
    }
    let paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();

    let loaded = match load_rules("pre") {
        Ok(Some(r)) => r,
        Ok(None) => {
            return CodexDecision {
                block_messages: Vec::new(),
                warn_messages: Vec::new(),
                additional_context: String::new(),
                files: paths,
            };
        }
        Err(e) => return block_decision(format!("rules error: {}", e)),
    };
    let call = ToolCall {
        payload,
        tool_name: "apply_patch",
        file_path: "",
    };

    // Providers get one batch-level view before the per-file views.
    let mut verdict = match evaluate_patch_batch(&call, &loaded, &paths, &root).await {
        Ok(verdict) => verdict,
        Err(decision) => return decision,
    };

    // Evaluate each file with its own network, mirroring the single-file
    // contract of the Claude hook: file_path facts (and their per-segment
    // file_path_matches derivatives) are scoped to one file at a time.
    for pf in &files {
        match evaluate_patch_file(&call, &loaded, pf, &root).await {
            Ok(Some(file_verdict)) => verdict.extend(file_verdict),
            Ok(None) => {}
            Err(decision) => return decision,
        }
    }

    verdict.finish_pre(&call, paths)
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
        build_pre_network(&loaded.rules, &loaded.override_facts, "", root).await;
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
    // yet for Add File), falling back to the current on-disk content.
    let content = if pf.added.is_empty() {
        match security::resolve_safe_path(&pf.path, root) {
            Ok(safe) => tokio::fs::read_to_string(&safe).await.unwrap_or_default(),
            Err(_) => return Ok(None),
        }
    } else {
        pf.added.clone()
    };

    let (network, stale_graph_rules) =
        build_pre_network(&loaded.rules, &loaded.override_facts, &pf.path, root).await;
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
    let fact_id = format!("new_content_{}", path.replace('/', "_"));
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
    let file_path = extract_file_path(payload.tool_input.as_ref());
    let call = ToolCall::from_payload(payload, &file_path);

    if !call.supported() {
        return empty_decision();
    }

    record_post_edits(&call, root);

    let loaded = match load_rules("post") {
        Ok(Some(r)) => r,
        Ok(None) => {
            log_event("post", &call, 0, &[]);
            journal_supported_post(payload, &file_path).await;
            return empty_decision();
        }
        Err(e) => {
            journal_supported_post(payload, &file_path).await;
            return warn_decision(format!("rules error: {}", e));
        }
    };

    let network = build_post_network(&loaded.rules, &loaded.override_facts, &file_path, root).await;

    let command = if call.tool_name == "Bash" {
        extract_bash_command(payload)
    } else {
        String::new()
    };
    check_post_bash_command(&call, &network, &loaded.rules, &command).await;
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
async fn check_post_bash_command(
    call: &ToolCall<'_>,
    network: &phr::ReteNetwork,
    rules: &[phr::Rule],
    command: &str,
) {
    if call.tool_name != "Bash" {
        return;
    }
    let bcp = crate::hook_facts::collect_bash_command_patterns(rules);
    let _ = crate::hook_facts::check_bash_command_patterns(network, command, &bcp).await;
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
    context::run_body_configured(root, kind.event(), 5, kind.metric_event()).await
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
) -> (phr::ReteNetwork, std::collections::BTreeSet<phr::RuleId>) {
    let mut net = crate::net::build_network();
    for rule in rules {
        let _ = net.add_rule(rule.clone()).await;
    }
    let _ = crate::hook_facts::assert_common_facts(&net, file_path, "Bash", "pre").await;
    for fact in override_facts {
        let _ = net.assert_fact(fact.clone()).await;
    }
    let _ = assert_journey_facts_into(&mut net, root, rules).await;
    crate::hook::assert_pack_marker_facts(&net, root).await;
    crate::hook::assert_confidence_signals(&net).await;
    let stale_graph_rules = assert_structural_facts(&net, root, rules, file_path).await;
    (net, stale_graph_rules)
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
) -> phr::ReteNetwork {
    let mut net = crate::net::build_network();
    for rule in rules {
        let _ = net.add_rule(rule.clone()).await;
    }
    let _ = crate::hook_facts::assert_common_facts(&net, file_path, "Bash", "post").await;
    for fact in override_facts {
        let _ = net.assert_fact(fact.clone()).await;
    }
    let _ = assert_journey_facts_into(&mut net, root, rules).await;
    crate::hook::assert_pack_marker_facts(&net, root).await;
    net
}

async fn assert_journey_facts_into(
    network: &mut phr::ReteNetwork,
    project_root: &Path,
    rules: &[phr::Rule],
) -> Result<(), journey::derive::DeriveError> {
    if std::env::var("PHRONESIS_NO_JOURNEY").is_ok() {
        return Ok(());
    }
    let cfg = match journey::load_config(project_root) {
        Ok(c) => c,
        Err(journey::ConfigError::NotFound(_)) => journey::tagger::TaggerConfig::default(),
        Err(e) => {
            eprintln!("phronesis: journey config skipped: {}", e);
            return Ok(());
        }
    };
    let sid = journey::current_sid(project_root);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let scope = journey::derive::WindowScope {
        current_sid: &sid,
        now_ts: now,
    };
    journey::derive::assert_facts(
        network,
        journey::derive::DeriveInput {
            project_root,
            rules,
            config: &cfg,
            scope,
        },
    )
    .await
}

// ---------------------------------------------------------------------------
// Rules loading
// ---------------------------------------------------------------------------

struct LoadedRules {
    rules: Vec<phr::Rule>,
    override_facts: Vec<phr::Fact>,
}

fn load_rules(phase: &str) -> Result<Option<LoadedRules>, String> {
    let root = security::project_root();
    let project_path = crate::rules_file::default_path(&root);
    let loader_path = crate::rule_layers::config_path(&root);
    if !project_path.exists() && !loader_path.exists() {
        return Ok(None);
    }
    let resolved = crate::rule_layers::resolve(&root).map_err(|e| e.to_string())?;
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

fn log_event(
    phase: &str,
    call: &ToolCall<'_>,
    exit: i32,
    logged: &[crate::hook_logged::LoggedConsequence],
) {
    let ToolCall {
        payload,
        tool_name,
        file_path,
    } = *call;
    let path = action_log::default_path(&security::project_root());
    let mut entry = action_log::LogEntry::new("hook", "codex_hook")
        .with("phase", phase.to_string())
        .with("tool", tool_name.to_string())
        .with("exit", exit)
        .with("host", "codex".to_string());
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
    let tag_result = journey::tagger::fire(&cfg, &file_path_facts(file_path))
        .await
        .unwrap_or_default();
    let (outcome_tags, subject, command_exit) = extract_post_outcomes(payload, &root, tool);
    let record = journey::journal::JournalRecord {
        v: 1,
        ts: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        sid: payload
            .session_id
            .clone()
            .unwrap_or_else(|| journey::current_sid(&root)),
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
                id: format!("file_path_matches_{}", part),
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
    pub(crate) use crate::hook::collect_logged;
}

#[cfg(test)]
mod tests {
    use super::assert_new_content;

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
}
