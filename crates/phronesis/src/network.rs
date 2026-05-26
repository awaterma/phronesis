//! The `ReteNetwork` — the posinating surface that wires alpha and
//! beta networks, the agenda, and the script evaluator into a single
//! engine instance. Hosts interact with phronesis primarily through
//! this type.
//!
//! The file is intentionally one module: a `ReteNetwork` is a single
//! coherent concept and its public methods are the engine surface. The
//! alternative — splitting routines across `network/rules.rs`,
//! `network/facts.rs`, etc. — would scatter operations on the same
//! state across files for the sake of a line-count threshold; the
//! cohesion that matters here is the type itself.
//!
//! phronesis-allow: audit-file-loc-high (single coherent engine surface)

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::agenda::Agenda;
use crate::alpha_network::AlphaNetwork;
use crate::beta_network::BetaNetwork;
use crate::consequence::Consequence;
use crate::engine_types::{Action, Condition, Fact, PerformanceValues, Rule};
use crate::production::ProductionNetwork;
use crate::script_evaluator::ScriptEvaluator;
use crate::wme::{WmeManager, WorkingMemoryElement};
use tracing::{debug, warn};

#[derive(Debug)]
pub struct ReteNetwork {
    pub wme_manager: Arc<Mutex<WmeManager>>,
    pub alpha_network: Arc<Mutex<AlphaNetwork>>,
    pub beta_network: Arc<Mutex<BetaNetwork>>,
    pub agenda: Arc<Mutex<Agenda>>,
    pub production_network: Arc<Mutex<ProductionNetwork>>,
    /// Performance tracking (interior mutability via Mutex)
    performance_values: Arc<Mutex<PerformanceValues>>,
    /// Track activations that have already been added to the agenda to avoid duplicates
    fired_activations: Arc<Mutex<HashSet<String>>>,
    /// Script condition evaluator for `__script__` pseudo-predicate conditions (038-score-progression)
    script_evaluator: ScriptEvaluator,
}

impl Default for ReteNetwork {
    fn default() -> Self {
        Self::new()
    }
}

impl ReteNetwork {
    pub fn new() -> Self {
        ReteNetwork {
            wme_manager: Arc::new(Mutex::new(WmeManager::new())),
            alpha_network: Arc::new(Mutex::new(AlphaNetwork::new())),
            beta_network: Arc::new(Mutex::new(BetaNetwork::new())),
            agenda: Arc::new(Mutex::new(Agenda::new())),
            production_network: Arc::new(Mutex::new(ProductionNetwork::new())),
            performance_values: Arc::new(Mutex::new(PerformanceValues::new())),
            fired_activations: Arc::new(Mutex::new(HashSet::new())),
            script_evaluator: ScriptEvaluator::new(),
        }
    }

    /// Process a new fact through the RETE network
    pub async fn assert_fact(&self, fact: Fact) -> Result<(), String> {
        let start = Instant::now();

        let wme = WorkingMemoryElement::new(fact);
        let wme_id = wme.id.clone();

        // Add WME to working memory
        {
            let mut wme_manager = self.wme_manager.lock().map_err(|e| e.to_string())?;
            wme_manager.assert(wme)?;
        }

        // Process through alpha network
        let alpha_match_results = {
            let mut alpha_network = self.alpha_network.lock().map_err(|e| e.to_string())?;
            let wme_manager = self.wme_manager.lock().map_err(|e| e.to_string())?;
            alpha_network.process_wme(
                wme_manager
                    .get(&wme_id)
                    .ok_or_else(|| "WME missing after assertion".to_string())?,
            )
        };

        // Process through beta network — p-states create activations directly (Forgy).
        // Hold the beta lock once across all alpha matches instead of re-acquiring
        // per iteration.
        let p_state_activations = {
            let mut beta_network = self.beta_network.lock().map_err(|e| e.to_string())?;
            let mut acts = Vec::new();
            for (state_id, token) in alpha_match_results {
                acts.extend(beta_network.process_token_from_source(&state_id, token));
            }
            acts
        };

        // Add p-state activations to agenda (multi-condition rules).
        // Hold fired_activations, production_network, and agenda for the whole
        // loop; previously each iteration re-acquired production_network and
        // agenda. wme_manager is left per-iteration because `evaluate_script_conditions`
        // re-enters it and std::sync::Mutex isn't reentrant.
        if !p_state_activations.is_empty() {
            let mut fired_activations = self.fired_activations.lock().map_err(|e| e.to_string())?;
            let production_network = self.production_network.lock().map_err(|e| e.to_string())?;
            let mut agenda = self.agenda.lock().map_err(|e| e.to_string())?;

            for activation in p_state_activations {
                let wme_ids: Vec<String> =
                    activation.token.wmes.iter().map(|w| w.id.clone()).collect();
                let activation_key = format!("{}:{}", activation.rule_id, wme_ids.join(","));

                if fired_activations.contains(&activation_key) {
                    debug!(
                        "Skipping already-fired p-state activation: {}",
                        activation_key
                    );
                    continue;
                }

                let rule = production_network
                    .find_by_rule_id(activation.rule_id.as_str())
                    .map(|pn| pn.rule.clone());

                if let Some(rule) = rule {
                    // evaluate_script_conditions locks wme_manager internally;
                    // see comment above for why we don't hold it across the loop.
                    let script_passes =
                        self.evaluate_script_conditions(&rule, &activation.token.bindings)?;
                    if !script_passes {
                        debug!(
                            "P-state activation blocked by script condition: rule '{}'",
                            rule.id
                        );
                        continue;
                    }

                    let wme_list = {
                        let wme_manager = self.wme_manager.lock().map_err(|e| e.to_string())?;
                        activation
                            .token
                            .wmes
                            .iter()
                            .filter_map(|wme_ref| wme_manager.get(&wme_ref.id).cloned())
                            .collect::<Vec<_>>()
                    };

                    debug!(
                        "P-state activation: rule '{}' with {} WMEs and bindings {:?}",
                        rule.id,
                        wme_list.len(),
                        activation.token.bindings
                    );

                    agenda.add_item(
                        rule,
                        wme_list,
                        activation.token.bindings,
                        activation.salience,
                    );
                    fired_activations.insert(activation_key);
                }
            }
        }

        // Handle single-condition rules (alpha-only path, no beta chain)
        self.update_agenda_for_wme_single_condition(&wme_id).await?;

        // Record metrics
        {
            let mut values = self.performance_values.lock().map_err(|e| e.to_string())?;
            values.record_assertion(start.elapsed());
        }

        Ok(())
    }

    /// Retract a fact from the RETE network
    pub async fn retract_fact(&self, wme_id: &str) -> Result<WorkingMemoryElement, String> {
        // Remove from working memory
        let removed_wme = {
            let mut wme_manager = self.wme_manager.lock().map_err(|e| e.to_string())?;
            wme_manager.retract(wme_id)?
        };

        // Update alpha network
        let _affected_alpha_states = {
            let mut alpha_network = self.alpha_network.lock().map_err(|e| e.to_string())?;
            alpha_network.retract_wme(wme_id)
        };

        // Update beta network
        {
            let mut beta_network = self.beta_network.lock().map_err(|e| e.to_string())?;
            beta_network.remove_wme_from_network(wme_id);
        }

        // CRITICAL FIX: Clean up fired_activations that reference this WME
        // This allows rules to fire again if the same fact is re-asserted later
        {
            let mut fired_activations = self.fired_activations.lock().map_err(|e| e.to_string())?;
            fired_activations.retain(|key| !key.contains(wme_id));
        }

        // Update agenda after removal
        self.update_agenda().await?;

        Ok(removed_wme)
    }

    /// Add a rule to the RETE network
    pub async fn add_rule(&self, rule: Rule) -> Result<String, String> {
        // Use a default salience value if not specified in the rule priority
        let salience = rule.priority;

        // Create alpha states for each condition in the rule.
        // Skip __script__ pseudo-predicate conditions — they are evaluated as post-filters
        // on activations, not matched through the alpha/beta network.
        let mut condition_state_ids = Vec::new();
        {
            let mut alpha_network = self.alpha_network.lock().map_err(|e| e.to_string())?;

            for condition in &rule.conditions {
                if condition.predicate == "__script__" {
                    continue; // Script conditions bypass alpha network
                }
                let state_id = alpha_network.get_or_create_state(condition.clone());
                condition_state_ids.push(state_id);
            }
        }

        // Create beta states to join the conditions together
        // Track the terminal state for this rule (where complete matches end up)
        let terminal_state_id: Option<String> = if condition_state_ids.len() > 1 {
            // For multiple conditions, we need to create join states
            let mut current_state_id = condition_state_ids[0].clone();

            for (i, next_condition_id) in condition_state_ids[1..].iter().enumerate() {
                // Create a beta join state between the current state and the next condition
                let join_state_id = {
                    let mut beta_network = self.beta_network.lock().map_err(|e| e.to_string())?;
                    beta_network.get_or_create_join(
                        current_state_id.clone(),
                        next_condition_id.clone(),
                        "default_join".to_string(),
                    )
                };

                // FIX: Connect alpha states to the beta network
                // For the first join, connect both alpha states to the beta state
                if i == 0 {
                    let mut alpha_network = self.alpha_network.lock().map_err(|e| e.to_string())?;
                    // First alpha state feeds left input
                    if let Some(alpha_state) = alpha_network.states.get_mut(&current_state_id) {
                        alpha_state.add_child(join_state_id.clone());
                    }
                    // Second alpha state feeds right input
                    if let Some(alpha_state) = alpha_network.states.get_mut(next_condition_id) {
                        alpha_state.add_child(join_state_id.clone());
                    }
                } else {
                    // For subsequent joins, connect the alpha state to right input
                    let mut alpha_network = self.alpha_network.lock().map_err(|e| e.to_string())?;
                    if let Some(alpha_state) = alpha_network.states.get_mut(next_condition_id) {
                        alpha_state.add_child(join_state_id.clone());
                    }
                }

                current_state_id = join_state_id;
            }
            Some(current_state_id)
        } else if condition_state_ids.len() == 1 {
            // For a single condition, the alpha state is effectively the terminal state
            Some(condition_state_ids[0].clone())
        } else {
            None
        };

        // Mark the terminal beta state as a p-state (Forgy's production terminal).
        // This links the terminal of the beta join chain to its production rule,
        // so activations are created by network topology rather than by scanning.
        // Use the real condition count (excluding __script__) for the check.
        if let Some(ref terminal_id) = terminal_state_id {
            if condition_state_ids.len() > 1 {
                let mut beta_network = self.beta_network.lock().map_err(|e| e.to_string())?;
                beta_network.mark_as_p_state(terminal_id, rule.id.clone(), salience);
            }
        }

        // Add the rule to the production network (with terminal link for retraction path)
        let state_id = {
            let mut production_network =
                self.production_network.lock().map_err(|e| e.to_string())?;
            production_network.add_rule_with_terminal(rule, salience, terminal_state_id)
        };

        Ok(state_id)
    }

    /// Incrementally update the agenda for single-condition rules when a new WME is asserted.
    /// Multi-condition rules are handled by p-state activations from beta network propagation.
    async fn update_agenda_for_wme_single_condition(&self, wme_id: &str) -> Result<(), String> {
        let wme = {
            let wme_manager = self.wme_manager.lock().map_err(|e| e.to_string())?;
            match wme_manager.get(wme_id) {
                Some(w) => w.clone(),
                None => return Ok(()),
            }
        };

        // Predicate-keyed lookup via ProductionNetwork::single_cond_index — only
        // single-cond rules whose condition shares this WME's predicate are
        // candidates. Replaces the full `states.clone()` + 30-rule scan that
        // profiling showed was ~80% of assert_fact's total cost.
        let candidates: Vec<crate::production::SingleCondRuleEntry> = {
            let production_network = self.production_network.lock().map_err(|e| e.to_string())?;
            match production_network.single_cond_index.get(&wme.fact.predicate) {
                Some(entries) => entries.clone(),
                None => return Ok(()),
            }
        };

        if candidates.is_empty() {
            return Ok(());
        }

        let mut fired_activations = self.fired_activations.lock().map_err(|e| e.to_string())?;

        for entry in candidates {
            if !entry.condition.matches(&wme.fact) {
                continue;
            }
            let activation_key = format!("{}:{}", entry.rule.id, wme.id);
            if fired_activations.contains(&activation_key) {
                debug!("Skipping already-fired activation: {}", activation_key);
                continue;
            }

            let mut bindings = crate::variable_binding::Bindings::new();
            for (cond_arg, fact_arg) in entry.condition.args.iter().zip(wme.fact.args.iter()) {
                if cond_arg.starts_with('?') {
                    bindings.add_binding(cond_arg, fact_arg).ok();
                }
            }

            let script_passes = self.evaluate_script_conditions(&entry.rule, &bindings)?;
            if !script_passes {
                debug!(
                    "Single-condition rule '{}' blocked by script condition for WME '{}'",
                    entry.rule.id, wme.id
                );
                continue;
            }

            {
                let mut agenda = self.agenda.lock().map_err(|e| e.to_string())?;
                debug!(
                    "Adding single-condition rule '{}' to agenda for WME '{}'",
                    entry.rule.id, wme.id
                );
                agenda.add_item(entry.rule, vec![wme.clone()], bindings, entry.salience);
            }

            fired_activations.insert(activation_key);
        }

        Ok(())
    }

    /// Update the agenda based on current network state
    /// FIXED: Now handles both single-condition (alpha only) and multi-condition (beta) rules
    /// NOTE: This does a full scan - prefer update_agenda_for_wme() for incremental updates
    pub async fn update_agenda(&self) -> Result<(), String> {
        // tracing::debug imported at module rank

        // Use the persistent fired_activations set
        let mut fired_activations = self.fired_activations.lock().map_err(|e| e.to_string())?;

        // Get all production rules
        let rules = {
            let production_network = self.production_network.lock().map_err(|e| e.to_string())?;
            production_network.states.clone()
        };

        // For each production rule, check if it's satisfied.
        // Use real_condition_count (excluding __script__) to determine single vs. multi path.
        for production_state in &rules {
            let rule = &production_state.rule;
            let real_count = Self::real_condition_count(rule);

            if real_count == 1 {
                // Single real condition rule: check alpha network directly
                let real_conds = Self::real_conditions(rule);
                let condition = real_conds[0];
                let alpha_matches = {
                    let alpha_network = self.alpha_network.lock().map_err(|e| e.to_string())?;
                    alpha_network
                        .get_wmes_by_condition(condition)
                        .into_iter()
                        .cloned()
                        .collect::<Vec<_>>()
                };

                // For each matching WME, create an agenda item
                for wme in alpha_matches {
                    let activation_key = format!("{}:{}", rule.id, wme.id);

                    if fired_activations.contains(&activation_key) {
                        debug!("Skipping duplicate activation: {}", activation_key);
                        continue;
                    }

                    // Build bindings from the WME
                    let mut bindings = crate::variable_binding::Bindings::new();
                    for (cond_arg, fact_arg) in condition.args.iter().zip(wme.fact.args.iter()) {
                        if cond_arg.starts_with('?') {
                            bindings.add_binding(cond_arg, fact_arg).ok();
                        }
                    }

                    // Evaluate __script__ conditions before adding to agenda
                    let script_passes = self.evaluate_script_conditions(rule, &bindings)?;
                    if !script_passes {
                        debug!(
                            "Full-scan: rule '{}' blocked by script condition for WME '{}'",
                            rule.id, wme.id
                        );
                        continue;
                    }

                    let mut agenda = self.agenda.lock().map_err(|e| e.to_string())?;

                    debug!(
                        "Adding single-condition rule '{}' to agenda with bindings {:?}",
                        rule.id, bindings
                    );
                    agenda.add_item(
                        rule.clone(),
                        vec![wme.clone()],
                        bindings,
                        production_state.salience,
                    );

                    fired_activations.insert(activation_key);
                }
            } else if real_count > 1 {
                // Multi-condition rule: use the terminal p-state ID to scope to this rule's tokens
                if let Some(ref terminal_id) = production_state.terminal_state_id {
                    let tokens = {
                        let beta_network = self.beta_network.lock().map_err(|e| e.to_string())?;
                        beta_network
                            .states
                            .get(terminal_id)
                            .map(|state| state.beta_memory.clone())
                            .unwrap_or_default()
                    };

                    for token in &tokens {
                        let wme_ids: Vec<String> =
                            token.wmes.iter().map(|w| w.id.clone()).collect();
                        let activation_key = format!("{}:{}", rule.id, wme_ids.join(","));

                        if fired_activations.contains(&activation_key) {
                            debug!("Skipping duplicate activation: {}", activation_key);
                            continue;
                        }

                        // Evaluate __script__ conditions before adding to agenda
                        let script_passes =
                            self.evaluate_script_conditions(rule, &token.bindings)?;
                        if !script_passes {
                            debug!(
                                "Full-scan multi-condition: rule '{}' blocked by script condition",
                                rule.id
                            );
                            continue;
                        }

                        let wme_list = {
                            let wme_manager = self.wme_manager.lock().map_err(|e| e.to_string())?;
                            token
                                .wmes
                                .iter()
                                .filter_map(|wme_ref| wme_manager.get(&wme_ref.id).cloned())
                                .collect::<Vec<_>>()
                        };

                        let mut agenda = self.agenda.lock().map_err(|e| e.to_string())?;

                        debug!("Adding multi-condition rule '{}' to agenda with {} WMEs and bindings {:?}",
                               rule.id, wme_list.len(), token.bindings);
                        agenda.add_item(
                            rule.clone(),
                            wme_list,
                            token.bindings.clone(),
                            production_state.salience,
                        );

                        fired_activations.insert(activation_key);
                    }
                }
            }
        }

        Ok(())
    }

    /// Evaluate all `__script__` conditions on a rule against current working memory.
    /// Returns `true` if all script conditions pass (or if there are none).
    /// Returns `false` if any script condition blocks the activation.
    fn evaluate_script_conditions(
        &self,
        rule: &Rule,
        bindings: &crate::variable_binding::Bindings,
    ) -> Result<bool, String> {
        // Collect script conditions
        let script_conditions: Vec<&Condition> = rule
            .conditions
            .iter()
            .filter(|c| c.predicate == "__script__" && c.script.is_some())
            .collect();

        if script_conditions.is_empty() {
            return Ok(true); // No script conditions, pass through
        }

        // Get all current facts from working memory
        let facts: Vec<Fact> = {
            let wme_manager = self.wme_manager.lock().map_err(|e| e.to_string())?;
            wme_manager
                .get_all()
                .iter()
                .map(|wme| wme.fact.clone())
                .collect()
        };

        // Convert Bindings to HashMap for the ScriptEvaluator interface
        let bindings_map: std::collections::HashMap<String, String> = bindings.bindings.clone();

        // Evaluate each script condition
        for condition in script_conditions {
            let script = condition
                .script
                .as_ref()
                .ok_or_else(|| format!("Script condition missing script in rule '{}'", rule.id))?;
            match self.script_evaluator.evaluate(script, &facts, &bindings_map) {
                Ok(true) => continue,
                Ok(false) => {
                    debug!("Script condition blocked rule '{}': {}", rule.id, script);
                    return Ok(false);
                }
                Err(e) => {
                    warn!(
                        "Script condition error in rule '{}': {} — treating as blocked",
                        rule.id, e
                    );
                    return Ok(false);
                }
            }
        }

        Ok(true)
    }

    /// Count the number of real (non-script) conditions in a rule
    fn real_condition_count(rule: &Rule) -> usize {
        rule.conditions
            .iter()
            .filter(|c| c.predicate != "__script__")
            .count()
    }

    /// Get the real (non-script) conditions from a rule
    fn real_conditions(rule: &Rule) -> Vec<&Condition> {
        rule.conditions
            .iter()
            .filter(|c| c.predicate != "__script__")
            .collect()
    }

    /// Execute the next agenda item
    pub fn execute_next_agenda_item(&self) -> Result<Vec<Action>, String> {
        let agenda_item = {
            let mut agenda = self.agenda.lock().map_err(|e| e.to_string())?;
            agenda.pop_next()
        };

        match agenda_item {
            Some(item) => {
                let actions = {
                    let production_network =
                        self.production_network.lock().map_err(|e| e.to_string())?;
                    production_network.execute_agenda_item(&item)?
                };

                Ok(actions)
            }
            None => Err("No items in agenda".to_string()),
        }
    }

    /// Execute all agenda items
    pub fn execute_all_agenda_items(&self) -> Result<Vec<Action>, String> {
        let start = Instant::now();

        let mut all_actions = Vec::new();

        while {
            let agenda = self.agenda.lock().map_err(|e| e.to_string())?;
            !agenda.is_empty()
        } {
            all_actions.extend(self.execute_next_agenda_item()?);
        }

        // Record metrics
        {
            let mut values = self.performance_values.lock().map_err(|e| e.to_string())?;
            values.record_evaluation(start.elapsed());
        }

        Ok(all_actions)
    }

    /// Drain the agenda by firing each item through `fire_agenda_item`,
    /// producing `Consequence`s rather than raw `Action`s. New high-rank
    /// entry point for callers that want rule_id + bindings on every fire.
    /// The legacy `execute_all_agenda_items` path stays available.
    pub fn fire_all_consequences(&self) -> Result<Vec<Consequence>, String> {
        let start = Instant::now();
        let mut all = Vec::new();

        loop {
            let next_item = {
                let mut agenda = self.agenda.lock().map_err(|e| e.to_string())?;
                if agenda.is_empty() {
                    break;
                }
                agenda.pop_next()
            };
            let item = match next_item {
                Some(i) => i,
                None => break,
            };

            let consequences = {
                let production_network =
                    self.production_network.lock().map_err(|e| e.to_string())?;
                production_network.fire_agenda_item(&item)?
            };
            all.extend(consequences);
        }

        {
            let mut values = self.performance_values.lock().map_err(|e| e.to_string())?;
            values.record_evaluation(start.elapsed());
        }

        Ok(all)
    }

    /// Get performance valueistics and log them
    pub fn log_performance_values(&self) {
        let rules_count = self
            .production_network
            .lock()
            .map(|pn| pn.get_rules_count())
            .unwrap_or(0);
        let facts_count = self
            .wme_manager
            .lock()
            .map(|wm| wm.get_all().len())
            .unwrap_or(0);
        if let Ok(values) = self.performance_values.lock() {
            values.log_summary(rules_count, facts_count);
        }
    }

    /// Reset per-cycle performance counters
    pub fn reset_cycle_values(&self) {
        if let Ok(mut values) = self.performance_values.lock() {
            values.reset_cycle();
        }
    }

    /// Get a copy of performance valueistics
    pub fn get_performance_values(&self) -> Option<PerformanceValues> {
        self.performance_values
            .lock()
            .ok()
            .map(|s| PerformanceValues {
                total_evaluation_time: s.total_evaluation_time,
                evaluation_count: s.evaluation_count,
                total_assertion_time: s.total_assertion_time,
                assertion_count: s.assertion_count,
                cycle_assertion_count: s.cycle_assertion_count,
                cycle_assertion_time: s.cycle_assertion_time,
                cycle_evaluation_count: s.cycle_evaluation_count,
                cycle_evaluation_time: s.cycle_evaluation_time,
            })
    }

    /// Get all WMEs currently in working memory
    pub async fn get_all_wmes(&self) -> Result<Vec<WorkingMemoryElement>, String> {
        let wme_manager = self.wme_manager.lock().map_err(|e| e.to_string())?;
        Ok(wme_manager.get_all().into_iter().cloned().collect())
    }

    /// Get the number of rules in the RETE network
    pub async fn get_rules_count(&self) -> Result<usize, String> {
        let production_network = self.production_network.lock().map_err(|e| e.to_string())?;
        Ok(production_network.get_rules_count())
    }

    /// Get WMEs matching a specific condition
    pub async fn get_wmes_by_condition(
        &self,
        condition: &Condition,
    ) -> Result<Vec<WorkingMemoryElement>, String> {
        let wme_manager = self.wme_manager.lock().map_err(|e| e.to_string())?;
        Ok(wme_manager
            .get_by_condition(condition)
            .into_iter()
            .cloned()
            .collect())
    }

    /// Get all persistent facts (score, goal state, etc.) for save game
    /// Filters out transient facts like time_advanced, movement_completed, etc.
    pub fn get_persistent_facts(&self) -> Vec<Fact> {
        // Predicates that represent persistent game state
        const PERSISTENT_PREDICATES: &[&str] = &[
            "score_change",
            "player_score_high",
            "player_score_low",
            "milestone_completed",
            "task_started",
            "subtask_completed",
            "module_standing",
            "agent_trust_rank",
            "hidden_debt_found",
            "artifact_generated",
            "goal_reached",
            // 038: Progression & Achievement System
            "contribution_logged",
            "high_rank_task_assigned",
            "platform_selected",
            "policy_violation",
            "policy_compliant_action",
            "compliance_rank",
            "directory_audited",
            "task_failed",
        ];

        if let Ok(wme_manager) = self.wme_manager.lock() {
            wme_manager
                .get_all()
                .into_iter()
                .filter(|wme| PERSISTENT_PREDICATES.contains(&wme.fact.predicate.as_str()))
                .map(|wme| wme.fact.clone())
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Restore persistent facts from a save game (async version)
    pub async fn restore_persistent_facts(&self, facts: Vec<Fact>) -> Result<(), String> {
        for fact in facts {
            self.assert_fact(fact).await?;
        }
        Ok(())
    }

    /// Restore persistent facts from a save game (sync version)
    /// This directly inserts into working memory without triggering rule evaluation
    pub fn restore_persistent_facts_sync(&self, facts: Vec<Fact>) -> Result<(), String> {
        let mut wme_manager = self.wme_manager.lock().map_err(|e| e.to_string())?;
        for fact in facts {
            let wme = WorkingMemoryElement::new(fact);
            wme_manager.assert(wme)?;
        }
        Ok(())
    }

    /// Get all rules currently loaded in the network
    pub fn get_all_rules(&self) -> Result<Vec<Rule>, String> {
        let production_network = self.production_network.lock().map_err(|e| e.to_string())?;
        Ok(production_network
            .states
            .iter()
            .map(|pn| pn.rule.clone())
            .collect())
    }

    /// Get a specific rule by its ID
    pub fn get_rule_by_id(&self, rule_id: &str) -> Result<Option<Rule>, String> {
        let production_network = self.production_network.lock().map_err(|e| e.to_string())?;
        Ok(production_network
            .find_by_rule_id(rule_id)
            .map(|pn| pn.rule.clone()))
    }

    /// Remove a rule from the production network by its ID
    pub fn remove_rule(&self, rule_id: &str) -> Result<(), String> {
        let mut production_network = self.production_network.lock().map_err(|e| e.to_string())?;
        let state_id = production_network
            .rule_index
            .remove(rule_id)
            .ok_or_else(|| format!("Rule '{}' not found", rule_id))?;
        production_network.states.retain(|n| n.id != state_id);
        for entries in production_network.single_cond_index.values_mut() {
            entries.retain(|e| e.rule.id != rule_id);
        }
        production_network
            .single_cond_index
            .retain(|_, entries| !entries.is_empty());
        Ok(())
    }
}

#[cfg(test)]
mod fire_all_consequences_tests {
    use super::*;
    use crate::consequence::Provenance;
    use crate::engine_types::{Action as RuleAction, Condition, Fact, Rule};

    #[tokio::test]
    async fn fire_all_consequences_returns_one_per_fired_action() {
        let net = ReteNetwork::new();
        let rule = Rule {
            id: "test-rule".to_string(),
            priority: 1,
            conditions: vec![Condition {
                predicate: "p".to_string(),
                args: vec!["?x".to_string()],
                script: None,
            }],
            actions: vec![RuleAction {
                action_type: "constraint_violation".to_string(),
                params: vec!["x=?x".to_string()],
            }],
        };
        net.add_rule(rule).await.unwrap();
        net.assert_fact(Fact {
            id: "f1".to_string(),
            predicate: "p".to_string(),
            args: vec!["hello".to_string()],
            timestamp: 0,
        })
        .await
        .unwrap();
        net.update_agenda().await.unwrap();

        let consequences = net.fire_all_consequences().unwrap();
        assert_eq!(consequences.len(), 1);
        let payload = &consequences[0].payload;
        assert_eq!(payload["message"], "x=hello");
        match &consequences[0].provenance {
            Provenance::RuleFiring {
                rule_id, bindings, ..
            } => {
                assert_eq!(rule_id, "test-rule");
                assert_eq!(bindings.get("?x").map(String::as_str), Some("hello"));
            }
            _ => panic!("expected RuleFiring provenance"),
        }
    }

    #[tokio::test]
    async fn fire_all_consequences_empty_when_no_agenda() {
        let net = ReteNetwork::new();
        let consequences = net.fire_all_consequences().unwrap();
        assert!(consequences.is_empty());
    }
}
