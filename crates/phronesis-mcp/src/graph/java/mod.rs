//! Java package identities and declaration-based import resolution.
//!
//! Build discovery supplies visibility; source declarations supply names.
//! Keeping those inputs separate prevents filesystem layout from inventing
//! packages or index membership from implying a compile dependency.

pub mod bazel;
pub mod grammar;
pub mod index;
pub mod maven;
pub mod parse;
pub mod project;
