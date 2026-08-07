//! Unified drift detection across corpora. See
//! `docs/specs/SPEC-drift-consolidation.md`.

pub mod types;

pub use types::{
    AggregateReport, Availability, Category, DriftItem, DriftReport, Evidence, Family,
    MissingReason, Source, Totals, Verdict, uncovered_count,
};
