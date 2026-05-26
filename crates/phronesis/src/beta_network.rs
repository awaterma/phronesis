use std::collections::HashMap;
use uuid::Uuid;

use crate::ids::{RuleId, StateId};
use crate::variable_binding::Token;

/// P-state metadata — marks this beta state as a production terminal (Forgy's p-state).
/// In Forgy's RETE, the p-state sits at the terminal of the beta chain and knows
/// which production it belongs to. When a token reaches a p-state, it immediately
/// becomes an instantiation in the conflict set.
#[derive(Debug, Clone)]
pub struct PStateInfo {
    pub rule_id: RuleId,
    pub salience: i32,
}

/// Activation produced when a token reaches a p-state (Forgy's instantiation).
/// These are collected during beta network propagation and added to the agenda.
#[derive(Debug, Clone)]
pub struct PStateActivation {
    pub rule_id: RuleId,
    pub token: Token,
    pub salience: i32,
}

#[derive(Debug, Clone)]
pub struct BetaState {
    pub id: String,
    pub left_input: Vec<String>, // State IDs from alpha or other beta states
    pub right_input: Vec<String>, // State IDs from alpha or other beta states
    pub beta_memory: Vec<Token>, // Complete tokens (full rule matches)
    pub children: Vec<String>,   // State IDs of child states
    pub join_conditions: Vec<String>, // Describes how left and right sides join
    pub left_memory: Vec<Token>, // Left side tokens
    pub right_memory: Vec<Token>, // Right side tokens
    /// P-state metadata — None for intermediate join states, Some for terminals (Forgy's p-state)
    pub p_state: Option<PStateInfo>,
}

impl Default for BetaState {
    fn default() -> Self {
        Self::new()
    }
}

impl BetaState {
    pub fn new() -> Self {
        BetaState {
            id: Uuid::new_v4().to_string(),
            left_input: Vec::new(),
            right_input: Vec::new(),
            beta_memory: Vec::new(),
            children: Vec::new(),
            join_conditions: Vec::new(),
            left_memory: Vec::new(),
            right_memory: Vec::new(),
            p_state: None,
        }
    }

    /// Mark this beta state as a p-state (Forgy's production terminal)
    pub fn set_p_state(&mut self, rule_id: impl Into<RuleId>, salience: i32) {
        self.p_state = Some(PStateInfo {
            rule_id: rule_id.into(),
            salience,
        });
    }

    /// Add a left input source (from alpha or beta state)
    pub fn add_left_input(&mut self, input_id: impl Into<StateId>) {
        let input_id = input_id.into().into_inner();
        if !self.left_input.contains(&input_id) {
            self.left_input.push(input_id);
        }
    }

    /// Add a right input source (from alpha or beta state)
    pub fn add_right_input(&mut self, input_id: impl Into<StateId>) {
        let input_id = input_id.into().into_inner();
        if !self.right_input.contains(&input_id) {
            self.right_input.push(input_id);
        }
    }

    /// Add a child state
    pub fn add_child(&mut self, child_id: impl Into<StateId>) {
        let child_id = child_id.into().into_inner();
        if !self.children.contains(&child_id) {
            self.children.push(child_id);
        }
    }

    /// Add join condition
    pub fn add_join_condition(&mut self, condition: String) {
        self.join_conditions.push(condition);
    }

    /// Process a token from the left input
    /// Returns newly created tokens that should be propagated to children
    pub fn process_left_token(&mut self, token: Token) -> Vec<Token> {
        // Store in left memory
        self.left_memory.push(token.clone());

        // Try to join with right memory and collect new tokens
        let mut new_tokens = Vec::new();
        for right_token in &self.right_memory {
            if let Some(joined_token) = self.join_tokens(&token, right_token) {
                self.beta_memory.push(joined_token.clone());
                new_tokens.push(joined_token);
            }
        }
        new_tokens
    }

    /// Process a token from the right input
    /// Returns newly created tokens that should be propagated to children
    pub fn process_right_token(&mut self, token: Token) -> Vec<Token> {
        // Store in right memory
        self.right_memory.push(token.clone());

        // Try to join with left memory and collect new tokens
        let mut new_tokens = Vec::new();
        for left_token in &self.left_memory {
            if let Some(joined_token) = self.join_tokens(left_token, &token) {
                self.beta_memory.push(joined_token.clone());
                new_tokens.push(joined_token);
            }
        }
        new_tokens
    }

    /// Attempt to join two tokens based on variable bindings
    fn join_tokens(&self, left: &Token, right: &Token) -> Option<Token> {
        // Try to merge the bindings from both tokens
        match left.bindings.merge(&right.bindings) {
            Ok(merged_bindings) => {
                // If bindings are consistent, combine the WMEs
                let mut combined_wmes = left.wmes.clone();
                combined_wmes.extend(right.wmes.clone());

                Some(Token::new_with_bindings(combined_wmes, merged_bindings))
            }
            Err(_) => {
                // Bindings conflict, cannot join these tokens
                None
            }
        }
    }

    /// Remove a WME from tokens in memory
    pub fn remove_wme_from_tokens(&mut self, wme_id: &str) {
        // Remove from left memory
        self.left_memory
            .retain(|token| !token.wmes.iter().any(|wme| wme.id == wme_id));

        // Remove from right memory
        self.right_memory
            .retain(|token| !token.wmes.iter().any(|wme| wme.id == wme_id));

        // Remove from beta memory
        self.beta_memory
            .retain(|token| !token.wmes.iter().any(|wme| wme.id == wme_id));
    }
}

/// Beta Network manages all beta states and their connections
#[derive(Debug)]
pub struct BetaNetwork {
    pub states: HashMap<String, BetaState>,
    /// Index to track join patterns for sharing
    join_index: HashMap<String, String>, // Maps join pattern hash to state ID
}

impl Default for BetaNetwork {
    fn default() -> Self {
        Self::new()
    }
}

impl BetaNetwork {
    pub fn new() -> Self {
        BetaNetwork {
            states: HashMap::new(),
            join_index: HashMap::new(),
        }
    }

    /// Create a new beta state and return its ID
    pub fn create_state(&mut self) -> String {
        let state = BetaState::new();
        let state_id = state.id.clone();
        self.states.insert(state_id.clone(), state);
        state_id
    }

    /// Mark a beta state as a p-state (Forgy's production terminal)
    pub fn mark_as_p_state(&mut self, state_id: &str, rule_id: impl Into<RuleId>, salience: i32) {
        if let Some(state) = self.states.get_mut(state_id) {
            state.set_p_state(rule_id, salience);
        }
    }

    /// Add a join between two sources resulting in a beta state
    pub fn add_join(
        &mut self,
        left_source: String,
        right_source: String,
        join_condition: String,
    ) -> String {
        // Create the hash before moving the values
        let join_hash = format!("{}-{}-{}", left_source, right_source, join_condition);

        let mut state = BetaState::new();
        let state_id = state.id.clone();

        state.add_left_input(left_source.clone());
        state.add_right_input(right_source.clone());
        state.add_join_condition(join_condition);

        self.states.insert(state_id.clone(), state);

        // FIX (T006): Register this state as a child of its parent states
        // Register as child of left source
        if let Some(left_parent) = self.states.get_mut(&left_source) {
            left_parent.add_child(state_id.clone());
        }
        // Register as child of right source
        if let Some(right_parent) = self.states.get_mut(&right_source) {
            right_parent.add_child(state_id.clone());
        }

        // Also add to the join index if not already present
        self.join_index
            .entry(join_hash)
            .or_insert_with(|| state_id.clone());

        state_id
    }

    /// Get or create a shared beta state for a specific join pattern
    pub fn get_or_create_join(
        &mut self,
        left_source: String,
        right_source: String,
        join_condition: String,
    ) -> String {
        // Create a hash based on the join pattern structure to enable sharing
        let join_hash = format!("{}-{}-{}", left_source, right_source, join_condition);

        if let Some(state_id) = self.join_index.get(&join_hash) {
            // Return existing shared state
            state_id.clone()
        } else {
            // No existing state found, create a new one
            let state_id = self.add_join(left_source, right_source, join_condition);
            // Add to join index for future sharing
            self.join_index.insert(join_hash, state_id.clone());
            state_id
        }
    }

    /// Process a token from a specific source with proper RETE propagation.
    /// Tokens are propagated through the network to children states.
    /// Returns PStateActivations when tokens reach p-states (Forgy's production terminals).
    pub fn process_token_from_source(
        &mut self,
        source_id: &str,
        token: Token,
    ) -> Vec<PStateActivation> {
        let mut activations = Vec::new();
        // Use a work queue to propagate tokens through the network
        let mut work_queue: Vec<(String, Token)> = vec![(source_id.to_string(), token)];

        while let Some((current_source, current_token)) = work_queue.pop() {
            // Find all beta states that have this source as input
            let mut states_to_update = Vec::new();

            for (state_id, state) in &self.states {
                if state.left_input.contains(&current_source) {
                    states_to_update.push((state_id.clone(), "left"));
                }
                if state.right_input.contains(&current_source) {
                    states_to_update.push((state_id.clone(), "right"));
                }
            }

            // Process the token through each relevant state and collect new tokens
            for (state_id, input_type) in states_to_update {
                let (new_tokens, children, p_state_info) = {
                    if let Some(state) = self.states.get_mut(&state_id) {
                        let new_tokens = match input_type {
                            "left" => state.process_left_token(current_token.clone()),
                            "right" => state.process_right_token(current_token.clone()),
                            _ => Vec::new(),
                        };
                        (new_tokens, state.children.clone(), state.p_state.clone())
                    } else {
                        (Vec::new(), Vec::new(), None)
                    }
                };

                // Forgy's p-state: if this state is a production terminal, create activations
                if let Some(ref p_info) = p_state_info {
                    for new_token in &new_tokens {
                        activations.push(PStateActivation {
                            rule_id: p_info.rule_id.clone(),
                            token: new_token.clone(),
                            salience: p_info.salience,
                        });
                    }
                }

                // RETE propagation: propagate new tokens to children
                if !new_tokens.is_empty() && !children.is_empty() {
                    for new_token in new_tokens {
                        // Non-terminal state - propagate to children
                        for child_id in &children {
                            if let Some(child_state) = self.states.get_mut(child_id) {
                                // Children receive tokens from their parent as left input
                                let child_new_tokens =
                                    child_state.process_left_token(new_token.clone());
                                // Continue propagation for child's new tokens
                                for child_token in child_new_tokens {
                                    work_queue.push((child_id.clone(), child_token));
                                }
                            }
                        }
                    }
                }
            }
        }

        activations
    }

    /// Get all terminal states (states with no children) that have complete matches
    /// These represent complete rule matches ready for the agenda
    pub fn get_terminal_matches(&self) -> Vec<(String, Vec<Token>)> {
        self.states
            .iter()
            .filter(|(_, state)| state.children.is_empty() && !state.beta_memory.is_empty())
            .map(|(id, state)| (id.clone(), state.beta_memory.clone()))
            .collect()
    }

    /// Remove a WME from all beta state memories
    pub fn remove_wme_from_network(&mut self, wme_id: &str) {
        for state in self.states.values_mut() {
            state.remove_wme_from_tokens(wme_id);
        }
    }
}
