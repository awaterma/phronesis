//! Pull-mode adapter surface.
//!
//! Hosts expose deterministic lookup functions. The actor invokes one,
//! and the result is wrapped as a [`Consequence`] with
//! [`Provenance::Lookup`] so the actor can treat it uniformly with
//! push-mode firings.
//!
//! # Two layers: dynamic core, typed convenience
//!
//! The crate ships two `Lookup` traits, modelled on the CUE/Rhai
//! relationship (CUE is to Rhai what TypeScript is to JavaScript):
//!
//! - [`DynLookup`] — the honest shape of the core. Request and response
//!   are both `serde_json::Value`. The core engine is intentionally
//!   **schema-agnostic**: it trusts the tagged dynamic values it's
//!   handed. Schema validation, if any, happens at module-load time in
//!   a higher layer (e.g. a CUE schema check) — not per-call at runtime.
//! - [`Lookup`] — a typed convenience layer for Rust-native hosts
//!   where the compiler can already check request/response shapes.
//!   Implementors get `serde_json::Value` marshalling for free via the
//!   blanket [`DynLookup`] impl, so a typed tool is also a dynamic tool
//!   without extra work.
//!
//! Use [`Lookup`] when the host is a Rust process authoring tools
//! directly in Rust. Use [`DynLookup`] when the tools are authored in
//! a dynamic layer like Rhai scripts, WASM modules, or remote RPC —
//! anywhere the types aren't known to the Rust compiler.
//!
//! # Usage pattern
//!
//! Each tool the host wants to expose implements one of the traits.
//! The host (a game engine, a conversational module, a sheet FFI
//! boundary, etc.) collects its implementations into a registry its
//! actor can invoke. The registry is out of scope for the core crate
//! — we don't want to commit to a naming scheme or dispatcher
//! protocol prematurely.
//!
//! # Why a trait and not just a function?
//!
//! - Tools carry metadata the core needs for provenance (name, schema
//!   version). A trait makes that contract explicit.
//! - The trait is object-safe via the blanket helpers below, so a
//!   registry of `Box<dyn ErasedLookup>` is straightforward when a host
//!   wants dynamic dispatch.

use serde::Serialize;

use crate::consequence::{Consequence, ConsequenceKind, Provenance};

/// A deterministic, side-effect-free lookup the host exposes to actors.
///
/// Implementations are sync because the pattern demands it — pull
/// lookups are supposed to be fast, trace-free, and cache-friendly.
/// Async or I/O-heavy "lookups" are actually push producers; use
/// `Consequence::from_rule_firing` for those.
pub trait Lookup {
    /// Typed request body.
    type Request;
    /// Typed response body. Must be serializable so we can embed it as
    /// the consequence payload.
    type Response: Serialize;

    /// Stable identifier for this tool. Becomes the consequence
    /// predicate and the `Provenance::Lookup::tool` field.
    fn name(&self) -> &'static str;

    /// Schema version of the response payload. Hosts bump this when
    /// the payload shape changes in a way actors must notice.
    fn schema_version(&self) -> u8;

    /// Execute the lookup. Errors are the host's concern — typically a
    /// malformed request or an upstream data-source issue. `Ok` with a
    /// response that has `available: false` or `found: false` is not
    /// an error; it's a real consequence the actor should reason from.
    fn invoke(&self, req: Self::Request) -> anyhow::Result<Self::Response>;
}

/// Dynamic-typed lookup. The honest shape of the core.
///
/// Request and response are both `serde_json::Value` — the engine does
/// not know or care about the schema. Schema validation, if any, lives
/// in a higher layer (CUE module manifest, WASM host policy, etc.)
/// applied at tool-load time, not per-call.
///
/// Any `Lookup` is automatically also a `DynLookup` via the blanket
/// impl below, so Rust-authored tools don't need to implement this
/// directly. Hosts that dispatch from a dynamic driver (Rhai scripts,
/// remote RPC, WASM) implement this directly.
pub trait DynLookup {
    fn name(&self) -> &'static str;
    fn schema_version(&self) -> u8;
    fn invoke_dyn(&self, req: serde_json::Value) -> anyhow::Result<serde_json::Value>;
}

impl<L> DynLookup for L
where
    L: Lookup,
    L::Request: for<'de> serde::Deserialize<'de>,
{
    fn name(&self) -> &'static str {
        <L as Lookup>::name(self)
    }

    fn schema_version(&self) -> u8 {
        <L as Lookup>::schema_version(self)
    }

    fn invoke_dyn(&self, req: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let typed_req: L::Request = serde_json::from_value(req)?;
        let typed_resp = self.invoke(typed_req)?;
        Ok(serde_json::to_value(typed_resp)?)
    }
}

/// Invoke a [`DynLookup`] and wrap the Value response as a
/// [`Consequence`]. The dynamic counterpart of [`lookup_as_consequence`].
pub fn dyn_lookup_as_consequence(
    tool: &dyn DynLookup,
    req: serde_json::Value,
) -> anyhow::Result<Consequence> {
    let payload = tool.invoke_dyn(req)?;
    Ok(Consequence {
        kind: ConsequenceKind::Snapshot,
        predicate: tool.name().to_string(),
        payload,
        provenance: Provenance::Lookup {
            tool: tool.name().to_string(),
            schema_version: tool.schema_version(),
        },
    })
}

impl Consequence {
    /// Wrap an arbitrary serializable payload as a pull-mode
    /// [`Consequence`]. The `predicate` typically equals the tool name
    /// but callers may override (e.g. for sub-kind routing).
    pub fn from_lookup<T: Serialize>(
        tool: impl Into<String>,
        schema_version: u8,
        predicate: impl Into<String>,
        payload: &T,
    ) -> Result<Self, serde_json::Error> {
        Ok(Consequence {
            kind: ConsequenceKind::Snapshot,
            predicate: predicate.into(),
            payload: serde_json::to_value(payload)?,
            provenance: Provenance::Lookup {
                tool: tool.into(),
                schema_version,
            },
        })
    }

    /// Wrap a rule-firing payload as a push-mode [`Consequence`].
    pub fn from_rule_firing<T: Serialize>(
        context: RuleFiringContext,
        payload: &T,
    ) -> Result<Self, serde_json::Error> {
        Ok(Consequence {
            kind: context.kind,
            predicate: context.predicate,
            payload: serde_json::to_value(payload)?,
            provenance: Provenance::RuleFiring {
                rule_id: context.rule_id,
                bound_facts: context.bound_facts,
                bindings: Default::default(),
            },
        })
    }
}

/// Metadata describing the rule activation that produced a consequence.
#[derive(Debug, Clone)]
pub struct RuleFiringContext {
    pub rule_id: crate::RuleId,
    pub predicate: String,
    pub bound_facts: Vec<String>,
    pub kind: ConsequenceKind,
}

impl RuleFiringContext {
    pub fn new(
        rule_id: impl Into<crate::RuleId>,
        predicate: impl Into<String>,
        bound_facts: Vec<String>,
        kind: ConsequenceKind,
    ) -> Self {
        Self {
            rule_id: rule_id.into(),
            predicate: predicate.into(),
            bound_facts,
            kind,
        }
    }
}

/// Convenience: invoke a [`Lookup`] and wrap its response as a
/// [`Consequence`] in one call. Callers that don't need the raw
/// response can use this; callers that do (e.g. sheet's FFI boundary)
/// should hold onto both.
pub fn lookup_as_consequence<L: Lookup>(tool: &L, req: L::Request) -> anyhow::Result<Consequence> {
    let resp = tool.invoke(req)?;
    Ok(Consequence::from_lookup(
        tool.name(),
        tool.schema_version(),
        tool.name(),
        &resp,
    )?)
}
