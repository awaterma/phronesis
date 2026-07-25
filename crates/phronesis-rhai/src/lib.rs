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
//! no file, network, `eval`, modules, or closures) and hard limits on
//! operations, call depth, and string size. Scripts run on every rule
//! evaluation, so a malformed or hostile script must not hang the engine
//! or touch the host.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use phronesis::{BuiltinScriptEvaluator, Fact, ScriptEval};
use rhai::packages::{Package, StandardPackage};
use rhai::{Array, Dynamic, Engine, ImmutableString, Map, Scope};

/// Maximum Rhai operations per script evaluation.
const MAX_OPERATIONS: u64 = 100_000;
/// Maximum nested call depth.
const MAX_CALL_LEVELS: usize = 16;
/// Maximum string size (bytes) a script may construct.
const MAX_STRING_SIZE: usize = 4096;
/// Maximum facts one provider may emit during one hook invocation.
const MAX_EMITTED_FACTS: usize = 128;
const MAX_EMITTED_ARGS: usize = 32;

fn sandbox_engine() -> Engine {
    let mut engine = Engine::new_raw();
    let package = StandardPackage::new();
    package.register_into_engine(&mut engine);
    engine.set_max_operations(MAX_OPERATIONS);
    engine.set_max_call_levels(MAX_CALL_LEVELS);
    engine.set_max_string_size(MAX_STRING_SIZE);
    engine.set_max_array_size(4096);
    engine.set_max_map_size(4096);
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

#[derive(Default)]
struct EmitterState {
    facts: Vec<EmittedFact>,
    error: Option<String>,
}

/// Sandboxed evaluator for project-defined LHS predicate providers.
///
/// Providers receive a read-only `event` map and may call
/// `emit_fact(predicate, args)`. Emitted facts are collected outside Rhai and
/// asserted by the host after the provider completes.
#[derive(Debug, Default)]
pub struct RhaiFactProvider;

impl RhaiFactProvider {
    pub fn new() -> Self {
        Self
    }

    pub fn validate(&self, script: &str) -> Result<(), String> {
        sandbox_engine()
            .compile(script)
            .map(|_| ())
            .map_err(|error| format!("rhai fact provider compile error: {error}"))
    }

    pub fn evaluate(
        &self,
        script: &str,
        event: &FactProviderEvent,
    ) -> Result<Vec<EmittedFact>, String> {
        let state = Arc::new(Mutex::new(EmitterState::default()));
        let emitter_state = Arc::clone(&state);
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
