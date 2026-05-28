use std::path::PathBuf;

use clap::{Parser, Subcommand};
use phronesis_mcp::{hook, init, server};
use rmcp::{ServiceExt, transport::stdio};

#[derive(Parser)]
#[command(
    name = "phr-mcp",
    version,
    about = "MCP server for the phronesis RETE rules engine"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Start the MCP stdio server (default)
    Serve,
    /// PreToolUse hook: validate proposed Edit/Write against rules
    PreCheck,
    /// PostToolUse hook: validate completed Edit/Write against rules
    PostCheck,
    /// SessionStart hook: emit `additionalContext` JSON summarizing the
    /// active phronesis rules. Exit 0 with empty stdout when no rules file
    /// is present. Wired by `init` into `.claude/settings.local.json` and
    /// `.gemini/settings.json`.
    SessionContext,
    /// UserPromptSubmit (Claude) / BeforeModelRequest (Gemini) hook: emit
    /// `additionalContext` JSON summarizing the last few hook decisions.
    /// Exit 0 with empty stdout when there's nothing recent to report.
    TurnContext {
        /// Number of recent log entries to consider. Default 5.
        #[arg(long, default_value_t = 5)]
        last: usize,
    },
    /// Print a per-rule summary of recent hook activity from
    /// `.phronesis/log.jsonl`. Default output is a terminal table; pass
    /// `--json` for machine-readable output. Read-only, never errors loudly.
    Stats {
        /// Window to consider. Examples: `30m`, `24h`, `7d`, `2w`. Default
        /// is all time. Unparseable input falls back to all time with a
        /// stderr warning.
        #[arg(long)]
        since: Option<String>,
        /// Show only this rule.
        #[arg(long)]
        rule: Option<String>,
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Audit the project tree against opted-in rules. Reports per-rule
    /// violation counts with the affected files and line numbers. Default
    /// output is a terminal table; pass `--json` for machine-readable output.
    /// Exits 0 by default; use `--fail-on warn` or `--fail-on block` to set
    /// a non-zero exit when violations of that level (or higher) exist.
    Audit {
        /// Show only this rule (expands per-file detail with line numbers).
        #[arg(long)]
        rule: Option<String>,
        /// Restrict scan to a subdirectory (default: project root).
        #[arg(long)]
        path: Option<PathBuf>,
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
        /// Exit 1 if violations of this level (or higher) exist. `block`
        /// fails only on blocked violations; `warn` fails on either.
        #[arg(long)]
        fail_on: Option<String>,
    },
    /// Show debt-over-time by diffing the most recent `audit_codebase`
    /// snapshots in the action log. Read-only; always exits 0.
    Trend {
        /// Most-recent N snapshots. Default 5.
        #[arg(long)]
        last: Option<u32>,
        /// Window (e.g. `30d`); overrides `--last` when set. Same values as `values --since`.
        #[arg(long)]
        since: Option<String>,
        /// Restrict to a single rule.
        #[arg(long)]
        rule: Option<String>,
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Detect drift between CLAUDE.md imperatives and the current rule
    /// pack. Heuristic: extracts bullets like "Don't X" / "Always Y" /
    /// "Prefer Z" and matches them against rule contents by token
    /// overlap. Output flags candidates with no confident rule match.
    /// Read-only; always exits 0.
    #[command(name = "claude-md-drift", alias = "drift")]
    ClaudeMdDrift {
        /// Project root (defaults to current directory).
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Convert a rules.json file from the v1 (predicate/args/action_type)
    /// shape to the v2 (when/then/predicate-as-key) shape. Preserves `or`
    /// clauses on disk (does not expand them). Idempotent.
    #[command(name = "migrate-rules")]
    MigrateRules {
        /// Path to the rules.json file to convert.
        path: PathBuf,
        /// Print the converted JSON to stdout; write nothing.
        #[arg(long)]
        dry_run: bool,
        /// Exit 0 if already v2, 1 if v1 (no writes). For CI gating.
        #[arg(long)]
        check: bool,
    },
    /// Detect drift between Claude Code's auto-memory store and the
    /// phronesis rule pack / durable directives file. Classifies each
    /// memory by frontmatter `metadata.type` and scores it against
    /// existing rules and `durable.md` by token overlap. Surfaces
    /// actionable memories without rule coverage and ambient memories
    /// without durable.md coverage. Read-only; always exits 0.
    #[command(name = "memory-drift")]
    MemoryDrift {
        /// Project root (defaults to current directory).
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Memory directory override. Defaults to
        /// `~/.claude/projects/<encoded-cwd>/memory/`.
        #[arg(long)]
        memory_dir: Option<PathBuf>,
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
        /// Emit draft phronesis-rule JSON for each uncovered actionable
        /// memory, on stderr. Drafts include a `// TODO: pick a substring`
        /// placeholder for the condition predicate — the operator picks
        /// the right predicate after review.
        #[arg(long)]
        suggest: bool,
    },
    /// One-command setup for a project. Writes hook config, MCP server
    /// registration, a starter rules file, and updates .gitignore.
    /// Also reachable as `setup` and `configure`.
    #[command(alias = "setup", alias = "configure")]
    Init {
        /// Project root (defaults to current directory).
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Comma-separated starter packs. Available: llm, rust, python,
        /// typescript, swift, none. The `llm` pack carries deflection rules that
        /// catch LLM-bad-behavior phrases ("pre-existing issue", etc.) and
        /// is independent of language. Language packs carry only
        /// language-specific enforcement. Compose freely (e.g. "llm,rust").
        #[arg(long, default_value = "llm")]
        packs: String,
        /// Deprecated alias for --packs. Single value; auto-composed with
        /// `llm` for backward compatibility with the pre-pack-split CLI.
        #[arg(long, hide = true)]
        language: Option<String>,
        /// Overwrite existing rules.json (backed up to rules.json.bak)
        #[arg(long)]
        force: bool,
        /// Print what would be done without writing anything
        #[arg(long)]
        dry_run: bool,
        /// Only touch .phronesis/rules.json. Skip hook config, MCP registration,
        /// and .gitignore. Use with --force to refresh just the rules pack.
        #[arg(long)]
        rules_only: bool,
        /// Only touch hook config. Skip rules.json and .gitignore. Use to
        /// refresh hook wiring without disturbing the rules pack.
        #[arg(long)]
        hooks_only: bool,
    },
    /// Register the phronesis MCP server at user scope (~/.claude.json) so it's
    /// available in every project without per-project .mcp.json. One-time
    /// setup; idempotent. Use `init` per-project for hooks and starter rules.
    Install {
        /// Print what would be done without writing anything
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove the phronesis MCP server from user-scope registration.
    /// Project-level config is not touched.
    Uninstall {
        /// Print what would be done without writing anything
        #[arg(long)]
        dry_run: bool,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => {
            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::from_default_env()
                        .add_directive(tracing::Level::INFO.into()),
                )
                .with_writer(std::io::stderr)
                .with_ansi(false)
                .init();

            let server = server::EpistemeMcp::new();
            // Hydrate from .phronesis/rules.json (silent no-op when missing).
            // Combined with the autosave-on-mutation behavior, this means the
            // session's view stays in sync with what the hook enforces.
            server.autoload().await;
            let service = server.serve(stdio()).await?;
            service.waiting().await?;
            Ok(())
        }
        Command::PreCheck => hook::run_pre_check().await,
        Command::PostCheck => hook::run_post_check().await,
        Command::SessionContext => {
            let root = phronesis_mcp::security::project_root();
            let out = phronesis_mcp::context::run_session_context(
                &root,
                phronesis_mcp::context::DEFAULT_MAX_BYTES,
            );
            if !out.is_empty() {
                println!("{}", out);
            }
            Ok(())
        }
        Command::TurnContext { last } => {
            let root = phronesis_mcp::security::project_root();
            let out = phronesis_mcp::context::run_turn_context(
                &root,
                last,
                phronesis_mcp::context::DEFAULT_MAX_BYTES,
            );
            if !out.is_empty() {
                println!("{}", out);
            }
            Ok(())
        }
        Command::Stats { since, rule, json } => {
            use phronesis_mcp::action_log::{self, ReadOpts};
            use phronesis_mcp::stats::{
                StatsOpts, aggregate, parse_since, render_json, render_table,
            };

            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);

            let since_secs = match since.as_deref() {
                None => None,
                Some(raw) => match parse_since(raw) {
                    Some(secs) => Some(secs),
                    None => {
                        eprintln!(
                            "phronesis: unrecognized --since `{}`, showing all time",
                            raw
                        );
                        None
                    }
                },
            };

            let path = action_log::default_path(&phronesis_mcp::security::project_root());
            let opts_log = ReadOpts {
                kind: Some("hook".to_string()),
                ..ReadOpts::default()
            };
            let entries = action_log::read_recent(&path, &opts_log).unwrap_or_default();

            let values_opts = StatsOpts {
                since_secs,
                rule_filter: rule,
                now_secs: now,
            };
            let values = aggregate(&entries, &values_opts);

            if json {
                println!("{}", render_json(&values));
            } else {
                print!("{}", render_table(&values));
            }
            Ok(())
        }
        Command::Audit {
            rule,
            path,
            json,
            fail_on,
        } => {
            use phronesis_mcp::audit::{AuditOpts, Level, render_json, render_table, run};
            use phronesis_mcp::rules_file;

            let project_root = phronesis_mcp::security::project_root();
            let rules_path = rules_file::default_path(&project_root);
            let rules = match rules_file::read(&rules_path) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("phronesis: cannot read rules file: {}", e);
                    return Ok(());
                }
            };
            if rules.rules.is_empty() {
                eprintln!("phronesis: no rules configured; run `phr-mcp init` first");
                return Ok(());
            }

            let scan_root = path
                .map(|p| {
                    if p.is_absolute() {
                        p
                    } else {
                        project_root.join(p)
                    }
                })
                .unwrap_or_else(|| project_root.clone());

            let opts = AuditOpts {
                project_root: project_root.clone(),
                scan_root,
                rule_filter: rule.clone(),
            };
            let report = run(&rules, &opts);

            // Write a snapshot entry so `phr-mcp trend` can read it.
            // Matches the shape the MCP `audit_codebase` tool writes.
            {
                use phronesis_mcp::action_log::{self, LogEntry};
                let mut per_rule = serde_json::Map::new();
                for r in &report.per_rule {
                    per_rule.insert(
                        r.rule_id.as_str().to_string(),
                        serde_json::json!({
                            "level": r.level.as_str(),
                            "hits": r.hits,
                        }),
                    );
                }
                let blocked: u32 = report
                    .per_rule
                    .iter()
                    .filter(|r| r.level == Level::Block)
                    .map(|r| r.hits)
                    .sum();
                let warned: u32 = report
                    .per_rule
                    .iter()
                    .filter(|r| r.level == Level::Warn)
                    .map(|r| r.hits)
                    .sum();
                let entry = LogEntry::new("mcp", "audit_codebase")
                    .with("files_scanned", report.files_scanned as u64)
                    .with("blocked_total", blocked as u64)
                    .with("warned_total", warned as u64)
                    .with("per_rule", serde_json::Value::Object(per_rule));
                let log_path = action_log::default_path(&project_root);
                let _ = action_log::append(&log_path, &entry);
            }

            if json {
                println!("{}", render_json(&report));
            } else {
                let expand = rule.is_some();
                print!("{}", render_table(&report, expand));
            }

            // Exit code logic.
            let has_block = report.per_rule.iter().any(|r| r.level == Level::Block);
            let has_warn = report.per_rule.iter().any(|r| r.level == Level::Warn);
            let should_fail = match fail_on.as_deref() {
                Some("block") => has_block,
                Some("warn") => has_block || has_warn,
                Some(other) => {
                    eprintln!(
                        "phronesis: unrecognized --fail-on `{}` (expected `block` or `warn`)",
                        other
                    );
                    false
                }
                None => false,
            };
            if should_fail {
                std::process::exit(1);
            }
            Ok(())
        }
        Command::Trend {
            last,
            since,
            rule,
            json,
        } => {
            use phronesis_mcp::action_log::{self, ReadOpts};
            use phronesis_mcp::audit::{
                TrendOpts, compute_trend, render_trend_json, render_trend_table,
            };
            use phronesis_mcp::stats::parse_since;

            let path = action_log::default_path(&phronesis_mcp::security::project_root());
            let opts_log = ReadOpts {
                event: Some("audit_codebase".to_string()),
                ..ReadOpts::default()
            };
            let entries = action_log::read_recent(&path, &opts_log).unwrap_or_default();

            let since_secs = since.as_deref().and_then(parse_since);
            let now_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);

            let opts = TrendOpts {
                last: last.map(|n| n as usize).or(Some(5)),
                since_secs,
                rule_filter: rule,
                now_secs,
            };
            let trend = compute_trend(&entries, &opts);

            if json {
                println!("{}", render_trend_json(&trend));
            } else {
                print!("{}", render_trend_table(&trend));
            }
            Ok(())
        }
        Command::ClaudeMdDrift { path, json } => {
            use phronesis_mcp::claude_md_drift::{DriftError, render_json, render_table, run};
            let root = if path.is_absolute() {
                path
            } else {
                std::env::current_dir()
                    .map(|p| p.join(&path))
                    .unwrap_or(path)
            };
            match run(&root) {
                Ok(report) => {
                    if json {
                        println!("{}", render_json(&report));
                    } else {
                        print!("{}", render_table(&report));
                    }
                    Ok(())
                }
                Err(DriftError::ClaudeMdMissing(p)) => {
                    eprintln!("error: CLAUDE.md not found at {}", p);
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("error: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Command::MigrateRules {
            path,
            dry_run,
            check,
        } => {
            use phronesis_mcp::rules_file::{self, SourceRule};

            // Read raw to detect shape: a rule with "conditions" is v1.
            let raw = std::fs::read_to_string(&path)
                .map_err(|e| anyhow::anyhow!("cannot read {}: {}", path.display(), e))?;
            let parsed: serde_json::Value = serde_json::from_str(&raw)
                .map_err(|e| anyhow::anyhow!("malformed rules file: {}", e))?;
            let is_v1 = parsed
                .get("rules")
                .and_then(|r| r.as_array())
                .map(|arr| arr.iter().any(|r| r.get("conditions").is_some()))
                .unwrap_or(false);

            if check {
                if is_v1 {
                    eprintln!(
                        "{}: pre-v2 schema — run `phr-mcp migrate-rules` to convert",
                        path.display()
                    );
                    std::process::exit(1);
                } else {
                    eprintln!("{}: already v2", path.display());
                    std::process::exit(0);
                }
            }

            // Parse to SourceRules (preserves OR), re-emit as v2.
            let sources: Vec<SourceRule> =
                rules_file::read_source(&path).map_err(|e| anyhow::anyhow!("{}", e))?;

            if dry_run {
                #[derive(serde::Serialize)]
                struct Wrapper<'a> {
                    rules: &'a [SourceRule],
                }
                println!(
                    "{}",
                    serde_json::to_string_pretty(&Wrapper { rules: &sources })?
                );
                return Ok(());
            }

            rules_file::write_source(&path, &sources).map_err(|e| anyhow::anyhow!("{}", e))?;
            println!(
                "migrated {} ({} rule(s)) to v2",
                path.display(),
                sources.len()
            );
            Ok(())
        }
        Command::MemoryDrift {
            path,
            memory_dir,
            json,
            suggest,
        } => {
            use phronesis_mcp::memory_drift::{
                DriftError, default_memory_dir, render_json, render_table, run_with_dir,
                suggest_rule,
            };
            let root = if path.is_absolute() {
                path
            } else {
                std::env::current_dir()
                    .map(|p| p.join(&path))
                    .unwrap_or(path)
            };
            let dir = memory_dir.unwrap_or_else(|| default_memory_dir(&root));
            match run_with_dir(&root, &dir) {
                Ok(report) => {
                    if json {
                        println!("{}", render_json(&report));
                    } else {
                        print!("{}", render_table(&report));
                    }
                    if suggest {
                        let drafts: Vec<String> =
                            report.items.iter().filter_map(suggest_rule).collect();
                        if !drafts.is_empty() {
                            eprintln!("\n--- draft rules for uncovered actionable memories ---\n");
                            for draft in drafts {
                                eprintln!("{}\n", draft);
                            }
                        }
                    }
                    Ok(())
                }
                Err(DriftError::MemoryDirMissing(p)) => {
                    eprintln!("error: memory directory not found at {}", p);
                    eprintln!(
                        "hint: Claude Code creates this directory on first save; \
                         try `--memory-dir <path>` to point elsewhere."
                    );
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("error: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Command::Init {
            path,
            packs,
            language,
            force,
            dry_run,
            rules_only,
            hooks_only,
        } => {
            // Backward-compat: --language X means --packs llm,X (the old bundled
            // behavior), unless the user supplied --packs explicitly with a
            // non-default value. The "none" and "minimal" legacy values are
            // special-cased to preserve their original semantics: "none" = no
            // rules at all, "minimal" = just the deflection pack.
            let packs_str = match &language {
                Some(lang) if packs == "llm" => match lang.to_lowercase().as_str() {
                    "none" => "none".to_string(),
                    "minimal" => "llm".to_string(),
                    other => format!("llm,{}", other),
                },
                _ => packs,
            };
            let pack_list = init::parse_packs(&packs_str).map_err(|e| anyhow::anyhow!("{}", e))?;
            let opts = init::InitOpts {
                project_root: path,
                packs: pack_list,
                force,
                dry_run,
                rules_only,
                hooks_only,
            };
            let report = init::run(opts).map_err(|e| anyhow::anyhow!("{}", e))?;
            for step in &report.steps {
                println!("{}", step);
            }
            for warning in &report.warnings {
                eprintln!("⚠ {}", warning);
            }
            if dry_run {
                println!("\n(dry-run: nothing was written)");
            } else {
                println!(
                    "\nNext: restart Claude Code / Gemini CLI in this project for hooks to take effect."
                );
            }
            Ok(())
        }
        Command::Install { dry_run } => {
            let report = init::install_globally(dry_run).map_err(|e| anyhow::anyhow!("{}", e))?;
            for step in &report.steps {
                println!("{}", step);
            }
            for warning in &report.warnings {
                eprintln!("⚠ {}", warning);
            }
            if dry_run {
                println!("\n(dry-run: nothing was written)");
            } else {
                println!(
                    "\nNext: restart Claude Code / Gemini CLI (any project) to pick up the user-level MCP server."
                );
            }
            Ok(())
        }
        Command::Uninstall { dry_run } => {
            let report = init::uninstall_globally(dry_run).map_err(|e| anyhow::anyhow!("{}", e))?;
            for step in &report.steps {
                println!("{}", step);
            }
            if dry_run {
                println!("\n(dry-run: nothing was written)");
            }
            Ok(())
        }
    }
}
