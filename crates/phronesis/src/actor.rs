//! The [`Actor`] trait — the LLM (or LLM-equivalent) bound to consume
//! [`Consequence`]s and produce expression or action.
//!
//! Actors do *not* inspect raw state. Their entire epistemic surface is
//! the consequence stream they're handed. This is the load-bearing
//! invariant of the episteme pattern.
//!
//! # Output modes
//!
//! [`ActorOutput`] is intentionally an enum so the same trait covers the
//! three concrete uses we already have or know we need:
//!
//! - **Text** — narration (e.g. game Opponents, chat responses).
//! - **Patch** — code or file edits (a future code-review actor).
//! - **Choice** — action selection from a set of [`Affordance`]
//!   consequences (a future autonomous play actor).
//!
//! # Status
//!
//! Phase E1 — trait shape only. No implementations yet. The first
//! implementation in E2 will wrap `crate::play::llm_generation` so
//! the existing narration path can run through this trait alongside its
//! current direct path. No deletions, no behavior changes.
//!
//! [`Affordance`]: super::consequence::ConsequenceKind::Affordance

use async_trait::async_trait;

use super::consequence::Consequence;

/// A consumer of [`Consequence`]s. Typically an LLM, but could be any
/// component that produces output bounded by rule-derived input.
///
/// Implementations are async because real LLM calls are.
#[async_trait]
pub trait Actor: Send + Sync {
    /// Consume a consequence stream and produce an output.
    ///
    /// The slice is the *complete* set of consequences the actor is
    /// allowed to reason from for this turn. Call sites are responsible
    /// for not leaking raw state into the actor by other channels.
    async fn act(&self, consequences: &[Consequence]) -> anyhow::Result<ActorOutput>;
}

/// What an actor produced. Intentionally narrow.
#[derive(Debug, Clone)]
pub enum ActorOutput {
    /// Free-form text. Narration, dialogue, explanation.
    Text(String),

    /// A proposed edit to one or more files. The patch format is
    /// deliberately opaque at this layer — concrete actors define it.
    Patch {
        /// Human-readable summary of the change (e.g. for a PR title).
        summary: String,
        /// Opaque patch payload. Format depends on the actor.
        body: serde_json::Value,
    },

    /// The actor selected one of the offered [`Affordance`] consequences.
    /// The string is the predicate of the chosen affordance.
    ///
    /// [`Affordance`]: super::consequence::ConsequenceKind::Affordance
    Choice(String),
}
