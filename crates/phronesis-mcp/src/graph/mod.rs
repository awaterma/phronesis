//! Structural code-graph facts.
//!
//! Implements `docs/specs/SPEC-triple-store-rete.md` (rev 4): a durable,
//! gitignored graph of architectural relations at `.phronesis/graph.jsonl`,
//! rebuilt incrementally by the extractor on `PostToolUse` and hydrated into
//! the RETE network on `PreToolUse`.
//!
//! No engine changes are required: edges hydrate as ordinary
//! `Fact { predicate, args }` with the relation as the predicate.

pub mod audit;
pub mod derive;
pub mod extract;
pub mod hydrate;
pub mod model;
pub mod python;
pub mod query;
pub mod resolve;
pub mod store;
pub mod sync;
pub mod unit;

pub use model::Edge;
