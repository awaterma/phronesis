use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::engine_types::{Condition, Fact};
use crate::variable_binding::Bindings;

/// Working Memory Element (WME) represents a fact in the RETE network
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkingMemoryElement {
    pub id: String,
    pub fact: Fact,
    pub timestamp: u64,
}

impl WorkingMemoryElement {
    /// Creates a new Working Memory Element with a unique ID and timestamp
    pub fn new(fact: Fact) -> Self {
        WorkingMemoryElement {
            id: fact.id.clone(), // Use the fact's ID as the WME ID
            fact,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        }
    }
}

/// WME Manager handles the lifecycle of Working Memory Elements
#[derive(Debug)]
pub struct WmeManager {
    wmes: HashMap<String, WorkingMemoryElement>,
    predicate_index: HashMap<String, Vec<String>>, // Index by predicate for fast retrieval
}

impl Default for WmeManager {
    fn default() -> Self {
        Self::new()
    }
}

impl WmeManager {
    pub fn new() -> Self {
        WmeManager {
            wmes: HashMap::new(),
            predicate_index: HashMap::new(),
        }
    }

    /// Assert a new WME into the working memory
    pub fn assert(&mut self, wme: WorkingMemoryElement) -> Result<(), String> {
        let wme_id = wme.id.clone();
        let predicate = wme.fact.predicate.clone();

        self.wmes.insert(wme_id.clone(), wme);

        // Update predicate index
        self.predicate_index
            .entry(predicate)
            .or_default()
            .push(wme_id.clone());

        Ok(())
    }

    /// Retract a WME from the working memory
    pub fn retract(&mut self, wme_id: &str) -> Result<WorkingMemoryElement, String> {
        if let Some(wme) = self.wmes.remove(wme_id) {
            // Clean up predicate index
            if let Some(ids) = self.predicate_index.get_mut(&wme.fact.predicate) {
                ids.retain(|id| id != wme_id);
            }

            Ok(wme)
        } else {
            Err(format!("WME with ID {} not found", wme_id))
        }
    }

    /// Get a WME by ID
    pub fn get(&self, wme_id: &str) -> Option<&WorkingMemoryElement> {
        self.wmes.get(wme_id)
    }

    /// Get all WMEs
    pub fn get_all(&self) -> Vec<&WorkingMemoryElement> {
        self.wmes.values().collect()
    }

    /// Get WMEs that match a condition
    pub fn get_by_condition(&self, condition: &Condition) -> Vec<&WorkingMemoryElement> {
        // Use predicate index for faster lookup
        if let Some(wme_ids) = self.predicate_index.get(&condition.predicate) {
            wme_ids
                .iter()
                .filter_map(|wme_id| self.wmes.get(wme_id))
                .filter(|wme| condition.matches(&wme.fact))
                .collect()
        } else {
            // If predicate not in index, check all WMEs
            self.wmes
                .values()
                .filter(|wme| condition.matches(&wme.fact))
                .collect()
        }
    }

    /// Number of WMEs currently in working memory
    pub fn len(&self) -> usize {
        self.wmes.len()
    }

    /// Whether working memory holds no WMEs
    pub fn is_empty(&self) -> bool {
        self.wmes.is_empty()
    }

    /// Get WMEs by predicate for faster access
    pub fn get_by_predicate(&self, predicate: &str) -> Vec<&WorkingMemoryElement> {
        if let Some(wme_ids) = self.predicate_index.get(predicate) {
            wme_ids
                .iter()
                .filter_map(|wme_id| self.wmes.get(wme_id))
                .collect()
        } else {
            vec![]
        }
    }
}

// Add missing methods to Condition to make it compatible with WME implementation
impl Condition {
    /// Check if a condition matches a fact without any existing bindings
    pub fn matches(&self, fact: &Fact) -> bool {
        // Check predicate match
        if fact.predicate != self.predicate {
            return false;
        }

        // Check argument match - simple implementation without variable binding
        if fact.args.len() != self.args.len() {
            return false;
        }

        // Check each argument - if condition has a variable (starts with ?), match any value
        for (fact_arg, condition_arg) in fact.args.iter().zip(self.args.iter()) {
            if !condition_arg.starts_with('?') && fact_arg != condition_arg {
                return false;
            }
        }

        true
    }

    /// Check if a condition matches a fact with existing bindings, returning new bindings
    pub fn matches_with_bindings(
        &self,
        fact: &Fact,
        existing_bindings: &Bindings,
    ) -> Result<Bindings, String> {
        existing_bindings.can_bind(self, fact)
    }
}
