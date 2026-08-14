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
    #[allow(dead_code)]
    exit: i32,
    block_messages: Vec<String>,
    warn_messages: Vec<String>,
    additional_context: String,
    /// Paths touched by the tool call; journaled once post-hook wiring lands.
    #[allow(dead_code)]
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
            exit: 2,
            block_messages: vec![message],
            warn_messages: Vec::new(),
            additional_context: String::new(),
            files: Vec::new(),
        }
    } else if matches!(event, "PostToolUse" | "post-tool-use") {
        CodexDecision {
            exit: 1,
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
        exit: 0,
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
        exit: 0,
        block_messages,
        warn_messages,
        additional_context: String::new(),
        files: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// PreToolUse
// ---------------------------------------------------------------------------

async fn handle_pre(payload: &CodexPayload, root: &Path) -> CodexDecision {
    let tool_name = payload.tool_name.as_deref().unwrap_or("");
    let file_path = extract_file_path(payload.tool_input.as_ref());

    if tool_name != "Bash" && tool_name != "apply_patch" {
        return empty_decision();
    }

    if tool_name == "apply_patch" {
        return handle_pre_patch(payload, &file_path).await;
    }

    // --- Bash ---
    let rules = match load_rules("pre") {
        Ok(Some(r)) => r,
        Ok(None) => return empty_decision(),
        Err(e) => {
            return CodexDecision {
                exit: 2,
                block_messages: vec![format!("rules error: {}", e)],
                warn_messages: Vec::new(),
                additional_context: String::new(),
                files: Vec::new(),
            };
        }
    };

    let (network, stale_graph_rules) = build_pre_network(&rules, &file_path, root).await;
    let command = extract_bash_command(payload);

    // Assert content fact
    if let Err(error) = assert_new_content(&network, "new_content", &command).await {
        return CodexDecision {
            exit: 2,
            block_messages: vec![format!("failed to assert content fact: {error}")],
            warn_messages: Vec::new(),
            additional_context: String::new(),
            files: Vec::new(),
        };
    }

    // Content pattern checks
    let cp = crate::hook_facts::collect_content_patterns(&rules);
    let bcp = crate::hook_facts::collect_bash_command_patterns(&rules);

    let mut violations = Vec::new();
    let mut warnings = Vec::new();

    if let Err(e) =
        crate::hook_facts::check_content_patterns(&network, &file_path, &command, &cp).await
    {
        violations.push(e.to_string());
    }
    if let Err(e) = crate::hook_facts::check_bash_command_patterns(&network, &command, &bcp).await {
        warnings.push(e.to_string());
    }

    // Cargo workspace scanner (sync-safe)
    crate::hook::assert_cargo_workspace_facts(&network, &command).await;
    let provider_event = codex_provider_event(payload, "Bash", &file_path, "pre", &command);
    if let Err(error) =
        crate::predicate_provider::assert_facts(&network, root, &provider_event).await
    {
        return CodexDecision {
            exit: 2,
            block_messages: vec![error.to_string()],
            warn_messages: Vec::new(),
            additional_context: String::new(),
            files: Vec::new(),
        };
    }

    // Fire rules
    // RETE consequences reach the agenda only after an explicit update, as
    // the Claude hook does before firing. Without it any verdict derived
    // purely from fact matching — every structural graph rule — is computed
    // and then dropped.
    let _ = network.update_agenda().await;
    let consequences = match network.fire_all_consequences() {
        Ok(c) => c,
        Err(e) => {
            return CodexDecision {
                exit: 2,
                block_messages: vec![format!("rule execution failed: {}", e)],
                warn_messages: Vec::new(),
                additional_context: String::new(),
                files: Vec::new(),
            };
        }
    };

    let (mut logged, mut block_msgs, mut warn_msgs) =
        hook::collect_logged(&consequences, &security::project_root());
    // Rules reading a drifted graph reason from evidence we cannot vouch for,
    // so the harness declines to act on their verdicts.
    if !stale_graph_rules.is_empty() {
        crate::hook_logged::demote_violations_from(&mut logged, &stale_graph_rules);
        let (v, w) = crate::hook_logged::split_messages_by_action_type(&logged);
        block_msgs = v;
        warn_msgs = w;
    }

    block_msgs.extend(violations);
    warn_msgs.extend(warnings);

    if !block_msgs.is_empty() {
        for v in &block_msgs {
            eprintln!("phronesis: BLOCKED — {}", v);
        }
        for w in &warn_msgs {
            eprintln!("phronesis: WARNING — {}", w);
        }
        log_event("pre", payload, "Bash", &file_path, 2, &logged);
        return CodexDecision {
            exit: 2,
            block_messages: block_msgs,
            warn_messages: warn_msgs,
            additional_context: String::new(),
            files: Vec::new(),
        };
    }
    if !warn_msgs.is_empty() {
        for w in &warn_msgs {
            eprintln!("phronesis: WARNING — {}", w);
        }
        log_event("pre", payload, "Bash", &file_path, 1, &logged);
        return CodexDecision {
            exit: 1,
            block_messages: Vec::new(),
            warn_messages: warn_msgs,
            additional_context: String::new(),
            files: Vec::new(),
        };
    }

    log_event("pre", payload, "Bash", &file_path, 0, &logged);
    empty_decision()
}

async fn handle_pre_patch(payload: &CodexPayload, _file_path: &str) -> CodexDecision {
    // The current Codex contract supplies both Bash and apply_patch input in
    // `tool_input.command`.
    let patch_text = payload
        .tool_input
        .as_ref()
        .and_then(|t| t.get("command"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("");

    let files = codex_patch::parse_patch(patch_text);
    if files.is_empty() {
        return CodexDecision {
            exit: 2,
            block_messages: vec!["malformed apply_patch input: no file blocks".to_string()],
            warn_messages: Vec::new(),
            additional_context: String::new(),
            files: Vec::new(),
        };
    }

    // Check traversal. NotFound is fine — Add File targets don't exist yet;
    // only genuine escape attempts (.. segments, outside root) block.
    for pf in &files {
        if Path::new(&pf.path).is_absolute()
            || pf
                .path
                .split(['/', std::path::MAIN_SEPARATOR])
                .any(|part| part == "..")
        {
            return CodexDecision {
                exit: 2,
                block_messages: vec![format!("unsafe patch path blocked: {}", pf.path)],
                warn_messages: Vec::new(),
                additional_context: String::new(),
                files: Vec::new(),
            };
        }
        if let Err(
            security::SecurityError::PathTraversal(_) | security::SecurityError::PathOutsideRoot(_),
        ) = security::resolve_safe_path(&pf.path, &security::project_root())
        {
            return CodexDecision {
                exit: 2,
                block_messages: vec![format!("path traversal blocked: {}", pf.path)],
                warn_messages: Vec::new(),
                additional_context: String::new(),
                files: Vec::new(),
            };
        }
    }

    let rules = match load_rules("pre") {
        Ok(Some(r)) => r,
        Ok(None) => {
            return CodexDecision {
                exit: 0,
                block_messages: Vec::new(),
                warn_messages: Vec::new(),
                additional_context: String::new(),
                files: files.iter().map(|f| f.path.clone()).collect(),
            };
        }
        Err(e) => {
            return CodexDecision {
                exit: 2,
                block_messages: vec![format!("rules error: {}", e)],
                warn_messages: Vec::new(),
                additional_context: String::new(),
                files: Vec::new(),
            };
        }
    };

    // Evaluate each file with its own network, mirroring the single-file
    // contract of the Claude hook: file_path facts (and their per-segment
    // file_path_matches derivatives) are scoped to one file at a time.
    let mut logged = Vec::new();
    let mut block_msgs = Vec::new();
    let mut warn_msgs = Vec::new();

    // Providers get one batch-level view before the existing per-file views.
    // Keeping `file_path` empty and `files` populated makes the two contexts
    // unambiguous and prevents existing per-file providers from double-firing.
    let (batch_network, _) = build_pre_network(&rules, "", &security::project_root()).await;
    let mut batch_event = codex_provider_event(payload, "apply_patch", "", "pre", "");
    batch_event.files = files.iter().map(|file| file.path.clone()).collect();
    if let Err(error) = crate::predicate_provider::assert_facts(
        &batch_network,
        &security::project_root(),
        &batch_event,
    )
    .await
    {
        return CodexDecision {
            exit: 2,
            block_messages: vec![error.to_string()],
            warn_messages: Vec::new(),
            additional_context: String::new(),
            files: Vec::new(),
        };
    }
    let _ = batch_network.update_agenda().await;
    let batch_consequences = match batch_network.fire_all_consequences() {
        Ok(consequences) => consequences,
        Err(error) => {
            return CodexDecision {
                exit: 2,
                block_messages: vec![format!("rule execution failed: {error}")],
                warn_messages: Vec::new(),
                additional_context: String::new(),
                files: Vec::new(),
            };
        }
    };
    let (batch_logged, batch_blocks, batch_warns) =
        hook::collect_logged(&batch_consequences, &security::project_root());
    logged.extend(batch_logged);
    block_msgs.extend(batch_blocks);
    warn_msgs.extend(batch_warns);

    for pf in &files {
        // Prefer the patch's own added lines (the file may not exist on disk
        // yet for Add File), falling back to the current on-disk content.
        let content = if pf.added.is_empty() {
            match security::resolve_safe_path(&pf.path, &security::project_root()) {
                Ok(safe) => std::fs::read_to_string(&safe).unwrap_or_default(),
                Err(_) => continue,
            }
        } else {
            pf.added.clone()
        };

        let (network, stale_graph_rules) =
            build_pre_network(&rules, &pf.path, &security::project_root()).await;
        if !content.is_empty() {
            let fact_id = format!("new_content_{}", pf.path.replace('/', "_"));
            if let Err(error) = assert_new_content(&network, &fact_id, &content).await {
                return CodexDecision {
                    exit: 2,
                    block_messages: vec![format!("failed to assert content fact: {error}")],
                    warn_messages: Vec::new(),
                    additional_context: String::new(),
                    files: Vec::new(),
                };
            }
            // Rules match on derived new_content_contains facts, not the raw
            // content — mirror the Claude pre-hook's pattern scan.
            let patterns = crate::hook_facts::collect_content_patterns(&rules);
            if let Err(e) =
                crate::hook_facts::check_content_patterns(&network, &pf.path, &content, &patterns)
                    .await
            {
                return CodexDecision {
                    exit: 2,
                    block_messages: vec![format!("pattern check failed: {}", e)],
                    warn_messages: Vec::new(),
                    additional_context: String::new(),
                    files: Vec::new(),
                };
            }
            if let Err(error) =
                crate::hook_facts::assert_language_pack_facts(&network, &pf.path, &content).await
            {
                return CodexDecision {
                    exit: 2,
                    block_messages: vec![format!("language-pack fact assertion failed: {error}")],
                    warn_messages: Vec::new(),
                    additional_context: String::new(),
                    files: Vec::new(),
                };
            }
        }
        let provider_event =
            codex_provider_event(payload, "apply_patch", &pf.path, "pre", &content);
        if let Err(error) = crate::predicate_provider::assert_facts(
            &network,
            &security::project_root(),
            &provider_event,
        )
        .await
        {
            return CodexDecision {
                exit: 2,
                block_messages: vec![error.to_string()],
                warn_messages: Vec::new(),
                additional_context: String::new(),
                files: Vec::new(),
            };
        }

        // RETE consequences reach the agenda only after an explicit update, as
        // the Claude hook does before firing. Without it any verdict derived
        // purely from fact matching — every structural graph rule — is computed
        // and then dropped.
        let _ = network.update_agenda().await;
        let consequences = match network.fire_all_consequences() {
            Ok(c) => c,
            Err(e) => {
                return CodexDecision {
                    exit: 2,
                    block_messages: vec![format!("rule execution failed: {}", e)],
                    warn_messages: Vec::new(),
                    additional_context: String::new(),
                    files: Vec::new(),
                };
            }
        };

        let (mut file_logged, file_blocks, file_warns) =
            hook::collect_logged(&consequences, &security::project_root());
        // A drifted graph makes structural verdicts unreliable, so they warn
        // rather than block — the harness declining to enforce on evidence it
        // cannot vouch for, exactly as the Claude pre-hook does.
        let (file_blocks, file_warns) = if stale_graph_rules.is_empty() {
            (file_blocks, file_warns)
        } else {
            crate::hook_logged::demote_violations_from(&mut file_logged, &stale_graph_rules);
            crate::hook_logged::split_messages_by_action_type(&file_logged)
        };
        logged.extend(file_logged);
        block_msgs.extend(file_blocks);
        warn_msgs.extend(file_warns);
    }

    if !block_msgs.is_empty() || !warn_msgs.is_empty() {
        for v in &block_msgs {
            eprintln!("phronesis: BLOCKED — {}", v);
        }
        for w in &warn_msgs {
            eprintln!("phronesis: WARNING — {}", w);
        }
        log_event(
            "pre",
            payload,
            "apply_patch",
            "",
            if !block_msgs.is_empty() { 2 } else { 1 },
            &logged,
        );
        return CodexDecision {
            exit: if !block_msgs.is_empty() { 2 } else { 1 },
            block_messages: block_msgs,
            warn_messages: warn_msgs,
            additional_context: String::new(),
            files: files.iter().map(|f| f.path.clone()).collect(),
        };
    }

    log_event("pre", payload, "apply_patch", "", 0, &logged);
    CodexDecision {
        exit: 0,
        block_messages: Vec::new(),
        warn_messages: Vec::new(),
        additional_context: String::new(),
        files: files.iter().map(|f| f.path.clone()).collect(),
    }
}

// ---------------------------------------------------------------------------
// PostToolUse
// ---------------------------------------------------------------------------

async fn handle_post(payload: &CodexPayload, root: &Path) -> CodexDecision {
    let tool_name = payload.tool_name.as_deref().unwrap_or("");
    let file_path = extract_file_path(payload.tool_input.as_ref());

    if tool_name != "Bash" && tool_name != "apply_patch" {
        return empty_decision();
    }

    // Structural sensor, matching the Claude post-hook. Without it a Codex
    // project's graph goes stale on every edit and its structural rules stay
    // demoted to warnings until someone runs `graph rebuild` by hand.
    // `apply_patch` writes several files at once, so each is recorded.
    if tool_name == "apply_patch" {
        for file in codex_patch::parse_patch(&extract_bash_command(payload)) {
            crate::graph::sync::record_from_disk(root, &file.path);
        }
    } else if !file_path.is_empty() {
        crate::graph::sync::record_from_disk(root, &file_path);
    }

    let rules = match load_rules("post") {
        Ok(Some(r)) => r,
        Ok(None) => {
            log_event("post", payload, tool_name, &file_path, 0, &[]);
            journal_supported_post(payload, &file_path).await;
            return empty_decision();
        }
        Err(e) => {
            journal_supported_post(payload, &file_path).await;
            return CodexDecision {
                exit: 0,
                block_messages: Vec::new(),
                warn_messages: vec![format!("rules error: {}", e)],
                additional_context: String::new(),
                files: Vec::new(),
            };
        }
    };

    let network = build_post_network(&rules, &file_path, root).await;

    // Command pattern check on already-executed command
    if tool_name == "Bash" {
        let command = extract_bash_command(payload);
        let bcp = crate::hook_facts::collect_bash_command_patterns(&rules);
        let _ = crate::hook_facts::check_bash_command_patterns(&network, &command, &bcp).await;
    }
    let provider_content = if tool_name == "Bash" {
        extract_bash_command(payload)
    } else {
        String::new()
    };
    let mut provider_event =
        codex_provider_event(payload, tool_name, &file_path, "post", &provider_content);
    if tool_name == "apply_patch" {
        provider_event.files = codex_patch::parse_patch(&extract_bash_command(payload))
            .into_iter()
            .map(|file| file.path)
            .collect();
    }
    if let Err(error) =
        crate::predicate_provider::assert_facts(&network, root, &provider_event).await
    {
        journal_supported_post(payload, &file_path).await;
        return CodexDecision {
            exit: 1,
            block_messages: Vec::new(),
            warn_messages: vec![error.to_string()],
            additional_context: String::new(),
            files: Vec::new(),
        };
    }

    // RETE consequences reach the agenda only after an explicit update, as
    // the Claude hook does before firing. Without it any verdict derived
    // purely from fact matching — every structural graph rule — is computed
    // and then dropped.
    let _ = network.update_agenda().await;
    let consequences = match network.fire_all_consequences() {
        Ok(c) => c,
        Err(e) => {
            journal_supported_post(payload, &file_path).await;
            return CodexDecision {
                exit: 1,
                block_messages: Vec::new(),
                warn_messages: vec![format!("rule execution failed: {}", e)],
                additional_context: String::new(),
                files: Vec::new(),
            };
        }
    };

    let (logged, block_msgs, warn_msgs) =
        hook::collect_logged(&consequences, &security::project_root());

    if !block_msgs.is_empty() || !warn_msgs.is_empty() {
        for v in &block_msgs {
            eprintln!("phronesis: WARNING — {}", v);
        }
        for w in &warn_msgs {
            eprintln!("phronesis: WARNING — {}", w);
        }
        log_event("post", payload, tool_name, &file_path, 1, &logged);
        journal_supported_post(payload, &file_path).await;
        return CodexDecision {
            exit: 1,
            block_messages: block_msgs,
            warn_messages: warn_msgs,
            additional_context: String::new(),
            files: Vec::new(),
        };
    }

    log_event("post", payload, tool_name, &file_path, 0, &logged);
    journal_supported_post(payload, &file_path).await;
    empty_decision()
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
        exit: 0,
        block_messages: Vec::new(),
        warn_messages: Vec::new(),
        additional_context: build_context_body(root, kind).await,
        files: Vec::new(),
    }
}

fn make_compact_decision(root: &Path, _pre: bool) -> CodexDecision {
    let durable = context::read_durable_directives(root);
    CodexDecision {
        exit: 0,
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
    file_path: &str,
    root: &Path,
) -> (phr::ReteNetwork, std::collections::BTreeSet<phr::RuleId>) {
    let mut net = crate::net::build_network();
    for rule in rules {
        let _ = net.add_rule(rule.clone()).await;
    }
    let _ = crate::hook_facts::assert_common_facts(&net, file_path, "Bash", "pre").await;
    let _ = assert_journey_facts_into(&mut net, root, rules).await;
    crate::hook::assert_pack_marker_facts(&net, root).await;
    crate::hook::assert_confidence_signals(&net).await;

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
    (net, stale_graph_rules)
}

async fn build_post_network(rules: &[phr::Rule], file_path: &str, root: &Path) -> phr::ReteNetwork {
    let mut net = crate::net::build_network();
    for rule in rules {
        let _ = net.add_rule(rule.clone()).await;
    }
    let _ = crate::hook_facts::assert_common_facts(&net, file_path, "Bash", "post").await;
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

fn load_rules(phase: &str) -> Result<Option<Vec<phr::Rule>>, String> {
    let path_buf = crate::rules_file::default_path(&security::project_root());
    if !path_buf.exists() {
        return Ok(None);
    }
    let rules_file = crate::rules_file::read(&path_buf).map_err(|e| e.to_string())?;
    let rules: Vec<phr::Rule> = rules_file
        .rules
        .into_iter()
        .filter(|r| r.phase == phase)
        .map(|r| crate::rules_file::rule_from_disk(&r).0)
        .collect();
    if rules.is_empty() {
        Ok(None)
    } else {
        Ok(Some(rules))
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
    payload: &CodexPayload,
    tool_name: &str,
    file_path: &str,
    phase: &str,
    content: &str,
) -> crate::predicate_provider::ProviderEvent {
    crate::predicate_provider::ProviderEvent {
        phase: phase.to_string(),
        tool_name: tool_name.to_string(),
        file_path: file_path.to_string(),
        file_rel: crate::graph::hydrate::repo_relative(&crate::security::project_root(), file_path)
            .unwrap_or_default(),
        files: Vec::new(),
        old_content: String::new(),
        new_content: content.to_string(),
        command: if tool_name == "Bash" {
            content.to_string()
        } else {
            String::new()
        },
        output: extract_tool_output_text(payload),
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
    payload: &CodexPayload,
    tool_name: &str,
    file_path: &str,
    exit: i32,
    logged: &[crate::hook_logged::LoggedConsequence],
) {
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
    let tag_result = journey::tagger::fire(&cfg, &facts)
        .await
        .unwrap_or_default();
    let command = extract_bash_command(payload);
    let output = extract_tool_output_text(payload);
    let command_exit = extract_command_exit(payload);
    let (outcome_tags, subject) =
        outcomes::adapter::extract_from(outcomes::adapter::ExtractFromInput {
            project_root: &root,
            tool_name: tool,
            command: Some(&command),
            output: &output,
            command_exit,
        });
    let ext = std::path::Path::new(file_path)
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string());
    let module = journey::tagger::resolve_module(&cfg, file_path);
    let mut all_tags = tag_result.tags;
    all_tags.extend(outcome_tags);
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
        ext,
        module,
        tags: all_tags,
        subject,
        command_exit,
    };
    let _ = journey::journal::append(&root, &record);
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
