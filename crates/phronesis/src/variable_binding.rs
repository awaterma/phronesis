use serde::{Deserialize, Serialize};

use crate::engine_types::{Condition, Fact};
use crate::error::ReteError;
use crate::wme::WorkingMemoryElement;
use std::collections::HashMap;

/// Represents variable bindings in the RETE network.
/// Maps variable names (e.g., "?x") to concrete values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bindings {
    pub bindings: HashMap<String, String>,
}

impl Default for Bindings {
    fn default() -> Self {
        Self::new()
    }
}

impl Bindings {
    pub fn new() -> Self {
        Bindings {
            bindings: HashMap::new(),
        }
    }

    /// Add a binding from a variable to a value
    pub fn add_binding(&mut self, var: &str, value: &str) -> Result<(), ReteError> {
        if !var.starts_with('?') {
            return Err(ReteError::InvalidVariable(var.to_string()));
        }

        // Check if the variable already has a different binding
        if let Some(existing) = self.bindings.get(var)
            && existing != value
        {
            return Err(ReteError::BindingConflict {
                variable: var.to_string(),
                existing: existing.clone(),
                attempted: value.to_string(),
            });
        }

        self.bindings.insert(var.to_string(), value.to_string());
        Ok(())
    }

    /// Get the value bound to a variable
    pub fn get_binding(&self, var: &str) -> Option<&String> {
        self.bindings.get(var)
    }

    /// Check if all variables in condition can be consistently bound with current bindings
    pub fn can_bind(&self, condition: &Condition, fact: &Fact) -> Result<Bindings, ReteError> {
        if condition.predicate != fact.predicate {
            return Err(ReteError::ConditionMismatch(
                "Predicate mismatch".to_string(),
            ));
        }

        if condition.args.len() != fact.args.len() {
            return Err(ReteError::ConditionMismatch(
                "Argument count mismatch".to_string(),
            ));
        }

        let mut new_bindings = self.clone();

        for (cond_arg, fact_arg) in condition.args.iter().zip(fact.args.iter()) {
            if cond_arg.starts_with('?') {
                // This is a variable - check if it's already bound
                if let Some(existing_value) = new_bindings.get_binding(cond_arg) {
                    // Variable already bound - values must match
                    if existing_value != fact_arg {
                        return Err(ReteError::BindingConflict {
                            variable: cond_arg.clone(),
                            existing: existing_value.clone(),
                            attempted: fact_arg.clone(),
                        });
                    }
                } else {
                    // Variable not yet bound - create new binding
                    new_bindings.add_binding(cond_arg, fact_arg)?;
                }
            } else {
                // This is a constant - must match exactly
                if cond_arg != fact_arg {
                    return Err(ReteError::ConditionMismatch(format!(
                        "Constant '{}' does not match fact argument '{}'",
                        cond_arg, fact_arg
                    )));
                }
            }
        }

        Ok(new_bindings)
    }

    /// Check if two binding sets are consistent and merge them
    pub fn merge(&self, other: &Bindings) -> Result<Bindings, ReteError> {
        let mut merged = self.clone();

        for (var, value) in &other.bindings {
            if let Some(existing_value) = merged.get_binding(var) {
                if existing_value != value {
                    return Err(ReteError::BindingConflict {
                        variable: var.clone(),
                        existing: existing_value.clone(),
                        attempted: value.clone(),
                    });
                }
            } else {
                merged.bindings.insert(var.clone(), value.clone());
            }
        }

        Ok(merged)
    }

    /// Check if a variable is bound in these bindings
    pub fn contains_var(&self, var: &str) -> bool {
        self.bindings.contains_key(var)
    }
}

/// A Token represents a partial match in the RETE network
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub wmes: Vec<WorkingMemoryElement>,
    pub bindings: Bindings,
    pub parent: Option<Box<Token>>, // Optional parent for token lineage
}

impl Default for Token {
    fn default() -> Self {
        Self::new()
    }
}

impl Token {
    pub fn new() -> Self {
        Token {
            wmes: Vec::new(),
            bindings: Bindings::new(),
            parent: None,
        }
    }

    pub fn new_with_wme(wme: WorkingMemoryElement) -> Self {
        Token {
            wmes: vec![wme],
            bindings: Bindings::new(),
            parent: None,
        }
    }

    pub fn new_with_bindings(wmes: Vec<WorkingMemoryElement>, bindings: Bindings) -> Self {
        Token {
            wmes,
            bindings,
            parent: None,
        }
    }

    /// Extend the token with a new WME and updated bindings
    pub fn extend_with_binding(
        &self,
        wme: WorkingMemoryElement,
        additional_bindings: &Bindings,
    ) -> Result<Token, ReteError> {
        let new_bindings = self.bindings.merge(additional_bindings)?;
        let mut new_wmes = self.wmes.clone();
        new_wmes.push(wme);

        Ok(Token {
            wmes: new_wmes,
            bindings: new_bindings,
            parent: Some(Box::new(self.clone())),
        })
    }
}
