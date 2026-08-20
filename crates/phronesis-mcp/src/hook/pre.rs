use std::process;

use phr::{Fact, ReteNetwork};

use crate::hook_facts::{
    assert_common_facts, assert_diff_facts, assert_language_pack_facts, assert_test_facts,
    assert_values_facts, check_bash_command_patterns, check_content_patterns,
    collect_bash_command_patterns, collect_content_patterns,
};
use crate::security;

use super::{HookError, HookPayload};

/// PreToolUse hook: validate proposed changes before they happen.
/// Exit 0 = allow, exit 2 = block.
///
/// Fails closed (exit 2) when:
/// - stdin payload exceeds the size cap
/// - `.phronesis/rules.json` exists but is malformed
/// - rule loading, fact assertion, or rule firing fails
pub async fn run_pre_check() -> anyhow::Result<()> {
    let payload = match super::read_payload("pre") {
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

    let (rules, override_facts, content_patterns, bash_command_patterns) = {
        let loaded = match super::load_rules("pre") {
            Ok(Some(r)) => r,
            Ok(None) => super::exit_ok(),
            Err(e) => {
                eprintln!("phronesis: BLOCKED — {}", e);
                process::exit(2);
            }
        };
        let rules = loaded.rules;
        let cp = collect_content_patterns(&rules);
        let bcp = collect_bash_command_patterns(&rules);
        (rules, loaded.override_facts, cp, bcp)
    };

    // Populated only when the structural graph has drifted; these rules'
    // violations are demoted to warnings before the exit decision.
    let mut stale_graph_rules: std::collections::BTreeSet<phr::RuleId> =
        std::collections::BTreeSet::new();

    let (new_content, file_path) = (
        super::extract_new_content(&payload, &tool_name),
        super::extract_file_path(&payload),
    );

    let network = {
        let rules_for_journey = rules.clone();
        let mut net = crate::net::build_network();
        for rule in rules {
            if let Err(e) = net.add_rule(rule).await {
                eprintln!("phronesis: BLOCKED — failed to load rule: {}", e);
                process::exit(2);
            }
        }
        if let Err(e) = assert_common_facts(&net, &file_path, &tool_name, "pre").await {
            eprintln!("phronesis: BLOCKED — failed to assert facts: {}", e);
            process::exit(2);
        }
        for fact in override_facts {
            if let Err(e) = net.assert_fact(fact).await {
                eprintln!("phronesis: BLOCKED — failed to assert rule override provenance: {e}");
                process::exit(2);
            }
        }
        if let Err(e) = super::assert_journey_facts_into(
            &mut net,
            &security::project_root(),
            &rules_for_journey,
        )
        .await
        {
            eprintln!("phronesis: BLOCKED — {}", e);
            process::exit(2);
        }
        super::assert_pack_marker_facts(&net, &security::project_root()).await;
        super::assert_confidence_signals(&net).await;
        // Structural graph facts. Costs nothing unless a loaded rule names a
        // graph relation. Only facts about the code are asserted; whether the
        // graph is trustworthy is machinery health, handled below.
        {
            let root = security::project_root();
            let edited = (!file_path.is_empty()).then_some(file_path.as_str());
            let h = crate::graph::hydrate::hydrate(&root, &rules_for_journey, edited);
            if !h.fresh && !h.facts.is_empty() {
                let cause = if h.outdated {
                    "was built by an older phronesis and names entities differently".to_string()
                } else {
                    format!(
                        "is stale ({} file(s) changed outside the hook)",
                        h.drifted.len()
                    )
                };
                eprintln!(
                    "phronesis: NOTE — structural graph {cause}; structural rules will warn, not block. Run `phr-mcp graph rebuild`."
                );
                // Rules reading a drifted graph reason from evidence we can't
                // vouch for, so the harness declines to act on their verdicts.
                stale_graph_rules = h.graph_rules;
            }
            for (rule_id, symbols) in crate::graph::bindings::stale_rules(&root, h.verified_fresh) {
                eprintln!(
                    "phronesis: NOTE — rule `{rule_id}` names `{}`, which the code graph no longer defines; this rule will warn, not block. Review or retire it.",
                    symbols.join("`, `")
                );
                stale_graph_rules.insert(rule_id);
            }
            for fact in h.facts {
                if let Err(e) = net.assert_fact(fact).await {
                    eprintln!("phronesis: WARNING — graph fact rejected: {}", e);
                    break;
                }
            }
        }
        net
    };

    if let Some(content) = &new_content
        && assert_pre_content_facts(
            &network,
            PreContentInput {
                payload: &payload,
                tool_name: &tool_name,
                content,
                file_path: &file_path,
                content_patterns: &content_patterns,
                bash_command_patterns: &bash_command_patterns,
            },
        )
        .await
        .is_err()
    {
        process::exit(2);
    }

    let provider_event = super::provider_event(&payload, &tool_name, &file_path, "pre");
    if let Err(error) = crate::predicate_provider::assert_facts(
        &network,
        &security::project_root(),
        &provider_event,
    )
    .await
    {
        eprintln!("phronesis: BLOCKED — {error}");
        process::exit(2);
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
    crate::capsule::capture_for_hook(&security::project_root(), &consequences);

    let (mut logged, violations, warnings) =
        super::collect_logged(&consequences, &security::project_root());
    // A drifted graph makes structural verdicts unreliable, so they warn
    // rather than block. The rules themselves are untouched — this is the
    // harness declining to enforce on evidence it cannot vouch for.
    let (violations, warnings) = if stale_graph_rules.is_empty() {
        (violations, warnings)
    } else {
        crate::hook_logged::demote_violations_from(&mut logged, &stale_graph_rules);
        super::split_messages_by_action_type(&logged)
    };

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
        super::log_hook_event(&super::LogEventInput {
            phase: "pre",
            tool_name: &tool_name,
            file_path: &file_path,
            exit: 2,
            command_exit: None,
            consequences: &logged,
        });
        process::exit(2);
    }

    if !warnings.is_empty() {
        for w in &warnings {
            eprintln!("phronesis: WARNING — {}", w);
        }
        super::log_hook_event(&super::LogEventInput {
            phase: "pre",
            tool_name: &tool_name,
            file_path: &file_path,
            exit: 1,
            command_exit: None,
            consequences: &logged,
        });
        process::exit(1);
    }

    super::log_hook_event(&super::LogEventInput {
        phase: "pre",
        tool_name: &tool_name,
        file_path: &file_path,
        exit: 0,
        command_exit: None,
        consequences: &logged,
    });
    super::exit_ok();
}

/// Assert all content-derived facts for the pre-check phase: new_content,
/// pattern checks, cargo-workspace scanner, diff facts, values facts, and
/// TDD test-existence facts.  Logs the specific error message to stderr on
/// failure and returns `Err` — the caller maps that to `process::exit(2)`.
struct PreContentInput<'a> {
    payload: &'a HookPayload,
    tool_name: &'a str,
    content: &'a str,
    file_path: &'a str,
    content_patterns: &'a [String],
    bash_command_patterns: &'a [String],
}

async fn assert_pre_content_facts(
    network: &ReteNetwork,
    input: PreContentInput<'_>,
) -> Result<(), HookError> {
    let PreContentInput {
        payload,
        tool_name,
        content,
        file_path,
        content_patterns,
        bash_command_patterns,
    } = input;
    network
        .assert_fact(Fact {
            id: "new_content".to_string(),
            predicate: "new_content".to_string(),
            args: vec![content.to_string()],
            timestamp: 0,
            source: Some("hook".to_string()),
        })
        .await
        .map_err(|e| {
            eprintln!("phronesis: BLOCKED — failed to assert content fact: {}", e);
            HookError::from(e)
        })?;

    check_content_patterns(network, file_path, content, content_patterns)
        .await
        .map_err(|e| {
            eprintln!("phronesis: BLOCKED — pattern check failed: {}", e);
            e
        })?;

    // Command-content regexes apply only to command tools — file
    // content quoting the same text must not trip command rules.
    if matches!(tool_name, "Bash" | "run_shell_command") {
        check_bash_command_patterns(network, content, bash_command_patterns)
            .await
            .map_err(|e| {
                eprintln!("phronesis: BLOCKED — command pattern check failed: {}", e);
                e
            })?;
    }

    // Cargo-workspace scanner: applies to Bash command content as well as
    // file content. Has no file-extension gate, so it can't go through
    // DiffFacts::extract (which returns empty for unknown extensions).
    super::assert_cargo_workspace_facts(network, content).await;

    let old_content = super::extract_old_content(payload, tool_name);

    // Diff-aware structural facts (function_added/removed, import_added/removed).
    assert_diff_facts(network, file_path, old_content.as_deref(), content)
        .await
        .map_err(|e| {
            eprintln!("phronesis: BLOCKED — diff-fact assertion failed: {}", e);
            e
        })?;

    // Syntax-aware structural facts (function_returns_result_string, etc.)
    // At pre-check, the edit hasn't applied yet, so disk still holds the
    // prior content — read it for the delta filter on heavy-clone facts.
    // Resolve `file_path` against the project root so the read works
    // regardless of process cwd (matches the post-check path-resolution).
    let old_disk_content = if !file_path.is_empty() {
        let root = security::project_root();
        match security::resolve_safe_path(file_path, &root) {
            Ok(safe) => std::fs::read_to_string(&safe).ok(),
            Err(_) => None,
        }
    } else {
        None
    };

    assert_values_facts(network, file_path, content, old_disk_content.as_deref())
        .await
        .map_err(|e| {
            eprintln!("phronesis: BLOCKED — values-fact assertion failed: {}", e);
            e
        })?;

    assert_language_pack_facts(network, file_path, content)
        .await
        .map_err(|e| {
            eprintln!(
                "phronesis: BLOCKED — language-pack fact assertion failed: {}",
                e
            );
            e
        })?;

    // TDD support: for each newly-added function, assert test_exists_for / no_test_for.
    let added =
        crate::diff_extract::extract(file_path, old_content.as_deref(), content).functions_added;
    let project_root = security::project_root();
    assert_test_facts(network, &project_root, file_path, &added)
        .await
        .map_err(|e| {
            eprintln!("phronesis: BLOCKED — test-fact assertion failed: {}", e);
            e
        })?;

    Ok(())
}
