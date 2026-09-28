pub mod collect;
pub mod hydrate;
pub mod import;
/// Pure cores of the coverage-store checks (identifier charset, FNV-1a 64,
/// line ordering), shared verbatim with the Verus harness.
///
/// Mechanism: `verification/harness.rs` `include!`s this same file, and the
/// Verus specs ride on the functions as `#[cfg_attr(verus_keep_ghost, ...)]`
/// attributes (Verus's attribute-form `verus_verify` / `verus_spec`), with
/// proof steps in `#[cfg(verus_keep_ghost)] proof! { ... }` statements.
/// rustc never sets `verus_keep_ghost`, so it compiles plain Rust; Verus
/// sets it and verifies these exact bodies. `store.rs` and `region_map.rs`
/// call the core through thin wrappers, so there is one executable source.
pub mod pure_core;
pub mod region_map;
pub mod select;
pub mod store;

pub use store::{COVERAGE_FORMAT, CoverageIndex, HitRecord};
