//! The [`Consequence`] type — the unit of communication from rules to actors.
//!
//! A consequence is a deterministic, rule-derived statement that the rules
//! engine vouches for. Actors (LLMs in the typical case) consume consequences
//! to produce expression or action. Actors never inspect raw state.
//!
//! # Push vs pull
//!
//! Same type, two delivery mechanisms:
//!
//! - **Push** — emitted by a RETE rule firing or an action handler.
//!   `Provenance::RuleFiring { rule_id, .. }`.
//! - **Pull** — returned by a deterministic lookup tool in response to an
//!   actor's query. `Provenance::Lookup { tool, .. }`.
//!
//! The actor doesn't need to care which transport produced the consequence.
//! It does need to know the consequence is trustworthy, which is what
//! [`Provenance`] records.
//!
//! # Status
//!
//! Phase E1 — types only. Not yet wired into rete or tools. See
//! `docs/research/episteme-extraction.md`.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

use crate::ids::RuleId;

/// A deterministic, rule-derived statement that an [`Actor`] may consume.
///
/// [`Actor`]: super::actor::Actor
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Consequence {
    /// Stable kind discriminator. Domain-specific consumers match on this
    /// to decide how to render or act.
    pub kind: ConsequenceKind,

    /// Predicate-style identifier — e.g. `"card.played"`,
    /// `"round.ended"`, `"compile.error"`.
    ///
    /// The same shape as a RETE fact predicate, deliberately. Push-mode
    /// consequences inherit their predicate from the firing rule's head;
    /// pull-mode consequences use the tool name (`"lookup_symbol"` etc.).
    pub predicate: String,

    /// Structured payload. Domain owns the schema. JSON is the lingua
    /// franca because it crosses the FFI / process boundaries that real
    /// integrations need (sheet's swift-bridge, headless stdio, future
    /// network transports).
    pub payload: serde_json::Value,

    /// Where this consequence came from. Lets actors weight or cite their
    /// inputs and lets debuggers trace LLM output back to ground truth.
    pub provenance: Provenance,
}

/// Discriminator for how a consequence should be treated by actors.
///
/// Intentionally small — the predicate carries the specifics. This enum
/// is for the actor's wiring decisions ("do I narrate, do I propose
/// code, do I refuse").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsequenceKind {
    /// Something happened in the game. The actor's job is to express it.
    /// e.g. a rule violation surfaced, a fact retracted, a card played.
    Event,

    /// A bounded snapshot of state. The actor may read it; it does not
    /// represent a change. e.g. a config snapshot, the current rule
    /// pack, a card-in-hand listing.
    Snapshot,

    /// A constraint or invariant the actor must respect. e.g. "this code
    /// must remain backwards compatible", "this response must not include
    /// secrets".
    Constraint,

    /// A concrete option the actor may choose among. e.g. "the patch can
    /// target file A or file B", "the next card may be drawn or
    /// discarded".
    Affordance,
}

/// Where a consequence came from. Used for trust weighting, citation,
/// and debugging.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum Provenance {
    /// Emitted by a rule firing in the rete network.
    RuleFiring {
        rule_id: RuleId,
        /// Optional fact IDs that satisfied the rule's conditions, for
        /// "why did this fire?" tracing.
        bound_facts: Vec<String>,
        /// `?var → value` bindings produced by the agenda item that fired
        /// this rule. Keys keep their leading `?` so they round-trip with
        /// the rule definition.
        #[serde(default)]
        bindings: HashMap<String, String>,
        /// Source labels for attributed bound facts, keyed by fact ID.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        fact_sources: BTreeMap<String, String>,
        /// Accepted ADR IDs governing this rule.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        decisions: Vec<String>,
    },

    /// Returned by a deterministic lookup tool. `tool` is a stable
    /// identifier the host defines (typically the dispatched tool's
    /// kind/name string).
    Lookup {
        tool: String,
        /// Schema version of the payload. Hosts define their own
        /// versioning scheme — commonly mirroring whatever their tool
        /// dispatch layer exposes (e.g. `tools::SCHEMA_VERSION`).
        schema_version: u8,
    },

    /// A rule fired, and its action invoked a deterministic lookup
    /// tool. The resulting consequence carries provenance from both
    /// layers — the push-mode trigger (rule_id, bound_facts) and the
    /// pull-mode resolution (tool, schema_version).
    ///
    /// This is the composed case: rule authors write actions that
    /// name a registered tool, and the pipeline routes the
    /// invocation, producing a consequence a narrator can trace
    /// back through both sources.
    RuleDrivenLookup {
        rule_id: RuleId,
        bound_facts: Vec<String>,
        /// See [`Provenance::RuleFiring`]'s `bindings` field for semantics.
        #[serde(default)]
        bindings: HashMap<String, String>,
        /// Source labels for attributed bound facts, keyed by fact ID.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        fact_sources: BTreeMap<String, String>,
        /// Accepted ADR IDs governing this rule.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        decisions: Vec<String>,
        tool: String,
        schema_version: u8,
    },

    /// Asserted directly by the host application without going through
    /// the rules engine — typically for boot-time invariants or test
    /// fixtures. Use sparingly: bypassing the rules forfeits the
    /// epistemic guarantee the abstraction is built on.
    Asserted { by: String },
}
