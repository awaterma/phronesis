//! Journey facts — durable, recomputed-per-call temporal predicates.
//!
//! See `docs/specs/SPEC-journey-facts.md`. The stateless hook stays stateless:
//! every invocation rebuilds the network and re-derives `journey_*` facts from
//! a bounded suffix of `.phronesis/journey/events.jsonl`. State lives on disk;
//! decay is the sliding window; determinism is a pure function of
//! (journal bytes, ts, sid).

pub mod journal;
pub mod tagger;
