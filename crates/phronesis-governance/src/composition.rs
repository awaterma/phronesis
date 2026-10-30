//! Pure rule-layer composition.
//!
//! Extracted from `phronesis-mcp/src/rule_layers.rs`: the ordered,
//! later-wins merge over caller-loaded rule layers and the
//! `rule_overridden` provenance facts it produces. There is no
//! filesystem, path, environment, or pack-loader dependency here:
//! callers supply already-loaded (and presumed validated) layers with
//! opaque string source IDs, and map real filesystem paths to and from
//! those IDs themselves.
//!
//! Layers are applied in the order given. A later rule with the same ID
//! replaces the earlier rule at its original position and records an
//! override; the winner's origin becomes the rule's origin. Every
//! successive replacement is recorded, including identical cross-layer
//! rules — equality of rule content is never consulted. OR-expanded
//! rule IDs are unaffected: composition only ever looks at whole rule
//! IDs as they arrive.

use std::collections::HashMap;

use phr::Fact;

use crate::policy::DiskRule;

/// Predicate emitted for every cross-layer same-ID replacement.
pub const OVERRIDE_PREDICATE: &str = "rule_overridden";

/// Where a composed rule came from. `source_id` is an opaque label owned
/// by the caller (a filesystem path, pack coordinate, or any other
/// identifier); composition treats it as an uninterpreted string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleOrigin {
    pub layer: String,
    pub source_id: String,
    pub decision: Option<String>,
}

/// One same-ID replacement: `winner` (the later layer's rule) replaced
/// `overridden` (the previously winning origin) for `rule_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleOverride {
    pub rule_id: String,
    pub overridden: RuleOrigin,
    pub winner: RuleOrigin,
}

/// One caller-loaded layer: its origin plus the disk rules it declares.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedLayer {
    pub origin: RuleOrigin,
    pub rules: Vec<DiskRule>,
}

/// The composed result: winning rules in stable first-seen order, the
/// winning origin per rule ID, and every override in application order.
#[derive(Debug, Clone, PartialEq)]
pub struct ComposedPolicy {
    pub rules: Vec<DiskRule>,
    pub origins: HashMap<String, RuleOrigin>,
    pub overrides: Vec<RuleOverride>,
}

/// Compose ordered layers: later same-ID rules replace earlier ones in
/// place, every replacement is recorded as an override, and each rule's
/// origin is the last layer that supplied it.
///
/// Layers are presumed validated by the caller; this function performs
/// no schema or content checking.
pub fn compose(layers: &[LoadedLayer]) -> ComposedPolicy {
    let mut rules = Vec::<DiskRule>::new();
    let mut positions = HashMap::<String, usize>::new();
    let mut origins = HashMap::<String, RuleOrigin>::new();
    let mut overrides = Vec::new();
    for layer in layers {
        for rule in &layer.rules {
            if let Some(position) = positions.get(&rule.id).copied() {
                let overridden = origins
                    .get(&rule.id)
                    .cloned()
                    .expect("indexed rule has origin");
                overrides.push(RuleOverride {
                    rule_id: rule.id.clone(),
                    overridden,
                    winner: layer.origin.clone(),
                });
                rules[position] = rule.clone();
            } else {
                positions.insert(rule.id.clone(), rules.len());
                rules.push(rule.clone());
            }
            origins.insert(rule.id.clone(), layer.origin.clone());
        }
    }
    ComposedPolicy {
        rules,
        origins,
        overrides,
    }
}

/// Render overrides as `rule_overridden` provenance facts, one per
/// override, in input order.
///
/// This preserves the fact shape produced by the MCP rule-layer
/// resolver, with the filesystem path strings replaced by the origins'
/// opaque source IDs: id `__rule_override__:{index}:{rule_id}`,
/// predicate [`OVERRIDE_PREDICATE`], args `[rule_id, overridden.layer,
/// overridden.source_id, winner.layer, winner.source_id,
/// winner.decision or ""]`, timestamp zero, and source
/// `"rule_layers"`. Use [`override_facts_at`] to supply an explicit timestamp.
pub fn override_facts(overrides: &[RuleOverride]) -> Vec<Fact> {
    override_facts_at(overrides, 0)
}

/// Render override facts with caller-supplied time; never reads the host clock.
pub fn override_facts_at(overrides: &[RuleOverride], timestamp: u64) -> Vec<Fact> {
    overrides
        .iter()
        .enumerate()
        .map(|(index, item)| Fact {
            id: format!("__rule_override__:{index}:{}", item.rule_id),
            predicate: OVERRIDE_PREDICATE.to_string(),
            args: vec![
                item.rule_id.clone(),
                item.overridden.layer.clone(),
                item.overridden.source_id.clone(),
                item.winner.layer.clone(),
                item.winner.source_id.clone(),
                item.winner.decision.clone().unwrap_or_default(),
            ],
            timestamp,
            source: Some("rule_layers".to_string()),
        })
        .collect()
}
