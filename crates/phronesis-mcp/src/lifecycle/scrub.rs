//! Prompt-text scrubbing for the action log. Wraps the text in a JSON object
//! so `Scrubber::scrub_value` applies its key-based rules, and pre-applies
//! regexes for session ids and transcript paths that appear as free text.

use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;

use crate::payload_scrub::Scrubber;

/// Any UUID-shaped token, with **no `session` context required**: a bare id in
/// free text is still an id (spec §"Privacy and scrubbing"). Requiring the word
/// `session` nearby was the earlier shape and it misses `resume 0f3c9a1e-…`.
fn uuid_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b").unwrap()
    })
}
/// The phronesis sid shape, `s-YYYY-MM-DD-<hex>`, which is not a UUID and which
/// a human pastes into a prompt all the time ("what happened in s-2026-09-18-3a9f1c?").
fn phronesis_sid_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\bs-\d{4}-\d{2}-\d{2}-[0-9a-f]{1,8}\b").unwrap())
}
/// Any path ending in `.jsonl` under a `.claude`, `.codex` or `.gemini`
/// directory, accepting both separators and relative as well as absolute forms
/// (the leading component is optional, so `.codex/sessions/x.jsonl` matches).
fn transcript_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?:[^\s"']*[/\\])?\.(?:claude|codex|gemini)[/\\][^\s"']*\.jsonl"#).unwrap()
    })
}

/// Scrub free prompt text before it is written to `.phronesis/log.jsonl`.
/// Never fails: without a usable `$HOME` it falls back to project-root-only
/// scrubbing and logs once to stderr.
pub fn scrub_prompt(project_root: &Path, text: &str) -> String {
    // All three run unconditionally, before `scrub_value`, and use the same
    // placeholders `scrub_value` uses for the keyed cases
    // (`payload_scrub.rs:107-109`). They run first so `scrub_value`'s
    // project-root and `$HOME` rewriting sees text with ids already gone.
    let pre = uuid_re().replace_all(text, "sess-00000000");
    let pre = phronesis_sid_re().replace_all(&pre, "sess-00000000");
    let pre = transcript_re().replace_all(&pre, "/home/dev/.claude/transcript.jsonl");
    let root = project_root.display().to_string();
    let home = std::env::var("HOME").unwrap_or_default();
    let mut scrubber = match Scrubber::new(&home, &root) {
        Ok(s) => s,
        Err(_) => {
            eprintln!("phronesis: lifecycle scrub: HOME unusable, scrubbing project root only");
            return pre.replace(&root, "/home/dev/project");
        }
    };
    let mut v = serde_json::json!({ "prompt": pre.as_ref() });
    scrubber.scrub_value(&mut v);
    v["prompt"].as_str().unwrap_or_default().to_string()
}
