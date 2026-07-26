//! `phr-mcp` CLI entry point.
//!
//! Clap declares every subcommand on one `Command` enum so `--help`
//! lists them coherently and `main()` dispatches each. Keeping the
//! whole CLI surface in one file mirrors how typical Rust binaries
//! are structured; splitting per-subcommand would scatter the
//! `Command` enum and break the single dispatch site.
//!
//! phronesis-allow: audit-file-loc-high (coherent CLI surface — all
//! subcommand declarations + dispatch live together by design)

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use phronesis_mcp::{hook, init, migrate_extracted, scrub_payload, server};

/// ISO-8601 date string for the local clock (YYYY-MM-DD). Uses chrono,
/// which is already a phronesis-mcp dep (clock_facts).
fn today_iso() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}
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
    /// UserPromptSubmit (Claude) / BeforeAgent (Gemini) hook: emit
    /// `additionalContext` JSON summarizing the last few hook decisions.
    /// Exit 0 with empty stdout when there's nothing recent to report.
    #[command(name = "interaction-context", alias = "turn-context")]
    InteractionContext {
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
    /// Report the confidence band (high/medium/low) and the grounded signals
    /// (compile / tests / known-bug) for the open work unit, or `--subject
    /// <id>`. Read-only; reflects `.phronesis/outcomes/`. See
    /// `docs/specs/SPEC-confidence-scoring.md`.
    Confidence {
        /// Report on a specific subject id instead of the open work unit.
        #[arg(long)]
        subject: Option<String>,
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// List active toolchain definitions (built-in + project).
    /// Shows ID, source, match patterns, and active signal refinements.
    Toolchains {
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Render the `journey_*` facts a derivation pass would assert against
    /// the current `.phronesis/journey/events.jsonl` and `.phronesis/rules.json`
    /// — a "why did this fire" view. Default output is a terminal table; pass
    /// `--json` for machine-readable output. `--explain <rule-id>` filters to
    /// the facts that specific rule references.
    Journey {
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
        /// Filter facts to those a specific rule references.
        #[arg(long, value_name = "RULE-ID")]
        explain: Option<String>,
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
    /// Rewrite pre-0.14.0 `extract_rules` output in a rules.json: strip
    /// bracketed metadata prefixes ([pattern], [anti_pattern], ...), demote
    /// `block` to `warn`, and demote to `log` rules the Rust pack already
    /// enforces structurally. Idempotent. Backs up to rules.json.bak.
    #[command(name = "migrate-extracted-rules")]
    MigrateExtractedRules {
        /// Path to the rules.json file to rewrite.
        path: PathBuf,
        /// Print the migrated JSON to stdout; write nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Regenerate the rule catalogue page from the shipped default packs.
    /// Rewrites the content between the GENERATED RULES markers in-place;
    /// run from the repo root (default --out docs/catalogue.html).
    Catalogue {
        /// Path to the catalogue HTML file to rewrite.
        #[arg(long, default_value = "docs/catalogue.html")]
        out: PathBuf,
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
    /// Detect drift between ADR-style decision documents in
    /// `.phronesis/wiki/decisions/` and the current rule pack.
    /// Heuristic — explicit `enforces:` frontmatter lookups beat
    /// Jaccard fallback. Read-only; always exits 0 on success.
    #[command(name = "wiki-drift")]
    WikiDrift {
        /// Project root (defaults to current directory).
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Override the decisions directory. Defaults to
        /// `<project_root>/.phronesis/wiki/decisions/`.
        #[arg(long)]
        wiki_dir: Option<PathBuf>,
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
        /// Emit draft v2 rule JSON for each uncovered decision, on stderr.
        #[arg(long)]
        suggest: bool,
    },
    /// Wiki-related helpers (scaffold a new ADR-style decision page).
    Decision {
        #[command(subcommand)]
        cmd: DecisionCmd,
    },
    /// Structural code-graph helpers. The graph at `.phronesis/graph.jsonl`
    /// is derived, gitignored state; rebuild it after `git checkout`, `git
    /// mv`, or a rebase, which bypass the PostToolUse sensor.
    Graph {
        #[command(subcommand)]
        cmd: GraphCmd,
    },
    /// One-command setup for a project. Writes hook config, MCP server
    /// registration, a starter rules file, and updates .gitignore.
    /// Also reachable as `setup` and `configure`.
    #[command(alias = "setup", alias = "configure")]
    Init {
        /// Project root (defaults to current directory).
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Comma-separated starter packs. Available: llm, rust, rhai, python,
        /// typescript, swift, confidence, none. The `llm` pack carries
        /// deflection rules that catch LLM-bad-behavior phrases ("pre-existing
        /// issue", etc.) and is independent of language. Language packs carry
        /// only language-specific enforcement. The `confidence` pack adds the
        /// commit-gating rules plus a .phronesis/confidence.json opt-in marker
        /// and a .phronesis/bugs.json registry. Compose freely (e.g.
        /// "llm,rust,confidence").
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
    /// Anonymize a captured payload file for committing as a fixture.
    ///
    /// Rewrites $HOME paths, the username, session ids, and transcript paths;
    /// leaves project-internal content verbatim. Prints scrubbed JSONL to
    /// stdout, or rewrites in place (with a .bak backup) under --write.
    /// After anonymizing, residual-risk detectors flag credential-bearing
    /// URLs, private-key headers, token/secret assignments, secret-suggesting
    /// environment keys, and absolute paths outside the placeholder roots
    /// (errors: nonzero exit, nothing written — not even the backup), plus
    /// email addresses and digit-less possible tokens (warnings on stderr,
    /// exit 0). Diagnostics truncate matched text; suspected secrets are
    /// never echoed in full. Raw JSON input is accepted by default; shape
    /// recognition is a parsing convenience, not a safety guarantee.
    ///
    /// scrub-payload performs deterministic anonymization and detects several
    /// common leak classes. It is not a proof that arbitrary source or
    /// command content contains no secrets. Review scrubbed fixtures before
    /// committing them.
    ScrubPayload {
        /// Capture file (JSONL from PHRONESIS_CAPTURE_DIR) or a single-JSON fixture.
        path: PathBuf,
        /// Rewrite the file in place, backing up the original to <path>.bak.
        #[arg(long)]
        write: bool,
        /// Home directory to scrub (defaults to $HOME).
        #[arg(long)]
        home: Option<String>,
        /// Project root whose paths map to /home/dev/project (defaults to CWD).
        #[arg(long)]
        project_root: Option<String>,
    },
    /// Codex hook adapter — reads a Codex hook JSON payload from stdin and
    /// writes a Codex-specific JSON response to stdout.
    ///
    /// The current event is decoded from stdin's `hook_event_name`. An
    /// optional event argument remains for manual smoke tests.
    ///
    /// Supported tools: `Bash` (command text) and `apply_patch` (patch
    /// parsing). MCP calls and other tools are allowed without comment.
    CodexHook {
        /// The Codex hook event name. The event from stdin takes precedence
        /// when available.
        #[arg(default_value = "PreToolUse")]
        event: String,
    },
}

#[derive(clap::Subcommand, Debug)]
enum GraphCmd {
    /// Rescan every tracked Rust file and rewrite the graph from scratch.
    Rebuild {
        /// Project root (defaults to current directory).
        #[arg(long, default_value = ".")]
        path: PathBuf,
        /// Emit JSON instead of a human-readable summary.
        #[arg(long)]
        json: bool,
    },
    /// Report whether the graph still matches what is on disk.
    Status {
        /// Project root (defaults to current directory).
        #[arg(long, default_value = ".")]
        path: PathBuf,
        /// Emit JSON instead of a human-readable summary.
        #[arg(long)]
        json: bool,
    },
}

#[derive(clap::Subcommand, Debug)]
enum DecisionCmd {
    /// Scaffold a new decision page at
    /// `.phronesis/wiki/decisions/<today>-<slug>.md`.
    New {
        /// Kebab-case slug for the decision. Must match `[a-z0-9-]+`.
        slug: String,
        /// Project root (defaults to current directory).
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => handle_serve().await,
        Command::PreCheck => hook::run_pre_check().await,
        Command::PostCheck => hook::run_post_check().await,
        Command::SessionContext => handle_session_context(),
        Command::InteractionContext { last } => handle_interaction_context(last),
        Command::Stats { since, rule, json } => handle_stats(since, rule, json),
        Command::Confidence { subject, json } => handle_confidence(subject, json),
        Command::Toolchains { json } => handle_toolchains(json),
        Command::Journey { json, explain } => handle_journey(json, explain).await,
        Command::Audit {
            rule,
            path,
            json,
            fail_on,
        } => handle_audit(rule, path, json, fail_on).await,
        Command::Trend {
            last,
            since,
            rule,
            json,
        } => handle_trend(last, since, rule, json),
        Command::ClaudeMdDrift { path, json } => handle_claude_md_drift(path, json),
        Command::MigrateRules {
            path,
            dry_run,
            check,
        } => handle_migrate_rules(path, dry_run, check),
        Command::MigrateExtractedRules { path, dry_run } => {
            handle_migrate_extracted_rules(path, dry_run)
        }
        Command::Catalogue { out } => handle_catalogue(out),
        Command::MemoryDrift {
            path,
            memory_dir,
            json,
            suggest,
        } => handle_memory_drift(path, memory_dir, json, suggest),
        Command::WikiDrift {
            path,
            wiki_dir,
            json,
            suggest,
        } => handle_wiki_drift(path, wiki_dir, json, suggest),
        Command::Decision { cmd } => handle_decision(cmd),
        Command::Graph { cmd } => handle_graph(cmd),
        Command::Init {
            path,
            packs,
            language,
            force,
            dry_run,
            rules_only,
            hooks_only,
        } => handle_init(InitCtx {
            path,
            packs,
            language,
            force,
            dry_run,
            rules_only,
            hooks_only,
        }),
        Command::Install { dry_run } => handle_install(dry_run),
        Command::Uninstall { dry_run } => handle_uninstall(dry_run),
        Command::ScrubPayload {
            path,
            write,
            home,
            project_root,
        } => scrub_payload::run(&path, write, home, project_root),
        Command::CodexHook { event } => phronesis_mcp::codex_hook::run(&event).await,
    }
}

async fn handle_serve() -> anyhow::Result<()> {
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

fn handle_session_context() -> anyhow::Result<()> {
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

fn handle_interaction_context(last: usize) -> anyhow::Result<()> {
    let root = phronesis_mcp::security::project_root();
    let out = phronesis_mcp::context::run_interaction_context(
        &root,
        last,
        phronesis_mcp::context::DEFAULT_MAX_BYTES,
    );
    if !out.is_empty() {
        println!("{}", out);
    }
    Ok(())
}

fn handle_stats(since: Option<String>, rule: Option<String>, json: bool) -> anyhow::Result<()> {
    use phronesis_mcp::action_log::{self, ReadOpts};
    use phronesis_mcp::stats::{StatsOpts, aggregate, parse_since, render_json, render_table};

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

fn handle_confidence(subject: Option<String>, json: bool) -> anyhow::Result<()> {
    let root = phronesis_mcp::security::project_root();
    match phronesis_mcp::outcomes::report(&root, subject.as_deref()) {
        Some(r) => {
            if json {
                let out = serde_json::json!({
                    "subject": r.subject,
                    "band": r.band.as_str(),
                    "signals": r.signals,
                });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                let signals = if r.signals.is_empty() {
                    "(none)".to_string()
                } else {
                    r.signals.join(", ")
                };
                println!("subject:    {}", r.subject);
                println!("confidence: {}", r.band.as_str());
                println!("signals:    {signals}");
            }
        }
        None => {
            if json {
                println!("{}", serde_json::json!({ "subject": null }));
            } else {
                println!("No open work unit. Run a build/test under the hook first.");
            }
        }
    }
    Ok(())
}

fn handle_toolchains(json: bool) -> anyhow::Result<()> {
    let root = phronesis_mcp::security::project_root();
    let defs = phronesis_mcp::outcomes::toolchain::registry(&root);
    if json {
        let items: Vec<serde_json::Value> = defs
            .iter()
            .map(|d| {
                let mut v = serde_json::to_value(&d.def).unwrap_or_default();
                if let Some(obj) = v.as_object_mut() {
                    obj.insert("source".to_string(), d.source.as_str().into());
                }
                v
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
        return Ok(());
    }
    if defs.is_empty() {
        println!("no toolchain definitions active");
        return Ok(());
    }
    println!("{:<12} {:<10} {:<40} SIGNALS", "ID", "SOURCE", "MATCHES");
    for d in &defs {
        let mut signals = vec!["exit"];
        if !d.def.compile_fail.is_empty() {
            signals.push("compile-fail");
        }
        if d.def.test_summary.is_some() {
            signals.push("summary");
        }
        if d.def.per_test.is_some() {
            signals.push("per-test");
        }
        println!(
            "{:<12} {:<10} {:<40} {}",
            d.def.id,
            d.source.as_str(),
            d.def.matches,
            signals.join("+")
        );
    }
    Ok(())
}

async fn handle_journey(json: bool, explain: Option<String>) -> anyhow::Result<()> {
    use phronesis_mcp::journey_cli;
    let root = phronesis_mcp::security::project_root();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Single source of truth for the sid — read-or-create at
    // `.phronesis/journey/session` (see `journey::current_sid`).
    let sid = phronesis_mcp::journey::current_sid(&root);
    let rows = match journey_cli::compute(&root, explain.as_deref(), now, &sid).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {}", e);
            std::process::exit(1);
        }
    };
    if json {
        match journey_cli::render_json(&rows) {
            Ok(s) => println!("{}", s),
            Err(e) => {
                eprintln!("error: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        print!("{}", journey_cli::render_table(&rows));
    }
    Ok(())
}

async fn handle_audit(
    rule: Option<String>,
    path: Option<PathBuf>,
    json: bool,
    fail_on: Option<String>,
) -> anyhow::Result<()> {
    use phronesis_mcp::audit::{AuditOpts, Level, render_json, render_table, run};
    use phronesis_mcp::rules_file;

    let project_root = phronesis_mcp::security::project_root();

    let rules = {
        let rules_path = rules_file::default_path(&project_root);
        match rules_file::read(&rules_path) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("phronesis: cannot read rules file: {}", e);
                return Ok(());
            }
        }
    };
    if rules.rules.is_empty() {
        eprintln!("phronesis: no rules configured; run `phr-mcp init` first");
        return Ok(());
    }

    let (report, diag) = {
        // Convert Option<PathBuf> → Option<&str> at the call site so we can
        // use the shared `resolve_scan_root` (which takes `Option<&str>`).
        // CLI paths come from clap and are always valid UTF-8; `to_str()`
        // failing would mean a non-UTF8 path, which would fall back to
        // project_root — the same behavior as the old `unwrap_or_else`.
        let path_str = path.as_ref().and_then(|p| p.to_str());
        let scan_root = phronesis_mcp::audit::resolve_scan_root(path_str, &project_root);
        let opts = AuditOpts {
            project_root: project_root.clone(),
            scan_root,
            rule_filter: rule.clone(),
        };
        let mut report = run(&rules, &opts);
        // Structural rules join relations across the whole repository, so the
        // file-scan loop above skips them. Evaluate them against the graph and
        // fold the findings in.
        {
            let engine_rules: Vec<phr::Rule> = rules
                .rules
                .iter()
                .filter(|r| r.audit == Some(true))
                .map(|r| rules_file::rule_from_disk(r).0)
                .collect();
            let hits =
                phronesis_mcp::graph::audit::audit_graph_rules(&project_root, &engine_rules).await;
            phronesis_mcp::audit::merge_graph_hits(&mut report, &hits, rule.as_deref());
        }
        let report = report;
        let audit_tagged_count = rules.rules.iter().filter(|r| r.audit == Some(true)).count();
        let diag = phronesis_mcp::audit::empty_result_diagnostic(
            &report,
            audit_tagged_count,
            &opts.scan_root,
        );
        (report, diag)
    };

    // Write a snapshot entry so `phr-mcp trend` can read it.
    // Shared helper keeps field names byte-identical to the MCP writer.
    {
        use phronesis_mcp::action_log::{self, LogEntry};
        let entry = phronesis_mcp::audit::audit_snapshot_entry(
            LogEntry::new("mcp", "audit_codebase"),
            &report,
        );
        let log_path = action_log::default_path(&project_root);
        let _ = action_log::append(&log_path, &entry);
    }
    if json {
        println!("{}", render_json(&report));
        if let Some(msg) = &diag {
            eprintln!("{}", msg);
        }
    } else if let Some(msg) = &diag {
        eprintln!("{}", msg);
    } else {
        print!("{}", render_table(&report, rule.is_some()));
    }

    // Exit code logic.
    let should_fail = match fail_on.as_deref() {
        Some("block") => report.per_rule.iter().any(|r| r.level == Level::Block),
        Some("warn") => {
            report.per_rule.iter().any(|r| r.level == Level::Block)
                || report.per_rule.iter().any(|r| r.level == Level::Warn)
        }
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

fn handle_trend(
    last: Option<u32>,
    since: Option<String>,
    rule: Option<String>,
    json: bool,
) -> anyhow::Result<()> {
    use phronesis_mcp::action_log::{self, ReadOpts};
    use phronesis_mcp::audit::{TrendOpts, compute_trend, render_trend_json, render_trend_table};
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

fn handle_claude_md_drift(path: PathBuf, json: bool) -> anyhow::Result<()> {
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

fn handle_migrate_rules(path: PathBuf, dry_run: bool, check: bool) -> anyhow::Result<()> {
    use phronesis_mcp::rules_file::{self, SourceRule};

    // Read raw to detect shape: a rule with "conditions" is v1.
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("cannot read {}: {}", path.display(), e))?;
    let parsed: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| anyhow::anyhow!("malformed rules file: {}", e))?;
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

fn handle_catalogue(out: PathBuf) -> anyhow::Result<()> {
    use phronesis_mcp::catalogue;

    let page = match std::fs::read_to_string(&out) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: cannot read {}: {e}", out.display());
            std::process::exit(1);
        }
    };
    let generated = catalogue::render_rules_html();
    let spliced = match catalogue::splice(&page, &generated) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {} in {}", e, out.display());
            std::process::exit(1);
        }
    };
    std::fs::write(&out, &spliced)?;
    let rules = generated.matches("<article class=\"rule\"").count();
    println!(
        "regenerated {} ({} rules) at v{}",
        out.display(),
        rules,
        env!("CARGO_PKG_VERSION")
    );
    Ok(())
}

fn handle_migrate_extracted_rules(path: PathBuf, dry_run: bool) -> anyhow::Result<()> {
    use phronesis_mcp::rules_file::{self, SourceRule};

    let mut sources: Vec<SourceRule> =
        rules_file::read_source(&path).map_err(|e| anyhow::anyhow!("{}", e))?;
    let summary = migrate_extracted::migrate_extracted(&mut sources);

    if summary.examined == 0 {
        println!("no extracted rules found in {}", path.display());
        return Ok(());
    }
    if summary.changed == 0 {
        println!(
            "{} extracted rule(s) already migrated in {}; nothing to do",
            summary.examined,
            path.display()
        );
        return Ok(());
    }

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
        "migrated {} extracted rule(s) in {} ({} prefix(es) stripped, {} demoted to warn, {} demoted to log)",
        summary.changed,
        path.display(),
        summary.prefixes_stripped,
        summary.demoted_to_warn,
        summary.demoted_to_log
    );
    Ok(())
}

fn handle_memory_drift(
    path: PathBuf,
    memory_dir: Option<PathBuf>,
    json: bool,
    suggest: bool,
) -> anyhow::Result<()> {
    use phronesis_mcp::memory_drift::{
        DriftError, default_memory_dir, render_json, render_table, run_with_dir, suggest_rule,
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
                let drafts: Vec<String> = report.items.iter().filter_map(suggest_rule).collect();
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

fn handle_wiki_drift(
    path: PathBuf,
    wiki_dir: Option<PathBuf>,
    json: bool,
    suggest: bool,
) -> anyhow::Result<()> {
    use phronesis_mcp::wiki;
    use phronesis_mcp::wiki_drift::{
        DriftError, render_json, render_table, run_with_dir, suggest_rule,
    };
    let root = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map(|p| p.join(&path))
            .unwrap_or(path)
    };
    let dir = wiki_dir.unwrap_or_else(|| wiki::default_wiki_dir(&root).join("decisions"));
    match run_with_dir(&root, &dir) {
        Ok(report) => {
            if json {
                println!("{}", render_json(&report));
            } else {
                print!("{}", render_table(&report));
            }
            if suggest {
                let drafts: Vec<String> = report.items.iter().filter_map(suggest_rule).collect();
                if !drafts.is_empty() {
                    eprintln!("\n--- draft rules for uncovered decisions ---\n");
                    for draft in drafts {
                        eprintln!("{}\n", draft);
                    }
                }
            }
            Ok(())
        }
        Err(DriftError::Wiki(phronesis_mcp::wiki::WikiError::DirMissing(p))) => {
            eprintln!("error: wiki decisions directory not found at {}", p);
            eprintln!("hint: run `phr-mcp init` to create it, or pass `--wiki-dir <path>`.");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("error: {}", e);
            std::process::exit(1);
        }
    }
}

fn handle_graph(cmd: GraphCmd) -> anyhow::Result<()> {
    use phronesis_mcp::graph::sync;

    match cmd {
        GraphCmd::Rebuild { path, json } => {
            let out = sync::rebuild(&path)?;
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "base_edges": out.base,
                        "derived_edges": out.derived,
                        "skipped_items": out.skipped,
                    })
                );
            } else {
                println!(
                    "Rebuilt graph: {} base edges, {} derived, {} items skipped.",
                    out.base, out.derived, out.skipped
                );
            }
            Ok(())
        }
        GraphCmd::Status { path, json } => {
            let index = sync::load_index(&sync::index_path(&path))?;
            let drifted = match sync::check_freshness(&path, &index) {
                sync::Freshness::Fresh => Vec::new(),
                sync::Freshness::Stale(files) => files,
            };
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "fresh": drifted.is_empty(),
                        "drifted_files": drifted,
                    })
                );
            } else if drifted.is_empty() {
                println!("Graph is fresh.");
            } else {
                println!(
                    "Graph is stale: {} file(s) drifted. Structural rules will warn, not block.",
                    drifted.len()
                );
                for f in drifted.iter().take(10) {
                    println!("  {f}");
                }
                println!("Run `phr-mcp graph rebuild` to resync.");
            }
            Ok(())
        }
    }
}

fn handle_decision(cmd: DecisionCmd) -> anyhow::Result<()> {
    match cmd {
        DecisionCmd::New { slug, path } => {
            use phronesis_mcp::wiki;
            let root = if path.is_absolute() {
                path
            } else {
                std::env::current_dir()
                    .map(|p| p.join(&path))
                    .unwrap_or(path)
            };
            // Validate slug: kebab-case, alphanumeric + hyphen, non-empty.
            let valid = !slug.is_empty()
                && slug
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
            if !valid {
                eprintln!(
                    "error: invalid slug `{}`. Slugs must match `[a-z0-9-]+` (kebab-case).",
                    slug
                );
                std::process::exit(1);
            }

            let date = today_iso();
            let dir = wiki::default_wiki_dir(&root).join("decisions");
            let filename = format!("{}-{}.md", date, slug);
            let dest = dir.join(&filename);
            if dest.exists() {
                eprintln!(
                    "error: {} already exists; refusing to overwrite.",
                    dest.display()
                );
                std::process::exit(1);
            }
            std::fs::create_dir_all(&dir)
                .map_err(|e| anyhow::anyhow!("create {}: {}", dir.display(), e))?;

            let template = format!(
                "---\n\
                 id: {slug}\n\
                 date: {date}\n\
                 status: proposed\n\
                 enforces: []\n\
                 superseded_by: null\n\
                 tags: []\n\
                 ---\n\
                 \n\
                 # {slug}\n\
                 \n\
                 ## Context\n\
                 \n\
                 What problem are we solving / what observations led here?\n\
                 \n\
                 ## Decision\n\
                 \n\
                 What we decided.\n\
                 \n\
                 ## Enforcement\n\
                 \n\
                 - (none yet — add `enforces:` rule ids in frontmatter when a rule lands)\n\
                 \n\
                 ## Consequences\n\
                 \n\
                 What follows from this.\n",
                slug = slug,
                date = date,
            );
            std::fs::write(&dest, template)
                .map_err(|e| anyhow::anyhow!("write {}: {}", dest.display(), e))?;
            println!("created {}", dest.display());
            Ok(())
        }
    }
}

/// Context for the `init` subcommand. Bundled so the handler stays at one
/// parameter instead of seven.
struct InitCtx {
    path: PathBuf,
    packs: String,
    language: Option<String>,
    force: bool,
    dry_run: bool,
    rules_only: bool,
    hooks_only: bool,
}

fn handle_init(ctx: InitCtx) -> anyhow::Result<()> {
    let packs_str = match &ctx.language {
        Some(lang) if ctx.packs == "llm" => match lang.to_lowercase().as_str() {
            "none" => "none".to_string(),
            "minimal" => "llm".to_string(),
            other => format!("llm,{}", other),
        },
        _ => ctx.packs,
    };
    let pack_list = init::parse_packs(&packs_str).map_err(|e| anyhow::anyhow!("{}", e))?;
    let opts = init::InitOpts {
        project_root: ctx.path,
        packs: pack_list,
        force: ctx.force,
        dry_run: ctx.dry_run,
        rules_only: ctx.rules_only,
        hooks_only: ctx.hooks_only,
    };
    let report = init::run(opts).map_err(|e| anyhow::anyhow!("{}", e))?;
    for step in &report.steps {
        println!("{}", step);
    }
    for warning in &report.warnings {
        eprintln!("⚠ {}", warning);
    }
    if ctx.dry_run {
        println!("\n(dry-run: nothing was written)");
    } else {
        println!(
            "\nNext: restart Claude Code / Gemini CLI, or review project hooks with Codex `/hooks`."
        );
    }
    Ok(())
}

fn handle_install(dry_run: bool) -> anyhow::Result<()> {
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
            "\nNext: restart Claude Code / Gemini CLI to pick up the user-level MCP server. \
             Codex setup is project-local; run `phr-mcp init --hooks-only` in each Codex project, \
             then review the generated hooks with `/hooks`."
        );
    }
    Ok(())
}

fn handle_uninstall(dry_run: bool) -> anyhow::Result<()> {
    let report = init::uninstall_globally(dry_run).map_err(|e| anyhow::anyhow!("{}", e))?;
    for step in &report.steps {
        println!("{}", step);
    }
    if dry_run {
        println!("\n(dry-run: nothing was written)");
    }
    Ok(())
}
