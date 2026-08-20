//! Composition of push and pull: rule firings that invoke tools.
//!
//! Pure push: a rule fires and produces an [`Action`] that the host
//! executes (side effect) or narrates (via [`rule_firing_to_consequences`]).
//!
//! Pure pull: an actor asks a [`Lookup`] for data.
//!
//! This module is the seam between them. A rule's action names a
//! tool that's been registered in a [`LookupRegistry`]; when the
//! action reaches [`invoke_rule_driven_lookups`], it's routed to the
//! tool's `DynLookup::invoke_dyn`, and the result becomes a
//! [`Consequence`] with [`Provenance::RuleDrivenLookup`] — provenance
//! that records both the rule that triggered and the tool that
//! answered.
//!
//! # Why this is different from pure push or pure pull
//!
//! An actor handed a `RuleDrivenLookup` consequence can trace
//! "why do you believe X?" through two layers:
//!
//! - The rule_id + bound_facts — the push-mode trigger
//! - The tool + schema_version — the pull-mode resolution
//!
//! That's load-bearing for evaluation: a judge can verify the narrator
//! against both the rule's ground truth and the tool's deterministic
//! output.
//!
//! # Convention
//!
//! A rule that wants to invoke a tool uses the tool's registered name
//! as the `action_type`:
//!
//! ```text
//! Rule:
//!   condition: opponent_appeared(?e)
//!   action:
//!     action_type: "lookup_opponent"   ← tool name
//!     params: ["?e"]                   ← becomes the request
//! ```
//!
//! When `invoke_rule_driven_lookups` sees `action_type = "lookup_opponent"`
//! and the registry has a tool by that name, it invokes. Otherwise, the
//! action passes through unchanged for the host's usual handling.
//!
//! Params become a `serde_json::Value::Array` of strings. Tools that
//! want structured input parse it themselves via their `DynLookup`
//! impl.
//!
//! [`Action`]: crate::engine_types::Action
//! [`Lookup`]: crate::pull::Lookup
//! [`rule_firing_to_consequences`]: crate::push::rule_firing_to_consequences
//! [`Provenance::RuleDrivenLookup`]: crate::consequence::Provenance::RuleDrivenLookup

use std::collections::HashMap;

use crate::consequence::{Consequence, ConsequenceKind, Provenance};
use crate::engine_types::Action;
use crate::ids::RuleId;
use crate::pull::DynLookup;

/// A tool invocation that returned `Err` while executing inside
/// [`try_invoke_rule_driven_lookups`]. Carries enough context for a
/// host to log structured diagnostics or surface the failure to an
/// operator without re-running the loop.
#[derive(Debug)]
pub struct ToolInvocationError {
    /// The rule whose firing produced the failed action.
    pub rule_id: RuleId,
    /// The registered tool name (== `action.action_type` at the time of dispatch).
    pub tool: String,
    /// Schema version reported by the tool at dispatch.
    pub schema_version: u8,
    /// The action that was being dispatched. Returned so the host may
    /// re-route it through a fallback path if it chooses not to abort.
    pub action: Action,
    /// Wall-clock time spent inside `invoke_dyn` before the error.
    pub latency_ms: u64,
    /// The underlying error.
    pub source: anyhow::Error,
}

impl std::fmt::Display for ToolInvocationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "tool '{}' (schema v{}) failed for rule '{}' after {}ms: {}",
            self.tool, self.schema_version, self.rule_id, self.latency_ms, self.source
        )
    }
}

impl std::error::Error for ToolInvocationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

/// A registry of pull-mode tools, keyed by the stable string name the
/// tool reports via `DynLookup::name()`. Hosts build this once at
/// startup, then hand it to [`invoke_rule_driven_lookups`].
#[derive(Default)]
pub struct LookupRegistry {
    tools: HashMap<String, Box<dyn DynLookup + Send + Sync>>,
}

impl std::fmt::Debug for LookupRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LookupRegistry")
            .field("tools", &self.tools.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl LookupRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a tool. Later re-registration under the same name
    /// replaces the previous tool — callers can use this to swap in
    /// mocks for tests.
    pub fn register<L>(&mut self, tool: L)
    where
        L: DynLookup + Send + Sync + 'static,
    {
        let name = tool.name().to_string();
        self.tools.insert(name, Box::new(tool));
    }

    /// Look up a registered tool by name.
    pub fn get(&self, name: &str) -> Option<&(dyn DynLookup + Send + Sync)> {
        self.tools.get(name).map(|b| &**b)
    }

    /// Check whether a tool is registered under this name.
    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Names of all registered tools, for debugging and introspection.
    pub fn tool_names(&self) -> Vec<&str> {
        self.tools.keys().map(String::as_str).collect()
    }
}

/// Translate an action's stateal string params into the JSON-array
/// request shape `DynLookup::invoke_dyn` expects. Tools that want
/// structured input decode it themselves.
fn action_to_request(action: &Action) -> serde_json::Value {
    serde_json::Value::Array(
        action
            .params
            .iter()
            .map(|s| serde_json::Value::String(s.clone()))
            .collect(),
    )
}

struct LookupConsequenceContext<'a> {
    rule_id: &'a str,
    bound_facts: &'a [String],
    fact_sources: &'a std::collections::BTreeMap<String, String>,
}

fn make_consequence(
    context: &LookupConsequenceContext<'_>,
    action: &Action,
    tool: &(dyn DynLookup + Send + Sync),
    payload: serde_json::Value,
) -> Consequence {
    Consequence {
        kind: ConsequenceKind::Snapshot,
        predicate: action.action_type.clone(),
        payload,
        provenance: Provenance::RuleDrivenLookup {
            rule_id: context.rule_id.into(),
            bound_facts: context.bound_facts.to_vec(),
            bindings: Default::default(),
            fact_sources: context.fact_sources.clone(),
            decisions: Default::default(),
            tool: tool.name().to_string(),
            schema_version: tool.schema_version(),
        },
    }
}

/// Route rule-firing actions through registered pull tools, falling
/// back to pass-through on tool-invocation errors.
///
/// Returns `(Vec<Consequence>, Vec<Action>)`:
///
/// - `Consequence`s: one per successfully-invoked tool action, carrying
///   [`Provenance::RuleDrivenLookup`] with the full chain of provenance
///   (rule → tool).
/// - `Action`s: the actions that *weren't* routed to tools — either
///   because no tool was registered under that name, or because the
///   tool's invocation returned an error. The caller handles these via
///   their usual Action pipeline (typically `rule_firing_to_consequences`
///   or direct execution).
///
/// Note: a tool that returns `available: false` inside its payload is
/// still a *successful* invocation from this function's point of view —
/// "I looked, the tool isn't wired yet" is a real consequence, consistent
/// with the pull-mode semantics.
///
/// Use [`try_invoke_rule_driven_lookups`] when invocation errors must
/// be surfaced rather than silently absorbed (e.g. opt-in testing
/// modes where masking is exactly what the operator is trying to avoid).
pub fn invoke_rule_driven_lookups(
    rule_id: &str,
    bound_facts: &[String],
    actions: Vec<Action>,
    registry: &LookupRegistry,
) -> (Vec<Consequence>, Vec<Action>) {
    invoke_rule_driven_lookups_with_sources(
        rule_id,
        bound_facts,
        &Default::default(),
        actions,
        registry,
    )
}

/// Lenient rule-driven lookup routing with bound-fact source provenance.
pub fn invoke_rule_driven_lookups_with_sources(
    rule_id: &str,
    bound_facts: &[String],
    fact_sources: &std::collections::BTreeMap<String, String>,
    actions: Vec<Action>,
    registry: &LookupRegistry,
) -> (Vec<Consequence>, Vec<Action>) {
    let context = LookupConsequenceContext {
        rule_id,
        bound_facts,
        fact_sources,
    };
    let mut consequences = Vec::new();
    let mut remaining = Vec::new();

    for action in actions {
        let Some(tool) = registry.get(&action.action_type) else {
            remaining.push(action);
            continue;
        };

        match tool.invoke_dyn(action_to_request(&action)) {
            Ok(payload) => {
                consequences.push(make_consequence(&context, &action, tool, payload));
            }
            Err(_) => remaining.push(action),
        }
    }

    (consequences, remaining)
}

/// Strict variant of [`invoke_rule_driven_lookups`]: tool-invocation
/// errors abort the loop and are returned as [`ToolInvocationError`].
///
/// Use this when the host has opted into a mode where silent fallback
/// would mask the very signal the operator is observing — for example,
/// a composed-narration test path where the whole point of turning
/// the toggle on is to *see* tool failures, not bury them.
///
/// On `Err`:
/// - Actions processed before the failure produce their `Consequence`s
///   but those are discarded along with the in-flight loop state. If
///   you need partial results, drive the loop yourself or call the
///   lenient variant.
/// - The remaining (post-failure) actions are not processed.
///
/// Pass-through for unregistered `action_type`s is unchanged from the
/// lenient variant: only tool *invocation* errors abort.
#[allow(clippy::result_large_err)] // Public error preserves the failed Action for fallback routing.
pub fn try_invoke_rule_driven_lookups(
    rule_id: &str,
    bound_facts: &[String],
    actions: Vec<Action>,
    registry: &LookupRegistry,
) -> Result<(Vec<Consequence>, Vec<Action>), ToolInvocationError> {
    try_invoke_rule_driven_lookups_with_sources(
        rule_id,
        bound_facts,
        &Default::default(),
        actions,
        registry,
    )
}

/// Strict rule-driven lookup routing with bound-fact source provenance.
#[allow(clippy::result_large_err)] // See the compatibility rationale on the wrapper above.
pub fn try_invoke_rule_driven_lookups_with_sources(
    rule_id: &str,
    bound_facts: &[String],
    fact_sources: &std::collections::BTreeMap<String, String>,
    actions: Vec<Action>,
    registry: &LookupRegistry,
) -> Result<(Vec<Consequence>, Vec<Action>), ToolInvocationError> {
    let context = LookupConsequenceContext {
        rule_id,
        bound_facts,
        fact_sources,
    };
    let mut consequences = Vec::new();
    let mut remaining = Vec::new();

    for action in actions {
        let Some(tool) = registry.get(&action.action_type) else {
            remaining.push(action);
            continue;
        };

        let start = std::time::Instant::now();
        match tool.invoke_dyn(action_to_request(&action)) {
            Ok(payload) => {
                consequences.push(make_consequence(&context, &action, tool, payload));
            }
            Err(source) => {
                return Err(ToolInvocationError {
                    rule_id: rule_id.into(),
                    tool: tool.name().to_string(),
                    schema_version: tool.schema_version(),
                    latency_ms: start.elapsed().as_millis() as u64,
                    action,
                    source,
                });
            }
        }
    }

    Ok((consequences, remaining))
}
