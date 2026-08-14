use phr::Fact;

use crate::journey;
use crate::outcomes;
use crate::security;

use super::HookPayload;

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

/// Trailing `exit code: N` line in captured output text — the last-resort
/// source when the payload has no structured exit field.
fn exit_from_text(text: &str) -> Option<i32> {
    text.lines().rev().find_map(|l| {
        l.trim()
            .strip_prefix("exit code: ")
            .and_then(|n| n.trim().parse().ok())
    })
}

/// Best-effort exit-code extraction from the tool-response object. The exact
/// location is CLI-specific (pinned by the payload-contract corpus); known
/// candidates are tried in order, then the trailing-text fallback. `None`
/// means the CLI didn't provide one — confidence falls back to Tier 2.
pub(super) fn payload_command_exit(payload: &HookPayload) -> Option<i32> {
    let v = payload.tool_output.as_ref()?;
    if let Some(obj) = v.as_object() {
        for key in ["exit_code", "exitCode", "returncode", "code", "status"] {
            if let Some(n) = obj.get(key).and_then(serde_json::Value::as_i64) {
                return i32::try_from(n).ok();
            }
        }
    }
    exit_from_text(&extract_tool_output_text(payload))
}

/// Compute the outcome tags + resolved subject for a post-check tool call.
/// The result is folded into the journey journal record at the post-check
/// tail (no separate ledger). Returns `(tags, subject, command_exit)`.
///
/// Mirrors what `capture_outcomes` did pre-0.13: subject lifecycle (open on
/// recognized commands, settle on `git commit`) and outcome parsing. The
/// storage write is now the single `journey::journal::append` call in the
/// hook tail.
fn outcomes_for_journal(
    payload: &HookPayload,
    tool_name: &str,
) -> (Vec<String>, Option<String>, Option<i32>) {
    let root = security::project_root();
    let command = super::extract_new_content(payload, tool_name);
    let output = extract_tool_output_text(payload);
    let command_exit = matches!(tool_name, "Bash" | "run_shell_command")
        .then(|| payload_command_exit(payload))
        .flatten();
    let (tags, subject) = outcomes::adapter::extract_from(outcomes::adapter::ExtractFromInput {
        project_root: &root,
        tool_name,
        command: command.as_deref(),
        output: &output,
        command_exit,
    });
    (tags, subject, command_exit)
}

/// Load the tagger config from the project root, falling back to default on
/// any error. Failure to find the config is not an error — projects without
/// `journey.json` use the zero-config default.
fn load_tagger_config(root: &std::path::Path) -> journey::tagger::TaggerConfig {
    match journey::load_config(root) {
        Ok(c) => c,
        Err(journey::ConfigError::NotFound(_)) => journey::tagger::TaggerConfig::default(),
        Err(e) => {
            eprintln!("phronesis: journey config skipped: {}", e);
            journey::tagger::TaggerConfig::default()
        }
    }
}

/// Assemble the `JournalRecord` from the tagger result, outcome tags, and
/// project-root metadata.
struct JournalRecordInput<'a> {
    tool_name: &'a str,
    file_path: &'a str,
    tag_result: journey::tagger::TagResult,
    outcome_tags: Vec<String>,
    module: Option<String>,
    subject: Option<String>,
    command_exit: Option<i32>,
    project_root: &'a std::path::Path,
}

fn build_journal_record(input: JournalRecordInput<'_>) -> journey::journal::JournalRecord {
    let ext = std::path::Path::new(input.file_path)
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string());
    let mut all_tags = input.tag_result.tags;
    all_tags.extend(input.outcome_tags);
    journey::journal::JournalRecord {
        v: 1,
        ts: super::unix_secs_now(),
        sid: journey::current_sid(input.project_root),
        seq: super::seq::next_seq(input.project_root),
        tool: input.tool_name.to_string(),
        path: input.file_path.to_string(),
        ext,
        module: input.module,
        tags: all_tags,
        subject: input.subject,
        command_exit: input.command_exit,
    }
}

/// Journey wiring at the **tail of `run_post_check`**: tag the call, resolve
/// its module, fold in outcome tags + subject, append one journal record.
/// Fail-open: any failure (config parse, tagger error, IO) is swallowed.
pub(super) async fn journey_record_post(payload: &HookPayload, tool_name: &str, file_path: &str) {
    if std::env::var("PHRONESIS_NO_JOURNEY").is_ok() {
        return;
    }
    let project_root = security::project_root();
    let cfg = load_tagger_config(&project_root);
    let facts = tagger_facts(payload, tool_name, file_path, &cfg);
    let tag_result = journey::tagger::fire(&cfg, &facts)
        .await
        .unwrap_or_default();
    let module = journey::tagger::resolve_module(&cfg, file_path);
    let (outcome_tags, subject, command_exit) = outcomes_for_journal(payload, tool_name);
    let record = build_journal_record(JournalRecordInput {
        tool_name,
        file_path,
        tag_result,
        outcome_tags,
        module,
        subject,
        command_exit,
        project_root: &project_root,
    });
    journey::journal::append(&project_root, &record).ok();
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
        source: Some("hook".to_string()),
    });
    for part in file_path.split('/') {
        if !part.is_empty() {
            facts.push(Fact {
                id: format!("file_path_matches_{}", part),
                predicate: "file_path_matches".to_string(),
                args: vec![part.to_string()],
                timestamp: 0,
                source: Some("hook".to_string()),
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
            source: Some("hook".to_string()),
        });
    }
    if let Some(content) = super::extract_new_content(payload, tool_name) {
        facts.push(Fact {
            id: "new_content".to_string(),
            predicate: "new_content".to_string(),
            args: vec![content.clone()],
            timestamp: 0,
            source: Some("hook".to_string()),
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
                        source: Some("hook".to_string()),
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

    fn payload_with_output(output: serde_json::Value) -> HookPayload {
        HookPayload {
            tool_name: Some("Bash".to_string()),
            tool_input: Some(serde_json::json!({ "command": "cargo build --workspace" })),
            tool_output: Some(output),
        }
    }

    #[test]
    fn command_exit_from_exit_code_key() {
        let p = payload_with_output(serde_json::json!({ "exit_code": 101, "stdout": "boom" }));
        assert_eq!(payload_command_exit(&p), Some(101));
    }

    #[test]
    fn command_exit_tries_alternate_keys_in_order() {
        for key in ["exit_code", "exitCode", "returncode", "code", "status"] {
            let p = payload_with_output(serde_json::json!({ key: 2 }));
            assert_eq!(
                payload_command_exit(&p),
                Some(2),
                "key {key} should be read"
            );
        }
    }

    #[test]
    fn command_exit_falls_back_to_trailing_text_line() {
        let p = payload_with_output(serde_json::json!({
            "stdout": "   Compiling foo\nerror: boom\nexit code: 101"
        }));
        assert_eq!(payload_command_exit(&p), Some(101));
    }

    #[test]
    fn command_exit_absent_is_none() {
        let p = payload_with_output(serde_json::json!({ "stdout": "Finished dev profile" }));
        assert_eq!(payload_command_exit(&p), None);
    }

    #[test]
    fn command_exit_none_without_tool_output() {
        let p = make_payload("Bash", serde_json::json!({ "command": "ls" }));
        assert_eq!(payload_command_exit(&p), None);
    }

    #[test]
    fn command_exit_string_output_uses_text_fallback() {
        let p = payload_with_output(serde_json::Value::String(
            "ran stuff\nexit code: 3".to_string(),
        ));
        assert_eq!(payload_command_exit(&p), Some(3));
    }

    #[test]
    fn command_exit_non_numeric_status_is_none() {
        // Some CLIs send status as a string ("success") — only numeric counts.
        let p = payload_with_output(serde_json::json!({ "status": "success" }));
        assert_eq!(payload_command_exit(&p), None);
    }
}
