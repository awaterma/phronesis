//! Taggers — project-defined rules that attach domain tags to journal records.
//!
//! See SPEC §"The project-defined seam". A tagger is structurally a rule
//! whose `when` is evaluated against the same point-in-time facts a normal
//! hook rule sees, and whose effect is "attach tag T" instead of "block/warn."
//! Zero new matching code: build a throwaway `ReteNetwork`, load the taggers
//! as rules whose action is the sentinel `tag` verb, fire, collect the
//! consequences whose `action_type == "tag"` and return their `message` as
//! the tag name.
//!
//! Module resolution is a separate concern: `resolve_module` walks
//! `cfg.modules` doing first-match-wins glob lookup over a minimal
//! hand-rolled glob (`**` matches anything including `/`, `*` matches
//! non-`/`, everything else literal). The journey hook calls it once per
//! tool-call record to stamp `module` in the journal.
//!
//! Perf: building a throwaway network per call is the dominant cost. To
//! keep within the SPEC's 2 ms p95 / 20 taggers / 100 facts budget across
//! repeated calls in the same process (the perf-smoke pattern and what
//! happens inside long-running CLI/MCP commands), the compiled
//! `Vec<Rule>` is cached behind a `OnceLock` per `TaggerConfig`. The hook
//! is per-process so the cache is single-use there; the perf smoke and
//! any in-process re-fires benefit.

use std::collections::HashSet;
use std::sync::OnceLock;

use phr::{Fact, ReteNetwork, Rule};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::rules_file::{self, SourceRule};

/// The sentinel action verb a tagger rule emits when its `when` matches.
/// Listed here so it survives a future rename — anything that grep'd
/// `"tag"` would land in the wrong places.
const TAG_ACTION: &str = "tag";

/// On-disk shape of `.phronesis/journey.json`. Owns the project's
/// risk-surface vocabulary (taggers) and the named-entity surface
/// (modules). Both are project-defined; the engine itself stays
/// vocabulary-neutral.
#[derive(Debug, Serialize, Deserialize)]
pub struct TaggerConfig {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub taggers: Vec<TaggerEntry>,
    #[serde(default)]
    pub modules: Vec<ModuleEntry>,
    /// Compiled rules, lazily populated by `fire`. `OnceLock` so the
    /// first `fire` call seeds it and the rest hit cache. Skipped on
    /// serialize/deserialize — it's pure runtime state.
    #[serde(skip)]
    compiled: OnceLock<Vec<Rule>>,
}

impl Default for TaggerConfig {
    /// The fail-open shape: schema version 1, no taggers, no modules.
    /// `load_config` returns this when `.phronesis/journey.json` is
    /// missing or malformed and the hook reaches for a default — see
    /// `journey::load_config` and SPEC §"Fail-open."
    fn default() -> Self {
        Self {
            version: 1,
            taggers: Vec::new(),
            modules: Vec::new(),
            compiled: OnceLock::new(),
        }
    }
}

impl Clone for TaggerConfig {
    fn clone(&self) -> Self {
        // The compiled cache is intentionally NOT cloned — a clone is a
        // fresh logical config and may evolve independently. Recompile on
        // first use.
        Self {
            version: self.version,
            taggers: self.taggers.clone(),
            modules: self.modules.clone(),
            compiled: OnceLock::new(),
        }
    }
}

fn default_version() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaggerEntry {
    pub tag: String,
    pub when: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleEntry {
    pub name: String,
    pub paths: Vec<String>,
}

/// What `fire` returns: the set of tags that matched, deterministically
/// sorted. `module` is always `None` from `fire` — module resolution is a
/// separate, path-based step (`resolve_module`). The caller composes them
/// when building the journal record.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TagResult {
    pub tags: Vec<String>,
    pub module: Option<String>,
}

#[derive(Debug, Error)]
pub enum TaggerError {
    #[error("malformed tagger config: {0}")]
    Config(String),
    #[error("engine error: {0}")]
    Engine(String),
}

/// Compile every tagger entry into one or more flat engine `Rule`s. OR
/// clauses in a tagger's `when` are expanded via the same `unfold_or`
/// path the hook uses for ordinary rules — DNF, deterministic id suffix
/// (`tagger:<tag>#or0`, etc.). The sentinel `tag` action survives the
/// expansion: every expanded rule carries it, so any branch that fires
/// emits a `tag`-typed consequence whose message is the tag name.
fn compile_taggers(cfg: &TaggerConfig) -> Result<Vec<Rule>, TaggerError> {
    let mut rules: Vec<Rule> = Vec::with_capacity(cfg.taggers.len());
    for entry in &cfg.taggers {
        let src = SourceRule::synthetic_tagger(&entry.tag, &entry.when)
            .map_err(|e| TaggerError::Config(e.to_string()))?;
        let disk_rules =
            rules_file::unfold_or(&src).map_err(|e| TaggerError::Config(e.to_string()))?;
        for dr in &disk_rules {
            let (rule, _phase) = rules_file::rule_from_disk(dr);
            rules.push(rule);
        }
    }
    Ok(rules)
}

/// Fire every tagger against `facts`. Returns the set of tags whose
/// `when` matched, sorted lexicographically for determinism. `module` is
/// always `None` here — resolve it separately via `resolve_module`.
///
/// Failures: config parse errors surface as `TaggerError::Config`;
/// engine errors (rule load, fact assertion, agenda update, firing) as
/// `TaggerError::Engine`. The hook treats both as soft failures and
/// records no tags rather than blocking the edit (see SPEC §"Where it
/// plugs into the hook" — journey paths fail open).
pub async fn fire(cfg: &TaggerConfig, facts: &[Fact]) -> Result<TagResult, TaggerError> {
    // Compile-once, fire-many: every fire call in the same process for
    // this config hits the OnceLock cache. The hook is per-process so
    // its single call seeds and tears down; long-lived callers (the
    // perf smoke, future in-conversation tools) benefit measurably.
    let rules = match cfg.compiled.get() {
        Some(r) => r,
        None => {
            let compiled = compile_taggers(cfg)?;
            // Race: if two threads compile in parallel, both produce the
            // same Vec<Rule>; `set` returns Err for the loser and we use
            // whichever made it in. Either way `get` is now Some.
            let _ = cfg.compiled.set(compiled);
            cfg.compiled.get().expect("compiled set above")
        }
    };

    let network = ReteNetwork::new();
    for rule in rules {
        network
            .add_rule(rule.clone())
            .await
            .map_err(|e| TaggerError::Engine(e.to_string()))?;
    }
    for f in facts {
        network
            .assert_fact(f.clone())
            .await
            .map_err(|e| TaggerError::Engine(e.to_string()))?;
    }
    network
        .update_agenda()
        .await
        .map_err(|e| TaggerError::Engine(e.to_string()))?;
    let consequences = network
        .fire_all_consequences()
        .map_err(|e| TaggerError::Engine(e.to_string()))?;

    // Pull tag names off the consequence payloads. Same shape
    // `LoggedConsequence::from_consequence` uses for stderr messages —
    // the payload always carries `action_type` and `message` for
    // RuleFiring consequences. Anything else (unlikely here) is
    // ignored.
    let mut fired: HashSet<String> = HashSet::new();
    for c in consequences {
        let action_type = c
            .payload
            .get("action_type")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if action_type == TAG_ACTION
            && let Some(msg) = c.payload.get("message").and_then(|v| v.as_str())
        {
            fired.insert(msg.to_string());
        }
    }
    let mut tags: Vec<String> = fired.into_iter().collect();
    tags.sort();
    Ok(TagResult { tags, module: None })
}

/// Resolve a file path to a configured module name, or `None` if no
/// glob matches. First-match-wins in `cfg.modules` order — config
/// authors who care about precedence put more-specific entries first.
pub fn resolve_module(cfg: &TaggerConfig, path: &str) -> Option<String> {
    for m in &cfg.modules {
        for g in &m.paths {
            if glob_match(g, path) {
                return Some(m.name.clone());
            }
        }
    }
    None
}

/// Minimal glob matcher: `**` matches any byte run including `/`; `*`
/// matches any byte run NOT containing `/`; anything else is literal.
/// No character classes, no `?`, no escaping — projects with stranger
/// path shapes can layer their own predicate. The journey spec calls out
/// only `src/auth/**`-style globs; this covers it.
fn glob_match(pattern: &str, path: &str) -> bool {
    glob_match_bytes(pattern.as_bytes(), path.as_bytes())
}

fn glob_match_bytes(pattern: &[u8], path: &[u8]) -> bool {
    let mut p = 0usize;
    let mut s = 0usize;
    while p < pattern.len() {
        if pattern[p] == b'*' {
            // Distinguish `**` (matches anything incl. `/`) from `*`
            // (matches non-`/`).
            if p + 1 < pattern.len() && pattern[p + 1] == b'*' {
                let rest = &pattern[p + 2..];
                if rest.is_empty() {
                    return true;
                }
                for i in s..=path.len() {
                    if glob_match_bytes(rest, &path[i..]) {
                        return true;
                    }
                }
                return false;
            }
            let rest = &pattern[p + 1..];
            for i in s..=path.len() {
                // `*` is anchored to non-`/`: if we've consumed a `/`
                // already in the candidate slice, stop.
                if i > s && path[i - 1] == b'/' {
                    break;
                }
                if glob_match_bytes(rest, &path[i..]) {
                    return true;
                }
            }
            return false;
        }
        if s >= path.len() || pattern[p] != path[s] {
            return false;
        }
        p += 1;
        s += 1;
    }
    s == path.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_double_star_matches_any() {
        assert!(glob_match("src/auth/**", "src/auth/login.rs"));
        assert!(glob_match("src/auth/**", "src/auth/sub/dir/file.rs"));
        assert!(!glob_match("src/auth/**", "src/payments/x.rs"));
    }

    #[test]
    fn glob_single_star_does_not_cross_slash() {
        assert!(glob_match("src/*/login.rs", "src/auth/login.rs"));
        assert!(!glob_match("src/*/login.rs", "src/auth/sub/login.rs"));
    }

    #[test]
    fn glob_literal_full_match_required() {
        assert!(glob_match("a/b/c", "a/b/c"));
        assert!(!glob_match("a/b/c", "a/b/c/d"));
        assert!(!glob_match("a/b/c", "a/b/"));
    }
}
