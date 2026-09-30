use std::path::PathBuf;

use serde_json::{Value, json};
use thiserror::Error;

use super::rules_core::*;
use super::rules_other::*;
use super::rules_python::*;
use super::rules_rust::*;

/// A starter rule pack. Packs are composable — caller picks a comma-separated
/// list and `compose_packs` merges them, deduping by rule_id.
///
/// `Llm` is independent of language: it carries the "stop deflecting" rules
/// that catch phrases like "pre-existing issue" wherever they appear. The
/// language packs (`Rust`, `Python`, `TypeScript`) carry only language-specific
/// enforcement; they don't bundle the LLM-behavior rules.
///
/// `None` exists for users who want hooks wired up but will author their own
/// rules from scratch.
///
/// `#[non_exhaustive]` because adding a pack is this crate's most routine
/// extension point — four have landed already (`Confidence`, `Journey`,
/// `Structural`, `Context`) and more languages are expected. Without it every
/// new pack is a semver break twice over: a variant added to an exhaustive
/// enum, and a discriminant shift for every variant after the insertion point.
/// Downstream matches need a `_` arm; in exchange, adding a pack stops being
/// an API event. Taken pre-1.0, when the one-time cost is lowest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Pack {
    Llm,
    Rust,
    Rhai,
    Python,
    /// Opinionated, opt-in rules derived from <https://python-patterns.guide/>.
    PythonPatterns,
    TypeScript,
    Swift,
    Confidence,
    Journey,
    Context,
    Structural,
    Lua,
    Cue,
    Json,
    Yaml,
    Helm3,
    Java,
    None,
}

impl Pack {
    /// Every pack, in the order documentation should present them.
    ///
    /// Consumers that must cover all packs (the catalogue generator) iterate
    /// this rather than keeping their own list, so a new pack cannot ship
    /// undocumented. `label()` below matches exhaustively, so adding a variant
    /// breaks the build — at which point this list is the next thing to fix.
    pub const ALL: &'static [Pack] = &[
        Self::Llm,
        Self::Rust,
        Self::Rhai,
        Self::Python,
        Self::PythonPatterns,
        Self::TypeScript,
        Self::Swift,
        Self::Confidence,
        Self::Journey,
        Self::Context,
        Self::Structural,
        Self::Lua,
        Self::Cue,
        Self::Json,
        Self::Yaml,
        Self::Helm3,
        Self::Java,
        Self::None,
    ];

    pub(super) fn parse(s: &str) -> Result<Self, InitError> {
        match s.trim().to_lowercase().as_str() {
            // `llm` and the deprecated alias `minimal` (pre-pack-split naming)
            "llm" | "minimal" => Ok(Self::Llm),
            "rust" | "rs" => Ok(Self::Rust),
            "rhai" => Ok(Self::Rhai),
            "python" | "py" => Ok(Self::Python),
            "python-patterns" | "py-patterns" => Ok(Self::PythonPatterns),
            "typescript" | "ts" | "javascript" | "js" => Ok(Self::TypeScript),
            "swift" => Ok(Self::Swift),
            "lua" => Ok(Self::Lua),
            "cue" => Ok(Self::Cue),
            "json" => Ok(Self::Json),
            "yaml" | "yml" => Ok(Self::Yaml),
            "helm3" | "helm" => Ok(Self::Helm3),
            "java" => Ok(Self::Java),
            "confidence" => Ok(Self::Confidence),
            "journey" => Ok(Self::Journey),
            "context" => Ok(Self::Context),
            "structural" | "graph" => Ok(Self::Structural),
            "none" => Ok(Self::None),
            other => Err(InitError::UnknownPack(other.to_string())),
        }
    }

    pub(crate) fn rules(self) -> Value {
        match self {
            Self::None => json!({"rules": []}),
            Self::Llm => deflection_rules(),
            Self::Rust => rust_rules(),
            Self::Rhai => rhai_rules(),
            Self::Python => python_rules(),
            Self::PythonPatterns => python_patterns_rules(),
            Self::TypeScript => typescript_rules(),
            Self::Swift => swift_rules(),
            Self::Confidence => confidence_rules(),
            Self::Structural => structural_rules(),
            Self::Context => json!({"rules": []}),
            // Journey ships no starter rules in v1 — the project defines its
            // own risk surface via `journey.json` and adds journey_* rules to
            // rules.json. The pack's contribution is the journey.json starter
            // config + gitignore carveout, written by `write_journey_scaffold`.
            Self::Journey => json!({"rules": []}),
            Self::Lua => lua_rules(),
            Self::Cue => cue_rules(),
            Self::Json => json_rules(),
            Self::Yaml => yaml_rules(),
            Self::Helm3 => helm3_rules(),
            Self::Java => java_rules(),
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Llm => "llm",
            Self::Rust => "rust",
            Self::Rhai => "rhai",
            Self::Python => "python",
            Self::PythonPatterns => "python-patterns",
            Self::TypeScript => "typescript",
            Self::Swift => "swift",
            Self::Lua => "lua",
            Self::Cue => "cue",
            Self::Json => "json",
            Self::Yaml => "yaml",
            Self::Helm3 => "helm3",
            Self::Java => "java",
            Self::Confidence => "confidence",
            Self::Journey => "journey",
            Self::Context => "context",
            Self::Structural => "structural",
            Self::None => "none",
        }
    }
}

/// What `base` expands to: every language-agnostic pack.
///
/// `context` is included on measured evidence, not on principle. On a
/// `base,rust` fixture carrying a 3,292-byte durable file and five blocked
/// edits, opting in cut the per-interaction payload from 4,038 to 1,556 bytes
/// (61.5%, against the specification's 60% target), kept every blocking item,
/// stopped the session charter truncating mid-rule-list, and recorded zero
/// raw truncations at a p95 construction latency of 3.4 ms.
///
/// Language packs are excluded on purpose. Several of their rules match raw
/// substrings gated only by path — the TypeScript `: any` rule fires on Rust's
/// `: anyhow::Error`, for instance — so composing every language at once is a
/// false-positive source rather than a convenience. Name the language you
/// want (for example, `rust`); the base is included automatically.
pub const BASE_PACKS: &[Pack] = &[
    Pack::Llm,
    Pack::Confidence,
    Pack::Journey,
    Pack::Structural,
    Pack::Context,
];

/// Parse a comma-separated pack list (e.g. `"rust"`). Whitespace is
/// tolerated. Every non-`none` selection includes the complete base, and
/// duplicates are deduped. `none` is the sole escape hatch.
pub fn parse_packs(s: &str) -> Result<Vec<Pack>, InitError> {
    if s.trim().is_empty() {
        return Ok(BASE_PACKS.to_vec());
    }
    let requested: Vec<_> = s.split(',').map(str::trim).collect();
    if requested.len() == 1 && requested[0].eq_ignore_ascii_case("none") {
        return Ok(vec![Pack::None]);
    }
    if requested
        .iter()
        .any(|part| part.eq_ignore_ascii_case("none"))
    {
        return Err(InitError::InvalidPackSelection(
            "`none` cannot be combined with other packs".to_string(),
        ));
    }
    let mut seen = std::collections::HashSet::new();
    let mut out = BASE_PACKS.to_vec();
    seen.extend(BASE_PACKS.iter().copied());
    let mut push = |pack: Pack, out: &mut Vec<Pack>| {
        if seen.insert(pack) {
            out.push(pack);
        }
    };
    for part in requested {
        // `base` is an expansion, not a pack: every downstream step
        // (scaffolding, gitignore carveouts, the catalogue) keys off the
        // concrete packs, so it must never see a composite variant.
        if part.trim().eq_ignore_ascii_case("base") {
            continue;
        }
        push(Pack::parse(part)?, &mut out);
    }
    Ok(out)
}

/// Compose multiple packs into a single rules.json value, deduping rules by ID.
/// Earlier packs take precedence on ID collision (first-write-wins).
pub fn compose_packs(packs: &[Pack]) -> Value {
    let pack_values: Vec<Value> = packs.iter().map(|p| p.rules()).collect();
    let mut by_id = std::collections::HashMap::<String, Value>::new();
    let mut order: Vec<String> = Vec::new();
    for pack in &pack_values {
        if let Some(rules) = pack["rules"].as_array() {
            for rule in rules {
                if let Some(id) = rule["id"].as_str() {
                    let key = id.to_string();
                    if let std::collections::hash_map::Entry::Vacant(e) = by_id.entry(key.clone()) {
                        order.push(key);
                        e.insert(rule.clone());
                    }
                }
            }
        }
    }
    let merged: Vec<Value> = order
        .into_iter()
        .filter_map(|id| by_id.remove(&id))
        .collect();
    json!({"rules": merged})
}

#[derive(Debug, Error)]
pub enum InitError {
    #[error(
        "unknown pack `{0}`; valid: base, llm, rust, rhai, python, python-patterns, typescript, swift, lua, cue, json, yaml, helm3, confidence, journey, context, structural, none"
    )]
    UnknownPack(String),
    #[error("invalid pack selection: {0}")]
    InvalidPackSelection(String),
    #[error("project root does not exist: {0}")]
    NoSuchPath(String),
    #[error("project root is not a directory: {0}")]
    NotADirectory(String),
    #[error("io error on {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid rules file: {0}")]
    InvalidRules(String),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

pub struct InitOpts {
    pub project_root: PathBuf,
    pub packs: Vec<Pack>,
    pub force: bool,
    pub dry_run: bool,
    /// When true, only touch rules and their starter baseline — leave the hook config,
    /// MCP registration, and .gitignore alone. Syncs starter rules while preserving
    /// local edits; `--force` replaces the rules pack.
    pub rules_only: bool,
    /// Only touch hook config (`.claude/settings.local.json`, `.mcp.json`,
    /// `.gemini/settings.json`). Skip rules.json and .gitignore. Use to
    /// refresh hook wiring on an existing project without disturbing its
    /// rules pack.
    pub hooks_only: bool,
}

#[derive(Debug, Default)]
pub struct InitReport {
    pub steps: Vec<String>,
    pub warnings: Vec<String>,
}
