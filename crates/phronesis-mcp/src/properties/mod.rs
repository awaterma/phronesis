//! Property records (SPEC-property-ontology.md): first-class semantic
//! properties — intent, version-controlled; evidence, derived.

pub mod allowlist;
pub mod execute;
pub mod hydrate;
pub mod process;
pub mod render;
pub mod status;
pub mod store;
pub mod validate;
pub mod verify_cli;

pub use hydrate::{
    EditedFile, PropertyFact, PropertyHydration, PropertyHydrationInput, RELATIONS,
    facts_for_event, hydrate,
};
pub use store::{
    LEGACY_RESULTS_FORMAT, PROPERTIES_FORMAT, Property, PropertySource, PropertyStatus,
    RESULT_STATUSES, RESULTS_FORMAT, ResultRecord, load_properties, load_results,
};
