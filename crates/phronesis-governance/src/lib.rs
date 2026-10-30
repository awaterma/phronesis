//! Embeddable governance core for the phronesis RETE engine.
//!
//! Extracted from `phronesis-mcp` as pure, filesystem-free pieces: the
//! `.phronesis/rules.json` parser and strict validation, OR expansion, disk
//! conversions, compiled-policy selection, and `emit_capsule` payload
//! validation. Storage, capture, expiry, and project discovery stay in the
//! MCP crate; this crate exposes only pure functions and value types.

pub mod capsule;
pub mod policy;

pub use capsule::{
    CapsuleLifecycle, CapsulePayloadError, CapsuleSpec, MAX_RECORD_BYTES, parse_duration,
    validate_emit_capsule_payload,
};
pub use policy::{
    ACTION_VERBS, CompiledPolicy, DiskAction, DiskCondition, DiskRule, ParsedPolicy, ParsedSource,
    Phase, PolicyDiagnostic, PolicyError, RULE_PHASES, RuleCheckError, RulesFile, Selection,
    SourceRule, WhenClause, base_rule_id, check_action_type, check_conditions_nonempty,
    check_single_action, parse, parse_source, rule_from_disk, rule_to_disk, unfold_or,
    unfold_or_with_diagnostics,
};
