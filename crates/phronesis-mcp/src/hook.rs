use std::path::Path;
use std::process;

use phr::{Fact, ReteNetwork, Rule};
use serde::Deserialize;
use thiserror::Error;

use crate::action_log::{self, LogEntry};
use crate::diff_extract;
use crate::journey;
use crate::outcomes;
use crate::security::{
    self, MAX_FACT_CONTENT_BYTES, read_file_capped, read_stdin_capped, resolve_safe_path,
};

#[derive(Debug, Error)]
enum RulesLoadError {
    #[error("rules file at {path} could not be loaded: {message}")]
    Load { path: String, message: String },
}

/// Hook-internal error type. Engine failures arrive typed as
/// [`phr::ReteError`]; the `Engine(String)` variant remains for non-engine
/// string sources. Future variants (e.g. `FactValidation`,
/// `ContentTooLarge`) can be added without changing helper signatures.
#[derive(Debug, Error)]
pub(crate) enum HookError {
    #[error("RETE engine error: {0}")]
    Engine(String),
    #[error("RETE engine error: {0}")]
    Rete(#[from] phr::ReteError),
}

impl From<String> for HookError {
    fn from(s: String) -> Self {
        HookError::Engine(s)
    }
}

#[derive(Debug, Deserialize)]
struct HookPayload {
    tool_name: Option<String>,
    tool_input: Option<serde_json::Value>,
    /// PostToolUse payloads carry the tool's output here. Claude Code sends
    /// this field as `tool_response`; Gemini and our own integration tests
    /// use `tool_output`. The serde alias accepts both so confidence scoring
    /// sees the captured stdout/stderr of a build/test command regardless of
    /// which runtime fired the hook.
    #[serde(default, alias = "tool_response")]
    tool_output: Option<serde_json::Value>,
}

/// Print `{}` to stdout and exit 0.
///
/// Gemini's hook protocol requires parseable JSON on stdout for every exit-0
/// response. Claude Code ignores stdout on exit 0, so this is safe for both
/// runtimes. Call this instead of `process::exit(0)` everywhere the hook
/// decides to allow without comment.
fn exit_ok() -> ! {
    println!("{{}}");
    process::exit(0);
}

/// PreToolUse hook: validate proposed changes before they happen.
/// Exit 0 = allow, exit 2 = block.
///
/// Fails closed (exit 2) when:
/// - stdin payload exceeds the size cap
/// - `.phronesis/rules.json` exists but is malformed
/// - rule loading, fact assertion, or rule firing fails
pub async fn run_pre_check() -> anyhow::Result<()> {
    let payload = match read_payload() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("phronesis: BLOCKED — invalid hook payload: {}", e);
            process::exit(2);
        }
    };

    let tool_name = match &payload.tool_name {
        Some(name)
            if name == "Edit"
                || name == "Write"
                || name == "MultiEdit"
                || name == "Bash"
                || name == "replace"
                || name == "write_file"
                || name == "run_shell_command" =>
        {
            name.clone()
        }
        _ => exit_ok(),
    };

    let rules = match load_rules("pre") {
        Ok(Some(rules)) => rules,
        Ok(None) => exit_ok(),
        Err(e) => {
            eprintln!("phronesis: BLOCKED — {}", e);
            process::exit(2);
        }
    };

    // Collect substring patterns the rules want scanned — drives the
    // pattern-fact assertion below, so new rules don't require code changes.
    let content_patterns = collect_content_patterns(&rules);
    let bash_command_patterns = collect_bash_command_patterns(&rules);

    let new_content = extract_new_content(&payload, &tool_name);
    let file_path = extract_file_path(&payload);

    let mut network = ReteNetwork::new();

    let rules_for_journey: Vec<Rule> = rules.clone();
    for rule in rules {
        if let Err(e) = network.add_rule(rule).await {
            eprintln!("phronesis: BLOCKED — failed to load rule: {}", e);
            process::exit(2);
        }
    }

    if let Err(e) = assert_common_facts(&network, &file_path, &tool_name, "pre").await {
        eprintln!("phronesis: BLOCKED — failed to assert facts: {}", e);
        process::exit(2);
    }

    // Journey facts: recomputed every invocation from the durable journal.
    // Fail-open — never block on a missing or corrupt journal.
    let project_root_pre = security::project_root();
    assert_journey_facts_into(&mut network, &project_root_pre, &rules_for_journey).await;

    // Confidence gate: assert the open work unit's grounded signals *before* any
    // command/content facts, so a gate rule's `__script__` count is evaluated
    // against the full signal set when its `bash_command_matches` condition is
    // asserted (the agenda updates incrementally per fact). Opt-in, fail-open —
    // never blocks an edit on a confidence-subsystem hiccup.
    assert_confidence_signals(&network).await;

    let old_content = extract_old_content(&payload, &tool_name);

    if let Some(content) = &new_content {
        if let Err(e) = network
            .assert_fact(Fact {
                id: "new_content".to_string(),
                predicate: "new_content".to_string(),
                args: vec![content.clone()],
                timestamp: 0,
            })
            .await
        {
            eprintln!("phronesis: BLOCKED — failed to assert content fact: {}", e);
            process::exit(2);
        }

        if let Err(e) =
            check_content_patterns(&network, &file_path, content, &content_patterns).await
        {
            eprintln!("phronesis: BLOCKED — pattern check failed: {}", e);
            process::exit(2);
        }

        // Command-content regexes apply only to command tools — file
        // content quoting the same text must not trip command rules.
        if matches!(tool_name.as_str(), "Bash" | "run_shell_command")
            && let Err(e) =
                check_bash_command_patterns(&network, content, &bash_command_patterns).await
        {
            eprintln!("phronesis: BLOCKED — command pattern check failed: {}", e);
            process::exit(2);
        }

        // Cargo-workspace scanner: applies to Bash command content as well as
        // file content. Has no file-extension gate, so it can't go through
        // DiffFacts::extract (which returns empty for unknown extensions).
        for cmd in diff_extract::cargo_commands_lacking_workspace(content) {
            let fact_id = format!("cargo_command_lacks_workspace_{}", cmd.replace(' ', "_"));
            network
                .assert_fact(Fact {
                    id: fact_id,
                    predicate: "cargo_command_lacks_workspace".to_string(),
                    args: vec![cmd],
                    timestamp: 0,
                })
                .await
                .ok();
        }

        // Diff-aware structural facts (function_added/removed, import_added/removed).
        if let Err(e) =
            assert_diff_facts(&network, &file_path, old_content.as_deref(), content).await
        {
            eprintln!("phronesis: BLOCKED — diff-fact assertion failed: {}", e);
            process::exit(2);
        }

        // Syntax-aware structural facts (function_returns_result_string, etc.)
        // At pre-check, the edit hasn't applied yet, so disk still holds the
        // prior content — read it for the delta filter on heavy-clone facts.
        // Resolve `file_path` against the project root so the read works
        // regardless of process cwd (matches the post-check path-resolution).
        let old_disk_content = if !file_path.is_empty() {
            let root = security::project_root();
            match resolve_safe_path(&file_path, &root) {
                Ok(safe) => std::fs::read_to_string(&safe).ok(),
                Err(_) => None,
            }
        } else {
            None
        };
        if let Err(e) =
            assert_values_facts(&network, &file_path, content, old_disk_content.as_deref()).await
        {
            eprintln!("phronesis: BLOCKED — values-fact assertion failed: {}", e);
            process::exit(2);
        }

        // TDD support: for each newly-added function, assert test_exists_for / no_test_for.
        let added =
            diff_extract::extract(&file_path, old_content.as_deref(), content).functions_added;
        let project_root = security::project_root();
        if let Err(e) = assert_test_facts(&network, &project_root, &file_path, &added).await {
            eprintln!("phronesis: BLOCKED — test-fact assertion failed: {}", e);
            process::exit(2);
        }
    }

    if let Err(e) = network.update_agenda().await {
        eprintln!("phronesis: BLOCKED — agenda update failed: {}", e);
        process::exit(2);
    }
    let consequences = match network.fire_all_consequences() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("phronesis: BLOCKED — rule execution failed: {}", e);
            process::exit(2);
        }
    };

    let logged: Vec<LoggedConsequence> = consequences
        .iter()
        .filter_map(LoggedConsequence::from_consequence)
        .collect();
    let (violations, warnings) = split_messages_by_action_type(&logged);

    // Severity order: violations (block) > warnings (allow-with-message) > clean.
    if !violations.is_empty() {
        for v in &violations {
            eprintln!("phronesis: BLOCKED — {}", v);
        }
        // Surface any warnings alongside the block so the agent sees the
        // full picture, not just the first reason the edit was rejected.
        for w in &warnings {
            eprintln!("phronesis: WARNING — {}", w);
        }
        log_hook_event("pre", &tool_name, &file_path, 2, &logged);
        process::exit(2);
    }

    if !warnings.is_empty() {
        for w in &warnings {
            eprintln!("phronesis: WARNING — {}", w);
        }
        log_hook_event("pre", &tool_name, &file_path, 1, &logged);
        process::exit(1);
    }

    log_hook_event("pre", &tool_name, &file_path, 0, &logged);
    exit_ok();
}

/// PostToolUse hook: validate the result after edit/write.
/// Exit 0 = pass, exit 1 = warn (Claude sees the message and can self-correct).
///
/// Warns (exit 1) when:
/// - stdin payload exceeds the size cap
/// - `.phronesis/rules.json` is malformed
/// - `file_path` resolves outside the project root
/// - rule loading or firing fails
pub async fn run_post_check() -> anyhow::Result<()> {
    let payload = match read_payload() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("phronesis: WARNING — invalid hook payload: {}", e);
            process::exit(1);
        }
    };

    let tool_name = match &payload.tool_name {
        Some(name)
            if name == "Edit"
                || name == "Write"
                || name == "MultiEdit"
                || name == "Bash"
                || name == "replace"
                || name == "write_file"
                || name == "run_shell_command" =>
        {
            name.clone()
        }
        _ => exit_ok(),
    };

    // Resolve project root once for the post-check journey paths below.
    let project_root_post = security::project_root();
    let file_path_for_journal = extract_file_path(&payload);

    let rules = match load_rules("post") {
        Ok(Some(rules)) => rules,
        Ok(None) => {
            // No post rules — still journal the executed call so future
            // pre-checks see this in journey aggregators. Journey is
            // fail-open and best-effort.
            journey_record_post(&payload, &tool_name, &file_path_for_journal).await;
            exit_ok();
        }
        Err(e) => {
            eprintln!("phronesis: WARNING — {}", e);
            // The call already executed; journal it before exiting with a
            // warning code.
            journey_record_post(&payload, &tool_name, &file_path_for_journal).await;
            process::exit(1);
        }
    };

    // Drive pattern scans from the loaded rules' conditions.
    let content_patterns = collect_content_patterns(&rules);
    let bash_command_patterns = collect_bash_command_patterns(&rules);
    let missing_patterns = collect_missing_patterns(&rules);

    let file_path = extract_file_path(&payload);

    let mut network = ReteNetwork::new();

    let rules_for_journey: Vec<Rule> = rules.clone();
    for rule in rules {
        if let Err(e) = network.add_rule(rule).await {
            eprintln!("phronesis: WARNING — failed to load rule: {}", e);
            process::exit(1);
        }
    }

    if let Err(e) = assert_common_facts(&network, &file_path, &tool_name, "post").await {
        eprintln!("phronesis: WARNING — failed to assert facts: {}", e);
        process::exit(1);
    }

    // Journey facts: recomputed every invocation from the durable journal,
    // before update_agenda — fail-open.
    assert_journey_facts_into(&mut network, &project_root_post, &rules_for_journey).await;

    // Validate the file path is inside the project root before reading.
    // An empty file_path means the hook input didn't include one — skip file read.
    let content_opt = if file_path.is_empty() {
        None
    } else {
        let root = security::project_root();
        match resolve_safe_path(&file_path, &root) {
            Ok(safe) => match read_file_capped(&safe) {
                Ok(content) => Some(content),
                Err(e) => {
                    eprintln!("phronesis: WARNING — could not read file: {}", e);
                    None
                }
            },
            Err(security::SecurityError::PathOutsideRoot(_))
            | Err(security::SecurityError::PathTraversal(_)) => {
                eprintln!(
                    "phronesis: WARNING — file_path {:?} is outside project root",
                    file_path
                );
                process::exit(1);
            }
            Err(_) => None,
        }
    };

    // Command tools carry no file_path, so the disk-read above yields no
    // content for them — their "content" is the command itself, taken from
    // the payload. Post-phase command rules are advisory (the command
    // already ran); they warn so the agent can correct course.
    if matches!(tool_name.as_str(), "Bash" | "run_shell_command")
        && let Some(command) = extract_new_content(&payload, &tool_name)
        && let Err(e) =
            check_bash_command_patterns(&network, &command, &bash_command_patterns).await
    {
        eprintln!("phronesis: WARNING — command pattern check failed: {}", e);
        process::exit(1);
    }

    if let Some(content) = &content_opt {
        // Only assert the full content as a fact when small enough to keep
        // working-memory growth bounded. Pattern checks below still operate on
        // the in-memory slice and emit the targeted predicates.
        if content.len() <= MAX_FACT_CONTENT_BYTES
            && let Err(e) = network
                .assert_fact(Fact {
                    id: "file_content".to_string(),
                    predicate: "file_content".to_string(),
                    args: vec![content.clone()],
                    timestamp: 0,
                })
                .await
        {
            eprintln!("phronesis: WARNING — failed to assert content fact: {}", e);
            process::exit(1);
        }

        if let Err(e) =
            check_content_patterns(&network, &file_path, content, &content_patterns).await
        {
            eprintln!("phronesis: WARNING — pattern check failed: {}", e);
            process::exit(1);
        }

        // Cargo-workspace scanner: applies to Bash command content as well as
        // file content. Has no file-extension gate, so it can't go through
        // DiffFacts::extract (which returns empty for unknown extensions).
        for cmd in diff_extract::cargo_commands_lacking_workspace(content) {
            let fact_id = format!("cargo_command_lacks_workspace_{}", cmd.replace(' ', "_"));
            network
                .assert_fact(Fact {
                    id: fact_id,
                    predicate: "cargo_command_lacks_workspace".to_string(),
                    args: vec![cmd],
                    timestamp: 0,
                })
                .await
                .ok();
        }

        if let Err(e) = check_missing_patterns(&network, content, &missing_patterns).await {
            eprintln!("phronesis: WARNING — missing-pattern check failed: {}", e);
            process::exit(1);
        }

        // For post-check, no `old`: treat every function/import in the resulting
        // file as "present" (added). Useful for rules that check the final state
        // (e.g., "every test file must have at least one `assert`").
        if let Err(e) = assert_diff_facts(&network, &file_path, None, content).await {
            eprintln!("phronesis: WARNING — diff-fact assertion failed: {}", e);
            process::exit(1);
        }

        // Syntax-aware structural facts so post-phase rules using AST-derived
        // predicates (function_is_public, function_param_type, struct_derives,
        // function_throws, function_uses_force_unwrap, ...) can fire on the
        // final post-edit state.
        if let Err(e) = assert_values_facts(&network, &file_path, content, None).await {
            eprintln!("phronesis: WARNING — values-fact assertion failed: {}", e);
            process::exit(1);
        }

        let added = diff_extract::extract(&file_path, None, content).functions_added;
        let project_root = security::project_root();
        if let Err(e) = assert_test_facts(&network, &project_root, &file_path, &added).await {
            eprintln!("phronesis: WARNING — test-fact assertion failed: {}", e);
            process::exit(1);
        }
    }

    if let Err(e) = network.update_agenda().await {
        eprintln!("phronesis: WARNING — agenda update failed: {}", e);
        process::exit(1);
    }
    let consequences = match network.fire_all_consequences() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("phronesis: WARNING — rule execution failed: {}", e);
            process::exit(1);
        }
    };

    let logged: Vec<LoggedConsequence> = consequences
        .iter()
        .filter_map(LoggedConsequence::from_consequence)
        .collect();
    let (violations, warnings) = split_messages_by_action_type(&logged);

    // Post-check can't undo the edit, so violations and warnings collapse to
    // the same exit code (1). The single `consequences` array on the log entry
    // preserves which rule emitted which severity for downstream consumers.
    if violations.is_empty() && warnings.is_empty() {
        log_hook_event("post", &tool_name, &file_path, 0, &logged);
        // Journal the executed call at the tail — see SPEC §"Where it
        // plugs into the hook" — after the decision is logged, before the
        // exit.
        journey_record_post(&payload, &tool_name, &file_path).await;
        exit_ok();
    }

    for v in &violations {
        eprintln!("phronesis: WARNING — {}", v);
    }
    for w in &warnings {
        eprintln!("phronesis: WARNING — {}", w);
    }
    log_hook_event("post", &tool_name, &file_path, 1, &logged);
    journey_record_post(&payload, &tool_name, &file_path).await;
    process::exit(1);
}

fn read_payload() -> anyhow::Result<HookPayload> {
    let input = read_stdin_capped()?;
    let payload: HookPayload = serde_json::from_str(&input)?;
    Ok(payload)
}

/// Best-effort extraction of a tool call's textual output for outcome parsing.
/// Claude Code's PostToolUse nests stdout/stderr; fall back to the whole JSON.
fn extract_tool_output_text(payload: &HookPayload) -> String {
    match &payload.tool_output {
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

/// Compute the outcome tags + resolved subject for a post-check tool call.
/// The result is folded into the journey journal record at the post-check
/// tail (no separate ledger). Returns `(tags, subject)`.
///
/// Mirrors what `capture_outcomes` did pre-0.13: subject lifecycle (open on
/// recognized commands, settle on `git commit`) and outcome parsing. The
/// storage write is now the single `journey::journal::append` call in the
/// hook tail.
fn outcomes_for_journal(payload: &HookPayload, tool_name: &str) -> (Vec<String>, Option<String>) {
    let root = security::project_root();
    let command = extract_new_content(payload, tool_name);
    let output = extract_tool_output_text(payload);
    outcomes::cargo::extract_from(&root, tool_name, command.as_deref(), &output)
}

/// Current epoch seconds — best-effort. Returns 0 on clock-before-epoch (won't
/// happen on real hardware) so callers don't need to thread errors through
/// what is fundamentally a stamp.
fn unix_secs_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Read-increment-write `.phronesis/journey/seq` under flock; return the new
/// value. The seq drives the `c` (last-N-calls) windows the rules can ask
/// for, so it must monotonically rise across concurrent hook processes.
///
/// Best-effort: any IO error returns 0. The journal still appends; ordering
/// degrades gracefully when many calls share seq=0 (call-window aggregators
/// use record position, not seq, for windowing). The seq is mostly a debug
/// aid in v1.
fn next_seq(project_root: &Path) -> u64 {
    use fs2::FileExt;
    use std::fs::OpenOptions;
    use std::io::{Read, Seek, SeekFrom, Write};

    let dir = project_root.join(".phronesis").join("journey");
    if std::fs::create_dir_all(&dir).is_err() {
        return 0;
    }
    let path = dir.join("seq");
    let mut file = match OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
    {
        Ok(f) => f,
        Err(_) => return 0,
    };
    if file.lock_exclusive().is_err() {
        return 0;
    }
    let mut buf = String::new();
    let _ = file.read_to_string(&mut buf);
    let current: u64 = buf.trim().parse().unwrap_or(0);
    let next = current + 1;
    let _ = file.seek(SeekFrom::Start(0));
    let _ = file.set_len(0);
    let _ = file.write_all(next.to_string().as_bytes());
    let _ = FileExt::unlock(&file);
    next
}

/// Journey wiring at the **tail of `run_post_check`**: tag the call, resolve
/// its module, fold in outcome tags + subject, append one journal record.
/// Fail-open: any failure (config parse, tagger error, IO) is swallowed.
async fn journey_record_post(payload: &HookPayload, tool_name: &str, file_path: &str) {
    if std::env::var("PHRONESIS_NO_JOURNEY").is_ok() {
        return;
    }
    let project_root = security::project_root();

    let cfg = match journey::load_config(&project_root) {
        Ok(c) => c,
        Err(journey::ConfigError::NotFound(_)) => journey::tagger::TaggerConfig::default(),
        Err(e) => {
            eprintln!("phronesis: journey config skipped: {}", e);
            journey::tagger::TaggerConfig::default()
        }
    };

    // Common facts the tagger reuses — same shape `assert_common_facts`
    // already asserts into the live network. Synthesizing here keeps the
    // tagger pass independent of post-check's error-bailout paths.
    let facts = tagger_facts(payload, tool_name, file_path, &cfg);

    let tag_result = journey::tagger::fire(&cfg, &facts)
        .await
        .unwrap_or_default();
    let module = journey::tagger::resolve_module(&cfg, file_path);

    let (outcome_tags, subject) = outcomes_for_journal(payload, tool_name);
    let mut all_tags = tag_result.tags;
    all_tags.extend(outcome_tags);

    let ext = std::path::Path::new(file_path)
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string());

    let record = journey::journal::JournalRecord {
        v: 1,
        ts: unix_secs_now(),
        sid: journey::current_sid(&project_root),
        seq: next_seq(&project_root),
        tool: tool_name.to_string(),
        path: file_path.to_string(),
        ext,
        module,
        tags: all_tags,
        subject,
    };
    let _ = journey::journal::append(&project_root, &record);
}

/// Build the common point-in-time facts the tagger evaluates against. Mirrors
/// `assert_common_facts` but produces a `Vec<Fact>` for the throwaway network
/// the tagger builds; no async I/O. Includes `file_path`, `file_path_matches`
/// for each path component, `file_extension_is`, `new_content_contains` for
/// the literal command/content, and — for command tools (`Bash` /
/// `run_shell_command`) — one `bash_command_matches:<pattern>` fact for
/// every pattern in `cfg`'s tagger `when` clauses that regex-matches the
/// command. Same shape `check_bash_command_patterns` in `hook_facts.rs`
/// uses for top-level rules: the engine matches on `args[0] == pattern`,
/// so the synthetic fact has to carry the pattern, not the command.
fn tagger_facts(
    payload: &HookPayload,
    tool_name: &str,
    file_path: &str,
    cfg: &journey::tagger::TaggerConfig,
) -> Vec<Fact> {
    let mut facts: Vec<Fact> = Vec::new();
    facts.push(Fact {
        id: "file_path".to_string(),
        predicate: "file_path".to_string(),
        args: vec![file_path.to_string()],
        timestamp: 0,
    });
    for part in file_path.split('/') {
        if !part.is_empty() {
            facts.push(Fact {
                id: format!("file_path_matches_{}", part),
                predicate: "file_path_matches".to_string(),
                args: vec![part.to_string()],
                timestamp: 0,
            });
        }
    }
    if let Some(ext) = file_path
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
    {
        facts.push(Fact {
            id: format!("file_extension_is_{}", ext),
            predicate: "file_extension_is".to_string(),
            args: vec![ext],
            timestamp: 0,
        });
    }
    if let Some(content) = extract_new_content(payload, tool_name) {
        facts.push(Fact {
            id: "new_content".to_string(),
            predicate: "new_content".to_string(),
            args: vec![content.clone()],
            timestamp: 0,
        });
        // For command tools, walk the tagger config's `when` clauses (and
        // nested `or` clauses) collecting every `bash_command_matches`
        // pattern, then regex-match each against the command. One synthetic
        // fact per match — the engine's equality matcher binds on the
        // pattern in `args[0]`.
        if matches!(tool_name, "Bash" | "run_shell_command") {
            let patterns = collect_tagger_bash_patterns(cfg);
            for pattern in patterns {
                let re = match regex::Regex::new(&pattern) {
                    Ok(re) => re,
                    Err(e) => {
                        eprintln!(
                            "phronesis: WARNING — invalid bash_command_matches regex in tagger '{}': {}",
                            pattern, e
                        );
                        continue;
                    }
                };
                if re.is_match(&content) {
                    facts.push(Fact {
                        id: format!("bash_command_matches_{}", sanitize_pattern(&pattern)),
                        predicate: "bash_command_matches".to_string(),
                        args: vec![pattern],
                        timestamp: 0,
                    });
                }
            }
        }
    }
    facts
}

/// Walk every tagger entry's `when` clauses (and any nested `or` clauses)
/// in `cfg`, collecting the `args[0]` of every `bash_command_matches`
/// predicate. Deterministic and de-duped: same pattern referenced by N
/// taggers contributes one entry. Returns the patterns in first-seen
/// order; callers regex-match each one against the command text.
fn collect_tagger_bash_patterns(cfg: &journey::tagger::TaggerConfig) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for entry in &cfg.taggers {
        for clause in &entry.when {
            collect_bash_patterns_from_value(clause, &mut seen, &mut out);
        }
    }
    out
}

/// Recursive walker: an `or` clause holds an array of nested clauses;
/// any other single-key object whose key is `bash_command_matches`
/// contributes its string value. Anything else (other predicates, non-
/// object values, malformed shapes) is silently skipped — taggers
/// authored against unrelated predicates are not our problem here.
fn collect_bash_patterns_from_value(
    value: &serde_json::Value,
    seen: &mut std::collections::HashSet<String>,
    out: &mut Vec<String>,
) {
    let Some(obj) = value.as_object() else { return };
    for (key, val) in obj {
        if key == "or" {
            if let Some(arr) = val.as_array() {
                for nested in arr {
                    collect_bash_patterns_from_value(nested, seen, out);
                }
            }
            continue;
        }
        if key == "bash_command_matches"
            && let Some(pat) = val.as_str()
            && seen.insert(pat.to_string())
        {
            out.push(pat.to_string());
        }
    }
}

/// Fact-id-safe transform: same rule `hook_facts::sanitize_fact_id_fragment`
/// uses — ASCII alphanumeric survive, everything else becomes `_`.
fn sanitize_pattern(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// Journey wiring shared by `run_pre_check` and `run_post_check`: derive the
/// `journey_*` facts the rules reference, assert them into the live network.
/// Fail-open: any error is logged and the hook continues without journey
/// facts (rules that don't reference journey_* are unaffected; rules that do
/// silently miss this call's enrichment).
async fn assert_journey_facts_into(network: &mut ReteNetwork, project_root: &Path, rules: &[Rule]) {
    if std::env::var("PHRONESIS_NO_JOURNEY").is_ok() {
        return;
    }
    let cfg = match journey::load_config(project_root) {
        Ok(c) => c,
        Err(journey::ConfigError::NotFound(_)) => journey::tagger::TaggerConfig::default(),
        Err(e) => {
            eprintln!("phronesis: journey config skipped: {}", e);
            journey::tagger::TaggerConfig::default()
        }
    };
    let sid = journey::current_sid(project_root);
    let now = unix_secs_now();
    if let Err(e) =
        journey::derive::assert_facts(network, project_root, rules, &cfg, &sid, now).await
    {
        eprintln!("phronesis: journey derivation skipped: {}", e);
    }
}

/// Pre-check side of confidence scoring: assert the open work unit's
/// `signal_pass` facts so gate rules (`facts_count('signal_pass', ...)`) can
/// fire. Opt-in and fail-open.
async fn assert_confidence_signals(network: &ReteNetwork) {
    let root = security::project_root();
    if !outcomes::enabled(&root) {
        return;
    }
    let Some(subject) = outcomes::subject::current(&root) else {
        return;
    };
    let signals = match outcomes::signals(&root, &subject) {
        Ok(s) => s,
        Err(_) => return,
    };
    for fact in signals {
        let id = format!("{}:{}", fact.predicate, fact.args.join(":"));
        let _ = network
            .assert_fact(Fact {
                id,
                predicate: fact.predicate.to_string(),
                args: fact.args,
                timestamp: 0,
            })
            .await;
    }
}

/// Load rules for the given phase.
///
/// Returns `Ok(None)` when no rules file exists (allow). Returns
/// `Err(_)` when the file exists but is unreadable or malformed — callers must
/// treat this as a fail-closed condition rather than silently allowing.
///
/// The path is resolved against the project root (`PHRONESIS_PROJECT_ROOT` or
/// CWD) rather than the bare CWD, so the hook reads the *project's* rules
/// regardless of where it was invoked from. This closes security finding #10.
fn load_rules(phase: &str) -> Result<Option<Vec<Rule>>, RulesLoadError> {
    let path_buf = crate::rules_file::default_path(&security::project_root());
    if !path_buf.exists() {
        return Ok(None);
    }
    let rules_file = crate::rules_file::read(&path_buf).map_err(|e| RulesLoadError::Load {
        path: path_buf.display().to_string(),
        message: e.to_string(),
    })?;

    let rules: Vec<Rule> = rules_file
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

fn extract_file_path(payload: &HookPayload) -> String {
    payload
        .tool_input
        .as_ref()
        .and_then(|input| input.get("file_path"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

fn extract_new_content(payload: &HookPayload, tool_name: &str) -> Option<String> {
    let input = payload.tool_input.as_ref()?;
    match tool_name {
        "Edit" => input
            .get("new_string")
            .and_then(|v| v.as_str())
            .map(String::from),
        "Write" => input
            .get("content")
            .and_then(|v| v.as_str())
            .map(String::from),
        // MultiEdit: `edits` is an array of {old_string, new_string}. Concatenate
        // every new_string so the pattern and diff checks see all additions
        // together. Order is preserved, joined with newlines.
        "MultiEdit" => extract_multiedit_field(input, "new_string"),
        // Bash: the full command string is treated as "new content" so rules
        // can match on its text. This is how we catch deflective language in
        // `git commit -m`, `gh pr create`, etc. without needing a separate
        // Bash-specific predicate.
        "Bash" | "run_shell_command" => input
            .get("command")
            .and_then(|v| v.as_str())
            .map(String::from),
        // Gemini: replace = Edit, write_file = Write (same field names)
        "replace" => input
            .get("new_string")
            .and_then(|v| v.as_str())
            .map(String::from),
        "write_file" => input
            .get("content")
            .and_then(|v| v.as_str())
            .map(String::from),
        _ => None,
    }
}

fn extract_old_content(payload: &HookPayload, tool_name: &str) -> Option<String> {
    let input = payload.tool_input.as_ref()?;
    match tool_name {
        "Edit" | "replace" => input
            .get("old_string")
            .and_then(|v| v.as_str())
            .map(String::from),
        // MultiEdit: same shape as `extract_new_content` but for the removed side.
        "MultiEdit" => extract_multiedit_field(input, "old_string"),
        _ => None,
    }
}

/// Collect a single string field from each entry in a MultiEdit `edits` array,
/// joining them with newlines. Returns `None` when the array is missing or empty.
fn extract_multiedit_field(input: &serde_json::Value, field: &str) -> Option<String> {
    let edits = input.get("edits")?.as_array()?;
    let parts: Vec<String> = edits
        .iter()
        .filter_map(|e| e.get(field).and_then(|v| v.as_str()).map(String::from))
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

// Fact-assertion family moved to hook_facts.rs for focus; brought
// back into this module via paths used at call sites below.
use crate::hook_facts::{
    assert_common_facts, assert_diff_facts, assert_test_facts, assert_values_facts,
    check_bash_command_patterns, check_content_patterns, check_missing_patterns,
    collect_bash_command_patterns, collect_content_patterns, collect_missing_patterns,
};

/// Record a hook event to `.phronesis/log.jsonl`. Best-effort: log failures
/// are intentionally swallowed because we never want logging to alter the
/// hook's exit code (which is the contract Claude Code depends on).
fn log_hook_event(
    phase: &str,
    tool_name: &str,
    file_path: &str,
    exit: i32,
    consequences: &[LoggedConsequence],
) {
    let event = match phase {
        "pre" => "pre_check",
        "post" => "post_check",
        _ => "hook_event",
    };
    let consequences_value = serde_json::to_value(consequences).unwrap_or(serde_json::Value::Null);
    let entry = LogEntry::new("hook", event)
        .with("phase", phase.to_string())
        .with("tool", tool_name.to_string())
        .with("file", file_path.to_string())
        .with("exit", exit)
        .with("consequences", consequences_value);
    let path = action_log::default_path(&security::project_root());
    let _ = action_log::append(&path, &entry);
}

pub(crate) use crate::hook_logged::{LoggedConsequence, split_messages_by_action_type};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hook_facts::filter_new_or_increased_clone_counts;
    use phr::Condition;
    use phr::consequence::Consequence;
    use std::collections::HashMap;

    fn make_payload(tool_name: &str, input: serde_json::Value) -> HookPayload {
        HookPayload {
            tool_name: Some(tool_name.to_string()),
            tool_input: Some(input),
            tool_output: None,
        }
    }

    #[test]
    fn extract_file_path_from_edit() {
        let payload = make_payload(
            "Edit",
            serde_json::json!({ "file_path": "src/main.rs", "old_string": "a", "new_string": "b" }),
        );
        assert_eq!(extract_file_path(&payload), "src/main.rs");
    }

    #[test]
    fn extract_file_path_missing() {
        let payload = make_payload("Edit", serde_json::json!({}));
        assert_eq!(extract_file_path(&payload), "");
    }

    #[test]
    fn extract_new_content_edit() {
        let payload = make_payload(
            "Edit",
            serde_json::json!({ "file_path": "f.rs", "new_string": "fn main() {}" }),
        );
        assert_eq!(
            extract_new_content(&payload, "Edit"),
            Some("fn main() {}".to_string())
        );
    }

    #[test]
    fn extract_new_content_write() {
        let payload = make_payload(
            "Write",
            serde_json::json!({ "file_path": "f.rs", "content": "hello game" }),
        );
        assert_eq!(
            extract_new_content(&payload, "Write"),
            Some("hello game".to_string())
        );
    }

    #[test]
    fn extract_new_content_unknown_tool() {
        let payload = make_payload("Read", serde_json::json!({ "file_path": "f.rs" }));
        assert_eq!(extract_new_content(&payload, "Read"), None);
    }

    #[test]
    fn extract_new_content_bash_returns_command() {
        let payload = make_payload(
            "Bash",
            serde_json::json!({ "command": "git commit -m 'pre-existing issue'" }),
        );
        assert_eq!(
            extract_new_content(&payload, "Bash"),
            Some("git commit -m 'pre-existing issue'".to_string())
        );
    }

    #[test]
    fn extract_new_content_bash_missing_command_is_none() {
        let payload = make_payload("Bash", serde_json::json!({}));
        assert_eq!(extract_new_content(&payload, "Bash"), None);
    }

    fn tagger_cfg_from_json(json: &str) -> journey::tagger::TaggerConfig {
        serde_json::from_str(json).expect("valid tagger config json")
    }

    #[test]
    fn tagger_facts_emits_bash_command_matches_for_default_build_tagger() {
        // Regression: the default `build` tagger keyed on
        // `bash_command_matches: "cargo (build|check|test)"` must surface a
        // synthetic fact carrying that pattern so the engine's equality
        // matcher can bind. Without this, the tagger silently never fires.
        let cfg = tagger_cfg_from_json(
            r#"{
                "version":1,
                "taggers":[
                    {"tag":"build","when":[{"bash_command_matches":"cargo (build|check|test)"}]}
                ],
                "modules":[]
            }"#,
        );
        let payload = make_payload(
            "Bash",
            serde_json::json!({ "command": "cargo check --workspace" }),
        );
        let facts = tagger_facts(&payload, "Bash", "", &cfg);
        let bash_args: Vec<&str> = facts
            .iter()
            .filter(|f| f.predicate == "bash_command_matches")
            .flat_map(|f| f.args.iter().map(String::as_str))
            .collect();
        assert_eq!(
            bash_args,
            vec!["cargo (build|check|test)"],
            "expected one bash_command_matches fact carrying the pattern; got facts: {:?}",
            facts
        );
    }

    #[test]
    fn tagger_facts_skips_bash_match_when_command_does_not_hit_pattern() {
        // Cargo pattern + non-cargo command — no synthetic fact emitted.
        let cfg = tagger_cfg_from_json(
            r#"{
                "version":1,
                "taggers":[
                    {"tag":"build","when":[{"bash_command_matches":"cargo (build|check|test)"}]}
                ],
                "modules":[]
            }"#,
        );
        let payload = make_payload("Bash", serde_json::json!({ "command": "ls -la" }));
        let facts = tagger_facts(&payload, "Bash", "", &cfg);
        assert!(
            !facts.iter().any(|f| f.predicate == "bash_command_matches"),
            "no bash_command_matches fact should be emitted; got: {:?}",
            facts
        );
    }

    #[test]
    fn tagger_facts_walks_nested_or_clauses_for_bash_patterns() {
        // The walker must descend into `or` arrays — taggers expressed as
        // disjunctions still need their bash patterns surfaced.
        let cfg = tagger_cfg_from_json(
            r#"{
                "version":1,
                "taggers":[
                    {
                        "tag":"build",
                        "when":[
                            {"or":[
                                {"bash_command_matches":"cargo (build|check)"},
                                {"bash_command_matches":"^make "}
                            ]}
                        ]
                    }
                ],
                "modules":[]
            }"#,
        );
        let payload = make_payload(
            "Bash",
            serde_json::json!({ "command": "cargo build --release" }),
        );
        let facts = tagger_facts(&payload, "Bash", "", &cfg);
        let bash_args: Vec<&str> = facts
            .iter()
            .filter(|f| f.predicate == "bash_command_matches")
            .flat_map(|f| f.args.iter().map(String::as_str))
            .collect();
        assert_eq!(bash_args, vec!["cargo (build|check)"]);
    }

    #[test]
    fn tagger_facts_does_not_emit_bash_match_for_non_command_tool() {
        // `Edit` is not a command tool — even with a matching content string,
        // we never emit `bash_command_matches`. (The predicate is about
        // commands being run, not about file content that quotes one.)
        let cfg = tagger_cfg_from_json(
            r#"{
                "version":1,
                "taggers":[
                    {"tag":"build","when":[{"bash_command_matches":"cargo (build|check|test)"}]}
                ],
                "modules":[]
            }"#,
        );
        let payload = make_payload(
            "Edit",
            serde_json::json!({
                "file_path": "README.md",
                "old_string": "x",
                "new_string": "run cargo check to verify"
            }),
        );
        let facts = tagger_facts(&payload, "Edit", "README.md", &cfg);
        assert!(
            !facts.iter().any(|f| f.predicate == "bash_command_matches"),
            "Edit must never emit bash_command_matches; got: {:?}",
            facts
        );
    }

    #[test]
    fn tagger_facts_invalid_regex_is_skipped_not_panicked() {
        // A rule-author typo in the regex must not blow up the hook.
        let cfg = tagger_cfg_from_json(
            r#"{
                "version":1,
                "taggers":[
                    {"tag":"oops","when":[{"bash_command_matches":"["}]}
                ],
                "modules":[]
            }"#,
        );
        let payload = make_payload("Bash", serde_json::json!({ "command": "cargo check" }));
        let facts = tagger_facts(&payload, "Bash", "", &cfg);
        assert!(
            !facts.iter().any(|f| f.predicate == "bash_command_matches"),
            "invalid regex must be skipped: {:?}",
            facts
        );
    }

    #[test]
    fn extract_new_content_multiedit_joins_all_new_strings() {
        let payload = make_payload(
            "MultiEdit",
            serde_json::json!({
                "file_path": "f.rs",
                "edits": [
                    { "old_string": "a", "new_string": "fn one() {}" },
                    { "old_string": "b", "new_string": "fn two() {}" },
                ]
            }),
        );
        assert_eq!(
            extract_new_content(&payload, "MultiEdit"),
            Some("fn one() {}\nfn two() {}".to_string())
        );
    }

    #[test]
    fn extract_old_content_multiedit_joins_all_old_strings() {
        let payload = make_payload(
            "MultiEdit",
            serde_json::json!({
                "file_path": "f.rs",
                "edits": [
                    { "old_string": "fn old_one() {}", "new_string": "x" },
                    { "old_string": "fn old_two() {}", "new_string": "y" },
                ]
            }),
        );
        assert_eq!(
            extract_old_content(&payload, "MultiEdit"),
            Some("fn old_one() {}\nfn old_two() {}".to_string())
        );
    }

    #[test]
    fn extract_multiedit_with_empty_edits_returns_none() {
        let payload = make_payload(
            "MultiEdit",
            serde_json::json!({ "file_path": "f.rs", "edits": [] }),
        );
        assert_eq!(extract_new_content(&payload, "MultiEdit"), None);
        assert_eq!(extract_old_content(&payload, "MultiEdit"), None);
    }

    #[test]
    fn extract_multiedit_with_missing_edits_returns_none() {
        let payload = make_payload("MultiEdit", serde_json::json!({ "file_path": "f.rs" }));
        assert_eq!(extract_new_content(&payload, "MultiEdit"), None);
    }

    fn patterns(s: &[&str]) -> Vec<String> {
        s.iter().map(|p| p.to_string()).collect()
    }

    #[tokio::test]
    async fn check_content_patterns_detects_unwrap() {
        let network = ReteNetwork::new();
        let content = "let x = foo.unwrap();";
        check_content_patterns(&network, "src/lib.rs", content, &patterns(&[".unwrap()"]))
            .await
            .unwrap();

        let wmes = network.get_all_wmes().await.unwrap();
        let predicates: Vec<_> = wmes.iter().map(|w| w.fact.predicate.as_str()).collect();
        assert!(predicates.contains(&"new_content_contains"));
        let args: Vec<_> = wmes
            .iter()
            .filter(|w| w.fact.predicate == "new_content_contains")
            .flat_map(|w| w.fact.args.clone())
            .collect();
        assert!(args.contains(&".unwrap()".to_string()));
    }

    #[tokio::test]
    async fn check_content_patterns_no_matches() {
        let network = ReteNetwork::new();
        let content = "let x = foo.map_err(|e| e)?;";
        check_content_patterns(&network, "src/lib.rs", content, &patterns(&[".unwrap()"]))
            .await
            .unwrap();

        let wmes = network.get_all_wmes().await.unwrap();
        assert!(wmes.is_empty());
    }

    #[tokio::test]
    async fn check_content_patterns_skips_unwrap_inside_test_block() {
        let network = ReteNetwork::new();
        let content = "\
fn production() { foo.map_err(|e| e)?; }

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        let v = thing.unwrap();
    }
}
";
        check_content_patterns(&network, "src/lib.rs", content, &patterns(&[".unwrap()"]))
            .await
            .unwrap();

        let wmes = network.get_all_wmes().await.unwrap();
        assert!(
            wmes.is_empty(),
            "test-scoped .unwrap() must not fire production rule"
        );
    }

    #[tokio::test]
    async fn check_content_patterns_still_fires_for_production_code() {
        let network = ReteNetwork::new();
        let content = "\
fn production() { foo.unwrap(); }

#[cfg(test)]
mod tests {
    fn t() { thing.unwrap(); }
}
";
        check_content_patterns(&network, "src/lib.rs", content, &patterns(&[".unwrap()"]))
            .await
            .unwrap();

        let wmes = network.get_all_wmes().await.unwrap();
        assert_eq!(
            wmes.len(),
            1,
            "exactly one production .unwrap() should be flagged"
        );
    }

    #[tokio::test]
    async fn check_content_patterns_honors_arbitrary_pattern() {
        // Regression test: a pattern not in the old hardcoded list (e.g. dbg!)
        // must be scanned when a rule asks for it.
        let network = ReteNetwork::new();
        let content = "fn x() { dbg!(state); }";
        check_content_patterns(&network, "src/lib.rs", content, &patterns(&["dbg!("]))
            .await
            .unwrap();
        let wmes = network.get_all_wmes().await.unwrap();
        let args: Vec<_> = wmes
            .iter()
            .filter(|w| w.fact.predicate == "new_content_contains")
            .flat_map(|w| w.fact.args.clone())
            .collect();
        assert!(
            args.contains(&"dbg!(".to_string()),
            "arbitrary patterns must work, not just the legacy hardcoded list"
        );
    }

    #[tokio::test]
    async fn check_missing_patterns_detects_missing() {
        let network = ReteNetwork::new();
        let content = "fn main() {}";
        check_missing_patterns(
            &network,
            content,
            &patterns(&["SCHEMA_VERSION", "mod tests", "#[cfg(test)]"]),
        )
        .await
        .unwrap();

        let wmes = network.get_all_wmes().await.unwrap();
        let missing: Vec<_> = wmes
            .iter()
            .filter(|w| w.fact.predicate == "file_missing_pattern")
            .flat_map(|w| w.fact.args.clone())
            .collect();
        assert!(missing.contains(&"SCHEMA_VERSION".to_string()));
        assert!(missing.contains(&"mod tests".to_string()));
        assert!(missing.contains(&"#[cfg(test)]".to_string()));
    }

    #[test]
    fn collect_content_patterns_extracts_from_rules() {
        let rules = vec![
            Rule {
                id: "a".to_string(),
                priority: 1,
                conditions: vec![Condition {
                    predicate: "new_content_contains".to_string(),
                    args: vec!["dbg!(".to_string()],
                    script: None,
                }],
                actions: vec![],
            },
            Rule {
                id: "b".to_string(),
                priority: 1,
                conditions: vec![Condition {
                    predicate: "new_content_contains".to_string(),
                    args: vec!["panic!(".to_string()],
                    script: None,
                }],
                actions: vec![],
            },
            // Should be ignored — different predicate
            Rule {
                id: "c".to_string(),
                priority: 1,
                conditions: vec![Condition {
                    predicate: "file_path_matches".to_string(),
                    args: vec!["src".to_string()],
                    script: None,
                }],
                actions: vec![],
            },
        ];
        let patterns = collect_content_patterns(&rules);
        assert_eq!(patterns.len(), 2);
        assert!(patterns.contains(&"dbg!(".to_string()));
        assert!(patterns.contains(&"panic!(".to_string()));
    }

    #[test]
    fn collect_content_patterns_deduplicates() {
        let rules = vec![
            Rule {
                id: "a".to_string(),
                priority: 1,
                conditions: vec![Condition {
                    predicate: "new_content_contains".to_string(),
                    args: vec![".unwrap()".to_string()],
                    script: None,
                }],
                actions: vec![],
            },
            Rule {
                id: "b".to_string(),
                priority: 1,
                conditions: vec![Condition {
                    predicate: "new_content_contains".to_string(),
                    args: vec![".unwrap()".to_string()],
                    script: None,
                }],
                actions: vec![],
            },
        ];
        let patterns = collect_content_patterns(&rules);
        assert_eq!(patterns.len(), 1);
    }

    #[tokio::test]
    async fn assert_common_facts_creates_path_components() {
        let network = ReteNetwork::new();
        assert_common_facts(&network, "src/lib/main.rs", "Edit", "pre")
            .await
            .unwrap();

        let wmes = network.get_all_wmes().await.unwrap();
        let path_matches: Vec<_> = wmes
            .iter()
            .filter(|w| w.fact.predicate == "file_path_matches")
            .flat_map(|w| w.fact.args.clone())
            .collect();
        assert!(path_matches.contains(&"src".to_string()));
        assert!(path_matches.contains(&"lib".to_string()));
        assert!(path_matches.contains(&"main.rs".to_string()));
    }

    #[test]
    fn logged_consequence_from_rule_firing() {
        use phr::consequence::{ConsequenceKind, Provenance};
        use serde_json::json;

        let mut bindings = HashMap::new();
        bindings.insert("?fn".to_string(), "bad".to_string());
        bindings.insert("?file".to_string(), "src/lib.rs".to_string());

        let c = Consequence {
            kind: ConsequenceKind::Constraint,
            predicate: "rust-error-thiserror-for-libraries".to_string(),
            payload: json!({
                "action_type": "constraint_violation",
                "message": "`bad` in src/lib.rs returns Result<_, String>",
                "params": ["`bad` in src/lib.rs returns Result<_, String>"],
            }),
            provenance: Provenance::RuleFiring {
                rule_id: "rust-error-thiserror-for-libraries".into(),
                bound_facts: vec![],
                bindings,
            },
        };

        let logged = LoggedConsequence::from_consequence(&c).unwrap();
        assert_eq!(logged.rule_id, "rust-error-thiserror-for-libraries");
        assert_eq!(logged.action_type, "constraint_violation");
        assert!(logged.message.contains("returns Result"));
        assert_eq!(logged.bindings.get("?fn").map(String::as_str), Some("bad"));
    }

    #[test]
    fn logged_consequence_returns_none_for_non_rule_firing_provenance() {
        use phr::consequence::{ConsequenceKind, Provenance};
        use serde_json::json;

        let c = Consequence {
            kind: ConsequenceKind::Snapshot,
            predicate: "lookup_symbol".to_string(),
            payload: json!({}),
            provenance: Provenance::Lookup {
                tool: "symbol_lookup".to_string(),
                schema_version: 1,
            },
        };
        assert!(LoggedConsequence::from_consequence(&c).is_none());
    }

    #[test]
    fn split_messages_partitions_by_action_type() {
        let items = vec![
            LoggedConsequence {
                rule_id: "r1".into(),
                action_type: "constraint_violation".to_string(),
                message: "v1".to_string(),
                bindings: HashMap::new(),
            },
            LoggedConsequence {
                rule_id: "r2".into(),
                action_type: "constraint_warning".to_string(),
                message: "w1".to_string(),
                bindings: HashMap::new(),
            },
            LoggedConsequence {
                rule_id: "r3".into(),
                action_type: "constraint_violation".to_string(),
                message: "v2".to_string(),
                bindings: HashMap::new(),
            },
        ];
        let (vs, ws) = split_messages_by_action_type(&items);
        assert_eq!(vs, vec!["v1", "v2"]);
        assert_eq!(ws, vec!["w1"]);
    }

    #[test]
    fn delta_filter_pass_through_when_old_is_none() {
        let new = vec![("foo".to_string(), 5_usize), ("bar".to_string(), 8)];
        let out = filter_new_or_increased_clone_counts(&new, None);
        assert_eq!(out, new);
    }

    #[test]
    fn delta_filter_suppresses_unchanged_count() {
        let old = vec![("foo".to_string(), 5_usize)];
        let new = vec![("foo".to_string(), 5_usize)];
        let out = filter_new_or_increased_clone_counts(&new, Some(&old));
        assert!(out.is_empty(), "unchanged count must be suppressed");
    }

    #[test]
    fn delta_filter_keeps_increased_count() {
        let old = vec![("foo".to_string(), 5_usize)];
        let new = vec![("foo".to_string(), 7_usize)];
        let out = filter_new_or_increased_clone_counts(&new, Some(&old));
        assert_eq!(out, vec![("foo".to_string(), 7_usize)]);
    }

    #[test]
    fn delta_filter_suppresses_decreased_count() {
        let old = vec![("foo".to_string(), 8_usize)];
        let new = vec![("foo".to_string(), 5_usize)];
        let out = filter_new_or_increased_clone_counts(&new, Some(&old));
        assert!(
            out.is_empty(),
            "count went down — partial improvement should not fire"
        );
    }

    #[test]
    fn delta_filter_keeps_new_function() {
        let old: Vec<(String, usize)> = vec![];
        let new = vec![("foo".to_string(), 4_usize)];
        let out = filter_new_or_increased_clone_counts(&new, Some(&old));
        assert_eq!(out, vec![("foo".to_string(), 4_usize)]);
    }

    // ── Gemini tool name tests ────────────────────────────────────────────────

    #[test]
    fn extract_new_content_gemini_replace_returns_new_string() {
        // Gemini "replace" maps to Claude "Edit": reads new_string
        let payload = make_payload(
            "replace",
            serde_json::json!({
                "file_path": "src/main.rs",
                "old_string": "fn old() {}",
                "new_string": "fn new() {}"
            }),
        );
        assert_eq!(
            extract_new_content(&payload, "replace"),
            Some("fn new() {}".to_string())
        );
    }

    #[test]
    fn extract_old_content_gemini_replace_returns_old_string() {
        // Gemini "replace" maps to Claude "Edit": reads old_string
        let payload = make_payload(
            "replace",
            serde_json::json!({
                "file_path": "src/main.rs",
                "old_string": "fn old() {}",
                "new_string": "fn new() {}"
            }),
        );
        assert_eq!(
            extract_old_content(&payload, "replace"),
            Some("fn old() {}".to_string())
        );
    }

    #[test]
    fn extract_new_content_gemini_write_file_returns_content() {
        // Gemini "write_file" maps to Claude "Write": reads content
        let payload = make_payload(
            "write_file",
            serde_json::json!({
                "file_path": "src/lib.rs",
                "content": "pub fn hello() {}"
            }),
        );
        assert_eq!(
            extract_new_content(&payload, "write_file"),
            Some("pub fn hello() {}".to_string())
        );
    }

    #[test]
    fn extract_new_content_gemini_run_shell_command_returns_command() {
        // Gemini "run_shell_command" maps to Claude "Bash": reads command
        let payload = make_payload(
            "run_shell_command",
            serde_json::json!({ "command": "cargo test --workspace" }),
        );
        assert_eq!(
            extract_new_content(&payload, "run_shell_command"),
            Some("cargo test --workspace".to_string())
        );
    }

    #[test]
    fn extract_new_content_gemini_replace_missing_new_string_is_none() {
        let payload = make_payload(
            "replace",
            serde_json::json!({ "file_path": "f.rs", "old_string": "x" }),
        );
        assert_eq!(extract_new_content(&payload, "replace"), None);
    }

    #[test]
    fn extract_old_content_gemini_replace_missing_old_string_is_none() {
        let payload = make_payload(
            "replace",
            serde_json::json!({ "file_path": "f.rs", "new_string": "x" }),
        );
        assert_eq!(extract_old_content(&payload, "replace"), None);
    }

    #[test]
    fn extract_new_content_gemini_write_file_missing_content_is_none() {
        let payload = make_payload("write_file", serde_json::json!({ "file_path": "f.rs" }));
        assert_eq!(extract_new_content(&payload, "write_file"), None);
    }

    #[test]
    fn extract_new_content_gemini_run_shell_command_missing_command_is_none() {
        let payload = make_payload("run_shell_command", serde_json::json!({}));
        assert_eq!(extract_new_content(&payload, "run_shell_command"), None);
    }

    #[test]
    fn extract_file_path_works_for_gemini_replace() {
        // file_path field name is the same for Gemini tools
        let payload = make_payload(
            "replace",
            serde_json::json!({ "file_path": "src/foo.rs", "old_string": "a", "new_string": "b" }),
        );
        assert_eq!(extract_file_path(&payload), "src/foo.rs");
    }

    #[test]
    fn extract_file_path_works_for_gemini_write_file() {
        let payload = make_payload(
            "write_file",
            serde_json::json!({ "file_path": "out/bar.rs", "content": "hello" }),
        );
        assert_eq!(extract_file_path(&payload), "out/bar.rs");
    }
}
