use std::process;

use phr::Fact;

use crate::diff_extract;
use crate::hook_facts::{
    assert_common_facts, assert_diff_facts, assert_test_facts, assert_values_facts,
    check_bash_command_patterns, check_content_patterns, check_missing_patterns,
    collect_bash_command_patterns, collect_content_patterns, collect_missing_patterns,
};
use crate::security::{self, MAX_FACT_CONTENT_BYTES, read_file_capped, resolve_safe_path};

use super::HookError;

/// Sentinel: `file_path` resolved outside the project root. The entrypoint
/// maps this to `process::exit(1)`.
struct PathViolation;

/// PostToolUse hook: validate the result after edit/write.
/// Exit 0 = pass, exit 1 = warn (Claude sees the message and can self-correct).
///
/// Warns (exit 1) when:
/// - stdin payload exceeds the size cap
/// - `.phronesis/rules.json` is malformed
/// - `file_path` resolves outside the project root
/// - rule loading or firing fails
pub async fn run_post_check() -> anyhow::Result<()> {
    let payload = match super::read_payload() {
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
        _ => super::exit_ok(),
    };

    // Resolve project root once; used in journey paths below.
    let file_path = super::extract_file_path(&payload);

    let (rules, content_patterns, bash_command_patterns, missing_patterns) = {
        let rules = match super::load_rules("post") {
            Ok(Some(r)) => r,
            Ok(None) => {
                // No post rules — still journal the executed call so future
                // pre-checks see this in journey aggregators. Journey is
                // fail-open and best-effort.
                super::journey_record::journey_record_post(&payload, &tool_name, &file_path).await;
                super::exit_ok();
            }
            Err(e) => {
                eprintln!("phronesis: WARNING — {}", e);
                // The call already executed; journal it before exiting with a
                // warning code.
                super::journey_record::journey_record_post(&payload, &tool_name, &file_path).await;
                process::exit(1);
            }
        };
        let cp = collect_content_patterns(&rules);
        let bcp = collect_bash_command_patterns(&rules);
        let mp = collect_missing_patterns(&rules);
        (rules, cp, bcp, mp)
    };

    let network = {
        let rules_for_journey = rules.clone();
        let mut net = crate::net::build_network();
        for rule in rules {
            if let Err(e) = net.add_rule(rule).await {
                eprintln!("phronesis: WARNING — failed to load rule: {}", e);
                process::exit(1);
            }
        }
        if let Err(e) = assert_common_facts(&net, &file_path, &tool_name, "post").await {
            eprintln!("phronesis: WARNING — failed to assert facts: {}", e);
            process::exit(1);
        }
        // Journey facts: recomputed every invocation from the durable journal,
        // before update_agenda. Fail-open on transient I/O; surface config
        // errors as a post-check warning (the action already happened — the
        // next pre-check will block until the config is fixed).
        if let Err(e) = super::assert_journey_facts_into(
            &mut net,
            &security::project_root(),
            &rules_for_journey,
        )
        .await
        {
            eprintln!("phronesis: WARNING — {}", e);
            process::exit(1);
        }
        // Pack-marker facts (e.g. `confidence_enabled`) — let rules from one
        // pack self-deactivate when a superseding pack is opted in.
        super::assert_pack_marker_facts(&net, &security::project_root()).await;
        net
    };

    // Command tools carry no file_path, so the disk-read below yields no
    // content for them — their "content" is the command itself, taken from
    // the payload. Post-phase command rules are advisory (the command
    // already ran); they warn so the agent can correct course.
    if matches!(tool_name.as_str(), "Bash" | "run_shell_command")
        && let Some(command) = super::extract_new_content(&payload, &tool_name)
        && let Err(e) =
            check_bash_command_patterns(&network, &command, &bash_command_patterns).await
    {
        eprintln!("phronesis: WARNING — command pattern check failed: {}", e);
        process::exit(1);
    }

    // Validate the file path is inside the project root before reading.
    // An empty file_path means the hook input didn't include one — skip file read.
    if let Some(content) = read_disk_content(&file_path).unwrap_or_else(|_| process::exit(1)) {
        assert_post_content_facts(
            &network,
            &file_path,
            &content,
            &content_patterns,
            &missing_patterns,
        )
        .await
        .unwrap_or_else(|_| process::exit(1));
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

    let (logged, violations, warnings) = super::collect_logged(&consequences);

    // Post-check can't undo the edit, so violations and warnings collapse to
    // the same exit code (1). The single `consequences` array on the log entry
    // preserves which rule emitted which severity for downstream consumers.
    if violations.is_empty() && warnings.is_empty() {
        super::log_hook_event("post", &tool_name, &file_path, 0, &logged);
        // Journal the executed call at the tail — see SPEC §"Where it
        // plugs into the hook" — after the decision is logged, before the
        // exit.
        super::journey_record::journey_record_post(&payload, &tool_name, &file_path).await;
        super::exit_ok();
    }

    for v in &violations {
        eprintln!("phronesis: WARNING — {}", v);
    }
    for w in &warnings {
        eprintln!("phronesis: WARNING — {}", w);
    }
    super::log_hook_event("post", &tool_name, &file_path, 1, &logged);
    super::journey_record::journey_record_post(&payload, &tool_name, &file_path).await;
    process::exit(1);
}

/// Read the post-edited file from disk, capped at the security limit.
///
/// Returns `Ok(None)` when the path is empty or the file is unreadable.
/// Returns `Err(PathViolation)` when the path resolves outside the project
/// root — the caller maps this to `process::exit(1)`.
fn read_disk_content(file_path: &str) -> Result<Option<String>, PathViolation> {
    if file_path.is_empty() {
        return Ok(None);
    }
    let root = security::project_root();
    match resolve_safe_path(file_path, &root) {
        Ok(safe) => match read_file_capped(&safe) {
            Ok(content) => Ok(Some(content)),
            Err(e) => {
                eprintln!("phronesis: WARNING — could not read file: {}", e);
                Ok(None)
            }
        },
        Err(security::SecurityError::PathOutsideRoot(_))
        | Err(security::SecurityError::PathTraversal(_)) => {
            eprintln!(
                "phronesis: WARNING — file_path {:?} is outside project root",
                file_path
            );
            Err(PathViolation)
        }
        Err(_) => Ok(None),
    }
}

/// Assert all content-derived facts for the post-check phase: file_content,
/// pattern checks, cargo-workspace scanner, missing-pattern checks, diff facts,
/// values facts, and TDD test-existence facts.  Logs the specific error message
/// to stderr on failure and returns `Err` — the caller maps that to
/// `process::exit(1)`.
async fn assert_post_content_facts(
    network: &phr::ReteNetwork,
    file_path: &str,
    content: &str,
    content_patterns: &[String],
    missing_patterns: &[String],
) -> Result<(), HookError> {
    // Only assert the full content as a fact when small enough to keep
    // working-memory growth bounded. Pattern checks below still operate on
    // the in-memory slice and emit the targeted predicates.
    if content.len() <= MAX_FACT_CONTENT_BYTES {
        network
            .assert_fact(Fact {
                id: "file_content".to_string(),
                predicate: "file_content".to_string(),
                args: vec![content.to_string()],
                timestamp: 0,
            })
            .await
            .map_err(|e| {
                eprintln!("phronesis: WARNING — failed to assert content fact: {}", e);
                HookError::from(e)
            })?;
    }

    check_content_patterns(network, file_path, content, content_patterns)
        .await
        .map_err(|e| {
            eprintln!("phronesis: WARNING — pattern check failed: {}", e);
            e
        })?;

    // Cargo-workspace scanner: applies to Bash command content as well as
    // file content. Has no file-extension gate, so it can't go through
    // DiffFacts::extract (which returns empty for unknown extensions).
    super::assert_cargo_workspace_facts(network, content).await;

    check_missing_patterns(network, content, missing_patterns)
        .await
        .map_err(|e| {
            eprintln!("phronesis: WARNING — missing-pattern check failed: {}", e);
            e
        })?;

    // For post-check, no `old`: treat every function/import in the resulting
    // file as "present" (added). Useful for rules that check the final state
    // (e.g., "every test file must have at least one `assert`").
    assert_diff_facts(network, file_path, None, content)
        .await
        .map_err(|e| {
            eprintln!("phronesis: WARNING — diff-fact assertion failed: {}", e);
            e
        })?;

    // Syntax-aware structural facts so post-phase rules using AST-derived
    // predicates (function_is_public, function_param_type, struct_derives,
    // function_throws, function_uses_force_unwrap, ...) can fire on the
    // final post-edit state.
    assert_values_facts(network, file_path, content, None)
        .await
        .map_err(|e| {
            eprintln!("phronesis: WARNING — values-fact assertion failed: {}", e);
            e
        })?;

    let added = diff_extract::extract(file_path, None, content).functions_added;
    let project_root = security::project_root();
    assert_test_facts(network, &project_root, file_path, &added)
        .await
        .map_err(|e| {
            eprintln!("phronesis: WARNING — test-fact assertion failed: {}", e);
            e
        })?;

    Ok(())
}
