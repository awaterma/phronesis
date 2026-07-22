//! `phr-mcp init` — one-command setup for a project.
//!
//! Writes (or merges) the config files phr-mcp needs:
//!
//! - `.claude/settings.local.json`             — hook registrations
//! - `.mcp.json`                                — MCP server registration
//! - `.gemini/settings.json`                    — Gemini CLI MCP + hooks
//! - `.phronesis/rules.json`                    — starter rule pack
//! - `.phronesis/durable.md`                    — re-injected directives
//! - `.phronesis/wiki/decisions/README.md`     — ADR scaffold
//! - `.gitignore`                                — log/backup paths + wiki carveout
//!
//! Idempotent and non-destructive by default. Existing permissions, hooks,
//! and MCP servers are preserved; only our entries are added or refreshed.
//! Existing rules files are left alone unless `--force` is set.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use thiserror::Error;

/// A starter rule pack. Packs are composable — caller picks a comma-separated
/// list and `compose_packs` merges them, deduping by rule_id.
///
/// `Llm` is independent of language: it carries the "stop deflecting" rules
/// that catch phrases like "pre-existing issue" wherever they appear. The
/// language packs (`Rust`, `Python`, `TypeScript`) carry only language-specific
/// enforcement; they don't bundle the LLM-behavior rules.
///
/// `None` exists for users who want hooks wired up but will author their own
/// rules from scratch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pack {
    Llm,
    Rust,
    Rhai,
    Python,
    TypeScript,
    Swift,
    Confidence,
    Journey,
    None,
}

impl Pack {
    fn parse(s: &str) -> Result<Self, InitError> {
        match s.trim().to_lowercase().as_str() {
            // `llm` and the deprecated alias `minimal` (pre-pack-split naming)
            "llm" | "minimal" => Ok(Self::Llm),
            "rust" | "rs" => Ok(Self::Rust),
            "rhai" => Ok(Self::Rhai),
            "python" | "py" => Ok(Self::Python),
            "typescript" | "ts" | "javascript" | "js" => Ok(Self::TypeScript),
            "swift" => Ok(Self::Swift),
            "confidence" => Ok(Self::Confidence),
            "journey" => Ok(Self::Journey),
            "none" => Ok(Self::None),
            other => Err(InitError::UnknownPack(other.to_string())),
        }
    }

    pub(crate) fn rules(self) -> Value {
        match self {
            Self::None => json!({"rules": []}),
            Self::Llm => deflection_rules(),
            Self::Rust => rust_rules(),
            Self::Rhai => rhai_rules(),
            Self::Python => python_rules(),
            Self::TypeScript => typescript_rules(),
            Self::Swift => swift_rules(),
            Self::Confidence => confidence_rules(),
            // Journey ships no starter rules in v1 — the project defines its
            // own risk surface via `journey.json` and adds journey_* rules to
            // rules.json. The pack's contribution is the journey.json starter
            // config + gitignore carveout, written by `write_journey_scaffold`.
            Self::Journey => json!({"rules": []}),
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Llm => "llm",
            Self::Rust => "rust",
            Self::Rhai => "rhai",
            Self::Python => "python",
            Self::TypeScript => "typescript",
            Self::Swift => "swift",
            Self::Confidence => "confidence",
            Self::Journey => "journey",
            Self::None => "none",
        }
    }
}

/// Parse a comma-separated pack list (e.g. `"llm,rust"`). Whitespace tolerated.
/// Empty input → just `[Llm]` (the default). Duplicates are deduped.
pub fn parse_packs(s: &str) -> Result<Vec<Pack>, InitError> {
    if s.trim().is_empty() {
        return Ok(vec![Pack::Llm]);
    }
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for part in s.split(',') {
        let pack = Pack::parse(part)?;
        if seen.insert(pack) {
            out.push(pack);
        }
    }
    Ok(out)
}

/// Compose multiple packs into a single rules.json value, deduping rules by ID.
/// Earlier packs take precedence on ID collision (first-write-wins).
pub fn compose_packs(packs: &[Pack]) -> Value {
    let pack_values: Vec<Value> = packs.iter().map(|p| p.rules()).collect();
    let mut by_id = std::collections::HashMap::<String, Value>::new();
    let mut order: Vec<String> = Vec::new();
    for pack in &pack_values {
        if let Some(rules) = pack["rules"].as_array() {
            for rule in rules {
                if let Some(id) = rule["id"].as_str() {
                    let key = id.to_string();
                    if let std::collections::hash_map::Entry::Vacant(e) = by_id.entry(key.clone()) {
                        order.push(key);
                        e.insert(rule.clone());
                    }
                }
            }
        }
    }
    let merged: Vec<Value> = order
        .into_iter()
        .filter_map(|id| by_id.remove(&id))
        .collect();
    json!({"rules": merged})
}

#[derive(Debug, Error)]
pub enum InitError {
    #[error(
        "unknown pack `{0}`; valid: llm, rust, rhai, python, typescript, swift, confidence, journey, none"
    )]
    UnknownPack(String),
    #[error("project root does not exist: {0}")]
    NoSuchPath(String),
    #[error("project root is not a directory: {0}")]
    NotADirectory(String),
    #[error("io error on {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

pub struct InitOpts {
    pub project_root: PathBuf,
    pub packs: Vec<Pack>,
    pub force: bool,
    pub dry_run: bool,
    /// When true, only touch `.phronesis/rules.json` — leave the hook config,
    /// MCP registration, and .gitignore alone. Pairs naturally with `--force`
    /// for "refresh just the rules pack" workflows.
    pub rules_only: bool,
    /// Only touch hook config (`.claude/settings.local.json`, `.mcp.json`,
    /// `.gemini/settings.json`). Skip rules.json and .gitignore. Use to
    /// refresh hook wiring on an existing project without disturbing its
    /// rules pack.
    pub hooks_only: bool,
}

#[derive(Debug, Default)]
pub struct InitReport {
    pub steps: Vec<String>,
    pub warnings: Vec<String>,
}

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
struct McpTarget<'a> {
    path: PathBuf,
    /// Human-readable label for report messages, e.g. `"~/.claude.json"`.
    label: &'a str,
}

/// Load `target.path`, upsert `mcpServers.phronesis`, back up and write.
/// Emits step messages on `report` using `target.label`.
fn install_one_target(
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
        .unwrap()
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
            .unwrap()
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
fn uninstall_one_target(
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

/// Run the installer. Returns a structured report; the CLI layer prints it.
pub fn run(opts: InitOpts) -> Result<InitReport, InitError> {
    let root = canonicalize_root(&opts.project_root)?;
    let mut report = InitReport::default();

    if !binary_on_path("phr-mcp") {
        report.warnings.push(
            "`phr-mcp` not found on PATH. Hooks won't function until it is. \
             Install via `cargo install --path .` from the phr-mcp repo."
                .to_string(),
        );
    }

    if !opts.rules_only {
        write_settings(&root, &opts, &mut report)?;
        write_mcp_json(&root, &opts, &mut report)?;
        write_gemini_settings(&root, &opts, &mut report)?;
        write_codex_hooks(&root, &opts, &mut report)?;
        write_codex_config(&root, &opts, &mut report)?;
    }
    if !opts.hooks_only {
        write_rules_file(&root, &opts, &mut report)?;
        write_durable_md(&root, &opts, &mut report)?;
        write_wiki_scaffold(&root, &opts, &mut report)?;
        write_confidence_scaffold(&root, &opts, &mut report)?;
        write_journey_scaffold(&root, &opts, &mut report)?;
    }
    if !opts.rules_only && !opts.hooks_only {
        update_gitignore(&root, &opts, &mut report)?;
    }

    Ok(report)
}

// ─────────────────────────────────────────────────────────────────────
// Path + binary checks
// ─────────────────────────────────────────────────────────────────────

fn canonicalize_root(p: &Path) -> Result<PathBuf, InitError> {
    if !p.exists() {
        return Err(InitError::NoSuchPath(p.display().to_string()));
    }
    if !p.is_dir() {
        return Err(InitError::NotADirectory(p.display().to_string()));
    }
    p.canonicalize().map_err(|e| InitError::Io {
        path: p.display().to_string(),
        source: e,
    })
}

fn binary_on_path(name: &str) -> bool {
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    path.split(if cfg!(windows) { ';' } else { ':' })
        .any(|dir| Path::new(dir).join(name).is_file())
}

// ─────────────────────────────────────────────────────────────────────
// File writers
// ─────────────────────────────────────────────────────────────────────

fn write_settings(root: &Path, opts: &InitOpts, report: &mut InitReport) -> Result<(), InitError> {
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

    // Context-injection hooks. Empty matcher → fires on every event.
    // SessionStart runs once per session; UserPromptSubmit fires every turn.
    let context_entry = |cmd: &str| {
        json!({
            "matcher": "",
            "hooks": [{"type":"command","command":cmd}]
        })
    };
    upsert_hook(
        &mut settings,
        "SessionStart",
        context_entry("phr-mcp session-context"),
    );
    upsert_hook(
        &mut settings,
        "UserPromptSubmit",
        context_entry("phr-mcp turn-context"),
    );

    write_json(&path, &settings, opts, "settings.local.json", report)?;
    Ok(())
}

fn write_mcp_json(root: &Path, opts: &InitOpts, report: &mut InitReport) -> Result<(), InitError> {
    let path = root.join(".mcp.json");
    let existing = read_json(&path)?;
    let mut mcp = existing.unwrap_or_else(|| json!({"mcpServers": {}}));
    if !mcp.is_object() {
        mcp = json!({"mcpServers": {}});
    }
    let servers = mcp
        .as_object_mut()
        .unwrap()
        .entry("mcpServers".to_string())
        .or_insert_with(|| json!({}));
    if !servers.is_object() {
        *servers = json!({});
    }
    servers.as_object_mut().unwrap().insert(
        "phronesis".to_string(),
        json!({
            "command": "phr-mcp",
            "args": ["serve"]
        }),
    );

    write_json(&path, &mcp, opts, ".mcp.json", report)?;
    Ok(())
}

fn write_gemini_settings(
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
        .unwrap()
        .entry("mcpServers".to_string())
        .or_insert_with(|| json!({}));
    if !servers.is_object() {
        *servers = json!({});
    }
    servers.as_object_mut().unwrap().insert(
        "phronesis".to_string(),
        json!({"command": "phr-mcp", "args": ["serve"]}),
    );

    // BeforeTool / AfterTool hooks
    let hook_entry = |cmd: &str| {
        json!({
            "matcher": "replace|write_file|run_shell_command",
            "hooks": [{"type": "command", "command": cmd}]
        })
    };
    upsert_hook(&mut settings, "BeforeTool", hook_entry("phr-mcp pre-check"));
    upsert_hook(&mut settings, "AfterTool", hook_entry("phr-mcp post-check"));

    // Context-injection hooks. Same shape as the Claude wiring — empty
    // matcher means fire on every event. SessionStart matches Claude's
    // event name; BeforeAgent is Gemini's per-turn equivalent of
    // Claude's UserPromptSubmit.
    let context_entry = |cmd: &str| {
        json!({
            "matcher": "",
            "hooks": [{"type": "command", "command": cmd}]
        })
    };
    upsert_hook(
        &mut settings,
        "SessionStart",
        context_entry("phr-mcp session-context"),
    );
    upsert_hook(
        &mut settings,
        "BeforeAgent",
        context_entry("phr-mcp turn-context"),
    );

    // Clean up legacy BeforeModelRequest hook if present
    if let Some(hooks) = settings.get_mut("hooks").and_then(|h| h.as_object_mut()) {
        hooks.remove("BeforeModelRequest");
    }

    write_json(&path, &settings, opts, ".gemini/settings.json", report)?;
    Ok(())
}

fn write_codex_hooks(
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
    upsert_hook(&mut settings, "PreToolUse", tool_entry("PreToolUse"));
    upsert_hook(&mut settings, "PostToolUse", tool_entry("PostToolUse"));
    for event in [
        "SessionStart",
        "UserPromptSubmit",
        "PreCompact",
        "PostCompact",
        "SubagentStart",
    ] {
        upsert_hook(
            &mut settings,
            event,
            json!({
                "matcher": "",
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

fn write_codex_config(
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

fn write_rules_file(
    root: &Path,
    opts: &InitOpts,
    report: &mut InitReport,
) -> Result<(), InitError> {
    let path = root.join(".phronesis").join("rules.json");

    if path.exists() && !opts.force {
        report.steps.push(
            "= .phronesis/rules.json already exists — leaving unchanged (re-run with --force to overwrite)"
                .to_string(),
        );
        return Ok(());
    }

    let rules = compose_packs(&opts.packs);
    let count = rules["rules"].as_array().map(Vec::len).unwrap_or(0);
    let pack_labels: Vec<&str> = opts.packs.iter().map(|p| p.label()).collect();
    let label = pack_labels.join("+");

    if opts.dry_run {
        report.steps.push(format!(
            "+ would write .phronesis/rules.json with {} {} rule(s)",
            count, label
        ));
        return Ok(());
    }

    ensure_parent(&path)?;
    // Back up the prior rules file (force-overwrite path) so users can recover
    // if the starter pack wasn't what they wanted.
    if path.exists() {
        let bak = with_extension(&path, "bak");
        std::fs::copy(&path, &bak).map_err(|e| InitError::Io {
            path: bak.display().to_string(),
            source: e,
        })?;
    }
    let content = serde_json::to_string_pretty(&rules)?;
    std::fs::write(&path, content).map_err(|e| InitError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    report.steps.push(format!(
        "+ wrote .phronesis/rules.json ({} {} rule(s))",
        count, label
    ));
    Ok(())
}

/// Default `.phronesis/durable.md` template. Re-injected into the
/// model's context at every SessionStart and UserPromptSubmit, so the
/// nudges below survive context-window decay. Tuned to be terse — the
/// whole file ships in every turn, so spend tokens carefully.
const DEFAULT_DURABLE_MD: &str = r#"# Durable Directives

Re-injected at every SessionStart and UserPromptSubmit by phronesis.
Contents do not fade with context-window compression.

## Drift discipline

Three heuristic tools surface the gap between prose guidance and
enforced rules. They are cheap, deterministic, and worth running
whenever the user asks about rules, memory, durable guidance, or
project conventions:

- `mcp__phronesis__get_claude_md_drift` — bullets in `CLAUDE.md`
  that no current rule covers. Candidates for rule porting or for
  marking "non-lintable by design."
- `mcp__phronesis__get_memory_drift` — entries in the auto-memory
  store that have no matching rule or `durable.md` paragraph.
  Actionable entries (named tool calls / commands) should become
  rules; ambient ones (project-shared prose) should be added here.
- `mcp__phronesis__get_wiki_drift` — ADR-style decisions under
  `.phronesis/wiki/decisions/` that no rule enforces. Decisions
  with explicit `enforces: [rule-id]` frontmatter resolve
  deterministically; others fall through to Jaccard matching.

Treat the output as a triage list, not authoritative ground truth —
the scoring is token-overlap Jaccard, no semantic match. When the
user says "remember X" or "make a rule for X", check drift first to
see whether the gap is real.

## Participatory governance

The model is both governed by rules and a participant in rule
evolution. Three workflows close the loop:

### Remember → decide → enforce

When the user says "remember X" or "make a rule for X":

1. Check drift tools — is the gap real?
2. Scaffold a decision: `phr-mcp decision new <slug>`
3. Fill in Context, Decision, Enforcement, Consequences
4. If enforceable (code-shape, command pattern):
   - Propose a rule using available predicates
     (`new_content_contains`, `file_path_matches`,
     `file_extension_is`, etc.)
   - Write it to `.phronesis/rules.json`
   - Wire `enforces: [rule-id]` in the decision frontmatter
5. If not enforceable (process, naming, social):
   - Note in Enforcement that no automated rule is possible
   - Offer to add prose guidance to this file instead
6. Ask the human to approve before committing

### Friction-driven proposals

When a rule blocks you 3+ times in the same session for the same
pattern, pause and assess:

- Use `get_action_log` with `only_nonzero_exit: true` to review
- If the rule scope is too broad (legitimate code keeps tripping
  it): propose a decision page that refines the scope — narrower
  `file_path_matches`, an exclusion, a predicate change. Present
  the proposal to the human.
- If you keep hitting it legitimately: the rule is working. Adjust
  your approach, don't propose weakening enforcement.

### Cross-session knowledge transfer

When you discover something significant — a bug pattern, a design
insight, a rollout lesson — consider writing a decision page. ADR
pages in `.phronesis/wiki/decisions/` travel with the repo and are
available to future sessions. This turns a session-local discovery
into durable project knowledge. Ask the human before writing —
not every insight warrants a formal decision.

## Project-specific guidance

(Add team-specific directives below. Anything written here is
re-read by the model every turn and so is safe from context-window
fade.)
"#;

fn write_durable_md(
    root: &Path,
    opts: &InitOpts,
    report: &mut InitReport,
) -> Result<(), InitError> {
    let path = root.join(".phronesis").join("durable.md");

    if path.exists() {
        report.steps.push(
            "= .phronesis/durable.md already exists — leaving unchanged (edit in place to customize)"
                .to_string(),
        );
        return Ok(());
    }

    if opts.dry_run {
        report.steps.push(
            "+ would write .phronesis/durable.md (default drift-discipline notes)".to_string(),
        );
        return Ok(());
    }

    ensure_parent(&path)?;
    std::fs::write(&path, DEFAULT_DURABLE_MD).map_err(|e| InitError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    report
        .steps
        .push("+ wrote .phronesis/durable.md (default drift-discipline notes)".to_string());
    Ok(())
}

const WIKI_DECISIONS_README: &str = "\
# `.phronesis/wiki/decisions/`

ADR-style decision pages. Each file is one decision (e.g. \
`2026-05-29-error-handling-policy.md`). The first block is YAML \
frontmatter (`id`, `date`, `status`, optional `enforces`, \
`superseded_by`, `tags`). The body uses Context / Decision / \
Enforcement / Consequences sections.

Run `phr-mcp wiki-drift` to see which decisions lack rule coverage.
Create new pages with `phr-mcp decision new <slug>`.

This directory is tracked in git (un-ignored from the broader \
`.phronesis/` ignore) because decisions are project knowledge. \
The rest of `.phronesis/` (rules.json, log.jsonl, etc.) stays \
gitignored.
";

fn write_wiki_scaffold(
    root: &Path,
    opts: &InitOpts,
    report: &mut InitReport,
) -> Result<(), InitError> {
    let dir = root.join(".phronesis").join("wiki").join("decisions");
    let readme = dir.join("README.md");

    if readme.exists() {
        report.steps.push(
            "= .phronesis/wiki/decisions/README.md already exists — leaving unchanged".to_string(),
        );
        return Ok(());
    }

    if opts.dry_run {
        report
            .steps
            .push("+ would create .phronesis/wiki/decisions/ + README.md".to_string());
        return Ok(());
    }

    std::fs::create_dir_all(&dir).map_err(|e| InitError::Io {
        path: dir.display().to_string(),
        source: e,
    })?;
    std::fs::write(&readme, WIKI_DECISIONS_README).map_err(|e| InitError::Io {
        path: readme.display().to_string(),
        source: e,
    })?;
    report
        .steps
        .push("+ created .phronesis/wiki/decisions/ + README.md".to_string());
    Ok(())
}

/// Default `.phronesis/confidence.json` — the opt-in marker that activates
/// confidence scoring for the project.
const CONFIDENCE_JSON: &str = "{\n  \"version\": 1\n}\n";

/// Default `.phronesis/bugs.json` — the known-bug registry, empty to start.
/// Each entry: `{ "bug_id": "...", "test": "module::test_name", "status": "open" }`.
const CONFIDENCE_BUGS_JSON: &str = "[]\n";

/// Example `.phronesis/toolchains.json` written with the confidence pack:
/// two real project-def examples (pytest, tsc) proving toolchain neutrality.
/// Matchers are head-anchored: recognition runs per command segment (split
/// on `&&`, `||`, `;`, `|`, newlines; leading env assignments stripped), so
/// `^pytest` matches `cd api && pytest -q` but not `echo pytest`.
/// Users edit/extend in place; `init` never overwrites it.
const TOOLCHAINS_JSON: &str = r#"[
  {
    "_doc": "Recognition regex is `matches`, applied to each command segment (split on &&, ||, ;, |, newlines; leading NAME=value and `env` prefixes stripped) — anchor with ^ to match only real invocations. Refinement fields are optional; `compile_success` lists explicit success evidence used when no exit code was captured (unused by these examples; the built-in cargo def uses it). See `phr-mcp toolchains`.",
    "id": "pytest",
    "matches": "^(python3? -m )?pytest(\\s|$)",
    "compile_fail": ["SyntaxError", "ImportError"],
    "test_summary": "(?P<failed>\\d+) failed|(?P<passed>\\d+) passed",
    "per_test": "(?m)^(?P<name>\\S+) (?P<status>PASSED|FAILED)",
    "pass_tokens": ["PASSED"]
  },
  {
    "id": "tsc",
    "matches": "^(npx )?tsc(\\s|$)",
    "compile_fail": ["error TS\\d+"]
  }
]
"#;

/// Write the confidence opt-in marker, known-bug registry, and
/// toolchains.json example when the `confidence` pack is selected. Idempotent (leaves existing files alone).
fn write_confidence_scaffold(
    root: &Path,
    opts: &InitOpts,
    report: &mut InitReport,
) -> Result<(), InitError> {
    if !opts.packs.contains(&Pack::Confidence) {
        return Ok(());
    }
    let phr = root.join(".phronesis");
    for (name, contents) in [
        ("confidence.json", CONFIDENCE_JSON),
        ("bugs.json", CONFIDENCE_BUGS_JSON),
        ("toolchains.json", TOOLCHAINS_JSON),
    ] {
        let path = phr.join(name);
        if path.exists() {
            report.steps.push(format!(
                "= .phronesis/{name} already exists — leaving unchanged"
            ));
            continue;
        }
        if opts.dry_run {
            report
                .steps
                .push(format!("+ would create .phronesis/{name}"));
            continue;
        }
        std::fs::create_dir_all(&phr).map_err(|e| InitError::Io {
            path: phr.display().to_string(),
            source: e,
        })?;
        std::fs::write(&path, contents).map_err(|e| InitError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
        report.steps.push(format!("+ created .phronesis/{name}"));
    }
    Ok(())
}

/// Starter `.phronesis/journey.json` — schema version, one example tagger
/// (`build` matches `cargo (build|check|test)`), empty `modules`. Authors
/// extend it with their project's risk surface (auth, sql, payments, …)
/// per SPEC-journey-facts §"The project-defined seam".
const JOURNEY_JSON: &str = r#"{
  "version": 1,
  "taggers": [
    { "tag": "build", "when": [ { "bash_command_matches": "cargo (build|check|test)" } ] }
  ],
  "modules": []
}
"#;

/// Write `.phronesis/journey.json` when the `journey` pack is selected.
/// Idempotent — leaves an existing file alone so a project's customized
/// tagger vocabulary isn't clobbered by a re-run.
fn write_journey_scaffold(
    root: &Path,
    opts: &InitOpts,
    report: &mut InitReport,
) -> Result<(), InitError> {
    if !opts.packs.contains(&Pack::Journey) {
        return Ok(());
    }
    let phr = root.join(".phronesis");
    let path = phr.join("journey.json");
    if path.exists() {
        report
            .steps
            .push("= .phronesis/journey.json already exists — leaving unchanged".to_string());
        return Ok(());
    }
    if opts.dry_run {
        report
            .steps
            .push("+ would create .phronesis/journey.json".to_string());
        return Ok(());
    }
    std::fs::create_dir_all(&phr).map_err(|e| InitError::Io {
        path: phr.display().to_string(),
        source: e,
    })?;
    std::fs::write(&path, JOURNEY_JSON).map_err(|e| InitError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    report
        .steps
        .push("+ created .phronesis/journey.json".to_string());
    Ok(())
}

fn update_gitignore(
    root: &Path,
    opts: &InitOpts,
    report: &mut InitReport,
) -> Result<(), InitError> {
    let path = root.join(".gitignore");
    let mut entries = vec![
        ".phronesis/log.jsonl",
        ".phronesis/log.jsonl.1",
        ".phronesis/rules.json.bak",
        // Broad ignore of .phronesis/ contents, then carve the wiki tree
        // back in. `.phronesis/*` (with the trailing `*`) — NOT
        // `.phronesis/` — because the latter prevents git from listing
        // the dir at all, making the un-ignore inert. Order matters:
        // un-ignores must come after the broad ignore.
        ".phronesis/*",
        "!.phronesis/wiki/",
        "!.phronesis/wiki/**",
    ];
    // Confidence config is project knowledge (track it); the per-subject
    // outcome ledger under .phronesis/outcomes/ stays ignored via `.phronesis/*`.
    if opts.packs.contains(&Pack::Confidence) {
        entries.push("!.phronesis/confidence.json");
        entries.push("!.phronesis/bugs.json");
        entries.push("!.phronesis/toolchains.json");
    }
    // Journey config is project knowledge (track it); the journal under
    // .phronesis/journey/ (events.jsonl, session, seq) stays ignored via
    // `.phronesis/*` — local state only.
    if opts.packs.contains(&Pack::Journey) {
        entries.push("!.phronesis/journey.json");
    }
    let original = if path.exists() {
        std::fs::read_to_string(&path).map_err(|e| InitError::Io {
            path: path.display().to_string(),
            source: e,
        })?
    } else {
        String::new()
    };

    // Migrate legacy bare `.phronesis/` to `.phronesis/*`. The bare form
    // tells git not to descend into the directory at all, which makes
    // any later `!.phronesis/wiki/**` un-ignore inert. Pre-0.9.0 init
    // wrote the bare form; rewrite it so the carveout takes effect.
    //
    // After rewriting, dedupe the lines we manage (broad-ignore and
    // un-ignore carveouts) preserving first occurrence — a project that
    // already had `.phronesis/*` *and* the legacy `.phronesis/` would
    // otherwise end up with two `.phronesis/*` lines side by side.
    let had_trailing_newline = original.ends_with('\n');
    let mut migrated_count = 0usize;
    let rewritten: Vec<String> = original
        .lines()
        .map(|line| {
            if line == ".phronesis/" {
                migrated_count += 1;
                ".phronesis/*".to_string()
            } else {
                line.to_string()
            }
        })
        .collect();

    let mut seen_managed: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut deduped_count = 0usize;
    let mut migrated_lines: Vec<String> = Vec::with_capacity(rewritten.len());
    for line in rewritten {
        let is_managed = line == ".phronesis/*" || line.starts_with("!.phronesis/");
        if is_managed && !seen_managed.insert(line.clone()) {
            deduped_count += 1;
            continue;
        }
        migrated_lines.push(line);
    }
    let mut migrated_content = migrated_lines.join("\n");
    if had_trailing_newline && !migrated_content.is_empty() {
        migrated_content.push('\n');
    }

    let present_lines: std::collections::HashSet<&str> = migrated_content.lines().collect();
    let missing: Vec<&str> = entries
        .iter()
        .filter(|e| !present_lines.contains(*e))
        .copied()
        .collect();

    if missing.is_empty() && migrated_count == 0 && deduped_count == 0 {
        report
            .steps
            .push("= .gitignore already contains phronesis entries".to_string());
        return Ok(());
    }

    if opts.dry_run {
        if migrated_count > 0 {
            report.steps.push(format!(
                "~ would migrate {} bare `.phronesis/` line(s) to `.phronesis/*`",
                migrated_count
            ));
        }
        if deduped_count > 0 {
            report.steps.push(format!(
                "~ would dedupe {} duplicate phronesis line(s)",
                deduped_count
            ));
        }
        if !missing.is_empty() {
            report.steps.push(format!(
                "+ would append {} line(s) to .gitignore: {:?}",
                missing.len(),
                missing
            ));
        }
        return Ok(());
    }

    let mut new_content = migrated_content;
    if !new_content.is_empty() && !new_content.ends_with('\n') {
        new_content.push('\n');
    }
    for line in &missing {
        new_content.push_str(line);
        new_content.push('\n');
    }
    std::fs::write(&path, new_content).map_err(|e| InitError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    if migrated_count > 0 {
        report.steps.push(format!(
            "~ migrated {} bare `.phronesis/` line(s) to `.phronesis/*` (carveout was inert)",
            migrated_count
        ));
    }
    if deduped_count > 0 {
        report.steps.push(format!(
            "~ removed {} duplicate phronesis line(s)",
            deduped_count
        ));
    }
    if !missing.is_empty() {
        report
            .steps
            .push(format!("+ updated .gitignore (+{} entries)", missing.len()));
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────
// JSON merge helpers
// ─────────────────────────────────────────────────────────────────────

fn read_json(path: &Path) -> Result<Option<Value>, InitError> {
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

fn write_json(
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

fn with_extension(path: &Path, ext: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".");
    s.push(ext);
    PathBuf::from(s)
}

fn ensure_parent(path: &Path) -> Result<(), InitError> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).map_err(|e| InitError::Io {
            path: p.display().to_string(),
            source: e,
        })?;
    }
    Ok(())
}

/// Insert (or replace) an entry for our matcher inside a hook array.
fn upsert_hook(settings: &mut Value, event: &str, new_entry: Value) {
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

// ─────────────────────────────────────────────────────────────────────
// Starter packs
// ─────────────────────────────────────────────────────────────────────

/// Confidence-scoring gate rules (SPEC-confidence-scoring §3, approach A).
/// They count the open work unit's passed `signal_pass` facts (asserted by the
/// pre-check hook) and gate a `git commit` by band: ≤1 signal blocks, exactly 2
/// warns, 3 passes clean. Paired with the `.phronesis/confidence.json` opt-in
/// marker + `.phronesis/bugs.json` registry written by `write_confidence_scaffold`.
fn confidence_rules() -> Value {
    json!({
        "rules": [
            {
                "id": "confidence-low-blocks-commit",
                "phase": "pre",
                "priority": 30,
                "when": [
                    {"bash_command_matches": "git (commit|merge|rebase|cherry-pick|revert|pull)"},
                    {"__script__": "facts_count('signal_pass', ['*','*']) <= 1"}
                ],
                "then": {"block": "Low confidence — compile/tests/known-bug not all green. Run the build and tests and resolve failing signals before committing."}
            },
            {
                "id": "confidence-medium-warns-commit",
                "phase": "pre",
                "priority": 29,
                "when": [
                    {"bash_command_matches": "git (commit|merge|rebase|cherry-pick|revert|pull)"},
                    {"__script__": "facts_count('signal_pass', ['*','*']) == 2"}
                ],
                "then": {"warn": "Medium confidence — one grounded signal is missing. Review before presenting this as done."}
            }
        ]
    })
}

fn deflection_rules() -> Value {
    json!({
        "rules": [
            {
                "id": "enforce-no-pre-existing-issue",
                "phase": "pre",
                "priority": 10,
                "when": [
                    {"new_content_contains": "pre-existing issue"}
                ],
                "then": {"block": "Don't deflect with 'pre-existing issue'. Either fix it as part of this change, defer with a clear rationale, or drop the disclaimer."}
            },
            {
                "id": "enforce-no-not-from-our-changes",
                "phase": "pre",
                "priority": 10,
                "when": [
                    {"new_content_contains": "not from our changes"}
                ],
                "then": {"block": "Drop the 'not from our changes' disclaimer. Name the issue and decide: fix or defer."}
            },
            {
                "id": "enforce-no-not-caused-by-our",
                "phase": "pre",
                "priority": 10,
                "when": [
                    {"new_content_contains": "not caused by our"}
                ],
                "then": {"block": "Drop the 'not caused by our' disclaimer. Own the fix or own the decision to defer."}
            },
            {
                "id": "enforce-no-should-work-claim",
                "phase": "pre",
                "priority": 10,
                "when": [
                    {"new_content_contains": "should work now"}
                ],
                "then": {"block": "Avoid claiming a fix is complete without evidence. Run the verification (test, manual exercise, traced call chain) before reporting, or explicitly label the work 'untested' so the human knows to check."}
            },
            {
                "id": "enforce-no-should-be-fixed-claim",
                "phase": "pre",
                "priority": 10,
                "when": [
                    {"new_content_contains": "should be fixed"}
                ],
                "then": {"block": "Don't make repair claims without verifying. Run the failing case end-to-end before reporting a fix; otherwise mark it 'untested' so the user knows to verify."}
            },
            {
                "id": "nudge-verify-before-commit",
                "phase": "pre",
                "priority": 5,
                "when": [
                    {"new_content_contains": "git commit -m"},
                    {"__script__": "facts_count('confidence_enabled', []) == 0"}
                ],
                "then": {"warn": "About to commit. Trace the call chain end-to-end before reporting done. Half-fixes where one layer is wired but another is not are a recurring failure mode."}
            },
            {
                "id": "llm-warn-git-add-all",
                "phase": "pre",
                "priority": 5,
                "when": [
                    {"bash_command_matches": "(^|[;&|]\\s*)git\\s+add\\s+(-A\\b|\\.($|\\s))"}
                ],
                "then": {"warn": "Stage files explicitly — git add -A / git add . sweeps unrelated changes into the commit. List the files you actually changed."}
            },
            {
                "id": "llm-warn-kill-build",
                "phase": "pre",
                "priority": 5,
                "when": [
                    {"bash_command_matches": "\\b(pkill|killall|kill)\\b[^;|&]*\\b(cargo|rustc)\\b"}
                ],
                "then": {"warn": "Builds are I/O-bound: a rustc at 0% CPU is usually in disk-wait, not hung. Give it time or check `ps` state before killing the build."}
            }
        ]
    })
}

fn rust_rules() -> Value {
    json!({
        "rules": [
            {
                "id": "enforce-no-unwrap-in-src",
                "phase": "pre",
                "priority": 10,
                "audit": true,
                "when": [
                    {"new_content_contains": ".unwrap()"},
                    {"file_path_matches": "src"}
                ],
                "then": {"block": "Avoid .unwrap() in src/ — use ? for error propagation, or expect() with a clear message if truly unreachable."}
            },
            {
                "id": "enforce-no-todo-in-src",
                "phase": "pre",
                "priority": 10,
                "audit": true,
                "when": [
                    {"new_content_contains": "todo!()"},
                    {"file_path_matches": "src"}
                ],
                "then": {"block": "Don't ship todo!() in src/ — finish the implementation or split it into a tracked task."}
            },
            {
                "id": "enforce-no-panic-in-src",
                "phase": "pre",
                "priority": 10,
                "audit": true,
                "when": [
                    {"new_content_contains": "panic!("},
                    {"file_path_matches": "src"}
                ],
                "then": {"block": "Avoid panic!() in src/ — return a Result and let the caller decide."}
            },
            {
                "id": "enforce-no-unimplemented-in-src",
                "phase": "pre",
                "priority": 10,
                "audit": true,
                "when": [
                    {"new_content_contains": "unimplemented!()"},
                    {"file_path_matches": "src"}
                ],
                "then": {"block": "Avoid unimplemented!() in src/ — implement the path or remove it."}
            },
            {
                "id": "enforce-no-result-string-error",
                "phase": "pre",
                "priority": 10,
                "when": [
                    {"function_returns_result_string": ["?file", "?fn"]}
                ],
                "then": {"block": "`?fn` in ?file returns Result<_, String>. Define a proper error enum with thiserror."}
            },
            {
                "id": "warn-dbg-in-src",
                "phase": "pre",
                "priority": 5,
                "audit": true,
                "when": [
                    {"new_content_contains": "dbg!("},
                    {"file_path_matches": "src"}
                ],
                "then": {"warn": "dbg!() in src/ — remove before committing, or use tracing::debug!() for diagnostics that stay."}
            },
            {
                "id": "warn-rust-public-fn-takes-string-ref",
                "phase": "post",
                "priority": 10,
                "when": [
                    {"function_is_public": ["?file", "?fn"]},
                    {"function_param_type": ["?file", "?fn", "?param", "&String"]}
                ],
                "then": {"warn": "Public `?fn` takes `?param: &String` — prefer `&str` for ergonomics and to avoid forcing callers to own a String."}
            },
            {
                "id": "warn-rust-function-param-count-high",
                "phase": "post",
                "priority": 5,
                "audit": true,
                "when": [
                    {"function_param_count_high": ["?file", "?fn", "?count"]}
                ],
                "then": {"warn": "Function `?fn` in ?file has ?count parameters. Consider grouping related params into a struct (builder/options pattern) or splitting the function — long signatures correlate with God-function debt."}
            },
            {
                "id": "audit-file-loc-high",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "doc_excepted": true,
                "when": [
                    {"file_extension_is": "rs"},
                    {"file_path_matches": "src"},
                    {"file_line_count_above": "800"}
                ],
                "then": {"warn": "File exceeds 800 lines — consider splitting into focused submodules. Long files correlate with God-object debt and slow down navigation. (Scoped to src/; test blocks excluded from the count; a top-of-file `//! phronesis-allow: audit-file-loc-high <reason>` doc-comment exempts intentional god-files.)"}
            },
            {
                "id": "warn-rust-public-fn-takes-vec-ref",
                "phase": "post",
                "priority": 10,
                "when": [
                    {"function_is_public": ["?file", "?fn"]},
                    {"function_param_is_vec_ref": ["?file", "?fn", "?param"]}
                ],
                "then": {"warn": "Public `?fn` takes `?param: &Vec<T>` — prefer `&[T]` for ergonomics; a slice accepts arrays, slices, and Vecs alike. From the patterns guide §API Design 1."}
            },
            {
                "id": "warn-cargo-build-without-workspace",
                "phase": "pre",
                "priority": 3,
                "when": [
                    {"cargo_command_lacks_workspace": "?cmd"}
                ],
                "then": {"warn": "Running `?cmd` without `--workspace` only checks part of the workspace. Use `cargo <subcommand> --workspace --tests --examples` to catch sibling-crate breakage, or pass `-p <crate>` if scope was intentional."}
            },
            {
                "id": "block-await-on-sync-execute-all-agenda-items",
                "phase": "pre",
                "priority": 10,
                "audit": true,
                "when": [
                    {"new_content_contains": "execute_all_agenda_items().await"},
                    {"file_extension_is": "rs"}
                ],
                "then": {"block": "`execute_all_agenda_items()` is sync as of the 039 refactor — drop the `.await`. Cargo will reject the call site as `Result<Vec<Action>, String> is not a future`."}
            },
            {
                "id": "block-await-on-sync-fire-all-consequences",
                "phase": "pre",
                "priority": 10,
                "audit": true,
                "when": [
                    {"new_content_contains": "fire_all_consequences().await"},
                    {"file_extension_is": "rs"}
                ],
                "then": {"block": "`fire_all_consequences()` is sync — drop the `.await`. Cargo will reject as `Result<Vec<Consequence>, ReteError> is not a future`."}
            },
            {
                "id": "warn-clone-heavy",
                "phase": "pre",
                "priority": 5,
                "when": [
                    {"function_clone_count_high": ["?file", "?fn", "?count"]}
                ],
                "then": {"warn": "`?fn` in ?file calls .clone() ?count times — review whether references or borrowed slices would work."}
            },
            {
                "id": "warn-empty-test",
                "phase": "post",
                "priority": 5,
                "when": [
                    {"test_without_assertion": ["?file", "?fn"]}
                ],
                "then": {"warn": "Test `?fn` in ?file has no assertions or `?` propagation — a placeholder test that always passes hides regressions."}
            },
            {
                "id": "warn-deref-for-non-pointer-type",
                "phase": "pre",
                "priority": 5,
                "audit": true,
                "when": [
                    {"new_content_contains": "impl Deref for"},
                    {"file_extension_is": "rs"}
                ],
                "then": {"warn": "`impl Deref for` — Deref polymorphism is an anti-pattern for non-pointer types. Reserve Deref for smart-pointer wrappers (Box/Arc/Rc); for other types, prefer explicit delegation methods so the API surface is intentional."}
            },
            {
                "id": "audit-manual-err-return",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"new_content_contains": "=> return Err("},
                    {"file_extension_is": "rs"}
                ],
                "then": {"warn": "Manual `=> return Err(...)` in a match arm — the `?` operator usually replaces this whole shape. Surface during one-time audit sweeps; deliberately silent at hook time so in-progress refactors aren't blocked."}
            },
            {
                "id": "audit-newtype-id-string",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "doc_excepted": true,
                "when": [
                    {"new_content_contains": "_id: String"},
                    {"file_extension_is": "rs"},
                    {"file_path_matches": "src"}
                ],
                "then": {"warn": "Field named `*_id: String` — consider a newtype like `StateId(String)` for type safety so one ID kind can't be passed where another is expected. From the patterns guide §Design Patterns 2 (Newtype Pattern). (Scoped to src/ — test fixtures are exempt. A `///` doc-comment immediately above the field marks an intentional string ID, e.g. one crossing a JSON registry boundary, as an accepted exception.)"}
            },
            {
                "id": "audit-newtype-id-u64",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"new_content_contains": "_id: u64"},
                    {"file_extension_is": "rs"},
                    {"file_path_matches": "src"}
                ],
                "then": {"warn": "Field named `*_id: u64` — consider a newtype like `UserId(u64)` to prevent mixing different ID types. From the patterns guide §Design Patterns 2 (Newtype Pattern). (Scoped to src/ — test fixtures are exempt.)"}
            },
            {
                "id": "audit-if-let-opportunity-none-empty",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"new_content_contains": "None => {}"},
                    {"file_extension_is": "rs"}
                ],
                "then": {"warn": "`match` with a `None => {}` arm — `if let Some(x) = ...` is usually clearer. From the patterns guide §Idioms 2."}
            },
            {
                "id": "audit-if-let-opportunity-err-empty",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"new_content_contains": "Err(_) => {}"},
                    {"file_extension_is": "rs"}
                ],
                "then": {"warn": "`match` arm `Err(_) => {}` silently swallows errors. Either handle the error (log/return) or use `if let Ok(x) = ...` to make the intent explicit."}
            },
            {
                "id": "block-deny-warnings-attribute",
                "phase": "pre",
                "priority": 10,
                "audit": true,
                "when": [
                    {"new_content_contains": "#![deny(warnings)]"},
                    {"file_extension_is": "rs"}
                ],
                "then": {"block": "`#![deny(warnings)]` breaks builds on toolchain upgrades, since each rustc release introduces new warnings. Move the policy to CI with `RUSTFLAGS=\"-D warnings\"` instead. From the patterns guide §Anti-patterns (deny-warnings)."}
            },
            {
                "id": "warn-public-fn-takes-box-ref",
                "phase": "pre",
                "priority": 5,
                "audit": true,
                "when": [
                    {"new_content_contains": ": &Box<"},
                    {"file_extension_is": "rs"}
                ],
                "then": {"warn": "Parameter type `&Box<T>` adds a useless layer of indirection — prefer `&T` directly. From the patterns guide §Idioms (borrowed-types-for-arguments)."}
            },
            {
                "id": "warn-expect-with-empty-message",
                "phase": "pre",
                "priority": 5,
                "audit": true,
                "when": [
                    {"new_content_contains": ".expect(\"\")"},
                    {"file_path_matches": "src"}
                ],
                "then": {"warn": "`.expect(\"\")` is strictly worse than `.unwrap()` — same panic, no explanation of the invariant. Either supply a real message or use `.unwrap()` and let the existing rule flag it."}
            },
            {
                "id": "audit-rc-refcell-in-src",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"new_content_contains": "Rc<RefCell<"},
                    {"file_path_matches": "src"}
                ],
                "then": {"warn": "`Rc<RefCell<T>>` is the textbook 'fighting the borrow checker' shape — often a signal that an arena, index-based references, or a redesigned ownership model would be a better fit. Confirm intent."}
            },
            {
                "id": "audit-string-concat-with-plus",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"new_content_contains": "\" + &"},
                    {"file_extension_is": "rs"}
                ],
                "then": {"warn": "String concatenation with `\" + &` — prefer `format!(\"{}{}\", a, b)` for readability and to avoid intermediate allocations. From the patterns guide §Idioms (concat-format)."}
            },
            {
                "id": "audit-allow-dead-code-in-src",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "doc_excepted": true,
                "when": [
                    {"new_content_contains": "#[allow(dead_code)]"},
                    {"file_path_matches": "src"}
                ],
                "then": {"warn": "`#[allow(dead_code)]` in src/ — either delete the code or add a `///` doc-comment immediately above explaining why it's kept (planned API, generic-constraint trick, intentional placeholder). Documented exceptions are not flagged."}
            },
            {
                "id": "audit-env-set-var-in-src",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"new_content_contains": "env::set_var("},
                    {"file_path_matches": "src"}
                ],
                "then": {"warn": "`env::set_var(` in src/ — mutating process environment variables is unsound under concurrent reads (which is why edition 2024 marks the call unsafe). Verify the call site is genuinely single-threaded, or refactor to pass configuration explicitly through function arguments / a context struct. Tests where you control the thread count are usually fine; library code almost never is."}
            },
            {
                "id": "audit-rust-let-binding-count-high",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "doc_excepted": true,
                "when": [
                    {"file_path_matches": "src"},
                    {"function_let_binding_count_high": ["?file", "?fn", "?count"]}
                ],
                "then": {"warn": "`?fn` in ?file has ?count outer-scope `let` bindings — consider scoping intermediate temporaries into a block (`let result = { let raw = ...; let parsed = ...; ... }`) so only the final value is visible to the rest of the function. Block pattern: John Nunley, 'Rust's Block Pattern' (Dec 2025). (Scoped to src/ — examples/benches/tests are not production code and are exempt.)"}
            },
            {
                "id": "audit-rust-let-mut-count-high",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "doc_excepted": true,
                "when": [
                    {"file_path_matches": "src"},
                    {"function_let_mut_count_high": ["?file", "?fn", "?count"]}
                ],
                "then": {"warn": "`?fn` in ?file has ?count outer-scope `let mut` declarations — consider John Nunley's block pattern: wrap the mutation in `let x = { let mut tmp = ...; ...; tmp }` so the surrounding scope sees an immutable binding. Block pattern: John Nunley, 'Rust's Block Pattern' (Dec 2025). (Scoped to src/ — examples/benches/tests are not production code and are exempt.)"}
            }
        ]
    })
}

/// Rhai-specific rules. Apply to projects that embed the Rhai scripting
/// language, whether via the `rhai` crate from Rust or as standalone `.rhai`
/// scripts. Messages are intentionally generic; project-specific guidance
/// (which loader helper to call, which response-proxy to use, etc.) should
/// be layered in via project-local rules in `.phronesis/rules.json`.
fn rhai_rules() -> Value {
    json!({
        "rules": [
            {
                "id": "block-rhai-inline-eval-string",
                "phase": "pre",
                "priority": 10,
                "when": [
                    {"engine_eval_string_literal": ["?file", "?fn"]},
                    {"file_extension_is": "rs"}
                ],
                "then": {"block": "`?fn` in ?file calls `engine.eval(<string literal>)`. Inline string-eval can't be tested independently of the surrounding Rust code and bypasses any script registry. Move the script to a `.rhai` file and load it via `engine.compile_file(...)` (or `compile(...)` on `include_str!`-ed content) so the AST can be cached, values-checked at build time, and exercised in isolation."}
            },
            {
                "id": "block-rhai-print-in-script",
                "phase": "pre",
                "priority": 10,
                "audit": true,
                "when": [
                    {"new_content_contains": "print("},
                    {"file_extension_is": "rhai"}
                ],
                "then": {"block": "`print(` appears in a .rhai script. `print` is Rhai's equivalent of `dbg!()` — debug output that bypasses whatever response/logging channel your host has registered. Use the host-registered function for emitting output (commonly a `log`, `emit`, or `response_*` proxy your `Engine` exposes via `register_fn`) so script output flows through the same path as the rest of your application."}
            }
        ]
    })
}

fn python_rules() -> Value {
    json!({
        "rules": [
            {
                "id": "warn-print-in-src",
                "phase": "pre",
                "priority": 5,
                "audit": true,
                "when": [
                    {"python_print_call": ["?file", "?fn"]},
                    {"file_path_matches": "src"}
                ],
                "then": {"warn": "print() in ?fn (?file) — consider logging.info()/debug() instead. Remove debug prints before committing. Upstream: style guidance, not a correctness rule."}
            },
            {
                "id": "enforce-no-bare-except",
                "phase": "pre",
                "priority": 10,
                "audit": true,
                "when": [
                    {"python_bare_except": ["?file", "?fn"]}
                ],
                "then": {"block": "Don't use bare `except:` — catch specific exception types. Bare except swallows KeyboardInterrupt and SystemExit. Upstream: PLE0704 (pylint), Bugbear B001."}
            },
            {
                "id": "warn-python-mutable-default-arg",
                "phase": "pre",
                "priority": 5,
                "audit": true,
                "when": [
                    {"python_mutable_default_arg": ["?file", "?fn", "?param"]}
                ],
                "then": {"warn": "Mutable default `?param` in ?fn — defaults are created once at def time and shared across calls. Use None and create inside. Upstream: Bugbear B006."}
            },
            {
                "id": "audit-python-call-in-default-arg",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"python_call_in_default_arg": ["?file", "?fn", "?param", "?callee"]}
                ],
                "then": {"warn": "Function call `?callee()` in default argument `?param` of ?fn — evaluated once at def time. If it returns a mutable or side-effectful value, this is a bug. Upstream: Bugbear B008 (narrower: only flags call expressions, not bare mutable literals which are covered by B006)."}
            },
            {
                "id": "warn-python-swallowed-exception",
                "phase": "pre",
                "priority": 5,
                "audit": true,
                "when": [
                    {"python_exception_handler_passes": ["?file", "?fn", "?exception"]}
                ],
                "then": {"warn": "Handler for ?exception in ?fn is empty (`pass`/`...`/comments) — the exception is silently swallowed. Recommend handling, re-raising, or documenting the intentional fallback. Upstream: Bugbear B110 (narrower: typed handlers only; bare handlers caught by enforce-no-bare-except)."}
            },
            {
                "id": "audit-python-high-param-count",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"python_function_param_count_high": ["?file", "?fn", "?count"]}
                ],
                "then": {"warn": "Function ?fn in ?file has ?count parameters — consider grouping into a config object or using the builder pattern. Exclude self/cls. Upstream: design smell (maintainability, not correctness)."}
            },
            {
                "id": "audit-python-missing-docstring",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"python_function_missing_docstring": ["?file", "?fn"]}
                ],
                "then": {"warn": "Public def ?fn in ?file has no docstring. Upstream: documentation best practice; audit-only because docstring policy is project-dependent."}
            }
        ]
    })
}

fn typescript_rules() -> Value {
    json!({
        "rules": [
            {
                "id": "warn-any-in-src",
                "phase": "pre",
                "priority": 5,
                "when": [
                    {"new_content_contains": ": any"},
                    {"file_path_matches": "src"}
                ],
                "then": {"warn": ": any in src/ — narrow the type. Use `unknown` if you really don't know, then refine with type guards."}
            },
            {
                "id": "warn-console-log-in-src",
                "phase": "pre",
                "priority": 5,
                "when": [
                    {"new_content_contains": "console.log("},
                    {"file_path_matches": "src"}
                ],
                "then": {"warn": "console.log in src/ — remove before committing, or use a proper logger."}
            },
            {
                "id": "warn-ts-explicit-any-ast",
                "phase": "pre",
                "priority": 5,
                "audit": true,
                "when": [
                    {"ts_explicit_any": ["?file", "?fn", "?count"]}
                ],
                "then": {"warn": "?count explicit `any` annotation(s) in ?fn (?file) — narrow the type, or use `unknown` and refine with type guards."}
            },
            {
                "id": "warn-ts-suppression-comment",
                "phase": "pre",
                "priority": 5,
                "audit": true,
                "when": [
                    {"ts_suppression_comment": ["?file", "?count"]}
                ],
                "then": {"warn": "?count @ts-ignore/@ts-expect-error/@ts-nocheck comment(s) in ?file — each one turns the type checker off somewhere. Fix the type instead."}
            },
            {
                "id": "audit-ts-non-null-assertion",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"ts_non_null_assertion": ["?file", "?fn", "?count"]}
                ],
                "then": {"warn": "?count non-null assertion(s) (`x!`) in ?fn (?file) — prefer explicit narrowing or optional chaining."}
            }
        ]
    })
}

fn swift_rules() -> Value {
    json!({
        "rules": [
            {
                "id": "warn-swift-force-unwrap",
                "phase": "post",
                "priority": 10,
                "when": [
                    {"function_uses_force_unwrap": ["?file", "?fn", "?count"]}
                ],
                "then": {"warn": "Function `?fn` in ?file uses ?count force-unwrap(s). Prefer guard let or if let; reserve ! for invariants you can document."}
            },
            {
                "id": "warn-swift-try-bang",
                "phase": "pre",
                "priority": 5,
                "when": [
                    {"new_content_contains": "try!"},
                    {"file_extension_is": "swift"}
                ],
                "then": {"warn": "try! crashes on error — prefer try with do/catch, or try? when an Optional result is acceptable."}
            },
            {
                "id": "warn-swift-force-cast",
                "phase": "pre",
                "priority": 10,
                "when": [
                    {"new_content_contains": "as!"},
                    {"file_extension_is": "swift"}
                ],
                "then": {"warn": "Force-cast `as!` crashes on type mismatch — prefer `as?` with `if let`/`guard let`. Completes the force-bang trio with `!` and `try!`."}
            },
            {
                "id": "audit-swift-fatal-error",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"new_content_contains": "fatalError("},
                    {"file_extension_is": "swift"}
                ],
                "then": {"warn": "`fatalError(` aborts the process — for recoverable conditions prefer a `throws` API and let the caller decide. Reserve `fatalError` for genuinely unreachable invariants (and prefer `precondition`/`assertionFailure` when the intent is a debug-only trap)."}
            },
            {
                "id": "audit-swift-mutable-singleton",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"new_content_contains": "static var shared"},
                    {"file_extension_is": "swift"}
                ],
                "then": {"warn": "`static var shared` is a mutable global — the Singleton pattern (eleev/swift-design-patterns §Creational/Singleton) uses `static let shared` so the instance can't be swapped at runtime. If mutability is intentional, add a comment or move state inside the instance."}
            },
            {
                "id": "audit-swift-legacy-constructor",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"or": [
                        {"new_content_contains": "CGRectMake("},
                        {"new_content_contains": "CGSizeMake("},
                        {"new_content_contains": "CGPointMake("},
                        {"new_content_contains": "CGVectorMake("},
                        {"new_content_contains": "UIEdgeInsetsMake("},
                        {"new_content_contains": "NSMakeRect("},
                        {"new_content_contains": "NSMakeSize("},
                        {"new_content_contains": "NSMakePoint("},
                        {"new_content_contains": "NSMakeRange("}
                    ]},
                    {"file_extension_is": "swift"}
                ],
                "then": {"warn": "Legacy C-style constructor — prefer the modern Swift initializer (e.g. `CGRect(x:y:width:height:)`, `UIEdgeInsets(top:left:bottom:right:)`). Mirrors SwiftLint's `legacy_constructor` rule."}
            },
            {
                "id": "audit-swift-legacy-random",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"or": [
                        {"new_content_contains": "arc4random("},
                        {"new_content_contains": "arc4random_uniform("},
                        {"new_content_contains": "drand48("}
                    ]},
                    {"file_extension_is": "swift"}
                ],
                "then": {"warn": "Legacy random API — Swift 4.2+ ships `Int.random(in:)`, `Double.random(in:)`, and `Collection.randomElement()`, which work on all platforms (not just Darwin) and are uniformly distributed without modulo bias. Mirrors SwiftLint's `legacy_random` rule."}
            }
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_parse_accepts_aliases() {
        assert_eq!(Pack::parse("llm").unwrap(), Pack::Llm);
        assert_eq!(Pack::parse("minimal").unwrap(), Pack::Llm); // legacy alias
        assert_eq!(Pack::parse("rust").unwrap(), Pack::Rust);
        assert_eq!(Pack::parse("rs").unwrap(), Pack::Rust);
        assert_eq!(Pack::parse("PYTHON").unwrap(), Pack::Python);
        assert_eq!(Pack::parse("ts").unwrap(), Pack::TypeScript);
        assert_eq!(Pack::parse("js").unwrap(), Pack::TypeScript);
        assert_eq!(Pack::parse("none").unwrap(), Pack::None);
        assert_eq!(Pack::parse("rhai").unwrap(), Pack::Rhai);
        assert_eq!(Pack::parse("RHAI").unwrap(), Pack::Rhai);
    }

    #[test]
    fn parses_swift_pack() {
        assert_eq!(Pack::parse("swift").unwrap(), Pack::Swift);
    }

    /// The Rhai pack carries the two formerly-rust-bundled rules with
    /// generalized (non-project-specific) messages, and the rust pack no
    /// longer ships them. This pins the 0.6.1 pack split against regression.
    #[test]
    fn rhai_pack_carries_rhai_rules_and_rust_does_not() {
        let rhai = Pack::Rhai.rules();
        let rhai_ids: Vec<&str> = rhai["rules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap())
            .collect();
        assert!(rhai_ids.contains(&"block-rhai-inline-eval-string"));
        assert!(rhai_ids.contains(&"block-rhai-print-in-script"));

        let rust = Pack::Rust.rules();
        let rust_ids: Vec<&str> = rust["rules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap())
            .collect();
        assert!(!rust_ids.contains(&"block-rhai-inline-eval-string"));
        assert!(!rust_ids.contains(&"block-rhai-print-in-script"));
    }

    /// The Rhai-pack messages should be project-neutral: no references
    /// to helper identifiers, file names, or project codenames from
    /// any particular host. Pins the "generalize messages" intent of
    /// the 0.6.1 split. The forbidden-token list below names some
    /// historical leaks the test exists to guard against.
    #[test]
    fn rhai_pack_messages_are_project_neutral() {
        let v = Pack::Rhai.rules();
        let arr = v["rules"].as_array().unwrap();
        for rule in arr {
            // v2 shape: message is in rule["then"]["block"] or rule["then"]["warn"]
            let then = &rule["then"];
            let msg = then
                .get("block")
                .or_else(|| then.get("warn"))
                .or_else(|| then.get("log"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            for forbidden in ["GameLogicLoader", "save.rhai", "response_append"] {
                assert!(
                    !msg.contains(forbidden),
                    "rhai pack message for {} contains project-specific reference {:?}",
                    rule["id"],
                    forbidden
                );
            }
        }
    }

    #[test]
    fn swift_pack_yields_rules() {
        let v = Pack::Swift.rules();
        let arr = v.get("rules").unwrap().as_array().unwrap();
        assert!(!arr.is_empty(), "swift pack should ship at least one rule");
        let ids: Vec<&str> = arr.iter().map(|r| r["id"].as_str().unwrap()).collect();
        assert!(ids.contains(&"warn-swift-force-unwrap"));
        assert!(ids.contains(&"warn-swift-try-bang"));
        assert!(ids.contains(&"warn-swift-force-cast"));
        assert!(ids.contains(&"audit-swift-fatal-error"));
        assert!(ids.contains(&"audit-swift-mutable-singleton"));
        assert!(ids.contains(&"audit-swift-legacy-constructor"));
        assert!(ids.contains(&"audit-swift-legacy-random"));
    }

    #[test]
    fn pack_parse_rejects_unknown() {
        assert!(matches!(
            Pack::parse("haskell"),
            Err(InitError::UnknownPack(_))
        ));
    }

    #[test]
    fn parse_packs_default_is_llm_only() {
        assert_eq!(parse_packs("").unwrap(), vec![Pack::Llm]);
    }

    #[test]
    fn parse_packs_handles_comma_separated_list() {
        let p = parse_packs("llm, rust").unwrap();
        assert_eq!(p, vec![Pack::Llm, Pack::Rust]);
    }

    #[test]
    fn parse_packs_dedupes_duplicates() {
        let p = parse_packs("rust,llm,rust,llm").unwrap();
        assert_eq!(p, vec![Pack::Rust, Pack::Llm]);
    }

    #[test]
    fn llm_pack_is_only_deflection_rules() {
        let v = Pack::Llm.rules();
        let ids: Vec<&str> = v["rules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap())
            .collect();
        assert!(ids.contains(&"enforce-no-pre-existing-issue"));
        assert!(ids.contains(&"enforce-no-not-from-our-changes"));
        assert!(ids.contains(&"enforce-no-not-caused-by-our"));
        // Should NOT carry language-specific rules
        assert!(!ids.contains(&"enforce-no-unwrap-in-src"));
    }

    #[test]
    fn rust_pack_carries_only_rust_rules() {
        let v = Pack::Rust.rules();
        let ids: Vec<&str> = v["rules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap())
            .collect();
        assert!(ids.contains(&"enforce-no-unwrap-in-src"));
        assert!(ids.contains(&"enforce-no-result-string-error"));
        assert!(ids.contains(&"warn-dbg-in-src"));
        // Should NOT bundle deflection rules — they're a separate pack
        assert!(!ids.contains(&"enforce-no-pre-existing-issue"));
    }

    #[test]
    fn rust_pack_includes_new_predicate_rules() {
        let v = Pack::Rust.rules();
        let arr = v.get("rules").unwrap().as_array().unwrap();
        let ids: Vec<&str> = arr.iter().map(|r| r["id"].as_str().unwrap()).collect();
        assert!(ids.contains(&"warn-rust-public-fn-takes-string-ref"));
        assert!(ids.contains(&"warn-rust-public-fn-takes-vec-ref"));
        assert!(ids.contains(&"warn-deref-for-non-pointer-type"));
    }

    #[test]
    fn rust_pack_includes_block_pattern_rules() {
        let v = Pack::Rust.rules();
        let ids: Vec<&str> = v["rules"]
            .as_array()
            .expect("rules array")
            .iter()
            .map(|r| r["id"].as_str().expect("rule id is a string"))
            .collect();
        assert!(
            ids.contains(&"audit-rust-let-binding-count-high"),
            "expected audit-rust-let-binding-count-high in rust pack, got {:?}",
            ids
        );
        assert!(
            ids.contains(&"audit-rust-let-mut-count-high"),
            "expected audit-rust-let-mut-count-high in rust pack, got {:?}",
            ids
        );
    }

    #[test]
    fn let_count_audit_rules_are_doc_excepted() {
        let v = Pack::Rust.rules();
        let arr = v.get("rules").unwrap().as_array().unwrap();
        for id in [
            "audit-rust-let-binding-count-high",
            "audit-rust-let-mut-count-high",
        ] {
            let rule = arr
                .iter()
                .find(|r| r["id"] == id)
                .unwrap_or_else(|| panic!("{id} missing from rust pack"));
            assert_eq!(
                rule["doc_excepted"], true,
                "{id} must honor //! phronesis-allow markers"
            );
        }
    }

    /// All audit-only rules added in 0.4.0 should carry both `phase: "audit"`
    /// and `audit: true`. Pins the audit-only convention against accidental
    /// regression to a hook phase.
    #[test]
    fn rust_pack_audit_only_rules_have_consistent_shape() {
        let v = Pack::Rust.rules();
        let arr = v.get("rules").unwrap().as_array().unwrap();
        let audit_only_ids = [
            "audit-manual-err-return",
            "audit-newtype-id-string",
            "audit-newtype-id-u64",
            "audit-if-let-opportunity-none-empty",
            "audit-if-let-opportunity-err-empty",
            "audit-rc-refcell-in-src",
            "audit-string-concat-with-plus",
            "audit-allow-dead-code-in-src",
            "audit-env-set-var-in-src",
        ];
        for id in audit_only_ids {
            let rule = arr
                .iter()
                .find(|r| r["id"] == id)
                .unwrap_or_else(|| panic!("rust pack must include {id}"));
            assert_eq!(rule["phase"], "audit", "{id} must be phase: audit");
            assert_eq!(rule["audit"], true, "{id} must be audit: true");
        }
    }

    /// Rules added in 0.6.1, sourced from the rust-unofficial/patterns book.
    /// Verifies presence and that the block/warn ones use the right severity.
    #[test]
    fn rust_pack_includes_patterns_book_rules() {
        let v = Pack::Rust.rules();
        let arr = v.get("rules").unwrap().as_array().unwrap();
        let by_id = |id: &str| {
            arr.iter()
                .find(|r| r["id"] == id)
                .unwrap_or_else(|| panic!("rust pack must include {id}"))
        };
        // v2 shape: severity is the key in rule["then"]
        assert!(
            by_id("block-deny-warnings-attribute")["then"]
                .get("block")
                .is_some(),
            "block-deny-warnings-attribute must use 'block' verb"
        );
        assert!(
            by_id("warn-public-fn-takes-box-ref")["then"]
                .get("warn")
                .is_some(),
            "warn-public-fn-takes-box-ref must use 'warn' verb"
        );
        assert!(
            by_id("warn-expect-with-empty-message")["then"]
                .get("warn")
                .is_some(),
            "warn-expect-with-empty-message must use 'warn' verb"
        );
        for id in [
            "audit-rc-refcell-in-src",
            "audit-string-concat-with-plus",
            "audit-allow-dead-code-in-src",
        ] {
            assert_eq!(by_id(id)["phase"], "audit");
        }
    }

    #[test]
    fn rust_pack_includes_tier_1_rules() {
        let rules = rust_rules();
        let arr = rules["rules"].as_array().unwrap();
        let ids: Vec<&str> = arr.iter().map(|r| r["id"].as_str().unwrap()).collect();
        for required in &[
            "warn-cargo-build-without-workspace",
            "block-await-on-sync-execute-all-agenda-items",
            "block-await-on-sync-fire-all-consequences",
            "warn-clone-heavy",
            "warn-empty-test",
        ] {
            assert!(
                ids.contains(required),
                "rust pack must include {}",
                required
            );
        }
        // The replaced rule is gone.
        assert!(
            !ids.contains(&"warn-rust-clone-count"),
            "warn-rust-clone-count must be removed (replaced by warn-clone-heavy)"
        );
    }

    #[test]
    fn compose_packs_llm_plus_rust_merges_both() {
        let v = compose_packs(&[Pack::Llm, Pack::Rust]);
        let ids: Vec<&str> = v["rules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap())
            .collect();
        assert!(ids.contains(&"enforce-no-pre-existing-issue"));
        assert!(ids.contains(&"enforce-no-unwrap-in-src"));
    }

    #[test]
    fn compose_packs_dedupes_by_rule_id() {
        // Composing the same pack twice doesn't duplicate rules.
        let v = compose_packs(&[Pack::Llm, Pack::Llm]);
        let count = v["rules"].as_array().unwrap().len();
        let single = compose_packs(&[Pack::Llm]);
        assert_eq!(count, single["rules"].as_array().unwrap().len());
    }

    #[test]
    fn none_pack_is_empty() {
        let v = Pack::None.rules();
        assert!(v["rules"].as_array().unwrap().is_empty());
    }

    #[test]
    fn upsert_hook_replaces_matching_matcher() {
        let mut settings = json!({
            "hooks": {
                "PreToolUse": [
                    {"matcher": "Edit|Write|MultiEdit|Bash", "hooks": [{"type":"command","command":"OLD"}]},
                    {"matcher": "Read", "hooks": [{"type":"command","command":"keep"}]}
                ]
            }
        });
        upsert_hook(
            &mut settings,
            "PreToolUse",
            json!({
                "matcher": "Edit|Write|MultiEdit|Bash",
                "hooks": [{"type":"command","command":"NEW"}]
            }),
        );
        let arr = settings["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        // The Read entry is preserved
        assert!(
            arr.iter()
                .any(|m| m["matcher"] == "Read" && m["hooks"][0]["command"] == "keep")
        );
        // The Edit|... entry is replaced (only one with that matcher)
        let ours: Vec<&Value> = arr
            .iter()
            .filter(|m| m["matcher"] == "Edit|Write|MultiEdit|Bash")
            .collect();
        assert_eq!(ours.len(), 1);
        assert_eq!(ours[0]["hooks"][0]["command"], "NEW");
    }

    #[test]
    fn upsert_hook_appends_when_no_matching_matcher() {
        let mut settings = json!({"hooks": {"PreToolUse": [
            {"matcher": "Read", "hooks":[{"type":"command","command":"keep"}]}
        ]}});
        upsert_hook(
            &mut settings,
            "PreToolUse",
            json!({"matcher":"Edit|Write|MultiEdit|Bash","hooks":[{"type":"command","command":"NEW"}]}),
        );
        assert_eq!(settings["hooks"]["PreToolUse"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn run_dry_run_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let opts = InitOpts {
            project_root: dir.path().to_path_buf(),
            packs: vec![Pack::Llm, Pack::Rust],
            force: false,
            dry_run: true,
            rules_only: false,
            hooks_only: false,
        };
        let report = run(opts).unwrap();
        assert!(!dir.path().join(".claude/settings.local.json").exists());
        assert!(!dir.path().join(".mcp.json").exists());
        assert!(!dir.path().join(".phronesis/rules.json").exists());
        assert!(!dir.path().join(".gitignore").exists());
        assert!(!dir.path().join(".gemini/settings.json").exists());
        // But the report should describe planned steps
        assert!(!report.steps.is_empty());
    }

    #[test]
    fn run_hooks_only_skips_rules_and_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        let opts = InitOpts {
            project_root: dir.path().to_path_buf(),
            packs: vec![Pack::Llm],
            force: false,
            dry_run: false,
            rules_only: false,
            hooks_only: true,
        };
        run(opts).unwrap();
        assert!(
            dir.path().join(".claude/settings.local.json").exists(),
            "hooks-only must still write claude settings"
        );
        assert!(
            !dir.path().join(".phronesis/rules.json").exists(),
            "hooks-only must skip rules.json"
        );
        assert!(
            !dir.path().join(".gitignore").exists(),
            "hooks-only must skip .gitignore"
        );
    }

    #[test]
    fn run_dry_run_does_not_write_gemini_settings() {
        let dir = tempfile::tempdir().unwrap();
        let opts = InitOpts {
            project_root: dir.path().to_path_buf(),
            packs: vec![Pack::Llm],
            force: false,
            dry_run: true,
            rules_only: false,
            hooks_only: false,
        };
        let report = run(opts).unwrap();
        assert!(!dir.path().join(".gemini/settings.json").exists());
        // Report should still mention that .gemini/settings.json would be written
        assert!(
            report
                .steps
                .iter()
                .any(|s| s.contains(".gemini/settings.json")),
            "dry-run report should mention .gemini/settings.json"
        );
    }

    #[test]
    fn run_writes_all_four_files_on_fresh_project() {
        let dir = tempfile::tempdir().unwrap();
        let opts = InitOpts {
            project_root: dir.path().to_path_buf(),
            packs: vec![Pack::Llm, Pack::Rust],
            force: false,
            dry_run: false,
            rules_only: false,
            hooks_only: false,
        };
        let report = run(opts).unwrap();
        assert!(dir.path().join(".claude/settings.local.json").exists());
        assert!(dir.path().join(".mcp.json").exists());
        assert!(dir.path().join(".phronesis/rules.json").exists());
        assert!(dir.path().join(".gitignore").exists());
        // No warnings about missing binary in this test (PATH may or may not have it)
        // Just sanity-check that we got progress steps
        assert!(
            report
                .steps
                .iter()
                .any(|s| s.contains("settings.local.json"))
        );
        assert!(report.steps.iter().any(|s| s.contains("rules.json")));
    }

    #[test]
    fn run_preserves_existing_permissions_in_settings() {
        let dir = tempfile::tempdir().unwrap();
        let settings_path = dir.path().join(".claude/settings.local.json");
        std::fs::create_dir_all(settings_path.parent().unwrap()).unwrap();
        // Pre-existing settings with permissions block (no hooks yet)
        std::fs::write(
            &settings_path,
            r#"{"permissions":{"allow":["Bash(ls:*)"]}}"#,
        )
        .unwrap();

        let opts = InitOpts {
            project_root: dir.path().to_path_buf(),
            packs: vec![Pack::Llm],
            force: false,
            dry_run: false,
            rules_only: false,
            hooks_only: false,
        };
        run(opts).unwrap();

        let content: Value =
            serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
        assert_eq!(content["permissions"]["allow"][0], "Bash(ls:*)");
        // Hooks block was added
        assert!(content["hooks"]["PreToolUse"].is_array());
    }

    #[test]
    fn run_does_not_overwrite_rules_without_force() {
        let dir = tempfile::tempdir().unwrap();
        let rules_path = dir.path().join(".phronesis/rules.json");
        std::fs::create_dir_all(rules_path.parent().unwrap()).unwrap();
        std::fs::write(
            &rules_path,
            r#"{"rules":[{"id":"mine","phase":"pre","priority":1,"conditions":[],"actions":[]}]}"#,
        )
        .unwrap();

        let opts = InitOpts {
            project_root: dir.path().to_path_buf(),
            packs: vec![Pack::Llm, Pack::Rust],
            force: false,
            dry_run: false,
            rules_only: false,
            hooks_only: false,
        };
        run(opts).unwrap();

        let content: Value =
            serde_json::from_str(&std::fs::read_to_string(&rules_path).unwrap()).unwrap();
        assert_eq!(content["rules"][0]["id"], "mine");
    }

    #[test]
    fn run_overwrites_rules_with_force() {
        let dir = tempfile::tempdir().unwrap();
        let rules_path = dir.path().join(".phronesis/rules.json");
        std::fs::create_dir_all(rules_path.parent().unwrap()).unwrap();
        std::fs::write(
            &rules_path,
            r#"{"rules":[{"id":"old","phase":"pre","priority":1,"conditions":[],"actions":[]}]}"#,
        )
        .unwrap();

        let opts = InitOpts {
            project_root: dir.path().to_path_buf(),
            packs: vec![Pack::Llm, Pack::Rust],
            force: true,
            dry_run: false,
            rules_only: false,
            hooks_only: false,
        };
        run(opts).unwrap();

        let content: Value =
            serde_json::from_str(&std::fs::read_to_string(&rules_path).unwrap()).unwrap();
        let ids: Vec<&str> = content["rules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap())
            .collect();
        // Old rule gone; new rust pack present
        assert!(!ids.contains(&"old"));
        assert!(ids.contains(&"enforce-no-unwrap-in-src"));
        // And a .bak of the old rules was created
        let bak_path = dir.path().join(".phronesis/rules.json.bak");
        assert!(
            bak_path.exists(),
            "force should back up the prior rules file"
        );
    }

    #[test]
    fn gitignore_appends_only_missing_entries() {
        let dir = tempfile::tempdir().unwrap();
        let gi_path = dir.path().join(".gitignore");
        std::fs::write(
            &gi_path,
            "/target\n.phronesis/log.jsonl\n", // one of our entries already there
        )
        .unwrap();

        let opts = InitOpts {
            project_root: dir.path().to_path_buf(),
            packs: vec![Pack::None],
            force: false,
            dry_run: false,
            rules_only: false,
            hooks_only: false,
        };
        run(opts).unwrap();

        let content = std::fs::read_to_string(&gi_path).unwrap();
        // Original target preserved
        assert!(content.contains("/target"));
        // Existing entry not duplicated
        assert_eq!(content.matches(".phronesis/log.jsonl\n").count(), 1);
        // New entries appended
        assert!(content.contains(".phronesis/log.jsonl.1"));
        assert!(content.contains(".phronesis/rules.json.bak"));
    }

    #[test]
    fn second_run_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let opts1 = InitOpts {
            project_root: dir.path().to_path_buf(),
            packs: vec![Pack::Llm, Pack::Rust],
            force: false,
            dry_run: false,
            rules_only: false,
            hooks_only: false,
        };
        run(opts1).unwrap();
        let first =
            std::fs::read_to_string(dir.path().join(".claude/settings.local.json")).unwrap();
        let rules_first =
            std::fs::read_to_string(dir.path().join(".phronesis/rules.json")).unwrap();

        let opts2 = InitOpts {
            project_root: dir.path().to_path_buf(),
            packs: vec![Pack::Llm, Pack::Rust],
            force: false,
            dry_run: false,
            rules_only: false,
            hooks_only: false,
        };
        run(opts2).unwrap();
        let second =
            std::fs::read_to_string(dir.path().join(".claude/settings.local.json")).unwrap();
        let rules_second =
            std::fs::read_to_string(dir.path().join(".phronesis/rules.json")).unwrap();

        assert_eq!(first, second, "settings should be unchanged on rerun");
        assert_eq!(
            rules_first, rules_second,
            "rules should not be touched on rerun"
        );
    }

    #[test]
    fn errors_when_path_missing() {
        let opts = InitOpts {
            project_root: PathBuf::from("/totally/nonexistent/path/xyz12345"),
            packs: vec![Pack::Llm],
            force: false,
            dry_run: false,
            rules_only: false,
            hooks_only: false,
        };
        assert!(matches!(run(opts), Err(InitError::NoSuchPath(_))));
    }

    // ─────────────────────────────────────────────────────────────────────
    // Global install / uninstall tests (using _with_home variants)
    // ─────────────────────────────────────────────────────────────────────

    #[test]
    fn run_writes_gemini_settings_with_mcp_and_hooks() {
        let dir = tempfile::tempdir().unwrap();
        let opts = InitOpts {
            project_root: dir.path().to_path_buf(),
            packs: vec![Pack::Llm],
            force: false,
            dry_run: false,
            rules_only: false,
            hooks_only: false,
        };
        let report = run(opts).unwrap();
        let gemini_path = dir.path().join(".gemini/settings.json");
        assert!(gemini_path.exists(), "should create .gemini/settings.json");
        let content: Value =
            serde_json::from_str(&std::fs::read_to_string(&gemini_path).unwrap()).unwrap();
        // MCP server registered
        assert_eq!(content["mcpServers"]["phronesis"]["command"], "phr-mcp");
        // Hooks wired
        let before = content["hooks"]["BeforeTool"].as_array().unwrap();
        assert!(!before.is_empty());
        assert!(before[0]["matcher"].as_str().unwrap().contains("replace"));
        let after = content["hooks"]["AfterTool"].as_array().unwrap();
        assert!(!after.is_empty());
        assert!(after[0]["matcher"].as_str().unwrap().contains("write_file"));
        // Report mentions gemini
        assert!(
            report
                .steps
                .iter()
                .any(|s| s.to_lowercase().contains("gemini"))
        );
    }

    #[test]
    fn install_globally_with_home_writes_gemini_settings() {
        let home = tempfile::tempdir().unwrap();
        let report = install_globally_with_home(home.path(), false).unwrap();

        let gemini_path = home.path().join(".gemini").join("settings.json");
        assert!(
            gemini_path.exists(),
            "~/.gemini/settings.json should be created"
        );

        let content: Value =
            serde_json::from_str(&std::fs::read_to_string(&gemini_path).unwrap()).unwrap();
        let entry = &content["mcpServers"]["phronesis"];
        assert_eq!(entry["command"], "phr-mcp");
        assert_eq!(entry["args"][0], "serve");

        assert!(
            report.steps.iter().any(|s| s.contains("gemini")),
            "report should mention gemini: {:?}",
            report.steps
        );
    }

    #[test]
    fn install_globally_with_home_writes_claude_json() {
        let home = tempfile::tempdir().unwrap();
        let report = install_globally_with_home(home.path(), false).unwrap();

        let claude_path = home.path().join(".claude.json");
        assert!(claude_path.exists(), "~/.claude.json should be created");

        let content: Value =
            serde_json::from_str(&std::fs::read_to_string(&claude_path).unwrap()).unwrap();
        let entry = &content["mcpServers"]["phronesis"];
        assert_eq!(entry["command"], "phr-mcp");
        assert_eq!(entry["args"][0], "serve");

        assert!(
            report.steps.iter().any(|s| s.contains("claude")),
            "report should mention claude: {:?}",
            report.steps
        );
    }

    #[test]
    fn install_globally_with_home_preserves_other_gemini_settings() {
        let home = tempfile::tempdir().unwrap();
        let gemini_dir = home.path().join(".gemini");
        std::fs::create_dir_all(&gemini_dir).unwrap();
        let gemini_path = gemini_dir.join("settings.json");

        // Pre-existing mcpServers entry that should survive
        std::fs::write(
            &gemini_path,
            r#"{"mcpServers":{"other-tool":{"command":"other","args":[]}},"theme":"dark"}"#,
        )
        .unwrap();

        install_globally_with_home(home.path(), false).unwrap();

        let content: Value =
            serde_json::from_str(&std::fs::read_to_string(&gemini_path).unwrap()).unwrap();
        // Our entry is present
        assert_eq!(content["mcpServers"]["phronesis"]["command"], "phr-mcp");
        // Pre-existing entry is preserved
        assert_eq!(content["mcpServers"]["other-tool"]["command"], "other");
        // Top-level key preserved
        assert_eq!(content["theme"], "dark");
    }

    #[test]
    fn install_globally_with_home_idempotent() {
        let home = tempfile::tempdir().unwrap();
        install_globally_with_home(home.path(), false).unwrap();
        let report2 = install_globally_with_home(home.path(), false).unwrap();

        // Second run: both files should report no-change
        assert!(
            report2
                .steps
                .iter()
                .any(|s| s.contains("already registers") && s.contains("claude")),
            "second run should report claude already registered: {:?}",
            report2.steps
        );
        assert!(
            report2
                .steps
                .iter()
                .any(|s| s.contains("already registers") && s.contains("gemini")),
            "second run should report gemini already registered: {:?}",
            report2.steps
        );
    }

    #[test]
    fn uninstall_globally_with_home_removes_from_gemini() {
        let home = tempfile::tempdir().unwrap();
        // First install
        install_globally_with_home(home.path(), false).unwrap();

        let gemini_path = home.path().join(".gemini").join("settings.json");
        assert!(gemini_path.exists());

        // Now uninstall
        let report = uninstall_globally_with_home(home.path(), false).unwrap();

        let content: Value =
            serde_json::from_str(&std::fs::read_to_string(&gemini_path).unwrap()).unwrap();
        assert!(
            content["mcpServers"].get("phronesis").is_none()
                || content["mcpServers"]["phronesis"].is_null(),
            "phronesis entry should be removed from gemini settings"
        );

        assert!(
            report.steps.iter().any(|s| s.contains("gemini")),
            "report should mention gemini removal: {:?}",
            report.steps
        );
    }

    #[test]
    fn uninstall_globally_with_home_removes_from_claude() {
        let home = tempfile::tempdir().unwrap();
        install_globally_with_home(home.path(), false).unwrap();

        let claude_path = home.path().join(".claude.json");
        assert!(claude_path.exists());

        let report = uninstall_globally_with_home(home.path(), false).unwrap();

        let content: Value =
            serde_json::from_str(&std::fs::read_to_string(&claude_path).unwrap()).unwrap();
        assert!(
            content["mcpServers"].get("phronesis").is_none()
                || content["mcpServers"]["phronesis"].is_null(),
            "phronesis entry should be removed from claude settings"
        );

        assert!(
            report.steps.iter().any(|s| s.contains("claude")),
            "report should mention claude removal: {:?}",
            report.steps
        );
    }

    #[test]
    fn uninstall_globally_with_home_idempotent_when_nothing_installed() {
        let home = tempfile::tempdir().unwrap();
        // No install — uninstall should be a no-op without error
        let report = uninstall_globally_with_home(home.path(), false).unwrap();
        assert!(
            report
                .steps
                .iter()
                .any(|s| s.contains("doesn't exist") || s.contains("nothing")),
            "should report nothing to uninstall: {:?}",
            report.steps
        );
    }

    #[test]
    fn write_settings_includes_session_start_and_user_prompt_submit_hooks() {
        let dir = tempfile::tempdir().unwrap();
        let opts = InitOpts {
            project_root: dir.path().to_path_buf(),
            packs: vec![Pack::Llm],
            force: false,
            dry_run: false,
            rules_only: false,
            hooks_only: false,
        };
        run(opts).unwrap();

        let path = dir.path().join(".claude/settings.local.json");
        let content: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();

        let session = content["hooks"]["SessionStart"].as_array().unwrap();
        assert!(!session.is_empty(), "SessionStart must be wired");
        let session_cmd = session[0]["hooks"][0]["command"].as_str().unwrap();
        assert_eq!(session_cmd, "phr-mcp session-context");

        let prompt = content["hooks"]["UserPromptSubmit"].as_array().unwrap();
        assert!(!prompt.is_empty(), "UserPromptSubmit must be wired");
        let prompt_cmd = prompt[0]["hooks"][0]["command"].as_str().unwrap();
        assert_eq!(prompt_cmd, "phr-mcp turn-context");
    }

    #[test]
    fn write_gemini_settings_includes_session_and_before_agent_hooks() {
        let dir = tempfile::tempdir().expect("tempdir failed");
        let opts = InitOpts {
            project_root: dir.path().to_path_buf(),
            packs: vec![Pack::Llm],
            force: false,
            dry_run: false,
            rules_only: false,
            hooks_only: false,
        };
        run(opts).expect("run failed");

        let path = dir.path().join(".gemini/settings.json");
        let content: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read settings failed"))
                .expect("parse json failed");

        let session = content["hooks"]["SessionStart"]
            .as_array()
            .expect("SessionStart not array");
        let cmd = session[0]["hooks"][0]["command"]
            .as_str()
            .expect("command not string");
        assert_eq!(cmd, "phr-mcp session-context");

        let before = content["hooks"]["BeforeAgent"]
            .as_array()
            .expect("BeforeAgent not array");
        let cmd = before[0]["hooks"][0]["command"]
            .as_str()
            .expect("command not string");
        assert_eq!(cmd, "phr-mcp turn-context");
    }

    #[test]
    fn install_globally_with_home_dry_run_writes_nothing() {
        let home = tempfile::tempdir().unwrap();
        let report = install_globally_with_home(home.path(), true).unwrap();

        assert!(
            !home.path().join(".claude.json").exists(),
            "dry run must not write ~/.claude.json"
        );
        assert!(
            !home.path().join(".gemini").join("settings.json").exists(),
            "dry run must not write ~/.gemini/settings.json"
        );
        assert!(
            !report.steps.is_empty(),
            "dry run should still report planned steps"
        );
    }
}
