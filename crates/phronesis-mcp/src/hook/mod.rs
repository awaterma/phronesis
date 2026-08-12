//! Hook subcommands: pre-check (PreToolUse) and post-check (PostToolUse).
//!
//! The top-level entry points are [`run_pre_check`] and [`run_post_check`],
//! re-exported from the sub-modules where they live.  Shared primitives
//! (payload parsing, rule loading, fact helpers, logging) stay here.

mod journey_record;
mod post;
mod pre;
pub(crate) mod seq;

pub use post::run_post_check;
pub use pre::run_pre_check;

use std::path::Path;
use std::process;

use phr::consequence::Consequence;
use phr::{Fact, ReteNetwork, Rule};
use serde::Deserialize;
use thiserror::Error;

use crate::action_log::{self, LogEntry};
use crate::clock_facts;
use crate::journey;
use crate::outcomes;
use crate::security;
use fs2::FileExt as _;

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
pub(super) struct HookPayload {
    pub(super) tool_name: Option<String>,
    pub(super) tool_input: Option<serde_json::Value>,
    /// PostToolUse payloads carry the tool's output here. Claude Code sends
    /// this field as `tool_response`; Gemini and our own integration tests
    /// use `tool_output`. The serde alias accepts both so confidence scoring
    /// sees the captured stdout/stderr of a build/test command regardless of
    /// which runtime fired the hook.
    #[serde(default, alias = "tool_response")]
    pub(super) tool_output: Option<serde_json::Value>,
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

fn read_payload(phase: &str) -> anyhow::Result<HookPayload> {
    let input = security::read_stdin_capped()?;
    capture_raw_payload(phase, &input);
    let payload: HookPayload = serde_json::from_str(&input)?;
    Ok(payload)
}

/// When `PHRONESIS_CAPTURE_DIR` is set, append the raw stdin payload as one
/// JSONL record to `<dir>/payloads.jsonl`. Best-effort: capture must never
/// change hook behavior or exit codes, so every failure path returns silently.
/// Uses an exclusive advisory file lock (fs2 flock) around the write so
/// concurrent hook processes cannot interleave lines.
fn capture_raw_payload(phase: &str, raw: &str) {
    let Ok(dir) = std::env::var("PHRONESIS_CAPTURE_DIR") else {
        return;
    };
    let record = serde_json::json!({
        "ts": unix_secs_now(),
        "phase": phase,
        "raw": serde_json::from_str::<serde_json::Value>(raw)
            .unwrap_or_else(|_| serde_json::Value::String(raw.to_string())),
    });
    let path = std::path::Path::new(&dir).join("payloads.jsonl");
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    if file.lock_exclusive().is_err() {
        return;
    }
    use std::io::Write as _;
    let _ = writeln!(file, "{record}");
    let _ = file.unlock();
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

pub(crate) fn provider_event(
    payload: &HookPayload,
    tool_name: &str,
    file_path: &str,
    phase: &str,
) -> crate::predicate_provider::ProviderEvent {
    let new_content = extract_new_content(payload, tool_name).unwrap_or_default();
    crate::predicate_provider::ProviderEvent {
        phase: phase.to_string(),
        tool_name: tool_name.to_string(),
        file_path: file_path.to_string(),
        // Relativized with the *same* helper the graph hydrator uses, so the
        // two agree by construction rather than by coincidence. A provider
        // fact and a graph fact can then join on a path.
        file_rel: crate::graph::hydrate::repo_relative(&crate::security::project_root(), file_path)
            .unwrap_or_default(),
        files: Vec::new(),
        old_content: extract_old_content(payload, tool_name).unwrap_or_default(),
        command: matches!(tool_name, "Bash" | "run_shell_command")
            .then_some(new_content.clone())
            .unwrap_or_default(),
        new_content,
        output: payload
            .tool_output
            .as_ref()
            .map(serde_json::Value::to_string)
            .unwrap_or_default(),
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

/// Pack-marker facts shared by `run_pre_check` and `run_post_check`: assert
/// a zero-arg fact (e.g. `confidence_enabled`) for each pack the project has
/// opted into. Lets rules from one pack self-deactivate when a superseding
/// pack is active. See `docs/specs/SPEC-pack-opt-in-facts.md`.
pub(crate) async fn assert_pack_marker_facts(network: &ReteNetwork, project_root: &Path) {
    for marker in clock_facts::pack_markers(project_root) {
        let fact_id = if marker.args.is_empty() {
            marker.predicate.to_string()
        } else {
            format!("{}_{}", marker.predicate, marker.args.join("_"))
        };
        if let Err(e) = network
            .assert_fact(Fact {
                id: fact_id,
                predicate: marker.predicate.to_string(),
                args: marker.args,
                timestamp: 0,
            })
            .await
        {
            eprintln!("phronesis: pack marker assertion skipped: {}", e);
        }
    }
}

/// Journey wiring shared by `run_pre_check` and `run_post_check`: derive the
/// `journey_*` facts the rules reference, assert them into the live network.
///
/// Failure policy is split by error category — see
/// `.phronesis/wiki/decisions/2026-06-23-undefined-selector-rejection.md`:
///
/// - **Configuration errors** (`BadWindow`, `UndefinedSelector`) bubble up
///   to the caller, which fails the hook closed. A typo in `rules.json` or
///   `journey.json` can't fix itself by retrying, and the most dangerous
///   shape (absence rules with a missing tagger) fires constantly without
///   the user noticing if we fail open.
/// - **I/O errors** (`Journal`) are logged and swallowed. Transient
///   journal hiccups shouldn't block every edit, and rules that don't
///   reference `journey_*` are unaffected anyway.
async fn assert_journey_facts_into(
    network: &mut ReteNetwork,
    project_root: &Path,
    rules: &[Rule],
) -> Result<(), journey::derive::DeriveError> {
    if std::env::var("PHRONESIS_NO_JOURNEY").is_ok() {
        return Ok(());
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
    let scope = journey::derive::WindowScope {
        current_sid: &sid,
        now_ts: now,
    };
    if let Err(e) = journey::derive::assert_facts(
        network,
        journey::derive::DeriveInput {
            project_root,
            rules,
            config: &cfg,
            scope,
        },
    )
    .await
    {
        if e.is_config_error() {
            return Err(e);
        }
        eprintln!("phronesis: journey derivation skipped: {}", e);
    }
    Ok(())
}

/// Pre-check side of confidence scoring: assert the open work unit's
/// `signal_pass` facts so gate rules (`facts_count('signal_pass', ...)`) can
/// fire. Opt-in and fail-open.
pub(crate) async fn assert_confidence_signals(network: &ReteNetwork) {
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

/// Record a hook event to `.phronesis/log.jsonl`. Best-effort: log failures
/// are intentionally swallowed because we never want logging to alter the
/// hook's exit code (which is the contract Claude Code depends on).
///
/// Parameters for [`log_hook_event`].
pub(super) struct LogEventInput<'a> {
    pub(super) phase: &'static str,
    pub(super) tool_name: &'a str,
    pub(super) file_path: &'a str,
    pub(super) exit: i32,
    pub(super) command_exit: Option<i32>,
    pub(super) consequences: &'a [LoggedConsequence],
}

pub(super) fn log_hook_event(input: &LogEventInput<'_>) {
    let LogEventInput {
        phase,
        tool_name,
        file_path,
        exit,
        command_exit,
        consequences,
    } = input;
    let event = match *phase {
        "pre" => "pre_check",
        "post" => "post_check",
        _ => "hook_event",
    };
    let consequences_value = serde_json::to_value(consequences).unwrap_or(serde_json::Value::Null);
    let mut entry = LogEntry::new("hook", event)
        .with("phase", phase.to_string())
        .with("tool", tool_name.to_string())
        .with("file", file_path.to_string())
        .with("exit", *exit)
        .with("consequences", consequences_value);
    if let Some(ce) = command_exit {
        entry = entry.with("command_exit", *ce);
    }
    let path = action_log::default_path(&security::project_root());
    let _ = action_log::append(&path, &entry);
}

pub(crate) use crate::hook_logged::{LoggedConsequence, split_messages_by_action_type};

/// Assert `cargo_command_lacks_workspace` facts for every cargo sub-command in
/// `content` that is missing `--workspace`. Shared by pre- and post-check.
pub(super) async fn assert_cargo_workspace_facts(network: &ReteNetwork, content: &str) {
    for cmd in crate::diff_extract::cargo_commands_lacking_workspace(content) {
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
}

/// Project `consequences` into logged entries and split into
/// `(logged, violations, warnings)`. Shared by pre- and post-check.
pub(super) fn collect_logged(
    consequences: &[Consequence],
) -> (Vec<LoggedConsequence>, Vec<String>, Vec<String>) {
    let logged: Vec<LoggedConsequence> = consequences
        .iter()
        .filter_map(LoggedConsequence::from_consequence)
        .collect();
    let (violations, warnings) = split_messages_by_action_type(&logged);
    (logged, violations, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hook_facts::{
        assert_common_facts, assert_language_pack_facts, check_content_patterns,
        check_missing_patterns, collect_content_patterns, filter_new_or_increased_clone_counts,
    };
    use phr::consequence::Consequence;
    use phr::{Action, Condition, Rule};
    use std::collections::HashMap;

    fn make_payload(tool_name: &str, input: serde_json::Value) -> HookPayload {
        HookPayload {
            tool_name: Some(tool_name.to_string()),
            tool_input: Some(input),
            tool_output: None,
        }
    }

    #[tokio::test]
    async fn proposed_lua_content_drives_a_pack_rule_end_to_end() {
        let network = ReteNetwork::new();
        network
            .add_rule(Rule {
                id: "warn-lua-dynamic-code-load".into(),
                priority: 10,
                conditions: vec![Condition {
                    predicate: "lua_dynamic_code_load".into(),
                    args: vec!["?file".into(), "?module".into(), "?loader".into()],
                    script: None,
                }],
                actions: vec![Action {
                    action_type: "constraint_warning".into(),
                    params: vec!["dynamic ?loader in ?file".into()],
                }],
            })
            .await
            .unwrap();

        assert_language_pack_facts(&network, "scripts/bootstrap.lua", "load(payload)")
            .await
            .unwrap();
        network.update_agenda().await.unwrap();
        let actions = network.execute_all_agenda_items().unwrap();

        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].action_type, "constraint_warning");
        assert_eq!(
            actions[0].params,
            vec!["dynamic load in scripts/bootstrap.lua"]
        );
    }

    #[tokio::test]
    async fn proposed_yaml_duplicate_drives_a_pack_rule_end_to_end() {
        let network = ReteNetwork::new();
        network
            .add_rule(Rule {
                id: "block-yaml-duplicate-key".into(),
                priority: 10,
                conditions: vec![Condition {
                    predicate: "yaml_duplicate_key".into(),
                    args: vec!["?file".into(), "?key".into(), "?line".into()],
                    script: None,
                }],
                actions: vec![Action {
                    action_type: "constraint_warning".into(),
                    params: vec!["duplicate ?key in ?file".into()],
                }],
            })
            .await
            .unwrap();

        assert_language_pack_facts(&network, "config/app.yaml", "name: one\nname: two\n")
            .await
            .unwrap();
        network.update_agenda().await.unwrap();
        let actions = network.execute_all_agenda_items().unwrap();

        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].params, vec!["duplicate name in config/app.yaml"]);
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
