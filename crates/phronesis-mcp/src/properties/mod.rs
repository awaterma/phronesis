//! Property records (SPEC-property-ontology.md): first-class semantic
//! properties — intent, version-controlled; evidence, derived.

pub mod allowlist;
pub mod execute;
pub mod hydrate;
pub mod store;
pub mod validate;

pub use hydrate::{EditedFile, PropertyFact, PropertyHydrationInput, RELATIONS, facts_for_event};
pub use store::{
    PROPERTIES_FORMAT, Property, PropertySource, PropertyStatus, RESULTS_FORMAT, ResultRecord,
    load_properties, load_results,
};
