use std::path::Path;

use serde_json::json;

use super::json_helpers::*;
use super::types::*;

pub(super) fn write_settings(
    root: &Path,
    opts: &InitOpts,
    report: &mut InitReport,
) -> Result<(), InitError> {
    let path = root.join(".claude").join("settings.local.json");
    let existing = read_json(&path)?;

    let mut settings = existing
        .clone()
        .unwrap_or_else(|| json!({"hooks": {"PreToolUse": [], "PostToolUse": []}}));

    let our_entry = |cmd: &str| {
        json!({
            "matcher": "Edit|Write|MultiEdit|Bash",
            "hooks": [{"type":"command","command":cmd}]
        })
    };

    upsert_hook(&mut settings, "PreToolUse", our_entry("phr-mcp pre-check"));
    upsert_hook(
        &mut settings,
        "PostToolUse",
        our_entry("phr-mcp post-check"),
    );

    // Lifecycle + context hooks, all empty-matcher (fire on every event) and
    // all served by one adapter. Command-keyed replacement so a user's own
    // empty-matcher hook on the same event survives `phr-mcp init`.
    let context_entry = |cmd: &str| {
        json!({
            "matcher": "",
            "hooks": [{"type":"command","command":cmd}]
        })
    };
    for event in [
        "SessionStart",
        "SessionEnd",
        "UserPromptSubmit",
        "SubagentStart",
        "SubagentStop",
        "Stop",
    ] {
        upsert_hook_by_command(
            &mut settings,
            event,
            context_entry(&format!("phr-mcp claude-hook {event}")),
        );
    }

    write_json(&path, &settings, opts, "settings.local.json", report)?;
    Ok(())
}

pub(super) fn write_mcp_json(
    root: &Path,
    opts: &InitOpts,
    report: &mut InitReport,
) -> Result<(), InitError> {
    let path = root.join(".mcp.json");
    let existing = read_json(&path)?;
    let mut mcp = existing.unwrap_or_else(|| json!({"mcpServers": {}}));
    if !mcp.is_object() {
        mcp = json!({"mcpServers": {}});
    }
    let servers = mcp
        .as_object_mut()
        .expect("mcp was just reset to an object when it was not one")
        .entry("mcpServers".to_string())
        .or_insert_with(|| json!({}));
    if !servers.is_object() {
        *servers = json!({});
    }
    servers
        .as_object_mut()
        .expect("servers was just reset to an object when it was not one")
        .insert(
            "phronesis".to_string(),
            json!({
                "command": "phr-mcp",
                "args": ["serve"]
            }),
        );

    write_json(&path, &mcp, opts, ".mcp.json", report)?;
    Ok(())
}

pub(super) fn write_gemini_settings(
    root: &Path,
    opts: &InitOpts,
    report: &mut InitReport,
) -> Result<(), InitError> {
    let path = root.join(".gemini").join("settings.json");
    let existing = read_json(&path)?;
    let mut settings = existing.unwrap_or_else(|| json!({}));
    if !settings.is_object() {
        settings = json!({});
    }

    // MCP server registration
    let servers = settings
        .as_object_mut()
        .expect("settings was just reset to an object when it was not one")
        .entry("mcpServers".to_string())
        .or_insert_with(|| json!({}));
    if !servers.is_object() {
        *servers = json!({});
    }
    servers
        .as_object_mut()
        .expect("servers was just reset to an object when it was not one")
        .insert(
            "phronesis".to_string(),
            json!({"command": "phr-mcp", "args": ["serve"]}),
        );

    // BeforeTool / AfterTool hooks. Gemini treats `matcher` as an unanchored
    // regex, so the previous `replace|write_file|run_shell_command` matched
    // any tool whose name merely contained one of those words. `invoke_agent`
    // joins the list because Gemini has no sub-agent event: pre-check and
    // post-check derive subagent_start / subagent_stop from that tool.
    let hook_entry = |cmd: &str| {
        json!({
            "matcher": "^(replace|write_file|run_shell_command|invoke_agent)$",
            "hooks": [{"type": "command", "command": cmd}]
        })
    };
    upsert_hook_by_command(&mut settings, "BeforeTool", hook_entry("phr-mcp pre-check"));
    upsert_hook_by_command(&mut settings, "AfterTool", hook_entry("phr-mcp post-check"));

    // Lifecycle + context hooks. An empty matcher fires on every event.
    // BeforeAgent is Gemini's UserPromptSubmit and AfterAgent is its Stop;
    // AfterAgent does not fire on interrupt, which is exactly what the
    // `open_turn` inference in `lifecycle::state::classify_prompt` keys on.
    let lifecycle_entry = |event: &str| {
        json!({
            "matcher": "",
            "hooks": [{"type": "command", "command": format!("phr-mcp claude-hook {event}")}]
        })
    };
    for event in ["SessionStart", "SessionEnd", "BeforeAgent", "AfterAgent"] {
        upsert_hook_by_command(&mut settings, event, lifecycle_entry(event));
    }

    // Clean up legacy BeforeModelRequest hook if present
    if let Some(hooks) = settings.get_mut("hooks").and_then(|h| h.as_object_mut()) {
        hooks.remove("BeforeModelRequest");
    }

    write_json(&path, &settings, opts, ".gemini/settings.json", report)?;
    report.steps.push(
        "  note: Gemini HTML-escapes additionalContext (< and > reach the model as entities) \
         and skips project hooks until the folder is trusted."
            .to_string(),
    );
    Ok(())
}

pub(super) fn write_codex_hooks(
    root: &Path,
    opts: &InitOpts,
    report: &mut InitReport,
) -> Result<(), InitError> {
    let path = root.join(".codex").join("hooks.json");
    let existing = read_json(&path)?;
    let mut settings = existing.unwrap_or_else(|| json!({}));
    if !settings.is_object() {
        settings = json!({});
    }
    let tool_entry = |event: &str| {
        json!({
            "matcher": "^(Bash|apply_patch)$",
            "hooks": [{"type": "command", "command": format!("phr-mcp codex-hook {event}")}]
        })
    };
    upsert_codex_hook(&mut settings, "PreToolUse", tool_entry("PreToolUse"));
    upsert_codex_hook(&mut settings, "PostToolUse", tool_entry("PostToolUse"));
    for (event, matcher) in [
        // Codex matchers are exact alternations; "startup|resume|clear" gave
        // compact and fork sessions no context. Empty matches every source.
        ("SessionStart", ""),
        ("SessionEnd", ""),
        ("UserPromptSubmit", ""),
        ("PreCompact", "manual|auto"),
        ("PostCompact", "manual|auto"),
        ("SubagentStart", ""),
        ("SubagentStop", ""),
        ("Stop", ""),
        // Interrupt ignores `matcher` entirely; the empty value is documentation.
        ("Interrupt", ""),
    ] {
        upsert_codex_hook(
            &mut settings,
            event,
            json!({
                "matcher": matcher,
                "hooks": [{"type": "command", "command": format!("phr-mcp codex-hook {event}")}]
            }),
        );
    }
    write_json(&path, &settings, opts, ".codex/hooks.json", report)?;
    report.warnings.push(
        "Codex skips new or changed project hooks until you review and trust them with `/hooks`."
            .to_string(),
    );
    Ok(())
}

pub(super) fn write_codex_config(
    root: &Path,
    opts: &InitOpts,
    report: &mut InitReport,
) -> Result<(), InitError> {
    let path = root.join(".codex").join("config.toml");
    let existing = if path.exists() {
        std::fs::read_to_string(&path).map_err(|e| InitError::Io {
            path: path.display().to_string(),
            source: e,
        })?
    } else {
        String::new()
    };
    if existing
        .lines()
        .any(|line| line.trim() == "[mcp_servers.phronesis]")
    {
        report
            .steps
            .push("= .codex/config.toml already registers `phronesis` (no changes)".to_string());
        return Ok(());
    }
    let separator = if existing.is_empty() || existing.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    let updated = format!(
        "{existing}{separator}\n[mcp_servers.phronesis]\ncommand = \"phr-mcp\"\nargs = [\"serve\"]\n"
    );
    if opts.dry_run {
        report
            .steps
            .push("+ would register `phronesis` in .codex/config.toml".to_string());
        return Ok(());
    }
    ensure_parent(&path)?;
    std::fs::write(&path, updated).map_err(|e| InitError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    report
        .steps
        .push("+ registered `phronesis` in .codex/config.toml".to_string());
    Ok(())
}
