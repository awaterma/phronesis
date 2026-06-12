use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use uuid::Uuid;

use crate::agenda::AgendaItem;
use crate::consequence::{Consequence, ConsequenceKind, Provenance};
use crate::engine_types::{Action, Condition, Rule};
use crate::error::ReteError;
use crate::variable_binding::Bindings;
use crate::wme::WorkingMemoryElement;
use tracing::warn;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProductionState {
    pub id: String,
    pub rule: Rule,    // The rule that triggers this production state
    pub salience: i32, // Priority of the rule
    /// The beta state ID of this rule's terminal (p-state) in the beta network.
    /// Used for the retraction path to scope agenda updates to the correct rule.
    pub terminal_state_id: Option<String>,
}

impl ProductionState {
    pub fn new(rule: Rule, salience: i32) -> Self {
        ProductionState {
            id: Uuid::new_v4().to_string(),
            rule,
            salience,
            terminal_state_id: None,
        }
    }

    pub fn new_with_terminal(rule: Rule, salience: i32, terminal_state_id: Option<String>) -> Self {
        ProductionState {
            id: Uuid::new_v4().to_string(),
            rule,
            salience,
            terminal_state_id,
        }
    }

    /// Execute the rule's actions given matching WMEs
    /// Substitutes variables in action parameters with values from WME bindings
    pub fn execute(&self, wme_list: &[WorkingMemoryElement]) -> Result<Vec<Action>, ReteError> {
        use crate::variable_binding::Bindings;

        // Extract bindings from the WMEs by matching against rule conditions
        let mut bindings = Bindings::new();

        // Match each condition with corresponding WMEs to build bindings
        for (idx, condition) in self.rule.conditions.iter().enumerate() {
            if idx < wme_list.len() {
                let wme = &wme_list[idx];
                // Try to bind this condition with this WME
                bindings = bindings.can_bind(condition, &wme.fact).unwrap_or(bindings);
                // Keep existing bindings if match fails
            }
        }

        // Substitute variables in action parameters
        let mut substituted_actions = Vec::new();
        for action in &self.rule.actions {
            let mut substituted_params = Vec::new();

            for param in &action.params {
                substituted_params.push(apply_bindings(param, &bindings));
            }

            substituted_actions.push(Action {
                action_type: action.action_type.clone(),
                params: substituted_params,
            });
        }

        Ok(substituted_actions)
    }
}

/// Entry registered for each single-real-condition rule, keyed by the
/// condition's predicate. Lets the assert path skip the full rule-set scan
/// (see `update_agenda_for_wme_single_condition`); built once at `add_rule`
/// time, never mutated after.
#[derive(Debug, Clone)]
pub struct SingleCondRuleEntry {
    pub rule: Rule,
    pub salience: i32,
    pub condition: Condition,
}

/// Production Network manages all production states
#[derive(Debug)]
pub struct ProductionNetwork {
    pub states: Vec<ProductionState>,
    pub rule_index: HashMap<String, String>, // Maps rule ID to production state ID
    /// Predicate → single-condition rules that match that predicate. Avoids a
    /// full `states.clone()` + scan per fact assertion on the hot path.
    pub single_cond_index: HashMap<String, Vec<SingleCondRuleEntry>>,
}

impl Default for ProductionNetwork {
    fn default() -> Self {
        Self::new()
    }
}

impl ProductionNetwork {
    pub fn new() -> Self {
        ProductionNetwork {
            states: Vec::new(),
            rule_index: HashMap::new(),
            single_cond_index: HashMap::new(),
        }
    }

    /// Add a rule as a production state
    pub fn add_rule(&mut self, rule: Rule, salience: i32) -> String {
        self.add_rule_with_terminal(rule, salience, None)
    }

    /// Add a rule as a production state with an associated beta terminal (p-state) ID
    pub fn add_rule_with_terminal(
        &mut self,
        rule: Rule,
        salience: i32,
        terminal_state_id: Option<String>,
    ) -> String {
        let production_state =
            ProductionState::new_with_terminal(rule.clone(), salience, terminal_state_id);
        let state_id = production_state.id.clone();

        // Single-cond fast-path index: if this rule has exactly one real
        // (non-script) condition, register it under that condition's predicate.
        let real_conds: Vec<&Condition> = rule
            .conditions
            .iter()
            .filter(|c| c.predicate != "__script__")
            .collect();
        if real_conds.len() == 1 {
            let cond = real_conds[0].clone();
            self.single_cond_index
                .entry(cond.predicate.clone())
                .or_default()
                .push(SingleCondRuleEntry {
                    rule: rule.clone(),
                    salience,
                    condition: cond,
                });
        }

        self.states.push(production_state);
        self.rule_index.insert(rule.id.clone(), state_id.clone());

        state_id
    }

    /// Get the number of rules in the production network
    pub fn get_rules_count(&self) -> usize {
        self.states.len()
    }

    /// Find a production state by rule ID
    pub fn find_by_rule_id(&self, rule_id: &str) -> Option<&ProductionState> {
        if let Some(state_id) = self.rule_index.get(rule_id) {
            self.states.iter().find(|state| state.id == *state_id)
        } else {
            None
        }
    }

    /// Execute a specific production state
    pub fn execute_state(
        &self,
        state_id: &str,
        wme_list: &[WorkingMemoryElement],
    ) -> Result<Vec<Action>, ReteError> {
        match self.states.iter().find(|state| state.id == state_id) {
            Some(state) => state.execute(wme_list),
            None => Err(ReteError::ProductionStateNotFound(state_id.to_string())),
        }
    }

    /// Execute the next agenda item
    pub fn execute_agenda_item(&self, agenda_item: &AgendaItem) -> Result<Vec<Action>, ReteError> {
        // Find the corresponding production state
        match self
            .states
            .iter()
            .find(|state| state.rule.id == agenda_item.rule.id)
        {
            Some(state) => {
                // Substitute variables in action parameters using bindings from the agenda item
                let mut substituted_actions = Vec::new();

                for action in &state.rule.actions {
                    let mut substituted_params = Vec::new();

                    for param in &action.params {
                        substituted_params.push(apply_bindings(param, &agenda_item.bindings));
                    }

                    // Defense-in-depth: skip actions with unbound variables (Forgy integrity check)
                    if substituted_params.iter().any(|p| p.starts_with('?')) {
                        warn!(
                            "RETE integrity: unbound variables in rule '{}' action '{}': {:?}",
                            agenda_item.rule.id, action.action_type, substituted_params
                        );
                        continue;
                    }

                    substituted_actions.push(Action {
                        action_type: action.action_type.clone(),
                        params: substituted_params,
                    });
                }

                Ok(substituted_actions)
            }
            None => Err(ReteError::ProductionStateNotFound(
                agenda_item.rule.id.clone(),
            )),
        }
    }

    /// Fire the matching production state for `agenda_item`, producing a
    /// `Consequence` per action with substituted params, rule_id, and bindings
    /// preserved on `Provenance::RuleFiring`. Newer caller path; the existing
    /// `execute_agenda_item` is retained for callers that only want `Action`s.
    pub fn fire_agenda_item(
        &self,
        agenda_item: &AgendaItem,
    ) -> Result<Vec<Consequence>, ReteError> {
        let state = match self
            .states
            .iter()
            .find(|n| n.rule.id == agenda_item.rule.id)
        {
            Some(n) => n,
            None => {
                return Err(ReteError::ProductionStateNotFound(
                    agenda_item.rule.id.clone(),
                ));
            }
        };

        let mut out = Vec::new();
        for action in &state.rule.actions {
            let substituted: Vec<String> = action
                .params
                .iter()
                .map(|p| apply_bindings(p, &agenda_item.bindings))
                .collect();

            // Defense in depth: skip actions with any residual unbound variable
            // in their first character — preserves the Forgy integrity check.
            if substituted.iter().any(|p| p.starts_with('?')) {
                warn!(
                    "RETE integrity: unbound variables in rule '{}' action '{}': {:?}",
                    agenda_item.rule.id, action.action_type, substituted
                );
                continue;
            }

            let kind = match action.action_type.as_str() {
                "constraint_violation" | "constraint_warning" => ConsequenceKind::Constraint,
                _ => ConsequenceKind::Event,
            };

            let message = substituted.join(" ");
            let payload = json!({
                "action_type": action.action_type,
                "message": message,
                "params": substituted,
            });

            out.push(Consequence {
                kind,
                predicate: state.rule.id.clone(),
                payload,
                provenance: Provenance::RuleFiring {
                    rule_id: state.rule.id.clone().into(),
                    bound_facts: Vec::new(),
                    bindings: agenda_item.bindings.bindings.clone(),
                },
            });
        }
        Ok(out)
    }
}

/// Substitute every `?var` occurrence in `param` with the bound value from
/// `bindings`. Variables not present in `bindings` are left as-is so that
/// the Forgy integrity check in `execute_agenda_item` can still detect them.
///
/// The keys in `Bindings::bindings` already include the leading `?`, so a
/// straight string-replace per entry produces the right output.
pub(crate) fn apply_bindings(param: &str, bindings: &Bindings) -> String {
    let mut out = param.to_string();
    for (var, value) in &bindings.bindings {
        out = out.replace(var, value);
    }
    out
}

#[cfg(test)]
mod apply_bindings_tests {
    use super::*;
    use crate::variable_binding::Bindings;

    fn b(pairs: &[(&str, &str)]) -> Bindings {
        let mut bindings = Bindings::new();
        for (k, v) in pairs {
            bindings.add_binding(k, v).unwrap();
        }
        bindings
    }

    #[test]
    fn substitutes_a_single_bare_variable() {
        let bindings = b(&[("?fn", "greet")]);
        assert_eq!(apply_bindings("?fn", &bindings), "greet");
    }

    #[test]
    fn substitutes_mixed_string_with_multiple_variables() {
        // This is the regression case for the existing bug: mixed strings
        // were not substituted because the old code only checked
        // `param.starts_with('?')`.
        let bindings = b(&[("?fn", "greet"), ("?file", "src/lib.rs")]);
        assert_eq!(
            apply_bindings("Function `?fn` in ?file uses Result", &bindings),
            "Function `greet` in src/lib.rs uses Result"
        );
    }

    #[test]
    fn leaves_unbound_variables_in_place() {
        let bindings = b(&[("?fn", "greet")]);
        assert_eq!(apply_bindings("?fn in ?file", &bindings), "greet in ?file");
    }

    #[test]
    fn returns_input_unchanged_when_no_variables() {
        let bindings = b(&[("?fn", "greet")]);
        assert_eq!(
            apply_bindings("no variables here", &bindings),
            "no variables here"
        );
    }
}

#[cfg(test)]
mod execute_agenda_item_substitution_tests {
    use super::*;
    use crate::agenda::AgendaItem;
    use crate::engine_types::{Action as RuleAction, Condition, Rule};
    use crate::variable_binding::Bindings;

    fn rule_with_mixed_param() -> Rule {
        Rule {
            id: "test-rule".to_string(),
            priority: 1,
            conditions: vec![Condition {
                predicate: "p".to_string(),
                args: vec!["?fn".to_string()],
                script: None,
            }],
            actions: vec![RuleAction {
                action_type: "constraint_violation".to_string(),
                params: vec!["Function `?fn` is bad".to_string()],
            }],
        }
    }

    #[test]
    fn execute_agenda_item_substitutes_mixed_string_params() {
        let mut net = ProductionNetwork::new();
        let rule = rule_with_mixed_param();
        net.add_rule(rule.clone(), 0);

        let mut bindings = Bindings::new();
        bindings.add_binding("?fn", "greet").unwrap();
        let agenda_item = AgendaItem {
            rule,
            wme_list: vec![],
            bindings,
            salience: 0,
            id: "ai-1".to_string(),
            seq: 0,
        };

        let actions = net.execute_agenda_item(&agenda_item).unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(
            actions[0].params,
            vec!["Function `greet` is bad".to_string()]
        );
    }
}

#[cfg(test)]
mod fire_agenda_item_tests {
    use super::*;
    use crate::agenda::AgendaItem;
    use crate::consequence::{ConsequenceKind, Provenance};
    use crate::engine_types::{Action as RuleAction, Condition, Rule};
    use crate::variable_binding::Bindings;

    fn build_rule() -> Rule {
        Rule {
            id: "rust-error-thiserror-for-libraries".to_string(),
            priority: 10,
            conditions: vec![Condition {
                predicate: "function_returns_result_string".to_string(),
                args: vec!["?file".to_string(), "?fn".to_string()],
                script: None,
            }],
            actions: vec![RuleAction {
                action_type: "constraint_violation".to_string(),
                params: vec![
                    "`?fn` in ?file returns Result<_, String>. Use thiserror.".to_string(),
                ],
            }],
        }
    }

    fn build_agenda(rule: Rule) -> AgendaItem {
        let mut bindings = Bindings::new();
        bindings.add_binding("?file", "src/lib.rs").unwrap();
        bindings.add_binding("?fn", "bad").unwrap();
        AgendaItem {
            rule,
            wme_list: vec![],
            bindings,
            salience: 0,
            id: "ai-1".to_string(),
            seq: 0,
        }
    }

    #[test]
    fn fire_agenda_item_emits_one_consequence_per_action() {
        let mut net = ProductionNetwork::new();
        let rule = build_rule();
        net.add_rule(rule.clone(), 0);
        let item = build_agenda(rule);

        let consequences = net.fire_agenda_item(&item).unwrap();

        assert_eq!(consequences.len(), 1);
        let c = &consequences[0];
        assert_eq!(c.predicate, "rust-error-thiserror-for-libraries");
        assert!(matches!(c.kind, ConsequenceKind::Constraint));
    }

    #[test]
    fn fire_agenda_item_populates_provenance_rule_id_and_bindings() {
        let mut net = ProductionNetwork::new();
        let rule = build_rule();
        net.add_rule(rule.clone(), 0);
        let item = build_agenda(rule);

        let consequences = net.fire_agenda_item(&item).unwrap();

        match &consequences[0].provenance {
            Provenance::RuleFiring {
                rule_id, bindings, ..
            } => {
                assert_eq!(rule_id, "rust-error-thiserror-for-libraries");
                assert_eq!(bindings.get("?fn").map(String::as_str), Some("bad"));
                assert_eq!(
                    bindings.get("?file").map(String::as_str),
                    Some("src/lib.rs")
                );
            }
            other => panic!("expected RuleFiring provenance, got {:?}", other),
        }
    }

    #[test]
    fn fire_agenda_item_populates_payload_message_with_substitutions() {
        let mut net = ProductionNetwork::new();
        let rule = build_rule();
        net.add_rule(rule.clone(), 0);
        let item = build_agenda(rule);

        let consequences = net.fire_agenda_item(&item).unwrap();
        let payload = &consequences[0].payload;
        assert_eq!(payload["action_type"], "constraint_violation");
        assert_eq!(
            payload["message"],
            "`bad` in src/lib.rs returns Result<_, String>. Use thiserror."
        );
        assert_eq!(
            payload["params"][0],
            "`bad` in src/lib.rs returns Result<_, String>. Use thiserror."
        );
    }

    #[test]
    fn fire_agenda_item_maps_constraint_warning_to_constraint_kind() {
        let mut net = ProductionNetwork::new();
        let mut rule = build_rule();
        rule.actions[0].action_type = "constraint_warning".to_string();
        net.add_rule(rule.clone(), 0);
        let item = build_agenda(rule);

        let consequences = net.fire_agenda_item(&item).unwrap();
        assert!(matches!(consequences[0].kind, ConsequenceKind::Constraint));
    }
}
