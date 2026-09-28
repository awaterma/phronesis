use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::types::{InitError, InitOpts, InitReport};

pub(crate) fn read_json(path: &Path) -> Result<Option<Value>, InitError> {
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(path).map_err(|e| InitError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    if content.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(&content)?))
}

pub(crate) fn write_json(
    path: &Path,
    value: &Value,
    opts: &InitOpts,
    label: &str,
    report: &mut InitReport,
) -> Result<(), InitError> {
    let serialized = serde_json::to_string_pretty(value)?;
    if path.exists() {
        let existing = std::fs::read_to_string(path).unwrap_or_default();
        if existing.trim() == serialized.trim() {
            report
                .steps
                .push(format!("= {} unchanged (already up to date)", label));
            return Ok(());
        }
    }
    if opts.dry_run {
        report.steps.push(format!("+ would write {}", label));
        return Ok(());
    }
    ensure_parent(path)?;
    if path.exists() && opts.force {
        let bak = with_extension(path, "bak");
        let _ = std::fs::copy(path, &bak);
    }
    std::fs::write(path, &serialized).map_err(|e| InitError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    report.steps.push(format!("+ wrote {}", label));
    Ok(())
}

pub(crate) fn with_extension(path: &Path, ext: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".");
    s.push(ext);
    PathBuf::from(s)
}

pub(crate) fn ensure_parent(path: &Path) -> Result<(), InitError> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).map_err(|e| InitError::Io {
            path: p.display().to_string(),
            source: e,
        })?;
    }
    Ok(())
}

/// Insert (or replace) an entry for our matcher inside a hook array.
pub(crate) fn upsert_hook(settings: &mut Value, event: &str, new_entry: Value) {
    let hooks = settings.as_object_mut().and_then(|o| {
        o.entry("hooks".to_string())
            .or_insert_with(|| json!({}))
            .as_object_mut()
    });
    let Some(hooks) = hooks else { return };
    let arr = hooks.entry(event.to_string()).or_insert_with(|| json!([]));
    if !arr.is_array() {
        *arr = json!([]);
    }
    let our_matcher = new_entry["matcher"].as_str().map(String::from);
    let arr = arr.as_array_mut().unwrap();
    arr.retain(|m| m["matcher"].as_str().map(String::from) != our_matcher);
    arr.push(new_entry);
}

/// Subcommands Phronesis registers as hooks. An entry that names one of these
/// after a `phr-mcp` token is ours; anything else is the user's.
pub(crate) const PHRONESIS_HOOK_SUBCOMMANDS: [&str; 5] = [
    "session-context",
    "interaction-context",
    "claude-hook",
    "codex-hook",
    // Gemini registers the tool phases directly (Plan 4).
    "pre-check",
];

/// Is this hook command one of ours? The binary may be invoked bare
/// (`phr-mcp claude-hook Stop`), by absolute path
/// (`/usr/local/bin/phr-mcp session-context`), or through a wrapper — all three
/// are the same installation and must be upgraded in place rather than
/// duplicated, because two entries on one event mean two context renders per
/// prompt.
///
/// Token-based, not prefix-based: `phr-mcp-notify --all` is a different binary
/// whose name merely starts the same way, and it must survive `init`.
pub(crate) fn is_phronesis_hook_command(command: &str) -> bool {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    let Some(bin_at) = tokens
        .iter()
        .position(|t| *t == "phr-mcp" || t.rsplit(['/', '\\']).next() == Some("phr-mcp"))
    else {
        return false;
    };
    tokens[bin_at + 1..]
        .iter()
        .any(|t| PHRONESIS_HOOK_SUBCOMMANDS.contains(t) || *t == "post-check")
}

/// Replace Phronesis's own entry for a hook event regardless of its former
/// matcher or command, and leave every other hook alone. Matcher-keyed
/// `upsert_hook` both deletes a user's hook that happens to share our matcher
/// (spec §"Adjacent findings" 7) and leaves a stale entry behind whenever we
/// change our own matcher or command; keying on the command does neither.
/// Phronesis registers at most one entry per event, so dropping every entry of
/// ours and pushing one back is exact. Migrating the four pre-existing
/// matcher-keyed registrations to this is a follow-up.
pub(crate) fn upsert_hook_by_command(settings: &mut Value, event: &str, new_entry: Value) {
    let hooks = settings.as_object_mut().and_then(|o| {
        o.entry("hooks".to_string())
            .or_insert_with(|| json!({}))
            .as_object_mut()
    });
    let Some(hooks) = hooks else { return };
    let arr = hooks.entry(event.to_string()).or_insert_with(|| json!([]));
    if !arr.is_array() {
        *arr = json!([]);
    }
    let arr = arr.as_array_mut().unwrap();
    arr.retain(|entry| {
        !entry["hooks"].as_array().is_some_and(|handlers| {
            handlers.iter().any(|hook| {
                hook["command"]
                    .as_str()
                    .is_some_and(is_phronesis_hook_command)
            })
        })
    });
    arr.push(new_entry);
}

/// Replace only Phronesis's entry for a Codex event, regardless of its former
/// matcher. This migrates generated matcher changes without deleting unrelated
/// user hooks that happen to use the same matcher.
pub(crate) fn upsert_codex_hook(settings: &mut Value, event: &str, new_entry: Value) {
    let hooks = settings.as_object_mut().and_then(|o| {
        o.entry("hooks".to_string())
            .or_insert_with(|| json!({}))
            .as_object_mut()
    });
    let Some(hooks) = hooks else { return };
    let arr = hooks.entry(event.to_string()).or_insert_with(|| json!([]));
    if !arr.is_array() {
        *arr = json!([]);
    }
    let command = format!("phr-mcp codex-hook {event}");
    let arr = arr.as_array_mut().unwrap();
    arr.retain(|entry| {
        !entry["hooks"]
            .as_array()
            .is_some_and(|handlers| handlers.iter().any(|hook| hook["command"] == command))
    });
    arr.push(new_entry);
}
