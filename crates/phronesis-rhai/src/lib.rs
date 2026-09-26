//! A [Rhai](https://rhai.rs) implementation of phronesis's
//! [`ScriptEval`](phronesis::ScriptEval) trait.
//!
//! The core `phronesis` crate ships a small, dependency-free
//! `BuiltinScriptEvaluator` supporting only `facts_contain` and
//! `facts_count`. This crate provides [`RhaiScriptEvaluator`], a full
//! scripting layer for `__script__` guard conditions: numeric comparisons,
//! boolean combinators, and array/string inspection over fact arguments.
//!
//! Wire it into a network with
//! [`ReteNetwork::with_script_evaluator`](phronesis::ReteNetwork::with_script_evaluator):
//!
//! ```
//! use phronesis::ReteNetwork;
//! use phronesis_rhai::RhaiScriptEvaluator;
//!
//! let network = ReteNetwork::with_script_evaluator(Box::new(RhaiScriptEvaluator::new()));
//! # let _ = network;
//! ```
//!
//! ## Script scope
//!
//! Each script sees exactly two variables:
//! - `facts` — an array of maps, each `#{ predicate: string, args: [string, ...] }`
//! - `bindings` — a map of RETE variable name to bound value, e.g. `bindings["?player"]`
//!
//! A script must evaluate to a `bool`. Any other return type, a syntax
//! error, or a sandbox-limit breach yields `Err`, which the network treats
//! as a *blocked* condition (a broken guard never silently passes).
//!
//! ## Sandbox
//!
//! The engine is built from [`rhai::Engine::new_raw`] with only the
//! standard package registered (arithmetic, logic, strings, arrays, maps —
//! no file, network, modules, or closures) and hard limits on operations,
//! call depth, and string size. `eval` is a Rhai keyword rather than a
//! package function, so the raw engine does not remove it on its own; every
//! engine this crate builds (guard, provider, render) disables it
//! explicitly, making any use of it a parse error. Scripts run on every rule
//! evaluation, so a malformed or hostile script must not hang the engine
//! or touch the host.
//!
//! ## Reserved predicates
//!
//! A [`RhaiFactProvider`] can emit any well-formed predicate — unless the
//! host reserves it. Hosts assert their own facts (confidence signals, rule
//! override provenance, coverage and graph relations, …) that rules trust;
//! a provider that could emit those would forge that evidence. Build the
//! provider with [`RhaiFactProvider::with_reserved`] and a
//! [`ReservedPredicates`] set: emitting a reserved name fails the whole
//! provider run, so none of that provider's facts for the event survive.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use phronesis::{BuiltinScriptEvaluator, Fact, ScriptEval};
pub use rhai;
use rhai::packages::{Package, StandardPackage};
use rhai::{Array, Dynamic, Engine, ImmutableString, Map, Scope};
use thiserror::Error;

/// Maximum Rhai operations per script evaluation.
const MAX_OPERATIONS: u64 = 100_000;
/// Maximum nested call depth.
const MAX_CALL_LEVELS: usize = 16;
/// Maximum string size (bytes) a script may construct.
const MAX_STRING_SIZE: usize = 4096;
/// Maximum facts one provider may emit during one hook invocation.
const MAX_EMITTED_FACTS: usize = 128;
const MAX_EMITTED_ARGS: usize = 32;

/// Symbols disabled in every engine this crate builds. `eval` is a built-in
/// keyword, not a package function, so [`Engine::new_raw`] keeps it;
/// disabling the symbol makes every spelling (`eval(...)`, `x.eval()`,
/// `let e = eval`) a parse error, and Rhai itself refuses `Fn("eval")`, so
/// no function pointer reaches it either. SPEC-C (render) and D7
/// (guards/providers) both require it: a string-built script would bypass
/// every static check over the source.
const DISABLED_SYMBOLS: &[&str] = &["eval"];

fn sandbox_engine() -> Engine {
    let mut engine = Engine::new_raw();
    let package = StandardPackage::new();
    package.register_into_engine(&mut engine);
    engine.set_max_operations(MAX_OPERATIONS);
    engine.set_max_call_levels(MAX_CALL_LEVELS);
    engine.set_max_string_size(MAX_STRING_SIZE);
    engine.set_max_array_size(4096);
    engine.set_max_map_size(4096);
    for symbol in DISABLED_SYMBOLS {
        engine.disable_symbol(*symbol);
    }
    engine
}

/// A [`ScriptEval`] implementation backed by a sandboxed Rhai engine.
///
/// Construct once and reuse: the engine is immutable configuration, and
/// each [`evaluate`](RhaiScriptEvaluator::evaluate) call runs in a fresh
/// scope, so evaluations don't leak state into one another.
pub struct RhaiScriptEvaluator {
    engine: Engine,
}

impl RhaiScriptEvaluator {
    /// Build a new evaluator with the standard package and sandbox limits.
    pub fn new() -> Self {
        Self {
            engine: sandbox_engine(),
        }
    }

    /// Build the Rhai `facts` array: one map per fact with `predicate` and
    /// `args` keys.
    fn facts_to_dynamic(facts: &[Fact]) -> Array {
        facts
            .iter()
            .map(|f| {
                let mut map = Map::new();
                map.insert("predicate".into(), Dynamic::from(f.predicate.clone()));
                let args: Array = f.args.iter().map(|a| Dynamic::from(a.clone())).collect();
                map.insert("args".into(), Dynamic::from(args));
                Dynamic::from(map)
            })
            .collect()
    }

    /// Build the Rhai `bindings` map from RETE variable bindings.
    fn bindings_to_dynamic(bindings: &HashMap<String, String>) -> Map {
        bindings
            .iter()
            .map(|(k, v)| (k.into(), Dynamic::from(v.clone())))
            .collect()
    }
}

/// Normalized, host-neutral tool event visible to a Rhai fact provider.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FactProviderEvent {
    pub phase: String,
    pub tool_name: String,
    pub file_path: String,
    /// `file_path` expressed relative to the project root, forward-slashed,
    /// and empty when the path lies outside the project.
    ///
    /// Hosts send absolute paths, but the code graph keys files
    /// repo-relative. Without this, a provider fact and a graph fact can
    /// never join on a path — and the rule silently never fires rather than
    /// erroring, which is the worst failure mode a rules engine has.
    pub file_rel: String,
    pub files: Vec<String>,
    pub old_content: String,
    pub new_content: String,
    pub command: String,
    pub output: String,
}

impl FactProviderEvent {
    fn to_dynamic(&self) -> Map {
        let mut event: Map = [
            ("phase", &self.phase),
            ("tool_name", &self.tool_name),
            ("file_path", &self.file_path),
            ("file_rel", &self.file_rel),
            ("old_content", &self.old_content),
            ("new_content", &self.new_content),
            ("command", &self.command),
            ("output", &self.output),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), Dynamic::from(value.clone())))
        .collect();
        let files: Array = self
            .files
            .iter()
            .map(|path| Dynamic::from(path.clone()))
            .collect();
        event.insert("files".into(), Dynamic::from(files));
        event
    }
}

/// A validated predicate fact emitted by a project Rhai provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmittedFact {
    pub predicate: String,
    pub args: Vec<String>,
}

/// Predicate names a host owns and a [`RhaiFactProvider`] may not emit.
///
/// Two forms, both explicit: an *exact* name reserves only that predicate
/// (`signal_pass` does not reserve `signal_pass_extra`), and a *prefix*
/// reserves a whole namespace the host asserts into (`journey_` reserves
/// every `journey_*`). Prefixes are for families the host owns outright;
/// everything else is reserved by exact name so project providers keep the
/// rest of the predicate space (e.g. `change_set_*`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReservedPredicates {
    exact: std::collections::BTreeSet<String>,
    prefixes: Vec<String>,
}

impl ReservedPredicates {
    /// An empty set: nothing is reserved.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reserve each of `names` exactly.
    pub fn with_exact<I, S>(mut self, names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.exact.extend(names.into_iter().map(Into::into));
        self
    }

    /// Reserve every predicate starting with one of `prefixes`.
    pub fn with_prefixes<I, S>(mut self, prefixes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.prefixes.extend(prefixes.into_iter().map(Into::into));
        self
    }

    /// Is `predicate` reserved for the host?
    pub fn is_reserved(&self, predicate: &str) -> bool {
        self.exact.contains(predicate)
            || self
                .prefixes
                .iter()
                .any(|prefix| predicate.starts_with(prefix.as_str()))
    }

    /// Reserved names emitted as string literals — `emit_fact("name", ...)` —
    /// anywhere in `script`. A best-effort static pre-check (comments are
    /// not skipped, so a commented-out forge is also refused); names built
    /// at run time are caught by the `emit_fact` check itself.
    fn literal_violations(&self, script: &str) -> Vec<String> {
        let mut found = Vec::new();
        let mut rest = script;
        while let Some(at) = rest.find("emit_fact") {
            rest = &rest[at + "emit_fact".len()..];
            let Some(after_paren) = rest.trim_start().strip_prefix('(') else {
                continue;
            };
            let Some(literal) = after_paren.trim_start().strip_prefix('"') else {
                continue;
            };
            let Some(end) = literal.find('"') else {
                continue;
            };
            let name = &literal[..end];
            if self.is_reserved(name) && !found.iter().any(|seen| seen == name) {
                found.push(name.to_string());
            }
        }
        found
    }
}

#[derive(Default)]
struct EmitterState {
    facts: Vec<EmittedFact>,
    error: Option<String>,
}

/// Sandboxed evaluator for project-defined LHS predicate providers.
///
/// Providers receive a read-only `event` map and may call
/// `emit_fact(predicate, args)`. Emitted facts are collected outside Rhai and
/// asserted by the host after the provider completes. Any rejected emit —
/// malformed, over a limit, or a [reserved](ReservedPredicates) predicate —
/// fails the whole run: the provider's other facts are dropped with it.
#[derive(Debug, Default)]
pub struct RhaiFactProvider {
    reserved: ReservedPredicates,
}

impl RhaiFactProvider {
    /// A provider evaluator that reserves nothing. Hosts that assert facts
    /// their rules trust should use [`with_reserved`](Self::with_reserved).
    pub fn new() -> Self {
        Self::default()
    }

    /// A provider evaluator that refuses to emit any of `reserved`.
    pub fn with_reserved(reserved: ReservedPredicates) -> Self {
        Self { reserved }
    }

    pub fn validate(&self, script: &str) -> Result<(), String> {
        sandbox_engine()
            .compile(script)
            .map_err(|error| format!("rhai fact provider compile error: {error}"))?;
        let violations = self.reserved.literal_violations(script);
        if violations.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "provider emits reserved host-owned predicate(s) `{}`; providers may not assert facts the host owns",
                violations.join("`, `")
            ))
        }
    }

    pub fn evaluate(
        &self,
        script: &str,
        event: &FactProviderEvent,
    ) -> Result<Vec<EmittedFact>, String> {
        let state = Arc::new(Mutex::new(EmitterState::default()));
        let emitter_state = Arc::clone(&state);
        let reserved = self.reserved.clone();
        let mut engine = sandbox_engine();
        engine.register_fn(
            "emit_fact",
            move |predicate: ImmutableString, args: Array| {
                let mut state = emitter_state.lock().unwrap_or_else(|e| e.into_inner());
                if state.error.is_some() {
                    return;
                }
                if state.facts.len() >= MAX_EMITTED_FACTS {
                    state.error = Some(format!(
                        "provider emitted more than {MAX_EMITTED_FACTS} facts"
                    ));
                    return;
                }
                let predicate = predicate.to_string();
                if !valid_predicate(&predicate) {
                    state.error = Some(format!("invalid emitted predicate `{predicate}`"));
                    return;
                }
                if reserved.is_reserved(&predicate) {
                    state.error = Some(format!(
                        "emitted reserved host-owned predicate `{predicate}`; providers may not assert facts the host owns"
                    ));
                    return;
                }
                if args.len() > MAX_EMITTED_ARGS {
                    state.error = Some(format!(
                        "emitted facts may have at most {MAX_EMITTED_ARGS} arguments"
                    ));
                    return;
                }
                let mut strings = Vec::with_capacity(args.len());
                for arg in args {
                    let Some(value) = arg.try_cast::<ImmutableString>() else {
                        state.error =
                            Some("emitted fact arguments must all be strings".to_string());
                        return;
                    };
                    if value.len() > MAX_STRING_SIZE {
                        state.error = Some(format!(
                            "emitted fact arguments may not exceed {MAX_STRING_SIZE} bytes"
                        ));
                        return;
                    }
                    strings.push(value.to_string());
                }
                state.facts.push(EmittedFact {
                    predicate,
                    args: strings,
                });
            },
        );
        let mut scope = Scope::new();
        scope.push("event", event.to_dynamic());
        let _ = engine
            .eval_with_scope::<Dynamic>(&mut scope, script)
            .map_err(|e| format!("rhai fact provider error: {e}"))?;
        let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(error) = state.error.take() {
            return Err(error);
        }
        Ok(std::mem::take(&mut state.facts))
    }
}

fn valid_predicate(predicate: &str) -> bool {
    let mut chars = predicate.chars();
    matches!(chars.next(), Some(first) if first == '_' || first.is_ascii_alphabetic())
        && predicate.len() <= 128
        && chars.all(|ch| ch == '_' || ch == '.' || ch == '-' || ch.is_ascii_alphanumeric())
}

impl Default for RhaiScriptEvaluator {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for RhaiScriptEvaluator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `rhai::Engine` is not Debug; expose the type without internals.
        f.debug_struct("RhaiScriptEvaluator")
            .finish_non_exhaustive()
    }
}

impl ScriptEval for RhaiScriptEvaluator {
    fn evaluate(
        &self,
        script: &str,
        facts: &[Fact],
        bindings: &HashMap<String, String>,
    ) -> Result<bool, String> {
        let mut scope = Scope::new();
        scope.push("facts", Self::facts_to_dynamic(facts));
        scope.push("bindings", Self::bindings_to_dynamic(bindings));

        let value = self
            .engine
            .eval_with_scope::<Dynamic>(&mut scope, script)
            .map_err(|e| format!("rhai evaluation error: {e}"))?;

        value
            .as_bool()
            .map_err(|actual| format!("script must return bool, got {actual}"))
    }
}

/// Is `script` written in the core [`BuiltinScriptEvaluator`] DSL rather
/// than Rhai?
///
/// The builtin DSL is the recognizable subset `facts_contain(...)` and
/// `facts_count(...) <op> N` (with an optional leading `!`). Neither
/// `facts_contain` nor `facts_count` is a Rhai function, and the `'*'`
/// wildcard is a Rhai char literal — so these forms must go to the builtin
/// evaluator, not Rhai. This mirrors the dispatch inside the builtin
/// evaluator itself.
fn is_builtin_dsl(script: &str) -> bool {
    let s = script.trim();
    let s = s.strip_prefix('!').map(str::trim).unwrap_or(s);
    s.starts_with("facts_contain(") || s.contains("facts_count(")
}

/// A [`ScriptEval`] that routes each script to the evaluator that can
/// handle it: builtin-DSL forms (`facts_contain`/`facts_count`) go to the
/// core [`BuiltinScriptEvaluator`], everything else to
/// [`RhaiScriptEvaluator`].
///
/// This is the evaluator an *embedding host* wants when it ships rule packs
/// written in the builtin DSL (e.g. phronesis-mcp's confidence and journey
/// gates) but also wants expressive Rhai guards for new rules — the two
/// coexist in one `.phronesis/rules.json` without a rewrite.
pub struct CompositeScriptEvaluator {
    builtin: BuiltinScriptEvaluator,
    rhai: RhaiScriptEvaluator,
}

impl CompositeScriptEvaluator {
    pub fn new() -> Self {
        Self {
            builtin: BuiltinScriptEvaluator::new(),
            rhai: RhaiScriptEvaluator::new(),
        }
    }
}

impl Default for CompositeScriptEvaluator {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for CompositeScriptEvaluator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompositeScriptEvaluator")
            .field("builtin", &self.builtin)
            .finish_non_exhaustive()
    }
}

impl ScriptEval for CompositeScriptEvaluator {
    fn evaluate(
        &self,
        script: &str,
        facts: &[Fact],
        bindings: &HashMap<String, String>,
    ) -> Result<bool, String> {
        if is_builtin_dsl(script) {
            // Call the trait method explicitly: the builtin's inherent
            // `evaluate` returns `ReteError`, but the `ScriptEval` impl maps
            // it to the `String` error this trait contract requires.
            ScriptEval::evaluate(&self.builtin, script, facts, bindings)
        } else {
            self.rhai.evaluate(script, facts, bindings)
        }
    }
}

// ---------------------------------------------------------------------------
// Dedicated render entry (SPEC-verification-artifact-generation.md): the
// artifact body is rendered in a scope containing ONLY the declared inputs —
// no emit_fact, no host functions, one typed string return.
// ---------------------------------------------------------------------------

/// The frozen render input (SPEC-C render contract): the property record as a
/// Rhai map, plus the frozen sorted dependency-fact set. Construct via
/// [`RenderInput::frozen`] — it sorts the fact set so identical inputs are
/// byte-identical across runs (the allowlist hashes rendered bytes).
#[derive(Debug, Clone)]
pub struct RenderInput {
    pub property: Map,
    pub dependency_facts: Vec<Map>,
}

impl RenderInput {
    /// Freeze: sort the dependency facts by their canonical serialization so
    /// the render is deterministic over its input set.
    pub fn frozen(property: Map, dependency_facts: Vec<Map>) -> Self {
        let mut facts = dependency_facts;
        facts.sort_by(|a, b| {
            let ka = a
                .iter()
                .map(|(k, v)| format!("{k}={v:?}"))
                .collect::<Vec<_>>()
                .join(",");
            let kb = b
                .iter()
                .map(|(k, v)| format!("{k}={v:?}"))
                .collect::<Vec<_>>()
                .join(",");
            ka.cmp(&kb)
        });
        Self {
            property,
            dependency_facts: facts,
        }
    }
}

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("render failed: {message}")]
    Eval { message: String },
    #[error("rendered body exceeds {limit} bytes — raise the cap by measurement, not by wish")]
    TooLarge { limit: usize },
    #[error("render must produce a string (got {found})")]
    NotAString { found: &'static str },
}

/// Maximum rendered body. Set by measurement against the proven 10-VC
/// harness (`crates/phronesis-mcp/verification/harness.rs`, 209 lines ≈
/// 7 KiB) — the review proposes 64 KiB + line cap; a provenance header is
/// budgeted outside this by the caller.
pub const MAX_RENDER_BYTES: usize = 64 * 1024;

/// The render engine: the guard/provider sandbox (which already disables
/// `eval` — SPEC-C: "`eval` and dynamic script evaluation are disabled")
/// plus a string budget equal to the rendered-body cap. Guards and
/// providers keep their own 4 KiB string limit — this budget is render-only.
fn render_engine() -> Engine {
    let mut engine = sandbox_engine();
    engine.set_max_string_size(MAX_RENDER_BYTES);
    engine
}

/// Map an engine failure to a [`RenderError`]. A string that outgrows the
/// engine's budget (which equals [`MAX_RENDER_BYTES`]) is the body-cap
/// breach, so it surfaces as `TooLarge` rather than a generic eval error.
fn render_eval_error(error: &rhai::EvalAltResult) -> RenderError {
    match error.unwrap_inner() {
        rhai::EvalAltResult::ErrorDataTooLarge(what, _) if what.contains("string") => {
            RenderError::TooLarge {
                limit: MAX_RENDER_BYTES,
            }
        }
        _ => RenderError::Eval {
            message: error.to_string(),
        },
    }
}

/// Render an artifact body from a template through a render-frozen engine.
///
/// Scope contract: the ONLY values in scope are `property` (the frozen map)
/// and `facts` (the frozen sorted array). No host functions are registered
/// beyond the sandbox package — no `emit_fact`, no file I/O (the raw engine
/// already denies it), no modules (`no_module`), and `eval` is disabled.
/// The operation, call-depth, array and map limits are the guard/provider
/// sandbox limits; only the string budget is raised, to [`MAX_RENDER_BYTES`].
pub fn render(template: &str, input: &RenderInput) -> Result<String, RenderError> {
    let engine = render_engine();
    let mut scope = Scope::new();
    scope.push_constant("property", Dynamic::from(input.property.clone()));
    let facts: Array = input
        .dependency_facts
        .iter()
        .map(|m| Dynamic::from(m.clone()))
        .collect();
    scope.push_constant("facts", Dynamic::from(facts));

    let out: Dynamic = engine
        .eval_with_scope(&mut scope, template)
        .map_err(|e| render_eval_error(&e))?;
    if !out.is_string() {
        // A template that produces nothing is a template bug — loud, not silent.
        return Err(RenderError::NotAString {
            found: "non-string",
        });
    }
    let s = out.into_string().map_err(|e| RenderError::Eval {
        message: e.to_string(),
    })?;
    if s.len() > MAX_RENDER_BYTES {
        return Err(RenderError::TooLarge {
            limit: MAX_RENDER_BYTES,
        });
    }
    Ok(s)
}

/// Probe templates for the scope-freeze check. Each one evaluates to a
/// STRING if its capability is present, so a rejection can only mean the
/// capability is absent — not that the template returned the wrong type.
const FORBIDDEN_RENDER_PROBES: &[&str] = &[
    // Fact emission (the provider-only host function).
    "emit_fact(\"x\", []); `emitted`",
    // File I/O.
    "open_file(\"/tmp/pwned\"); `opened`",
    // Dynamic script evaluation, direct and aliased.
    "eval(\"`x` + `y`\")",
    "let code = \"`x`\"; eval(code)",
    "Fn(\"eval\").call(\"`x`\")",
    "call(Fn(\"eval\"), \"`x`\")",
    "\"`x`\".eval()",
    // Modules.
    "import \"std\" as s; `imported`",
];

/// The scope-freeze check, callable: runs every forbidden-capability probe
/// through [`render`] and holds only if each one is rejected. Fact
/// emission, file I/O, module import, and `eval` (direct or through a
/// function pointer) must fail loudly in render scope instead of
/// half-working; any future host registration or engine change that makes a
/// probe render flips this to `false`.
pub fn render_scope_freeze_holds() -> bool {
    let input = RenderInput::frozen(Map::new(), Vec::new());
    FORBIDDEN_RENDER_PROBES
        .iter()
        .all(|probe| matches!(render(probe, &input), Err(RenderError::Eval { .. })))
}
