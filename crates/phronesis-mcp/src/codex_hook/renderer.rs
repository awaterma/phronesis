//! Render Codex hook JSON responses.
//!
//! PreToolUse:
//! - block → `{ hookSpecificOutput: { hookEventName, additionalContext,
//!   permissionDecision: "deny", permissionDecisionReason } }`
//! - warn → `{ hookSpecificOutput: { hookEventName, additionalContext,
//!   additionalContext } }` (allow, inject context)
//! - clean → `{}`
//!
//! PostToolUse:
//! - warning/violation → `{ additionalContext, continue: false }`
//! - clean → `{}`
//!
//! Context events (SessionStart, etc.):
//! → `{ hookSpecificOutput: { hookEventName, additionalContext } }`
//!
//! Stop/SubagentStop:
//! - low confidence → `{ continue: false, stopReason, systemMessage }`
//! - medium confidence → `{ systemMessage }`
//! - high/no active work → `{}`

use serde::Serialize;

use crate::context;

use super::CodexDecision;

/// Render a Codex hook response as a JSON string.
pub fn render_codex_response(event: &str, decision: &CodexDecision) -> String {
    match event {
        "PreToolUse" | "pre-tool-use" => render_pre(decision),
        "PostToolUse" | "post-tool-use" => render_post(decision),
        "SessionStart" | "UserPromptSubmit" | "PreCompact" | "PostCompact" | "SubagentStart"
        | "session-start" | "user-prompt-submit" | "pre-compact" | "post-compact"
        | "subagent-start" => render_context(canonical_event(event), decision),
        "SubagentStop" | "Stop" | "subagent-stop" | "stop" => render_completion(decision),
        // Interrupt permits `systemMessage` only and SessionEnd is advisory;
        // Phronesis has nothing to say on either, and any extra key would fail
        // the whole hook under deny_unknown_fields.
        "Interrupt" | "interrupt" | "SessionEnd" | "session-end" => "{}".to_string(),
        _ => "{}".to_string(),
    }
}

fn render_pre(d: &CodexDecision) -> String {
    // Block: deny
    if !d.block_messages.is_empty() {
        let reason = d.block_messages.join(". ");
        let obj = codex_pre_deny(&reason);
        return serde_json::to_string(&obj).unwrap_or_default();
    }
    // Warn: allow with context
    if !d.warn_messages.is_empty() {
        let ctx = d.warn_messages.join("\n\n");
        let obj = codex_pre_allow_warn(&ctx);
        return serde_json::to_string(&obj).unwrap_or_default();
    }
    // Clean
    "{}".to_string()
}

fn render_post(d: &CodexDecision) -> String {
    if d.block_messages.is_empty() && d.warn_messages.is_empty() {
        return "{}".to_string();
    }
    let ctx = [d.block_messages.join(". "), d.warn_messages.join("\n\n")].concat();
    let obj = codex_post_warn(&ctx);
    serde_json::to_string(&obj).unwrap_or_default()
}

fn render_context(event: &str, d: &CodexDecision) -> String {
    if d.additional_context.is_empty() {
        return "{}".to_string();
    }
    let truncated = if d.additional_context.len() > context::DEFAULT_MAX_BYTES {
        let max = context::DEFAULT_MAX_BYTES;
        const MARKER: &str = "\n…[truncated]";
        let budget = max.saturating_sub(MARKER.len());
        let mut cut = budget;
        while cut > 0 && !d.additional_context.is_char_boundary(cut) {
            cut -= 1;
        }
        format!("{}{}", &d.additional_context[..cut], MARKER)
    } else {
        d.additional_context.clone()
    };
    let obj = codex_context(event, &truncated);
    serde_json::to_string(&obj).unwrap_or_default()
}

fn render_completion(d: &CodexDecision) -> String {
    if let Some(reason) = d.block_messages.first() {
        return serde_json::to_string(&CodexCompletionBlock {
            decision: "block",
            continue_turn: false,
            stop_reason: reason,
            system_message: reason,
        })
        .unwrap_or_default();
    }
    if let Some(message) = d.warn_messages.first() {
        return serde_json::to_string(&CodexCompletionWarn {
            system_message: message,
        })
        .unwrap_or_default();
    }
    "{}".to_string()
}

// ---------------------------------------------------------------------------
// JSON shapes
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct CodexCompletionBlock<'a> {
    /// `"block"` — the same signal `blocks` in the adapter reads, and a key
    /// the Stop/SubagentStop schema permits. Without it a blocked stop is
    /// indistinguishable from a clean `{continue: false}` stopReason response.
    decision: &'static str,
    #[serde(rename = "continue")]
    continue_turn: bool,
    #[serde(rename = "stopReason")]
    stop_reason: &'a str,
    #[serde(rename = "systemMessage")]
    system_message: &'a str,
}

#[derive(Serialize)]
struct CodexCompletionWarn<'a> {
    #[serde(rename = "systemMessage")]
    system_message: &'a str,
}

#[derive(Serialize)]
struct CodexPreDeny<'a> {
    #[serde(rename = "hookSpecificOutput")]
    hook_specific_output: HookSpecificPreDeny<'a>,
}

#[derive(Serialize)]
struct HookSpecificPreDeny<'a> {
    #[serde(rename = "hookEventName")]
    event_name: &'a str,
    #[serde(rename = "permissionDecision")]
    decision: &'a str,
    #[serde(rename = "permissionDecisionReason")]
    reason: &'a str,
}

#[derive(Serialize)]
struct CodexPreAllowWarn<'a> {
    #[serde(rename = "hookSpecificOutput")]
    hook_specific_output: HookSpecificPreAllow<'a>,
}

#[derive(Serialize)]
struct HookSpecificPreAllow<'a> {
    #[serde(rename = "hookEventName")]
    event_name: &'a str,
    #[serde(rename = "additionalContext")]
    context: &'a str,
}

#[derive(Serialize)]
struct CodexPostWarn<'a> {
    #[serde(rename = "systemMessage")]
    system_message: &'a str,
    #[serde(rename = "hookSpecificOutput")]
    hook_specific_output: HookSpecificContext<'a>,
}

#[derive(Serialize)]
struct CodexContext<'a> {
    #[serde(rename = "hookSpecificOutput")]
    hook_specific_output: HookSpecificContext<'a>,
}

#[derive(Serialize)]
struct HookSpecificContext<'a> {
    #[serde(rename = "hookEventName")]
    event_name: &'a str,
    #[serde(rename = "additionalContext")]
    context: &'a str,
}

fn codex_pre_deny(reason: &str) -> CodexPreDeny<'_> {
    CodexPreDeny {
        hook_specific_output: HookSpecificPreDeny {
            event_name: "PreToolUse",
            decision: "deny",
            reason,
        },
    }
}

fn codex_pre_allow_warn(context: &str) -> CodexPreAllowWarn<'_> {
    CodexPreAllowWarn {
        hook_specific_output: HookSpecificPreAllow {
            event_name: "PreToolUse",
            context,
        },
    }
}

fn codex_post_warn(context: &str) -> CodexPostWarn<'_> {
    CodexPostWarn {
        system_message: context,
        hook_specific_output: HookSpecificContext {
            event_name: "PostToolUse",
            context,
        },
    }
}

fn canonical_event(event: &str) -> &str {
    match event {
        "session-start" => "SessionStart",
        "user-prompt-submit" => "UserPromptSubmit",
        "pre-compact" => "PreCompact",
        "post-compact" => "PostCompact",
        "subagent-start" => "SubagentStart",
        other => other,
    }
}

fn codex_context<'a>(event_name: &'a str, context: &'a str) -> CodexContext<'a> {
    CodexContext {
        hook_specific_output: HookSpecificContext {
            event_name,
            context,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::super::CodexDecision;
    use super::*;

    #[test]
    fn render_pre_block() {
        let d = CodexDecision {
            block_messages: vec!["Found .unwrap() in src/".to_string()],
            warn_messages: Vec::new(),
            additional_context: String::new(),
            files: Vec::new(),
        };
        let json = render_codex_response("pre-tool-use", &d);
        let val: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(val["hookSpecificOutput"]["permissionDecision"], "deny");
    }

    #[test]
    fn render_pre_clean() {
        let d = CodexDecision {
            block_messages: Vec::new(),
            warn_messages: Vec::new(),
            additional_context: String::new(),
            files: Vec::new(),
        };
        let json = render_codex_response("pre-tool-use", &d);
        assert_eq!(json, "{}");
    }

    #[test]
    fn render_post_clean() {
        let d = CodexDecision {
            block_messages: Vec::new(),
            warn_messages: Vec::new(),
            additional_context: String::new(),
            files: Vec::new(),
        };
        let json = render_codex_response("post-tool-use", &d);
        assert_eq!(json, "{}");
    }

    #[test]
    fn render_post_warn() {
        let d = CodexDecision {
            block_messages: Vec::new(),
            warn_messages: vec!["Consider using ?".to_string()],
            additional_context: String::new(),
            files: Vec::new(),
        };
        let json = render_codex_response("post-tool-use", &d);
        let val: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(val["systemMessage"].is_string());
        assert_eq!(val["hookSpecificOutput"]["hookEventName"], "PostToolUse");
    }

    #[test]
    fn render_context_with_body() {
        let d = CodexDecision {
            block_messages: Vec::new(),
            warn_messages: Vec::new(),
            additional_context: "## Rules\n- rule-a".to_string(),
            files: Vec::new(),
        };
        let json = render_codex_response("session-start", &d);
        let val: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(val["hookSpecificOutput"]["hookEventName"], "SessionStart");
        assert!(val["hookSpecificOutput"]["additionalContext"].is_string());
    }

    #[test]
    fn render_unsupported_event_empty() {
        let d = CodexDecision {
            block_messages: Vec::new(),
            warn_messages: Vec::new(),
            additional_context: String::new(),
            files: Vec::new(),
        };
        let json = render_codex_response("some-unknown-event", &d);
        assert_eq!(json, "{}");
    }

    /// Permitted stdout keys per event, from SPEC-agent-lifecycle-events
    /// §"Host adapters / Codex CLI". Codex rejects the whole response on any
    /// unlisted key. PreCompact/PostCompact are deliberately absent: today's
    /// renderer violates their schemas (spec Adjacent finding 1, separate PR).
    fn permitted_keys(event: &str) -> &'static [&'static str] {
        const CTX: &[&str] = &[
            "continue",
            "stopReason",
            "suppressOutput",
            "systemMessage",
            "hookSpecificOutput",
        ];
        match event {
            "SessionStart" | "SubagentStart" => CTX,
            "UserPromptSubmit" => &[
                "continue",
                "stopReason",
                "suppressOutput",
                "systemMessage",
                "hookSpecificOutput",
                "decision",
                "reason",
            ],
            // The spec's table says "as today (decision, reason,
            // hookSpecificOutput)" for these two. `systemMessage` is added
            // deliberately, not copied: `render_post` already emits it and the
            // Codex PostToolUse schema permits it. Keep the deliberate widening
            // documented here rather than silently inside the table.
            "PreToolUse" | "PostToolUse" => {
                &["decision", "reason", "systemMessage", "hookSpecificOutput"]
            }
            "Stop" | "SubagentStop" => &[
                "continue",
                "decision",
                "reason",
                "stopReason",
                "suppressOutput",
                "systemMessage",
            ],
            "Interrupt" => &["systemMessage"],
            "SessionEnd" => &[],
            other => panic!("no permitted-key set for {other}"),
        }
    }

    fn decision(block: &[&str], warn: &[&str], context: &str) -> CodexDecision {
        CodexDecision {
            block_messages: block.iter().map(|s| s.to_string()).collect(),
            warn_messages: warn.iter().map(|s| s.to_string()).collect(),
            additional_context: context.to_string(),
            files: Vec::new(),
        }
    }

    #[test]
    fn every_event_response_stays_within_its_permitted_keys() {
        let cases = [
            decision(&[], &[], ""),
            decision(&["blocked"], &[], ""),
            decision(&[], &["warned"], ""),
            decision(&[], &[], "## Rules\n- rule-a"),
            decision(&["blocked"], &["warned"], "## Rules\n- rule-a"),
        ];
        for event in [
            "PreToolUse",
            "PostToolUse",
            "SessionStart",
            "UserPromptSubmit",
            "SubagentStart",
            "SubagentStop",
            "Stop",
            "Interrupt",
            "SessionEnd",
        ] {
            for d in &cases {
                let json = render_codex_response(event, d);
                let value: serde_json::Value =
                    serde_json::from_str(&json).unwrap_or_else(|e| panic!("{event}: {json}: {e}"));
                let obj = value
                    .as_object()
                    .unwrap_or_else(|| panic!("{event}: {json}"));
                for key in obj.keys() {
                    assert!(
                        permitted_keys(event).contains(&key.as_str()),
                        "{event} response key `{key}` is not permitted: {json}"
                    );
                }
            }
        }
    }

    #[test]
    fn completion_events_never_emit_hook_specific_output() {
        for event in ["Stop", "SubagentStop", "stop", "subagent-stop"] {
            for d in [
                decision(&["Low confidence for unit-1"], &[], ""),
                decision(&[], &["Medium confidence for unit-1"], ""),
                decision(&[], &[], "context that must be dropped"),
            ] {
                let json = render_codex_response(event, &d);
                // Structural, not substring: a `reason` string that merely
                // mentioned the word would false-fail a containment check.
                let v: serde_json::Value =
                    serde_json::from_str(&json).unwrap_or_else(|e| panic!("{event}: {json}: {e}"));
                assert!(v.get("hookSpecificOutput").is_none(), "{event}: {json}");
            }
        }
    }

    /// The permitted-key test alone would still pass if the block vanished
    /// entirely (`{}` is within every key set). This pins that a blocking
    /// decision actually blocks and carries its reason.
    #[test]
    fn a_blocking_completion_decision_still_reaches_the_host() {
        let d = decision(&["Low confidence for unit-1"], &[], "");
        for event in ["Stop", "SubagentStop"] {
            let json = render_codex_response(event, &d);
            assert_ne!(json, "{}", "{event} dropped the block");
            let v: serde_json::Value = serde_json::from_str(&json).expect("JSON");
            let obj = v.as_object().expect("object");
            for key in obj.keys() {
                assert!(
                    permitted_keys(event).contains(&key.as_str()),
                    "{event}: {key}: {json}"
                );
            }
            assert!(
                ["reason", "stopReason", "systemMessage"].iter().any(|k| {
                    obj.get(*k)
                        .and_then(|x| x.as_str())
                        .is_some_and(|s| s.contains("unit-1"))
                }),
                "{event}: the gate text must survive somewhere: {json}"
            );
        }
    }

    #[test]
    fn interrupt_and_session_end_render_empty() {
        let d = decision(&["ignored"], &["ignored"], "ignored");
        for event in ["Interrupt", "interrupt", "SessionEnd", "session-end"] {
            assert_eq!(render_codex_response(event, &d), "{}");
        }
    }
}
