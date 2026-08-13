use std::collections::HashMap;
use uuid::Uuid;

use crate::beta_network::BetaNetwork;
use crate::engine_types::Condition;
use crate::error::ReteError;
use crate::ids::StateId;
use crate::variable_binding::{Bindings, Token};
use crate::wme::WorkingMemoryElement;

#[derive(Debug, Clone)]
pub struct AlphaState {
    pub id: String,
    pub condition: Condition,
    pub alpha_memory: Vec<WorkingMemoryElement>, // Store matching WMEs
    pub children: Vec<String>,                   // IDs of connected beta states
    pub shared: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine_types::Fact;

    #[test]
    fn repeated_variable_conflicts_are_returned_and_not_stored() {
        let condition = Condition {
            predicate: "same".to_string(),
            args: vec!["?value".to_string(), "?value".to_string()],
            script: None,
        };
        let mut state = AlphaState::new(condition);
        let wme = WorkingMemoryElement::new(Fact {
            id: "fact".to_string(),
            predicate: "same".to_string(),
            args: vec!["left".to_string(), "right".to_string()],
            timestamp: 0,
            source: None,
        });

        assert!(matches!(
            state.process_wme(&wme),
            Err(ReteError::BindingConflict { .. })
        ));
        assert!(state.alpha_memory.is_empty());
    }
}

impl AlphaState {
    pub fn new(condition: Condition) -> Self {
        AlphaState {
            id: Uuid::new_v4().to_string(),
            condition,
            alpha_memory: Vec::new(),
            children: Vec::new(),
            shared: false,
        }
    }

    /// Add a child beta state to this alpha state
    pub fn add_child(&mut self, child_id: impl Into<StateId>) {
        let child_id = child_id.into().into_inner();
        if !self.children.contains(&child_id) {
            self.children.push(child_id);
        }
    }

    /// Process a WME through this alpha state, returning tokens if it matches
    pub fn process_wme(&mut self, wme: &WorkingMemoryElement) -> Result<Option<Token>, ReteError> {
        // Check if WME matches condition with no existing bindings (first match)
        if self.condition.matches(&wme.fact) {
            let bindings = Bindings::new().can_bind(&self.condition, &wme.fact)?;
            // Add to alpha memory if not already present
            if !self
                .alpha_memory
                .iter()
                .any(|existing| existing.id == wme.id)
            {
                self.alpha_memory.push(wme.clone());
            }

            // Create a token with the matching WME and appropriate bindings
            Ok(Some(Token::new_with_bindings(vec![wme.clone()], bindings)))
        } else {
            Ok(None) // WME did not match condition
        }
    }

    /// Send tokens to child beta states
    pub fn send_tokens_to_children(&self, beta_network: &mut BetaNetwork) -> Result<(), ReteError> {
        // For each WME in alpha memory, create a token and send to child beta states
        for wme in &self.alpha_memory {
            // Create bindings for this WME
            let bindings = Bindings::new().can_bind(&self.condition, &wme.fact)?;

            let token = Token::new_with_bindings(vec![wme.clone()], bindings);
            for child_id in &self.children {
                beta_network.process_token_from_source(child_id, token.clone());
            }
        }
        Ok(())
    }

    /// Remove a WME from alpha memory (for retraction)
    pub fn remove_wme(&mut self, wme_id: &str) -> bool {
        let initial_len = self.alpha_memory.len();
        self.alpha_memory.retain(|wme| wme.id != wme_id);
        self.alpha_memory.len() != initial_len
    }
}

/// Alpha Network contains all alpha states and manages their creation
#[derive(Debug)]
pub struct AlphaNetwork {
    pub states: HashMap<String, AlphaState>,
    pub condition_index: HashMap<String, String>, // Maps condition hash to state ID for sharing
}

impl Default for AlphaNetwork {
    fn default() -> Self {
        Self::new()
    }
}

impl AlphaNetwork {
    pub fn new() -> Self {
        AlphaNetwork {
            states: HashMap::new(),
            condition_index: HashMap::new(),
        }
    }

    /// Get or create an alpha state for a condition (with sharing)
    pub fn get_or_create_state(&mut self, condition: Condition) -> String {
        // Create a hash based on the condition structure
        let condition_hash = format!("{}-{:?}", condition.predicate, condition.args);

        if let Some(state_id) = self.condition_index.get(&condition_hash) {
            // Return existing shared state
            state_id.clone()
        } else {
            // Create new state
            let mut state = AlphaState::new(condition);
            state.shared = true;
            let state_id = state.id.clone();
            self.states.insert(state_id.clone(), state);
            self.condition_index
                .insert(condition_hash, state_id.clone());
            state_id
        }
    }

    /// Process a WME through all alpha states
    pub fn process_wme(
        &mut self,
        wme: &WorkingMemoryElement,
    ) -> Result<Vec<(String, Token)>, ReteError> {
        let mut matching_states = Vec::new();

        for (state_id, state) in self.states.iter_mut() {
            if let Some(token) = state.process_wme(wme)? {
                matching_states.push((state_id.clone(), token));
            }
        }

        Ok(matching_states)
    }

    /// Process a WME through all alpha states and propagate tokens to beta network
    pub fn process_and_propagate_wme(
        &mut self,
        wme: &WorkingMemoryElement,
        beta_network: &mut BetaNetwork,
    ) -> Result<Vec<String>, ReteError> {
        let _matching_state_ids: Vec<String> = Vec::new();

        // Process WMEs to identify matching states and get tokens
        for (_state_id, state) in self.states.iter_mut() {
            if let Some(token) = state.process_wme(wme)? {
                // Send this specific token to connected beta states
                for child_id in &state.children {
                    beta_network.process_token_from_source(child_id, token.clone());
                }
            }
        }

        // Collect all the state IDs that matched
        let mut matching_states = Vec::new();
        for (state_id, state) in self.states.iter() {
            if state.alpha_memory.iter().any(|w| w.id == wme.id) {
                matching_states.push(state_id.clone());
            }
        }

        Ok(matching_states)
    }

    /// Remove a WME from all alpha memories
    pub fn retract_wme(&mut self, wme_id: &str) -> Vec<String> {
        let mut affected_states = Vec::new();

        for (state_id, state) in self.states.iter_mut() {
            if state.remove_wme(wme_id) {
                affected_states.push(state_id.clone());
            }
        }

        affected_states
    }

    /// Pattern match and find WMEs that match a specific condition
    pub fn get_wmes_by_condition(&self, condition: &Condition) -> Vec<&WorkingMemoryElement> {
        // Create a hash based on the condition structure like we do when creating states
        let condition_hash = format!("{}-{:?}", condition.predicate, condition.args);

        if let Some(state_id) = self.condition_index.get(&condition_hash)
            && let Some(state) = self.states.get(state_id)
        {
            return state.alpha_memory.iter().collect();
        }

        // If we don't have a specific state for this condition,
        // return all WMEs that match the condition in any alpha state
        let mut results = Vec::new();
        for state in self.states.values() {
            // Check each WME in the state's memory against the condition
            for wme in &state.alpha_memory {
                if condition.matches(&wme.fact) {
                    results.push(wme);
                }
            }
        }
        results
    }
}
