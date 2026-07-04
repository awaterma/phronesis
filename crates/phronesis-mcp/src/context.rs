//! Builders for the `additionalContext` payloads consumed by Claude Code's
//! SessionStart / UserPromptSubmit hooks and Gemini CLI's SessionStart /
//! BeforeModelRequest hooks. Pure formatters: each public entry point takes
//! an already-parsed input (a `RulesFile` or `Vec<LogEntry>`) and returns
//! either an empty string (suppress injection) or a JSON object string.

use serde_json::json;

use crate::action_log::{self, LogEntry, ReadOpts};
use crate::rules_file::{DiskRule, RulesFile};

/// Default hard cap per payload, in bytes. Truncates the body with an
/// elision marker when exceeded. Matches the value documented in the spec.
pub const DEFAULT_MAX_BYTES: usize = 4 * 1024;

/// Wrap a markdown body in the Claude/Gemini hook-output envelope.
///
/// `hook_event_name` must be one of the documented values:
/// `"SessionStart"`, `"UserPromptSubmit"`, `"BeforeModelRequest"`.
///
/// Returns the serialized JSON string. The body is truncated to `max_bytes`
/// before wrapping (the cap applies to the body, not the JSON envelope, so
/// the final output is slightly larger).
pub fn wrap_additional_context(hook_event_name: &str, body: &str, max_bytes: usize) -> String {
    let truncated = truncate_with_elision(body, max_bytes);
    let payload = json!({
        "hookSpecificOutput": {
            "hookEventName": hook_event_name,
            "additionalContext": truncated,
        }
    });
    payload.to_string()
}

/// Build the markdown body for the SessionStart context payload.
///
/// One bullet per rule: `- <id> — <intent>`. Intent is the first parameter
/// of the rule's first `constraint_violation` or `constraint_warning` action
/// (i.e. the message it would print on fire). Falls back to the rule id
/// when no such action is present, so even rules without a user-facing
/// message still surface.
///
/// Rules with `silent: true` are excluded from the listing AND from the
/// count, so an entirely-silent pack produces an empty body.
///
/// Returns the empty string when the visible rules list is empty — the
/// caller should suppress emitting a JSON payload in that case (no point
/// telling the model "you have no rules").
pub fn build_session_body(rules: &RulesFile) -> String {
    let visible: Vec<&DiskRule> = rules
        .rules
        .iter()
        .filter(|r| r.silent != Some(true))
        .collect();
    if visible.is_empty() {
        return String::new();
    }
    let mut out = format!("## Active phronesis rules ({})\n", visible.len());
    for r in visible {
        let intent = rule_intent(r);
        out.push_str(&format!("- {} — {}\n", r.id, intent));
    }
    out
}

fn rule_intent(r: &DiskRule) -> String {
    r.actions
        .iter()
        .find(|a| {
            matches!(
                a.action_type.as_str(),
                "constraint_violation" | "constraint_warning"
            )
        })
        .and_then(|a| a.params.first().cloned())
        .unwrap_or_else(|| r.id.clone())
}

/// Truncate `s` to at most `max_bytes`, appending an elision marker when a
/// cut was made. Cuts on a char boundary so the result is always valid UTF-8.
fn truncate_with_elision(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    const MARKER: &str = "\n…[truncated]";
    let budget = max_bytes.saturating_sub(MARKER.len());
    let mut cut = budget;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}{}", &s[..cut], MARKER)
}

use crate::rules_file;
use std::path::Path;

/// Default filename for the user-curated "durable directives" file. The
/// content is re-injected at SessionStart AND UserPromptSubmit so the
/// directives stay live even after CLAUDE.md has been compressed out of
/// the model's working context. Intentionally a separate file from
/// CLAUDE.md so the user can choose a small subset that *must* survive.
pub const DURABLE_DIRECTIVES_FILENAME: &str = ".phronesis/durable.md";

/// Read `.phronesis/durable.md` if present. Returns the file contents trimmed,
/// or an empty string for missing/empty/malformed files. Never panics.
pub fn read_durable_directives(project_root: &Path) -> String {
    let path = project_root.join(DURABLE_DIRECTIVES_FILENAME);
    std::fs::read_to_string(&path)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Compose a body section for durable directives. Empty input → empty
/// output (caller should suppress the section). Otherwise wraps with a
/// short heading so the model can recognize the block.
fn build_durable_section(content: &str) -> String {
    if content.is_empty() {
        return String::new();
    }
    format!("## Durable directives\n{}\n", content)
}

/// Top-level entry point for the `turn-context` subcommand.
///
/// Reads up to `last_n` recent `kind=hook` entries from
/// `<project_root>/.phronesis/log.jsonl` (including its rotated
/// predecessor) and emits a markdown summary of recent decisions.
/// Returns an empty string when nothing useful would be injected —
/// missing log, empty log, or no consequences in the recent tail.
///
/// Never panics. Any I/O or parse error is swallowed and produces
/// empty output: context injection must never break the model turn.
pub fn run_turn_context(project_root: &Path, last_n: usize, max_bytes: usize) -> String {
    let path = action_log::default_path(project_root);
    let opts = ReadOpts {
        limit: Some(last_n),
        kind: Some("hook".to_string()),
        ..ReadOpts::default()
    };
    let entries = action_log::read_recent(&path, &opts).unwrap_or_default();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let activity = build_turn_body(&entries, now);

    let durable = build_durable_section(&read_durable_directives(project_root));

    let body = match (durable.is_empty(), activity.is_empty()) {
        (true, true) => return String::new(),
        (true, false) => activity,
        (false, true) => durable,
        (false, false) => format!("{}\n{}", durable, activity),
    };
    wrap_additional_context("UserPromptSubmit", &body, max_bytes)
}

/// Top-level entry point for the `session-context` subcommand.
///
/// Reads `<project_root>/.phronesis/rules.json`. Returns the JSON envelope
/// string the hook should print to stdout, or an empty string when there
/// is nothing to inject (missing file, malformed file, or zero rules).
///
/// Never panics. Malformed files produce empty output rather than failing
/// — context injection is best-effort and must not break the model turn.
///
/// Side effect: stamps `.phronesis/journey/session` with a fresh session
/// id if it doesn't already exist. The hook reads this file to label
/// journal records with a sid the `s` window can filter on. Failures are
/// swallowed — journey is advisory enrichment.
pub fn run_session_context(project_root: &Path, max_bytes: usize) -> String {
    // Stamp `.phronesis/journey/session` so the journal records have a sid
    // to label on the first hook of the session. Single source of truth in
    // `journey::current_sid` — see SPEC-journey-facts §sid.
    let _ = crate::journey::current_sid(project_root);

    let path = rules_file::default_path(project_root);
    let rules_body = rules_file::read(&path)
        .map(|rules| build_session_body(&rules))
        .unwrap_or_default();

    let durable = build_durable_section(&read_durable_directives(project_root));

    let body = match (durable.is_empty(), rules_body.is_empty()) {
        (true, true) => return String::new(),
        (true, false) => rules_body,
        (false, true) => durable,
        (false, false) => format!("{}\n{}", durable, rules_body),
    };
    wrap_additional_context("SessionStart", &body, max_bytes)
}

/// Render all user-visible decision bullets for a single log entry.
///
/// Returns a `Vec` of formatted bullet strings, one per `constraint_violation`
/// or `constraint_warning` consequence. Returns an empty `Vec` when the entry
/// has no consequences or none that surface to the user (e.g. log-only actions).
fn render_entry_bullets(entry: &LogEntry, now_secs: u64) -> Vec<String> {
    let Some(consequences) = entry.data.get("consequences").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    if consequences.is_empty() {
        return Vec::new();
    }
    let file = entry
        .data
        .get("file")
        .and_then(|v| v.as_str())
        .unwrap_or("<unknown>");
    let ago = humanize_ago(now_secs.saturating_sub(entry.ts));
    consequences
        .iter()
        .filter_map(|c| {
            let rule_id = c.get("rule_id").and_then(|v| v.as_str()).unwrap_or("?");
            let action_type = c.get("action_type").and_then(|v| v.as_str()).unwrap_or("");
            let decision = match action_type {
                "constraint_violation" => "BLOCKED",
                "constraint_warning" => "WARNED",
                _ => return None, // log/other actions aren't user-visible decisions
            };
            Some(format!(
                "- {} {} ago: {} in {}",
                decision, ago, rule_id, file
            ))
        })
        .collect()
}

/// Build the markdown body for the UserPromptSubmit / BeforeModelRequest
/// context payload.
///
/// Iterates the supplied log entries (assumed newest-last, as produced by
/// `action_log::read_recent`) and renders one bullet per consequence that
/// actually fired. Entries without consequences are skipped so passing
/// checks don't add noise.
///
/// Returns the empty string when there is nothing useful to say — the
/// caller should suppress emitting any JSON in that case (vs. emitting an
/// empty "## Recent phronesis activity" block on every turn).
///
/// `now_secs` is taken as a parameter so tests can pin "Ns ago" formatting
/// deterministically.
pub fn build_turn_body(entries: &[LogEntry], now_secs: u64) -> String {
    // Newest first, matching the spec's example output.
    let rendered: Vec<String> = entries
        .iter()
        .rev()
        .flat_map(|entry| render_entry_bullets(entry, now_secs))
        .collect();
    if rendered.is_empty() {
        return String::new();
    }
    format!("## Recent phronesis activity\n{}\n", rendered.join("\n"))
}

fn humanize_ago(secs: u64) -> String {
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_emits_documented_envelope() {
        let out = wrap_additional_context("SessionStart", "## hello", DEFAULT_MAX_BYTES);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "SessionStart");
        assert_eq!(v["hookSpecificOutput"]["additionalContext"], "## hello");
    }

    #[test]
    fn wrap_truncates_oversized_body_with_marker() {
        let big = "x".repeat(10_000);
        let out = wrap_additional_context("SessionStart", &big, 100);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let ctx = v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(
            ctx.len() <= 100,
            "ctx body must respect cap, got {}",
            ctx.len()
        );
        assert!(
            ctx.ends_with("…[truncated]"),
            "ctx must carry elision marker"
        );
    }

    #[test]
    fn truncate_at_char_boundary() {
        // A multibyte char (é = 2 bytes) right at the budget edge must not
        // produce invalid UTF-8.
        let s = "aaaé";
        let out = truncate_with_elision(s, 4);
        // Should not panic; result must be valid UTF-8 by construction.
        assert!(out.is_char_boundary(out.len() - "\n…[truncated]".len()));
    }

    use crate::rules_file::{DiskAction, DiskCondition, DiskRule, RulesFile};

    fn rule(id: &str, action_type: &str, message: &str) -> DiskRule {
        DiskRule {
            id: id.to_string(),
            phase: "pre".to_string(),
            priority: 10,
            conditions: vec![DiskCondition {
                predicate: "new_content_contains".to_string(),
                args: vec!["x".to_string()],
                script: None,
            }],
            actions: vec![DiskAction {
                action_type: action_type.to_string(),
                params: vec![message.to_string()],
            }],
            silent: None,
            audit: None,
            doc_excepted: None,
        }
    }

    #[test]
    fn session_body_lists_active_rules_with_intent() {
        let rules = RulesFile {
            rules: vec![
                rule("no-unwrap", "constraint_violation", "Avoid .unwrap()"),
                rule("warn-clone", "constraint_warning", "Too many clones"),
            ],
        };
        let body = build_session_body(&rules);
        assert!(body.contains("Active phronesis rules (2)"));
        assert!(body.contains("- no-unwrap — Avoid .unwrap()"));
        assert!(body.contains("- warn-clone — Too many clones"));
    }

    #[test]
    fn session_body_falls_back_to_rule_id_when_no_intent() {
        let mut r = rule("bare", "log", "anything");
        r.actions.clear();
        let rules = RulesFile { rules: vec![r] };
        let body = build_session_body(&rules);
        assert!(body.contains("- bare — bare"));
    }

    #[test]
    fn session_body_empty_when_no_rules() {
        let rules = RulesFile { rules: vec![] };
        assert_eq!(build_session_body(&rules), "");
    }

    #[test]
    fn session_body_excludes_silent_rules() {
        let mut loud = rule("loud", "constraint_violation", "shout");
        let mut quiet = rule("quiet", "constraint_violation", "hush");
        quiet.silent = Some(true);
        // Confirm the count line reflects only the visible rule.
        let rules = RulesFile {
            rules: vec![loud.clone(), quiet],
        };
        let body = build_session_body(&rules);
        assert!(body.contains("Active phronesis rules (1)"));
        assert!(body.contains("loud"));
        assert!(!body.contains("quiet"));
        // Also confirm silent=Some(false) is treated as visible.
        loud.silent = Some(false);
        let rules = RulesFile { rules: vec![loud] };
        let body = build_session_body(&rules);
        assert!(body.contains("loud"));
    }

    #[test]
    fn session_body_empty_when_all_rules_silent() {
        let mut r = rule("only", "constraint_violation", "x");
        r.silent = Some(true);
        let rules = RulesFile { rules: vec![r] };
        assert_eq!(build_session_body(&rules), "");
    }

    use crate::action_log::LogEntry;
    use serde_json::json;

    fn hook_entry(event: &str, file: &str, exit: i32, consequences: serde_json::Value) -> LogEntry {
        let mut e = LogEntry::new("hook", event)
            .with("phase", if event == "pre_check" { "pre" } else { "post" })
            .with("tool", "Edit")
            .with("file", file.to_string())
            .with("exit", exit);
        e.data.insert("consequences".to_string(), consequences);
        // Pin the timestamp so "Ns ago" formatting is deterministic in tests.
        e.ts = 1_700_000_000;
        e
    }

    #[test]
    fn turn_body_renders_blocked_consequence() {
        let entries = vec![hook_entry(
            "pre_check",
            "src/foo.rs",
            2,
            json!([{
                "rule_id": "no-unwrap",
                "action_type": "constraint_violation",
                "message": "Avoid .unwrap()",
                "bindings": {}
            }]),
        )];
        let body = build_turn_body(&entries, 1_700_000_120); // 120s later
        assert!(body.contains("Recent phronesis activity"));
        assert!(body.contains("BLOCKED"));
        assert!(body.contains("no-unwrap"));
        assert!(body.contains("src/foo.rs"));
    }

    #[test]
    fn turn_body_renders_warned_consequence() {
        let entries = vec![hook_entry(
            "post_check",
            "src/bar.rs",
            0,
            json!([{
                "rule_id": "warn-clone",
                "action_type": "constraint_warning",
                "message": "too many clones",
                "bindings": {}
            }]),
        )];
        let body = build_turn_body(&entries, 1_700_000_005);
        assert!(body.contains("WARNED"));
        assert!(body.contains("warn-clone"));
    }

    #[test]
    fn turn_body_empty_when_no_consequences() {
        let entries = vec![hook_entry("pre_check", "src/ok.rs", 0, json!([]))];
        let body = build_turn_body(&entries, 1_700_000_010);
        assert_eq!(body, "", "passing checks must not produce noise");
    }

    #[test]
    fn turn_body_empty_when_no_entries() {
        let body = build_turn_body(&[], 1_700_000_000);
        assert_eq!(body, "");
    }

    use std::io::Write;

    #[test]
    fn session_driver_returns_empty_when_rules_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let out = run_session_context(dir.path(), DEFAULT_MAX_BYTES);
        assert_eq!(out, "");
    }

    #[test]
    fn session_driver_returns_json_envelope_when_rules_present() {
        let dir = tempfile::tempdir().unwrap();
        let rules_dir = dir.path().join(".phronesis");
        std::fs::create_dir_all(&rules_dir).unwrap();
        let rules_path = rules_dir.join("rules.json");
        let mut f = std::fs::File::create(&rules_path).unwrap();
        write!(
            f,
            r#"{{"rules":[{{"id":"r1","phase":"pre","priority":10,"conditions":[],"actions":[{{"action_type":"constraint_violation","params":["Don't do X"]}}]}}]}}"#
        )
        .unwrap();

        let out = run_session_context(dir.path(), DEFAULT_MAX_BYTES);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "SessionStart");
        assert!(
            v["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap()
                .contains("Don't do X")
        );
    }

    #[test]
    fn session_driver_includes_durable_directives_when_present() {
        let dir = tempfile::tempdir().unwrap();
        let ep = dir.path().join(".phronesis");
        std::fs::create_dir_all(&ep).unwrap();
        std::fs::write(
            ep.join("durable.md"),
            "Always run tests before claiming done.\n",
        )
        .unwrap();
        // No rules file — durable.md alone should still produce a payload.
        let out = run_session_context(dir.path(), DEFAULT_MAX_BYTES);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let body = v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(body.contains("## Durable directives"));
        assert!(body.contains("Always run tests before claiming done"));
    }

    #[test]
    fn session_driver_combines_durable_and_rules() {
        let dir = tempfile::tempdir().unwrap();
        let ep = dir.path().join(".phronesis");
        std::fs::create_dir_all(&ep).unwrap();
        std::fs::write(ep.join("durable.md"), "Trace call chains end-to-end.").unwrap();
        let mut f = std::fs::File::create(ep.join("rules.json")).unwrap();
        write!(
            f,
            r#"{{"rules":[{{"id":"r1","phase":"pre","priority":10,"conditions":[],"actions":[{{"action_type":"constraint_violation","params":["Don't do X"]}}]}}]}}"#
        ).unwrap();
        let out = run_session_context(dir.path(), DEFAULT_MAX_BYTES);
        let body = serde_json::from_str::<serde_json::Value>(&out).unwrap()
            ["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .to_string();
        let durable_pos = body
            .find("Durable directives")
            .expect("durable section present");
        let rules_pos = body
            .find("Active phronesis rules")
            .expect("rules section present");
        assert!(
            durable_pos < rules_pos,
            "durable directives should come before rules in the body"
        );
    }

    #[test]
    fn turn_driver_includes_durable_directives_even_when_log_empty() {
        let dir = tempfile::tempdir().unwrap();
        let ep = dir.path().join(".phronesis");
        std::fs::create_dir_all(&ep).unwrap();
        std::fs::write(ep.join("durable.md"), "Keep responses concise.").unwrap();
        let out = run_turn_context(dir.path(), 5, DEFAULT_MAX_BYTES);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "UserPromptSubmit");
        assert!(
            v["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap()
                .contains("Keep responses concise")
        );
    }

    #[test]
    fn session_driver_returns_empty_for_malformed_rules_file() {
        let dir = tempfile::tempdir().unwrap();
        let rules_dir = dir.path().join(".phronesis");
        std::fs::create_dir_all(&rules_dir).unwrap();
        std::fs::write(rules_dir.join("rules.json"), b"{this isn't json").unwrap();
        let out = run_session_context(dir.path(), DEFAULT_MAX_BYTES);
        assert_eq!(
            out, "",
            "malformed rules file must produce no context, not panic"
        );
    }

    #[test]
    fn turn_body_orders_newest_first() {
        let mut older = hook_entry(
            "pre_check",
            "src/older.rs",
            2,
            json!([{"rule_id": "r1", "action_type": "constraint_violation", "message": "m", "bindings": {}}]),
        );
        older.ts = 1_700_000_000;
        let mut newer = hook_entry(
            "pre_check",
            "src/newer.rs",
            2,
            json!([{"rule_id": "r2", "action_type": "constraint_violation", "message": "m", "bindings": {}}]),
        );
        newer.ts = 1_700_000_100;

        // read_recent returns oldest-first; build_turn_body must reorder.
        let body = build_turn_body(&[older, newer], 1_700_000_200);
        let p_newer = body.find("src/newer.rs").unwrap();
        let p_older = body.find("src/older.rs").unwrap();
        assert!(p_newer < p_older, "newest entry must appear first");
    }

    #[test]
    fn turn_driver_returns_empty_when_log_missing() {
        let dir = tempfile::tempdir().unwrap();
        let out = run_turn_context(dir.path(), 5, DEFAULT_MAX_BYTES);
        assert_eq!(out, "");
    }

    #[test]
    fn turn_driver_returns_json_when_log_has_consequences() {
        use crate::action_log;
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join(".phronesis").join("log.jsonl");
        let entry = LogEntry::new("hook", "pre_check")
            .with("phase", "pre")
            .with("tool", "Edit")
            .with("file", "src/x.rs")
            .with("exit", 2)
            .with(
                "consequences",
                json!([{
                    "rule_id": "no-unwrap",
                    "action_type": "constraint_violation",
                    "message": "Avoid .unwrap()",
                    "bindings": {}
                }]),
            );
        action_log::append(&log_path, &entry).unwrap();

        let out = run_turn_context(dir.path(), 5, DEFAULT_MAX_BYTES);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "UserPromptSubmit");
        let ctx = v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(ctx.contains("BLOCKED"));
        assert!(ctx.contains("no-unwrap"));
    }
}
