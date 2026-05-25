//! Episteme — domain-neutral surface for rules-bounded LLM interaction.
//!
//! Extracted from phronesis as an in-tree workspace member crate. Has no
//! dependency on phronesis; phronesis depends on it. See
//! `docs/research/episteme-extraction.md` for the design thesis.
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
//!   In phronesis, backed by the `rete` network feeding `play::llm_generation`.
//! - **Pull**: actor asks → deterministic lookup returns a [`Consequence`].
//!   In phronesis, backed by `tools::*` (spec 046, also wraps the swift-bridge
//!   FFI that powers the `sheet` companion app).
//!
//! Phase E1 (current) defines the types only. No behavior. No adapters.
//! Adapters land in phronesis in E2 (push) and E3 (pull) without moving any
//! existing files. See the design doc for the phase plan.

pub mod actor;
pub mod agenda;
pub mod alpha_network;
pub mod beta_network;
pub mod compose;
pub mod consequence;
pub mod engine_types;
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
    invoke_rule_driven_lookups, try_invoke_rule_driven_lookups, LookupRegistry, ToolInvocationError,
};
pub use consequence::{Consequence, ConsequenceKind, Provenance};
pub use engine_types::{Action, Condition, Fact, PerformanceValues, Rule};
pub use network::*;
pub use production::*;
pub use pull::{dyn_lookup_as_consequence, lookup_as_consequence, DynLookup, Lookup};
pub use push::rule_firing_to_consequences;
pub use script_evaluator::ScriptEvaluator;
pub use variable_binding::{Bindings, Token};
pub use wme::{WmeManager, WorkingMemoryElement};
