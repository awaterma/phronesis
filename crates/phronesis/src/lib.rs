//! Phronesis — domain-neutral surface for rules-bounded LLM interaction.
//!
//! This crate is intentionally minimal and has no domain-specific
//! dependencies. Its job is to host the abstraction (Consequence, Actor,
//! Provenance) that hosting applications implement against.
//!
//! # The pattern
//!
//! Facts are asserted into a working memory. Rules fire deterministically
//! against those facts. The **consequences** of those firings — not raw
//! state — are what an LLM sees. The LLM then *expresses* (narration) or
//! *acts* (code, file edits, decisions) within those derived bounds.
//!
//! Two transports of the same idea coexist:
//!
//! - **Push**: rule fires → [`Consequence`] → [`Actor`] consumes it.
//!   Typically backed by a RETE network feeding a narration or
//!   code-generation layer.
//! - **Pull**: actor asks → deterministic lookup returns a [`Consequence`].
//!   Typically backed by a registry of tool implementations the host
//!   exposes to its agent.
//!
//! The crate defines the types and the engine; integration with any
//! particular host (an MCP server, a game engine, a sheet-FFI bridge,
//! a conversational module) lives outside this crate.
//!
//! # Features
//!
//! - **`embedding-host`** (off by default) — extends [`ReteNetwork`]'s
//!   public surface with methods only an external embedding host needs:
//!   bulk save/restore (`restore_persistent_facts*`), single-step agenda
//!   (`execute_next_agenda_item`), batch-retraction id collection
//!   (`fact_ids_matching`), and instrumentation getters
//!   (`get_performance_stats`, `get_rules_count`, …). The default surface
//!   equals what the bundled MCP consumes, so the compiler enforces that
//!   symmetry; a host that drives the engine directly enables this feature.
//! - **`schemars`** (off by default) — derive JSON schemas for the ID
//!   newtypes so downstream MCP tool-parameter structs keep their schema.

pub mod actor;
pub mod agenda;
pub mod alpha_network;
pub mod beta_network;
pub mod compose;
pub mod consequence;
pub mod engine_types;
pub mod error;
pub mod ids;
pub mod network;
pub mod production;
pub mod pull;
pub mod push;
pub mod script_evaluator;
pub mod variable_binding;
pub mod wme;

pub use actor::{Actor, ActorOutput};
pub use agenda::*;
pub use alpha_network::*;
pub use beta_network::*;
pub use compose::{
    LookupRegistry, ToolInvocationError, invoke_rule_driven_lookups, try_invoke_rule_driven_lookups,
};
pub use consequence::{Consequence, ConsequenceKind, Provenance};
pub use engine_types::{Action, Condition, Fact, PerformanceStats, Rule};
pub use error::ReteError;
pub use ids::{FactId, RuleId, StateId};
pub use network::*;
pub use production::*;
pub use pull::{
    DynLookup, Lookup, RuleFiringContext, dyn_lookup_as_consequence, lookup_as_consequence,
};
pub use push::rule_firing_to_consequences;
pub use script_evaluator::{BuiltinScriptEvaluator, ScriptEval, ScriptEvaluator};
pub use variable_binding::{Bindings, Token};
pub use wme::{WmeManager, WorkingMemoryElement};
