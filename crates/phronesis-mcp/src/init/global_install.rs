use std::path::{Path, PathBuf};

use serde_json::json;

use super::binary_on_path;
use super::json_helpers::*;
use super::types::*;

/// Path to Claude Code's user-level config (`~/.claude.json` on Unix). Returns
/// None when `$HOME` isn't set (rare; only happens in degenerate environments).
pub fn user_claude_config_path() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".claude.json"))
}

/// Path to Gemini CLI's user-level settings (`~/.gemini/settings.json`).
/// Returns None when `$HOME` isn't set.
pub fn user_gemini_config_path() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".gemini").join("settings.json"))
}

/// Represents one user-level MCP config target (e.g. `~/.claude.json` or
/// `~/.gemini/settings.json`). Used by `install_one_target` and
/// `uninstall_one_target` to eliminate duplicated install/uninstall logic.
pub(super) struct McpTarget<'a> {
    pub(super) path: PathBuf,
    /// Human-readable label for report messages, e.g. `"~/.claude.json"`.
    pub(super) label: &'a str,
}

/// Load `target.path`, upsert `mcpServers.phronesis`, back up and write.
/// Emits step messages on `report` using `target.label`.
pub(super) fn install_one_target(
    target: &McpTarget<'_>,
    dry_run: bool,
    report: &mut InitReport,
) -> Result<(), InitError> {
    let existing = read_json(&target.path)?;
    let mut config = existing.unwrap_or_else(|| json!({}));
    if !config.is_object() {
        config = json!({});
    }

    let servers = config
        .as_object_mut()
        .expect("config was just reset to an object when it was not one")
        .entry("mcpServers".to_string())
        .or_insert_with(|| json!({}));
    if !servers.is_object() {
        *servers = json!({});
    }

    let our_entry = json!({"command": "phr-mcp", "args": ["serve"]});
    if servers.get("phronesis") == Some(&our_entry) {
        report.steps.push(format!(
            "= {} already registers `phronesis` (no changes)",
            target.label
        ));
    } else {
        servers
            .as_object_mut()
            .expect("servers was just reset to an object when it was not one")
            .insert("phronesis".to_string(), our_entry);

        if dry_run {
            report.steps.push(format!(
                "+ would register `phronesis` in {}::mcpServers",
                target.label
            ));
        } else {
            ensure_parent(&target.path)?;
            if target.path.exists() {
                let bak = with_extension(&target.path, "bak");
                std::fs::copy(&target.path, &bak).map_err(|e| InitError::Io {
                    path: bak.display().to_string(),
                    source: e,
                })?;
            }
            let serialized = serde_json::to_string_pretty(&config)?;
            std::fs::write(&target.path, serialized).map_err(|e| InitError::Io {
                path: target.path.display().to_string(),
                source: e,
            })?;
            report.steps.push(format!(
                "+ registered `phronesis` in {}",
                target.path.display()
            ));
        }
    }
    Ok(())
}

/// Load `target.path`, remove `mcpServers.phronesis`, back up and write.
/// Emits step messages on `report` using `target.label`.
pub(super) fn uninstall_one_target(
    target: &McpTarget<'_>,
    dry_run: bool,
    report: &mut InitReport,
) -> Result<(), InitError> {
    let existing = read_json(&target.path)?;
    match existing {
        None => {
            report.steps.push(format!(
                "= {} doesn't exist (nothing to uninstall)",
                target.label
            ));
        }
        Some(mut config) => {
            let removed = config
                .as_object_mut()
                .and_then(|o| o.get_mut("mcpServers"))
                .and_then(|s| s.as_object_mut())
                .and_then(|servers| servers.remove("phronesis"))
                .is_some();

            if !removed {
                report.steps.push(format!(
                    "= {} has no `phronesis` entry (nothing to do)",
                    target.label
                ));
            } else if dry_run {
                report.steps.push(format!(
                    "- would remove `phronesis` from {}::mcpServers",
                    target.label
                ));
            } else {
                let bak = with_extension(&target.path, "bak");
                std::fs::copy(&target.path, &bak).map_err(|e| InitError::Io {
                    path: bak.display().to_string(),
                    source: e,
                })?;
                let serialized = serde_json::to_string_pretty(&config)?;
                std::fs::write(&target.path, serialized).map_err(|e| InitError::Io {
                    path: target.path.display().to_string(),
                    source: e,
                })?;
                report.steps.push(format!(
                    "- removed `phronesis` from {}",
                    target.path.display()
                ));
            }
        }
    }
    Ok(())
}

/// Register the phronesis MCP server at user scope (in `~/.claude.json` and
/// `~/.gemini/settings.json`). After this runs, every project the user opens
/// in Claude Code or Gemini CLI can call `mcp__phronesis__*` tools without
/// needing a per-project `.mcp.json`.
///
/// Idempotent: if the entry is already present and identical, this is a no-op.
/// Non-destructive: other `mcpServers` entries and all other top-level keys are
/// preserved untouched.
pub fn install_globally(dry_run: bool) -> Result<InitReport, InitError> {
    let home = std::env::var("HOME")
        .map_err(|_| InitError::NoSuchPath("HOME environment variable not set".to_string()))?;
    install_globally_with_home(Path::new(&home), dry_run)
}

/// Inner implementation of `install_globally` that accepts an explicit home
/// directory, enabling tests to call it without touching environment variables.
pub fn install_globally_with_home(home: &Path, dry_run: bool) -> Result<InitReport, InitError> {
    let mut report = InitReport::default();

    if !binary_on_path("phr-mcp") {
        report.warnings.push(
            "`phr-mcp` not found on PATH. The user-level registration \
             still records the binary name; install via `cargo install --path .`."
                .to_string(),
        );
    }

    let targets = [
        McpTarget {
            path: home.join(".claude.json"),
            label: "~/.claude.json",
        },
        McpTarget {
            path: home.join(".gemini").join("settings.json"),
            label: "~/.gemini/settings.json",
        },
    ];
    for target in &targets {
        install_one_target(target, dry_run, &mut report)?;
    }

    Ok(report)
}

/// Remove the phronesis MCP server from `~/.claude.json::mcpServers` and
/// `~/.gemini/settings.json::mcpServers`. Idempotent (does nothing if no entry
/// is present). Doesn't touch project-level config.
pub fn uninstall_globally(dry_run: bool) -> Result<InitReport, InitError> {
    let home = std::env::var("HOME")
        .map_err(|_| InitError::NoSuchPath("HOME environment variable not set".to_string()))?;
    uninstall_globally_with_home(Path::new(&home), dry_run)
}

/// Inner implementation of `uninstall_globally` that accepts an explicit home
/// directory, enabling tests to call it without touching environment variables.
pub fn uninstall_globally_with_home(home: &Path, dry_run: bool) -> Result<InitReport, InitError> {
    let mut report = InitReport::default();

    let targets = [
        McpTarget {
            path: home.join(".claude.json"),
            label: "~/.claude.json",
        },
        McpTarget {
            path: home.join(".gemini").join("settings.json"),
            label: "~/.gemini/settings.json",
        },
    ];
    for target in &targets {
        uninstall_one_target(target, dry_run, &mut report)?;
    }

    Ok(report)
}
