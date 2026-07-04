use std::process;

use phr::{Fact, Rule};

use crate::diff_extract;
use crate::hook_facts::{
    assert_common_facts, assert_diff_facts, assert_test_facts, assert_values_facts,
    check_bash_command_patterns, check_content_patterns, collect_bash_command_patterns,
    collect_content_patterns,
};
use crate::security;

use super::{LoggedConsequence, split_messages_by_action_type};

/// PreToolUse hook: validate proposed changes before they happen.
/// Exit 0 = allow, exit 2 = block.
///
/// Fails closed (exit 2) when:
/// - stdin payload exceeds the size cap
/// - `.phronesis/rules.json` exists but is malformed
/// - rule loading, fact assertion, or rule firing fails
pub async fn run_pre_check() -> anyhow::Result<()> {
    let payload = match super::read_payload() {
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
        _ => super::exit_ok(),
    };

    let rules = match super::load_rules("pre") {
        Ok(Some(rules)) => rules,
        Ok(None) => super::exit_ok(),
        Err(e) => {
            eprintln!("phronesis: BLOCKED — {}", e);
            process::exit(2);
        }
    };

    // Collect substring patterns the rules want scanned — drives the
    // pattern-fact assertion below, so new rules don't require code changes.
    let content_patterns = collect_content_patterns(&rules);
    let bash_command_patterns = collect_bash_command_patterns(&rules);

    let new_content = super::extract_new_content(&payload, &tool_name);
    let file_path = super::extract_file_path(&payload);

    let mut network = crate::net::build_network();

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
    // Fail-open on transient I/O; fail-closed on rule/config typos (see
    // assert_journey_facts_into for the split).
    let project_root_pre = security::project_root();
    if let Err(e) =
        super::assert_journey_facts_into(&mut network, &project_root_pre, &rules_for_journey).await
    {
        eprintln!("phronesis: BLOCKED — {}", e);
        process::exit(2);
    }

    // Pack-marker facts (e.g. `confidence_enabled`) — let rules from one
    // pack self-deactivate when a superseding pack is opted in.
    super::assert_pack_marker_facts(&network, &project_root_pre).await;

    // Confidence gate: assert the open work unit's grounded signals *before* any
    // command/content facts, so a gate rule's `__script__` count is evaluated
    // against the full signal set when its `bash_command_matches` condition is
    // asserted (the agenda updates incrementally per fact). Opt-in, fail-open —
    // never blocks an edit on a confidence-subsystem hiccup.
    super::assert_confidence_signals(&network).await;

    let old_content = super::extract_old_content(&payload, &tool_name);

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
            match security::resolve_safe_path(&file_path, &root) {
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
        super::log_hook_event("pre", &tool_name, &file_path, 2, &logged);
        process::exit(2);
    }

    if !warnings.is_empty() {
        for w in &warnings {
            eprintln!("phronesis: WARNING — {}", w);
        }
        super::log_hook_event("pre", &tool_name, &file_path, 1, &logged);
        process::exit(1);
    }

    super::log_hook_event("pre", &tool_name, &file_path, 0, &logged);
    super::exit_ok();
}
