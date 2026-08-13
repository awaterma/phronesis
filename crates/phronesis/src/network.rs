//! The `ReteNetwork` — the coordinating surface that wires alpha and
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

use crate::agenda::{Agenda, AgendaItem};
use crate::alpha_network::AlphaNetwork;
use crate::beta_network::{BetaNetwork, PStateActivation};
use crate::consequence::Consequence;
use crate::engine_types::{Action, Condition, Fact, PerformanceStats, Rule};
use crate::error::ReteError;
use crate::production::{ProductionNetwork, ProductionState};
use crate::script_evaluator::{BuiltinScriptEvaluator, ScriptEval};
use crate::wme::{WmeManager, WorkingMemoryElement};
use tracing::{debug, warn};

#[derive(Debug)]
pub struct ReteNetwork {
    wme_manager: Arc<Mutex<WmeManager>>,
    alpha_network: Arc<Mutex<AlphaNetwork>>,
    beta_network: Arc<Mutex<BetaNetwork>>,
    agenda: Arc<Mutex<Agenda>>,
    production_network: Arc<Mutex<ProductionNetwork>>,
    /// Performance tracking (interior mutability via Mutex)
    performance_stats: Arc<Mutex<PerformanceStats>>,
    /// Track activations that have already been added to the agenda to avoid duplicates
    fired_activations: Arc<Mutex<HashSet<String>>>,
    /// Script condition evaluator for `__script__` pseudo-predicate conditions (038-xp-progression).
    /// Defaults to [`BuiltinScriptEvaluator`]; swap via [`ReteNetwork::with_script_evaluator`].
    script_evaluator: Box<dyn ScriptEval>,
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
            performance_stats: Arc::new(Mutex::new(PerformanceStats::new())),
            fired_activations: Arc::new(Mutex::new(HashSet::new())),
            script_evaluator: Box::new(BuiltinScriptEvaluator::new()),
        }
    }

    /// Construct a network that evaluates `__script__` conditions with a
    /// custom [`ScriptEval`] implementation instead of the default
    /// [`BuiltinScriptEvaluator`]. All other subsystems are initialized as
    /// in [`ReteNetwork::new`].
    ///
    /// Embedding hosts that need richer guard expressions (numeric
    /// comparisons, boolean combinators over fact arguments) wire in the
    /// `phronesis-rhai` evaluator here.
    pub fn with_script_evaluator(evaluator: Box<dyn ScriptEval>) -> Self {
        ReteNetwork {
            script_evaluator: evaluator,
            ..ReteNetwork::new()
        }
    }

    /// Process a new fact through the RETE network
    pub async fn assert_fact(&self, fact: Fact) -> Result<(), ReteError> {
        let start = Instant::now();

        let wme = WorkingMemoryElement::new(fact);
        let wme_id = wme.id.clone();

        // Add WME to working memory
        {
            let mut wme_manager = self
                .wme_manager
                .lock()
                .map_err(|_| ReteError::poisoned("wme_manager"))?;
            wme_manager.assert(wme)?;
        }

        // Process through alpha network
        let alpha_match_results =
            {
                let mut alpha_network = self
                    .alpha_network
                    .lock()
                    .map_err(|_| ReteError::poisoned("alpha_network"))?;
                let wme_manager = self
                    .wme_manager
                    .lock()
                    .map_err(|_| ReteError::poisoned("wme_manager"))?;
                alpha_network.process_wme(wme_manager.get(&wme_id).ok_or_else(|| {
                    ReteError::Internal("WME missing after assertion".to_string())
                })?)?
            };

        // Process through beta network — p-states create activations directly (Forgy).
        // Hold the beta lock once across all alpha matches instead of re-acquiring
        // per iteration.
        let p_state_activations = {
            let mut beta_network = self
                .beta_network
                .lock()
                .map_err(|_| ReteError::poisoned("beta_network"))?;
            let mut acts = Vec::new();
            for (state_id, token) in alpha_match_results {
                acts.extend(beta_network.process_token_from_source(&state_id, token));
            }
            acts
        };

        if !p_state_activations.is_empty() {
            self.add_p_state_activations(&p_state_activations)?;
        }

        // Handle single-condition rules (alpha-only path, no beta chain)
        self.update_agenda_for_wme_single_condition(&wme_id).await?;

        // Record metrics
        {
            let mut values = self
                .performance_stats
                .lock()
                .map_err(|_| ReteError::poisoned("performance_stats"))?;
            values.record_assertion(start.elapsed());
        }

        Ok(())
    }

    /// Add p-state activations to the agenda.  Holds `fired_activations`,
    /// `production_network`, and `agenda` for the whole loop to avoid
    /// repeated lock acquisition; `wme_manager` is locked per-iteration
    /// because `evaluate_script_conditions` re-enters it and
    /// `std::sync::Mutex` is not reentrant.
    fn add_p_state_activations(&self, activations: &[PStateActivation]) -> Result<(), ReteError> {
        let mut fired_activations = self
            .fired_activations
            .lock()
            .map_err(|_| ReteError::poisoned("fired_activations"))?;
        let production_network = self
            .production_network
            .lock()
            .map_err(|_| ReteError::poisoned("production_network"))?;
        let mut agenda = self
            .agenda
            .lock()
            .map_err(|_| ReteError::poisoned("agenda"))?;

        for activation in activations {
            let wme_ids: Vec<String> = activation.token.wmes.iter().map(|w| w.id.clone()).collect();
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
                if !self.evaluate_script_conditions(&rule, &activation.token.bindings)? {
                    debug!(
                        "P-state activation blocked by script condition: rule '{}'",
                        rule.id
                    );
                    continue;
                }

                let wme_list = self.activation_wmes(activation)?;

                debug!(
                    "P-state activation: rule '{}' with {} WMEs and bindings {:?}",
                    rule.id,
                    wme_list.len(),
                    activation.token.bindings
                );

                agenda.add_item(
                    rule,
                    wme_list,
                    activation.token.bindings.clone(),
                    activation.salience,
                );
                fired_activations.insert(activation_key);
            }
        }
        Ok(())
    }

    fn activation_wmes(
        &self,
        activation: &PStateActivation,
    ) -> Result<Vec<WorkingMemoryElement>, ReteError> {
        let manager = self
            .wme_manager
            .lock()
            .map_err(|_| ReteError::poisoned("wme_manager"))?;
        Ok(activation
            .token
            .wmes
            .iter()
            .filter_map(|reference| manager.get(&reference.id).cloned())
            .collect())
    }

    /// Retract a fact from the RETE network
    pub async fn retract_fact(&self, wme_id: &str) -> Result<WorkingMemoryElement, ReteError> {
        // Remove from working memory
        let removed_wme = {
            let mut wme_manager = self
                .wme_manager
                .lock()
                .map_err(|_| ReteError::poisoned("wme_manager"))?;
            wme_manager.retract(wme_id)?
        };

        // Update alpha network
        let _affected_alpha_states = {
            let mut alpha_network = self
                .alpha_network
                .lock()
                .map_err(|_| ReteError::poisoned("alpha_network"))?;
            alpha_network.retract_wme(wme_id)
        };

        // Update beta network
        {
            let mut beta_network = self
                .beta_network
                .lock()
                .map_err(|_| ReteError::poisoned("beta_network"))?;
            beta_network.remove_wme_from_network(wme_id);
        }

        // Clean up fired_activations that reference this WME so rules can
        // fire again if the same fact is re-asserted later. Keys have the
        // shape "rule_id:wme1,wme2,..." — compare exact id components, not
        // substrings: retracting "f1" must not clobber the key for "f10".
        {
            let mut fired_activations = self
                .fired_activations
                .lock()
                .map_err(|_| ReteError::poisoned("fired_activations"))?;
            fired_activations.retain(|key| match key.split_once(':') {
                Some((_, wmes)) => !wmes.split(',').any(|w| w == wme_id),
                None => true,
            });
        }

        // Purge pending agenda items that reference the retracted WME — a
        // stale activation must not fire with a fact that is no longer true.
        {
            let mut agenda = self
                .agenda
                .lock()
                .map_err(|_| ReteError::poisoned("agenda"))?;
            agenda.remove_by_condition(|item| item.wme_list.iter().any(|w| w.id == wme_id));
        }

        // Update agenda after removal
        self.update_agenda().await?;

        Ok(removed_wme)
    }

    /// Add a rule to the RETE network
    pub async fn add_rule(&self, rule: Rule) -> Result<String, ReteError> {
        let salience = rule.priority;
        let condition_state_ids = self.create_condition_states(&rule)?;
        let terminal_state_id = self.create_join_chain(&condition_state_ids)?;

        // Mark the terminal beta state as a p-state (Forgy's production terminal).
        // This links the terminal of the beta join chain to its production rule,
        // so activations are created by network topology rather than by scanning.
        // Use the real condition count (excluding __script__) for the check.
        if let Some(ref terminal_id) = terminal_state_id
            && condition_state_ids.len() > 1
        {
            let mut beta_network = self
                .beta_network
                .lock()
                .map_err(|_| ReteError::poisoned("beta_network"))?;
            beta_network.mark_as_p_state(terminal_id, rule.id.clone(), salience);
        }

        // Add the rule to the production network (with terminal link for retraction path)
        let state_id = {
            let mut production_network = self
                .production_network
                .lock()
                .map_err(|_| ReteError::poisoned("production_network"))?;
            production_network.add_rule_with_terminal(rule, salience, terminal_state_id)
        };

        Ok(state_id)
    }

    fn create_condition_states(&self, rule: &Rule) -> Result<Vec<String>, ReteError> {
        let mut alpha_network = self
            .alpha_network
            .lock()
            .map_err(|_| ReteError::poisoned("alpha_network"))?;
        Ok(rule
            .conditions
            .iter()
            .filter(|condition| condition.predicate != "__script__")
            .map(|condition| alpha_network.get_or_create_state(condition.clone()))
            .collect())
    }

    fn create_join_chain(&self, state_ids: &[String]) -> Result<Option<String>, ReteError> {
        let Some(first) = state_ids.first() else {
            return Ok(None);
        };
        let mut current = first.clone();
        for (index, next) in state_ids[1..].iter().enumerate() {
            let join = self.create_join(&current, next)?;
            self.connect_alpha_to_join(&current, next, &join, index == 0)?;
            current = join;
        }
        Ok(Some(current))
    }

    fn create_join(&self, left: &str, right: &str) -> Result<String, ReteError> {
        let mut beta = self
            .beta_network
            .lock()
            .map_err(|_| ReteError::poisoned("beta_network"))?;
        Ok(beta.get_or_create_join(
            left.to_string(),
            right.to_string(),
            "default_join".to_string(),
        ))
    }

    fn connect_alpha_to_join(
        &self,
        left: &str,
        right: &str,
        join: &str,
        connect_left: bool,
    ) -> Result<(), ReteError> {
        let mut alpha = self
            .alpha_network
            .lock()
            .map_err(|_| ReteError::poisoned("alpha_network"))?;
        if connect_left && let Some(state) = alpha.states.get_mut(left) {
            state.add_child(join.to_string());
        }
        if let Some(state) = alpha.states.get_mut(right) {
            state.add_child(join.to_string());
        }
        Ok(())
    }

    /// Incrementally update the agenda for single-condition rules when a new WME is asserted.
    /// Multi-condition rules are handled by p-state activations from beta network propagation.
    async fn update_agenda_for_wme_single_condition(&self, wme_id: &str) -> Result<(), ReteError> {
        let wme = {
            let wme_manager = self
                .wme_manager
                .lock()
                .map_err(|_| ReteError::poisoned("wme_manager"))?;
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
            let production_network = self
                .production_network
                .lock()
                .map_err(|_| ReteError::poisoned("production_network"))?;
            match production_network
                .single_cond_index
                .get(&wme.fact.predicate)
            {
                Some(entries) => entries.clone(),
                None => return Ok(()),
            }
        };

        if candidates.is_empty() {
            return Ok(());
        }

        let mut fired_activations = self
            .fired_activations
            .lock()
            .map_err(|_| ReteError::poisoned("fired_activations"))?;

        for entry in candidates {
            if !entry.condition.matches(&wme.fact) {
                continue;
            }
            let activation_key = format!("{}:{}", entry.rule.id, wme.id);
            if fired_activations.contains(&activation_key) {
                debug!("Skipping already-fired activation: {}", activation_key);
                continue;
            }

            let bindings =
                crate::variable_binding::Bindings::new().can_bind(&entry.condition, &wme.fact)?;

            let script_passes = self.evaluate_script_conditions(&entry.rule, &bindings)?;
            if !script_passes {
                debug!(
                    "Single-condition rule '{}' blocked by script condition for WME '{}'",
                    entry.rule.id, wme.id
                );
                continue;
            }

            {
                let mut agenda = self
                    .agenda
                    .lock()
                    .map_err(|_| ReteError::poisoned("agenda"))?;
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
    pub async fn update_agenda(&self) -> Result<(), ReteError> {
        // tracing::debug imported at module level

        // Use the persistent fired_activations set
        let mut fired_activations = self
            .fired_activations
            .lock()
            .map_err(|_| ReteError::poisoned("fired_activations"))?;

        // Get all production rules
        let rules = {
            let production_network = self
                .production_network
                .lock()
                .map_err(|_| ReteError::poisoned("production_network"))?;
            production_network.states.clone()
        };

        // For each production rule, check if it's satisfied.
        // Use real_condition_count (excluding __script__) to determine single vs. multi path.
        for production_state in &rules {
            let rule = &production_state.rule;
            let real_count = Self::real_condition_count(rule);

            if real_count == 1 {
                self.update_single_condition_agenda(production_state, &mut fired_activations)?;
            } else if real_count == 0 {
                self.update_pure_script_agenda(production_state, &mut fired_activations)?;
            } else if real_count > 1 {
                self.update_multi_condition_agenda(production_state, &mut fired_activations)?;
            }
        }

        Ok(())
    }

    fn update_single_condition_agenda(
        &self,
        state: &ProductionState,
        fired: &mut HashSet<String>,
    ) -> Result<(), ReteError> {
        let rule = &state.rule;
        let condition = Self::real_conditions(rule)[0];
        let matches = self
            .alpha_network
            .lock()
            .map_err(|_| ReteError::poisoned("alpha_network"))?
            .get_wmes_by_condition(condition)
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();

        for wme in matches {
            let activation_key = format!("{}:{}", rule.id, wme.id);
            if fired.contains(&activation_key) {
                debug!("Skipping duplicate activation: {}", activation_key);
                continue;
            }
            let bindings =
                crate::variable_binding::Bindings::new().can_bind(condition, &wme.fact)?;
            if !self.evaluate_script_conditions(rule, &bindings)? {
                debug!(
                    "Full-scan: rule '{}' blocked by script condition for WME '{}'",
                    rule.id, wme.id
                );
                continue;
            }
            debug!(
                "Adding single-condition rule '{}' to agenda with bindings {:?}",
                rule.id, bindings
            );
            self.agenda
                .lock()
                .map_err(|_| ReteError::poisoned("agenda"))?
                .add_item(rule.clone(), vec![wme], bindings, state.salience);
            fired.insert(activation_key);
        }
        Ok(())
    }

    fn update_pure_script_agenda(
        &self,
        state: &ProductionState,
        fired: &mut HashSet<String>,
    ) -> Result<(), ReteError> {
        let rule = &state.rule;
        if rule.conditions.is_empty() {
            return Ok(());
        }
        let activation_key = format!("{}:", rule.id);
        if fired.contains(&activation_key) {
            debug!(
                "Skipping duplicate pure-script activation: {}",
                activation_key
            );
            return Ok(());
        }
        let bindings = crate::variable_binding::Bindings::new();
        if !self.evaluate_script_conditions(rule, &bindings)? {
            debug!("Pure-script rule '{}' blocked by script condition", rule.id);
            return Ok(());
        }
        debug!(
            "Adding pure-script rule '{}' to agenda (no WMEs, empty bindings)",
            rule.id
        );
        self.agenda
            .lock()
            .map_err(|_| ReteError::poisoned("agenda"))?
            .add_item(rule.clone(), Vec::new(), bindings, state.salience);
        fired.insert(activation_key);
        Ok(())
    }

    fn update_multi_condition_agenda(
        &self,
        state: &ProductionState,
        fired: &mut HashSet<String>,
    ) -> Result<(), ReteError> {
        let Some(terminal_id) = state.terminal_state_id.as_ref() else {
            return Ok(());
        };
        let tokens = self
            .beta_network
            .lock()
            .map_err(|_| ReteError::poisoned("beta_network"))?
            .states
            .get(terminal_id)
            .map(|terminal| terminal.beta_memory.clone())
            .unwrap_or_default();
        for token in tokens {
            let activation_key = format!(
                "{}:{}",
                state.rule.id,
                token
                    .wmes
                    .iter()
                    .map(|wme| wme.id.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            );
            if fired.contains(&activation_key) {
                debug!("Skipping duplicate activation: {}", activation_key);
                continue;
            }
            if !self.evaluate_script_conditions(&state.rule, &token.bindings)? {
                debug!(
                    "Full-scan multi-condition: rule '{}' blocked by script condition",
                    state.rule.id
                );
                continue;
            }
            let manager = self
                .wme_manager
                .lock()
                .map_err(|_| ReteError::poisoned("wme_manager"))?;
            let wmes = token
                .wmes
                .iter()
                .filter_map(|reference| manager.get(&reference.id).cloned())
                .collect::<Vec<_>>();
            drop(manager);
            debug!(
                "Adding multi-condition rule '{}' to agenda with {} WMEs and bindings {:?}",
                state.rule.id,
                wmes.len(),
                token.bindings
            );
            self.agenda
                .lock()
                .map_err(|_| ReteError::poisoned("agenda"))?
                .add_item(state.rule.clone(), wmes, token.bindings, state.salience);
            fired.insert(activation_key);
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
    ) -> Result<bool, ReteError> {
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
            let wme_manager = self
                .wme_manager
                .lock()
                .map_err(|_| ReteError::poisoned("wme_manager"))?;
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
                .ok_or_else(|| ReteError::ScriptMissing {
                    rule_id: rule.id.clone(),
                })?;
            match self
                .script_evaluator
                .evaluate(script, &facts, &bindings_map)
            {
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

    /// Execute the next agenda item. Internal building block for
    /// [`execute_all_agenda_items`](Self::execute_all_agenda_items); the
    /// public single-step surface is [`execute_next_agenda_item`] behind the
    /// `embedding-host` feature.
    fn execute_next_agenda_item_inner(&self) -> Result<Vec<Action>, ReteError> {
        let agenda_item = {
            let mut agenda = self
                .agenda
                .lock()
                .map_err(|_| ReteError::poisoned("agenda"))?;
            agenda.pop_next()
        };

        match agenda_item {
            Some(item) => {
                let actions = {
                    let production_network = self
                        .production_network
                        .lock()
                        .map_err(|_| ReteError::poisoned("production_network"))?;
                    production_network.execute_agenda_item(&item)?
                };

                Ok(actions)
            }
            None => Err(ReteError::EmptyAgenda),
        }
    }

    /// Execute the next agenda item (single-step agenda drive).
    ///
    /// Requires the `embedding-host` feature; not consumed by the bundled
    /// MCP, which drives the agenda via
    /// [`execute_all_agenda_items`](Self::execute_all_agenda_items).
    #[cfg(feature = "embedding-host")]
    pub fn execute_next_agenda_item(&self) -> Result<Vec<Action>, ReteError> {
        self.execute_next_agenda_item_inner()
    }

    /// Execute all agenda items
    pub fn execute_all_agenda_items(&self) -> Result<Vec<Action>, ReteError> {
        let start = Instant::now();

        let mut all_actions = Vec::new();

        while {
            let agenda = self
                .agenda
                .lock()
                .map_err(|_| ReteError::poisoned("agenda"))?;
            !agenda.is_empty()
        } {
            all_actions.extend(self.execute_next_agenda_item_inner()?);
        }

        // Record metrics
        {
            let mut values = self
                .performance_stats
                .lock()
                .map_err(|_| ReteError::poisoned("performance_stats"))?;
            values.record_evaluation(start.elapsed());
        }

        Ok(all_actions)
    }

    /// Drain the agenda by firing each item through `fire_agenda_item`,
    /// producing `Consequence`s rather than raw `Action`s. New high-level
    /// entry point for callers that want rule_id + bindings on every fire.
    /// The legacy `execute_all_agenda_items` path stays available.
    pub fn fire_all_consequences(&self) -> Result<Vec<Consequence>, ReteError> {
        let start = Instant::now();
        let mut all = Vec::new();

        loop {
            let next_item = {
                let mut agenda = self
                    .agenda
                    .lock()
                    .map_err(|_| ReteError::poisoned("agenda"))?;
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
                let production_network = self
                    .production_network
                    .lock()
                    .map_err(|_| ReteError::poisoned("production_network"))?;
                production_network.fire_agenda_item(&item)?
            };
            all.extend(consequences);
        }

        {
            let mut values = self
                .performance_stats
                .lock()
                .map_err(|_| ReteError::poisoned("performance_stats"))?;
            values.record_evaluation(start.elapsed());
        }

        Ok(all)
    }

    /// Get performance statistics and log them.
    ///
    /// Requires the `embedding-host` feature; not consumed by the bundled MCP.
    #[cfg(feature = "embedding-host")]
    pub fn log_performance_stats(&self) {
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
        if let Ok(values) = self.performance_stats.lock() {
            values.log_summary(rules_count, facts_count);
        }
    }

    /// Reset per-cycle performance counters.
    ///
    /// Requires the `embedding-host` feature; not consumed by the bundled MCP.
    #[cfg(feature = "embedding-host")]
    pub fn reset_cycle_values(&self) {
        if let Ok(mut values) = self.performance_stats.lock() {
            values.reset_cycle();
        }
    }

    /// Get a copy of performance statistics.
    ///
    /// Requires the `embedding-host` feature; not consumed by the bundled MCP.
    #[cfg(feature = "embedding-host")]
    pub fn get_performance_stats(&self) -> Option<PerformanceStats> {
        self.performance_stats
            .lock()
            .ok()
            .map(|s| PerformanceStats {
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
    pub async fn get_all_wmes(&self) -> Result<Vec<WorkingMemoryElement>, ReteError> {
        let wme_manager = self
            .wme_manager
            .lock()
            .map_err(|_| ReteError::poisoned("wme_manager"))?;
        Ok(wme_manager.get_all().into_iter().cloned().collect())
    }

    /// Get the number of rules in the RETE network.
    ///
    /// Requires the `embedding-host` feature; not consumed by the bundled MCP.
    #[cfg(feature = "embedding-host")]
    pub async fn get_rules_count(&self) -> Result<usize, ReteError> {
        let production_network = self
            .production_network
            .lock()
            .map_err(|_| ReteError::poisoned("production_network"))?;
        Ok(production_network.get_rules_count())
    }

    /// Get WMEs matching a specific condition.
    ///
    /// Requires the `embedding-host` feature; not consumed by the bundled MCP.
    #[cfg(feature = "embedding-host")]
    pub async fn get_wmes_by_condition(
        &self,
        condition: &Condition,
    ) -> Result<Vec<WorkingMemoryElement>, ReteError> {
        let wme_manager = self
            .wme_manager
            .lock()
            .map_err(|_| ReteError::poisoned("wme_manager"))?;
        Ok(wme_manager
            .get_by_condition(condition)
            .into_iter()
            .cloned()
            .collect())
    }

    // ===== Public fact-query surface (v0.11) =====
    //
    // Sync snapshot queries over working memory. These exist so embedding
    // hosts never need to reach into `wme_manager` directly; the shapes
    // cover the common embedding-host needs (snapshot-all, predicate filter,
    // positional-arg filters, id collection for batch retraction, by-id).
    // Results are owned clones sorted by fact id — deterministic output
    // for hosts that replay-test against recorded sessions.

    /// Snapshot every fact currently in working memory, sorted by fact id.
    pub fn facts_snapshot(&self) -> Result<Vec<Fact>, ReteError> {
        let wme_manager = self
            .wme_manager
            .lock()
            .map_err(|_| ReteError::poisoned("wme_manager"))?;
        let mut facts: Vec<Fact> = wme_manager
            .get_all()
            .into_iter()
            .map(|wme| wme.fact.clone())
            .collect();
        facts.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(facts)
    }

    /// All facts with the given predicate, sorted by fact id.
    ///
    /// Requires the `embedding-host` feature; not consumed by the bundled
    /// MCP (use [`facts_matching`](Self::facts_matching) with an empty filter,
    /// or [`facts_matching_predicates`](Self::facts_matching_predicates)).
    #[cfg(feature = "embedding-host")]
    pub fn facts_matching_predicate(&self, predicate: &str) -> Result<Vec<Fact>, ReteError> {
        self.facts_matching(predicate, &[])
    }

    /// All facts whose predicate is in `predicates` (set membership), sorted
    /// by fact id. Duplicate predicates in the input do not duplicate facts.
    /// The generic, caller-owned way to say "give me the facts I treat as a
    /// category" — the caller owns the predicate set.
    pub fn facts_matching_predicates(&self, predicates: &[&str]) -> Result<Vec<Fact>, ReteError> {
        let wme_manager = self
            .wme_manager
            .lock()
            .map_err(|_| ReteError::poisoned("wme_manager"))?;
        let mut facts: Vec<Fact> = predicates
            .iter()
            .flat_map(|p| wme_manager.get_by_predicate(p))
            .map(|wme| wme.fact.clone())
            .collect();
        facts.sort_by(|a, b| a.id.cmp(&b.id));
        facts.dedup_by(|a, b| a.id == b.id);
        Ok(facts)
    }

    /// Facts with the given predicate whose args match every
    /// `(position, required value)` filter exactly. A filter position
    /// beyond a fact's arg list never matches. An empty filter list
    /// matches every fact with the predicate.
    pub fn facts_matching(
        &self,
        predicate: &str,
        arg_filters: &[(usize, &str)],
    ) -> Result<Vec<Fact>, ReteError> {
        let wme_manager = self
            .wme_manager
            .lock()
            .map_err(|_| ReteError::poisoned("wme_manager"))?;
        let mut facts: Vec<Fact> = wme_manager
            .get_by_predicate(predicate)
            .into_iter()
            .filter(|wme| {
                arg_filters
                    .iter()
                    .all(|(idx, value)| wme.fact.args.get(*idx).map(String::as_str) == Some(*value))
            })
            .map(|wme| wme.fact.clone())
            .collect();
        facts.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(facts)
    }

    /// Ids of facts matching the predicate + arg filters — the shape
    /// batch retraction wants: collect ids, then `retract_fact` each.
    ///
    /// Requires the `embedding-host` feature; not consumed by the bundled MCP.
    #[cfg(feature = "embedding-host")]
    pub fn fact_ids_matching(
        &self,
        predicate: &str,
        arg_filters: &[(usize, &str)],
    ) -> Result<Vec<String>, ReteError> {
        Ok(self
            .facts_matching(predicate, arg_filters)?
            .into_iter()
            .map(|f| f.id)
            .collect())
    }

    /// The fact with the given id, if present.
    pub fn get_fact_by_id(&self, fact_id: &str) -> Result<Option<Fact>, ReteError> {
        let wme_manager = self
            .wme_manager
            .lock()
            .map_err(|_| ReteError::poisoned("wme_manager"))?;
        Ok(wme_manager.get(fact_id).map(|wme| wme.fact.clone()))
    }

    /// Number of facts currently in working memory.
    ///
    /// Requires the `embedding-host` feature; not consumed by the bundled MCP.
    #[cfg(feature = "embedding-host")]
    pub fn fact_count(&self) -> Result<usize, ReteError> {
        let wme_manager = self
            .wme_manager
            .lock()
            .map_err(|_| ReteError::poisoned("wme_manager"))?;
        Ok(wme_manager.len())
    }

    /// Snapshot pending activations in deterministic firing order.
    ///
    /// The returned values are detached from the engine, so callers cannot
    /// hold an internal agenda lock or mutate scheduling state.
    pub fn agenda_snapshot(&self) -> Result<Vec<AgendaItem>, ReteError> {
        Ok(self
            .agenda
            .lock()
            .map_err(|_| ReteError::poisoned("agenda"))?
            .get_all_items()
            .into_iter()
            .cloned()
            .collect())
    }

    /// Bulk-assert a batch of facts (async version).
    ///
    /// Each fact goes through the full [`assert_fact`](Self::assert_fact)
    /// path (rule evaluation fires). Embedding hosts use this to rehydrate a
    /// previously-snapshotted fact set — pair with
    /// [`facts_matching_predicates`](Self::facts_matching_predicates) to take
    /// the snapshot.
    ///
    /// Requires the `embedding-host` feature; not consumed by the bundled MCP.
    #[cfg(feature = "embedding-host")]
    pub async fn restore_persistent_facts(&self, facts: Vec<Fact>) -> Result<(), ReteError> {
        for fact in facts {
            self.assert_fact(fact).await?;
        }
        Ok(())
    }

    /// Bulk-insert a batch of facts directly into working memory (sync
    /// version), without triggering rule evaluation.
    ///
    /// Requires the `embedding-host` feature; not consumed by the bundled MCP.
    #[cfg(feature = "embedding-host")]
    pub fn restore_persistent_facts_sync(&self, facts: Vec<Fact>) -> Result<(), ReteError> {
        let mut wme_manager = self
            .wme_manager
            .lock()
            .map_err(|_| ReteError::poisoned("wme_manager"))?;
        for fact in facts {
            let wme = WorkingMemoryElement::new(fact);
            wme_manager.assert(wme)?;
        }
        Ok(())
    }

    /// Get all rules currently loaded in the network
    pub fn get_all_rules(&self) -> Result<Vec<Rule>, ReteError> {
        let production_network = self
            .production_network
            .lock()
            .map_err(|_| ReteError::poisoned("production_network"))?;
        Ok(production_network
            .states
            .iter()
            .map(|pn| pn.rule.clone())
            .collect())
    }

    /// Get a specific rule by its ID
    pub fn get_rule_by_id(&self, rule_id: &str) -> Result<Option<Rule>, ReteError> {
        let production_network = self
            .production_network
            .lock()
            .map_err(|_| ReteError::poisoned("production_network"))?;
        Ok(production_network
            .find_by_rule_id(rule_id)
            .map(|pn| pn.rule.clone()))
    }

    /// Remove a rule from the production network by its ID
    pub fn remove_rule(&self, rule_id: &str) -> Result<(), ReteError> {
        let mut production_network = self
            .production_network
            .lock()
            .map_err(|_| ReteError::poisoned("production_network"))?;
        let state_id = production_network
            .rule_index
            .remove(rule_id)
            .ok_or_else(|| ReteError::RuleNotFound(rule_id.to_string()))?;
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
            source: None,
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

    /// Cover the multi-join chain branch in `add_rule` (three leaf
    /// conditions → two join states; the "subsequent joins" alpha→beta
    /// wiring path on line ~332).
    #[tokio::test]
    async fn three_leaf_condition_rule_fires_via_join_chain() {
        let net = ReteNetwork::new();
        let rule = Rule {
            id: "three-conds".to_string(),
            priority: 0,
            conditions: vec![
                Condition {
                    predicate: "a".to_string(),
                    args: vec!["?x".to_string()],
                    script: None,
                },
                Condition {
                    predicate: "b".to_string(),
                    args: vec!["?x".to_string()],
                    script: None,
                },
                Condition {
                    predicate: "c".to_string(),
                    args: vec!["?x".to_string()],
                    script: None,
                },
            ],
            actions: vec![RuleAction {
                action_type: "constraint_warning".to_string(),
                params: vec!["x=?x".to_string()],
            }],
        };
        net.add_rule(rule).await.unwrap();
        for (i, p) in ["a", "b", "c"].iter().enumerate() {
            net.assert_fact(Fact {
                id: format!("f{i}"),
                predicate: p.to_string(),
                args: vec!["v".to_string()],
                timestamp: 0,
                source: None,
            })
            .await
            .unwrap();
        }
        net.update_agenda().await.unwrap();
        let consequences = net.fire_all_consequences().unwrap();
        assert_eq!(consequences.len(), 1, "three-cond rule fires on join chain");
    }
}
