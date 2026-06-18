//! Confidence scoring — grounded build/test outcome facts.
//!
//! phronesis's other fact families describe *syntax* (diff_extract, syntax/)
//! or *time* (clock_facts). This family describes **grounded outcomes**: did
//! the suggested code compile, did the tests pass. The signals come from the
//! output of the slow-clock tool calls that run the toolchain (`cargo test`,
//! `cargo build`, …) — see `HookPayload.tool_output`.
//!
//! The design keeps the engine domain-neutral the same way `syntax/` does:
//! per-toolchain **adapters** parse one toolchain's output and emit the *same*
//! neutral facts (`build_outcome`, `test_outcome`). Rules never name a
//! toolchain. Adding a `pytest` adapter generalizes confidence scoring beyond
//! Rust without touching a rule.
//!
//! See `docs/specs/SPEC-confidence-scoring.md` for the full design, including
//! how these facts become a discretized confidence band and gate the
//! done-claim / commit.

pub mod adapter;
pub mod cargo;
pub mod derive;
pub mod facts;
pub mod ledger;
pub mod subject;

pub use adapter::extract;
pub use derive::{band, signals};
pub use facts::{Band, OutcomeFact};
