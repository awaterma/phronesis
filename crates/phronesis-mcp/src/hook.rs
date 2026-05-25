use std::collections::HashMap;
use std::path::Path;
use std::process;

use phr::consequence::{Consequence, Provenance};
use phr::{Action, Condition, Fact, ReteNetwork, Rule};
use serde::Deserialize;
use thiserror::Error;

use crate::action_log::{self, LogEntry};
use crate::diff_extract;
use crate::security::{
    self, read_file_capped, read_stdin_capped, resolve_safe_path, MAX_FACT_CONTENT_BYTES,
};
use crate::stats;

#[derive(Debug, Error)]
enum RulesLoadError {
    #[error("io error reading {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("rules file at {path} is malformed: {source}")]
    Malformed {
        path: String,
        #[source]
        source: serde_json::Error,
    },
}

/// Hook-internal error type. Currently wraps engine `String` errors; future
/// variants (e.g. `FactValidation`, `ContentTooLarge`) can be added without
/// changing any helper signatures.
#[derive(Debug, Error)]
enum HookError {
    #[error("RETE engine error: {0}")]
    Engine(String),
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
    #[serde(default)]
    #[allow(dead_code)]
    tool_output: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct RulesFile {
    rules: Vec<HookRule>,
}

#[derive(Debug, Deserialize)]
struct HookRule {
    id: String,
    phase: String,
    priority: i32,
    conditions: Vec<HookCondition>,
    actions: Vec<HookAction>,
}

#[derive(Debug, Deserialize)]
struct HookCondition {
    predicate: String,
    args: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct HookAction {
    action_type: String,
    params: Vec<String>,
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

    let new_content = extract_new_content(&payload, &tool_name);
    let file_path = extract_file_path(&payload);

    let network = ReteNetwork::new();

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

    let rules = match load_rules("post") {
        Ok(Some(rules)) => rules,
        Ok(None) => exit_ok(),
        Err(e) => {
            eprintln!("phronesis: WARNING — {}", e);
            process::exit(1);
        }
    };

    // Drive pattern scans from the loaded rules' conditions.
    let content_patterns = collect_content_patterns(&rules);
    let missing_patterns = collect_missing_patterns(&rules);

    let file_path = extract_file_path(&payload);

    let network = ReteNetwork::new();

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

    if let Some(content) = &content_opt {
        // Only assert the full content as a fact when small enough to keep
        // working-memory growth bounded. Pattern checks below still operate on
        // the in-memory slice and emit the targeted predicates.
        if content.len() <= MAX_FACT_CONTENT_BYTES {
            if let Err(e) = network
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
        exit_ok();
    }

    for v in &violations {
        eprintln!("phronesis: WARNING — {}", v);
    }
    for w in &warnings {
        eprintln!("phronesis: WARNING — {}", w);
    }
    log_hook_event("post", &tool_name, &file_path, 1, &logged);
    process::exit(1);
}

fn read_payload() -> anyhow::Result<HookPayload> {
    let input = read_stdin_capped()?;
    let payload: HookPayload = serde_json::from_str(&input)?;
    Ok(payload)
}

/// Load rules for the given phase.
///
/// Returns `Ok(None)` when no rules file exists (allow). Returns
/// `Err(_)` when the file exists but is unreadable or malformed — callers must
/// treat this as a fail-closed condition rather than silently allowing.
///
/// The path is resolved against the project root (`EPISTEME_PROJECT_ROOT` or
/// CWD) rather than the bare CWD, so the hook reads the *project's* rules
/// regardless of where it was invoked from. This closes security finding #10.
fn load_rules(phase: &str) -> Result<Option<Vec<Rule>>, RulesLoadError> {
    let path_buf = crate::rules_file::default_path(&security::project_root());
    if !path_buf.exists() {
        return Ok(None);
    }

    let path_display = path_buf.display().to_string();
    let content = std::fs::read_to_string(&path_buf).map_err(|e| RulesLoadError::Io {
        path: path_display.clone(),
        source: e,
    })?;
    let rules_file: RulesFile =
        serde_json::from_str(&content).map_err(|e| RulesLoadError::Malformed {
            path: path_display,
            source: e,
        })?;

    let rules: Vec<Rule> = rules_file
        .rules
        .into_iter()
        .filter(|r| r.phase == phase)
        .map(|r| Rule {
            id: r.id,
            priority: r.priority,
            conditions: r
                .conditions
                .into_iter()
                .map(|c| Condition {
                    predicate: c.predicate,
                    args: c.args,
                    script: None,
                })
                .collect(),
            actions: r
                .actions
                .into_iter()
                .map(|a| Action {
                    action_type: a.action_type,
                    params: a.params,
                })
                .collect(),
        })
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

/// Assert diff-extracted facts: `function_added(file, name)`,
/// `function_removed(file, name)`, `import_added(file, target)`,
/// `import_removed(file, target)`.
async fn assert_diff_facts(
    network: &ReteNetwork,
    file_path: &str,
    old: Option<&str>,
    new: &str,
) -> Result<(), HookError> {
    let facts = diff_extract::extract(file_path, old, new);

    for (predicate, items) in [
        ("function_added", &facts.functions_added),
        ("function_removed", &facts.functions_removed),
        ("import_added", &facts.imports_added),
        ("import_removed", &facts.imports_removed),
    ] {
        for (i, item) in items.iter().enumerate() {
            network
                .assert_fact(Fact {
                    id: format!("{}_{}_{}", predicate, item, i),
                    predicate: predicate.to_string(),
                    args: vec![file_path.to_string(), item.clone()],
                    timestamp: 0,
                })
                .await?;
        }
    }
    Ok(())
}

/// Filter heavy-clone counts to only entries that are new or have increased.
///
/// `new` is the list of `(fn_name, count)` pairs extracted from the
/// post-edit content. `old`, when `Some`, is the same list extracted from
/// the prior content; when `None`, no filtering is applied.
///
/// Returns only entries where the function did not exist in `old` (implicit
/// count of 0) or its count strictly exceeds the matching old entry's count.
/// A decreased count is suppressed — the edit imanaroved things, even if the
/// fn is still heavy.
fn filter_new_or_increased_clone_counts(
    new: &[(String, usize)],
    old: Option<&[(String, usize)]>,
) -> Vec<(String, usize)> {
    let Some(old_counts) = old else {
        return new.to_vec();
    };
    new.iter()
        .filter(|(fn_name, new_count)| {
            let old_count = old_counts
                .iter()
                .find(|(n, _)| n == fn_name)
                .map(|(_, c)| *c)
                .unwrap_or(0);
            *new_count > old_count
        })
        .cloned()
        .collect()
}

/// Run the values analyzer over the post-edit content and assert facts about
/// structural properties the diff extractor can't see. The set of predicates
/// emitted is whatever `SyntaxFacts::all_facts` produces; see
/// `src/values/facts.rs` for the canonical list.
async fn assert_values_facts(
    network: &ReteNetwork,
    file_path: &str,
    content: &str,
    old_content: Option<&str>,
) -> Result<(), HookError> {
    // Production-only predicates run against test-stripped content so rules
    // like `function_returns_result_string` don't fire on inline test code.
    let production = diff_extract::strip_test_blocks(file_path, content);
    let prod_facts = syntax::extract(file_path, &production);

    // Test-quality predicates need the unstripped content so they can see
    // `#[test] fn` bodies that strip_test_blocks would otherwise remove.
    let unstripped = syntax::extract(file_path, content);

    // Merge: take production predicates from `prod_facts`, take test-quality
    // predicates from `unstripped`. Currently only `tests_without_assertion`
    // belongs to the test-quality group.
    let mut facts = prod_facts;
    facts.tests_without_assertion = unstripped.tests_without_assertion;

    // Delta filter: warn-clone-heavy should only fire when a heavy-clone
    // function is newly added or its count increased compared to prior
    // content. Otherwise the rule re-fires every edit to a file with a
    // long-standing heavy function. `old_content` is only available at
    // pre-check (post-check disk already has new content).
    if let Some(old) = old_content {
        let old_stripped = diff_extract::strip_test_blocks(file_path, old);
        let old_facts = syntax::extract(file_path, &old_stripped);
        facts.function_clone_counts_high = filter_new_or_increased_clone_counts(
            &facts.function_clone_counts_high,
            Some(&old_facts.function_clone_counts),
        );
    }

    for fact in facts.all_facts(file_path) {
        network.assert_fact(fact).await?;
    }
    Ok(())
}

/// Assert `test_exists_for(name)` or `no_test_for(name)` per function name.
///
/// Heuristic search:
/// 1. The source file itself (inline `#[test]` / `def test_X` patterns)
/// 2. Conventional sibling test paths (`<stem>_test.<ext>`, `tests/<stem>_test.<ext>`)
///
/// A function is "tested" if its name appears anywhere in any of the candidate
/// test bodies. This is intentionally permissive — false positives are safer
/// than blocking a legitimate edit because the test exists but doesn't match
/// our regex.
async fn assert_test_facts(
    network: &ReteNetwork,
    project_root: &Path,
    file_path: &str,
    function_names: &[String],
) -> Result<(), HookError> {
    if function_names.is_empty() {
        return Ok(());
    }
    let candidates = test_candidate_paths(project_root, file_path);
    let test_bodies: Vec<String> = candidates
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .collect();

    for (i, name) in function_names.iter().enumerate() {
        let has_test = test_bodies.iter().any(|body| body.contains(name.as_str()));
        let predicate = if has_test {
            "test_exists_for"
        } else {
            "no_test_for"
        };
        network
            .assert_fact(Fact {
                id: format!("{}_{}_{}", predicate, name, i),
                predicate: predicate.to_string(),
                args: vec![name.clone()],
                timestamp: 0,
            })
            .await?;
    }
    Ok(())
}

fn test_candidate_paths(project_root: &Path, file_path: &str) -> Vec<std::path::PathBuf> {
    let path = Path::new(file_path);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("rs");

    let mut candidates = vec![
        // The file itself (Rust inline #[cfg(test)] mod tests, Python def test_…)
        project_root.join(file_path),
        // Sibling test files
        path.parent()
            .unwrap_or(Path::new(""))
            .join(format!("{}_test.{}", stem, ext)),
        path.parent()
            .unwrap_or(Path::new(""))
            .join(format!("test_{}.{}", stem, ext)),
        // tests/ directory at project root
        project_root
            .join("tests")
            .join(format!("{}_test.{}", stem, ext)),
        project_root
            .join("tests")
            .join(format!("test_{}.{}", stem, ext)),
        project_root.join("tests").join(format!("{}.{}", stem, ext)),
    ];

    // Resolve relative paths against project root
    candidates.iter_mut().for_each(|p| {
        if p.is_relative() {
            *p = project_root.join(&*p);
        }
    });
    candidates.retain(|p| p.exists());
    candidates
}

async fn assert_common_facts(
    network: &ReteNetwork,
    file_path: &str,
    tool_name: &str,
    phase: &str,
) -> Result<(), HookError> {
    let facts = vec![
        Fact {
            id: "file_path".to_string(),
            predicate: "file_path".to_string(),
            args: vec![file_path.to_string()],
            timestamp: 0,
        },
        Fact {
            id: "hook_phase".to_string(),
            predicate: "hook_phase".to_string(),
            args: vec![phase.to_string()],
            timestamp: 0,
        },
        Fact {
            id: "change_type".to_string(),
            predicate: "change_type".to_string(),
            args: vec![tool_name.to_lowercase()],
            timestamp: 0,
        },
    ];

    for part in file_path.split('/') {
        if !part.is_empty() {
            let fact_id = format!("file_path_matches_{}", part);
            network
                .assert_fact(Fact {
                    id: fact_id,
                    predicate: "file_path_matches".to_string(),
                    args: vec![part.to_string()],
                    timestamp: 0,
                })
                .await?;
        }
    }

    // Extension fact for rules that need to scope by file type.
    // Emitted once per file as `file_extension_is("rs")`, `file_extension_is("rhai")`, etc.
    if let Some(ext) = file_path
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
    {
        let fact_id = format!("file_extension_is_{}", ext);
        network
            .assert_fact(Fact {
                id: fact_id,
                predicate: "file_extension_is".to_string(),
                args: vec![ext],
                timestamp: 0,
            })
            .await?;
    }

    for fact in facts {
        network.assert_fact(fact).await?;
    }
    Ok(())
}

async fn check_content_patterns(
    network: &ReteNetwork,
    file_path: &str,
    content: &str,
    patterns: &[String],
) -> Result<(), HookError> {
    // Strip out test-scoped regions so patterns inside `#[cfg(test)]` blocks
    // or `#[test] fn` bodies don't fire production-only rules. For unsupported
    // languages this is an identity no-op.
    let production = diff_extract::strip_test_blocks(file_path, content);

    for pattern in patterns {
        if production.contains(pattern.as_str()) {
            let fact_id = format!(
                "new_content_contains_{}",
                sanitize_fact_id_fragment(pattern)
            );
            network
                .assert_fact(Fact {
                    id: fact_id,
                    predicate: "new_content_contains".to_string(),
                    args: vec![pattern.clone()],
                    timestamp: 0,
                })
                .await?;
        }
    }
    Ok(())
}

/// Make a fact-id-safe fragment from an arbitrary pattern string. Whitespace
/// and any character that isn't ASCII alphanumeric becomes `_`. Stable for a
/// given input, but not necessarily reversible — IDs are opaque keys.
fn sanitize_fact_id_fragment(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// Collect every distinct `args[0]` from rules' `new_content_contains`
/// conditions. The hook scans for exactly these patterns; any rule whose
/// condition references a pattern automatically gets it checked.
fn collect_content_patterns(rules: &[Rule]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    rules
        .iter()
        .flat_map(|r| &r.conditions)
        .filter(|c| c.predicate == "new_content_contains")
        .filter_map(|c| c.args.first())
        .filter(|s| seen.insert((*s).clone()))
        .cloned()
        .collect()
}

/// Same for `file_missing_pattern` — used by post-check.
fn collect_missing_patterns(rules: &[Rule]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    rules
        .iter()
        .flat_map(|r| &r.conditions)
        .filter(|c| c.predicate == "file_missing_pattern")
        .filter_map(|c| c.args.first())
        .filter(|s| seen.insert((*s).clone()))
        .cloned()
        .collect()
}

async fn check_missing_patterns(
    network: &ReteNetwork,
    content: &str,
    patterns: &[String],
) -> Result<(), HookError> {
    for pattern in patterns {
        if !content.contains(pattern.as_str()) {
            let fact_id = format!(
                "file_missing_pattern_{}",
                sanitize_fact_id_fragment(pattern)
            );
            network
                .assert_fact(Fact {
                    id: fact_id,
                    predicate: "file_missing_pattern".to_string(),
                    args: vec![pattern.clone()],
                    timestamp: 0,
                })
                .await?;
        }
    }
    Ok(())
}

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

/// Flattened projection of `phr::Consequence` used in action-log
/// entries. We don't serialize the raw `Consequence` because its nested
/// `Provenance::RuleFiring { rule_id, bindings, .. }` is awkward to query
/// (`.provenance.RuleFiring.rule_id` etc.). Pulling rule_id and bindings
/// up to the top rank gives `jq` users a flat, predictable shape.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct LoggedConsequence {
    pub rule_id: String,
    pub action_type: String,
    pub message: String,
    #[serde(default)]
    pub bindings: HashMap<String, String>,
}

impl LoggedConsequence {
    /// Project a `Consequence` whose provenance is `RuleFiring` into a
    /// flat log entry. Returns `None` for other provenance variants
    /// (Lookup, RuleDrivenLookup, Asserted) which the hook doesn't
    /// currently emit.
    pub(crate) fn from_consequence(c: &Consequence) -> Option<Self> {
        let (rule_id, bindings) = match &c.provenance {
            Provenance::RuleFiring {
                rule_id, bindings, ..
            } => (rule_id.clone(), bindings.clone()),
            _ => return None,
        };
        let action_type = c
            .payload
            .get("action_type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let message = c
            .payload
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        Some(Self {
            rule_id,
            action_type,
            message,
            bindings,
        })
    }
}

/// Split `LoggedConsequence`s into the (violations, warnings) string-lists
/// the hook uses for stderr output. Violations are messages whose
/// `action_type == "constraint_violation"`, warnings are
/// `"constraint_warning"`. Order preserved.
pub(crate) fn split_messages_by_action_type(
    items: &[LoggedConsequence],
) -> (Vec<String>, Vec<String>) {
    let mut violations = Vec::new();
    let mut warnings = Vec::new();
    for c in items {
        match c.action_type.as_str() {
            "constraint_violation" => violations.push(c.message.clone()),
            "constraint_warning" => warnings.push(c.message.clone()),
            _ => {}
        }
    }
    (violations, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

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
                rule_id: "rust-error-thiserror-for-libraries".to_string(),
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
                rule_id: "r1".to_string(),
                action_type: "constraint_violation".to_string(),
                message: "v1".to_string(),
                bindings: HashMap::new(),
            },
            LoggedConsequence {
                rule_id: "r2".to_string(),
                action_type: "constraint_warning".to_string(),
                message: "w1".to_string(),
                bindings: HashMap::new(),
            },
            LoggedConsequence {
                rule_id: "r3".to_string(),
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
            "count went down — partial imanarovement should not fire"
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
        let payload = make_payload(
            "write_file",
            serde_json::json!({ "file_path": "f.rs" }),
        );
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
