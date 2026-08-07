//! Unified drift detection across corpora. See
//! `docs/specs/SPEC-drift-consolidation.md`.

pub mod registry;
pub mod types;

pub use registry::{DEFAULT_LIMIT, MAX_LIMIT, SourceInputs, run_all, run_source};
pub use types::{
    AggregateReport, Availability, Category, DriftItem, DriftReport, Evidence, Family,
    MissingReason, Source, Totals, Verdict, uncovered_count,
};
