use serde::{Deserialize, Serialize};

use crate::engine_types::{Condition, Fact};
use crate::error::ReteError;
use crate::wme::WorkingMemoryElement;
use std::collections::HashMap;

/// Backing map type shared by `Bindings` and everywhere else in the crate
/// that carries a `?var -> value` binding map (`ScriptEval::evaluate`'s
/// `bindings` parameter, `Provenance::RuleFiring`/`RuleDrivenLookup`'s
/// `bindings` field). Under normal builds this is an ordinary alias for
/// `HashMap<String, String>` — the default `RandomState` hasher, seeded
/// from the OS RNG — so it is not a new type and changes nothing about any
/// public signature.
///
/// Under `cargo kani` it is [`KaniMap`] instead, for two independent
/// reasons:
/// 1. `RandomState::new()` calls a foreign function
///    (`CCRandomGenerateBytes` on macOS / `getrandom` elsewhere) that Kani
///    cannot model at all (kani-rs/kani#2423).
/// 2. Even with that worked around (e.g. `BuildHasherDefault<DefaultHasher>`,
///    a real, deterministic SipHash), CBMC's bit-precise modeling of
///    `hashbrown`'s SIMD probing plus a real byte-wise hash over symbolic
///    string content does not terminate in practical time: measured on
///    this machine, `add_binding_totality` alone did not finish symbolic
///    execution in 5 minutes even with the hasher fixed.
///
/// `KaniMap` is a `Vec<(String, String)>`-backed linear map exposing the
/// exact subset of the `HashMap` API this module and its callers use
/// (`get`, `insert`, `contains_key`, and `&BindingMap` iteration). Because
/// `add_binding`, `get_binding`, `can_bind`, `merge`, and `contains_var`
/// call these methods by name rather than naming `HashMap` directly, their
/// source is identical in both builds — only the backing collection
/// changes. For the bounded domains these harnesses use (at most two
/// entries), `KaniMap`'s linear scan is exact and inexpensive to unwind,
/// eliminating the hashing cost entirely without touching production
/// semantics.
#[cfg(not(kani))]
pub(crate) type BindingMap = HashMap<String, String>;
#[cfg(kani)]
pub(crate) type BindingMap = KaniMap;

/// See [`BindingMap`]. A minimal linear map used only under `cargo kani`.
#[cfg(kani)]
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct KaniMap {
    entries: Vec<(String, String)>,
}

#[cfg(kani)]
impl KaniMap {
    pub(crate) fn new() -> Self {
        KaniMap {
            entries: Vec::new(),
        }
    }

    pub(crate) fn get(&self, key: &str) -> Option<&String> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub(crate) fn insert(&mut self, key: String, value: String) -> Option<String> {
        if let Some(pair) = self.entries.iter_mut().find(|(k, _)| *k == key) {
            Some(std::mem::replace(&mut pair.1, value))
        } else {
            self.entries.push((key, value));
            None
        }
    }

    pub(crate) fn contains_key(&self, key: &str) -> bool {
        self.entries.iter().any(|(k, _)| k == key)
    }
}

#[cfg(kani)]
impl<'a> IntoIterator for &'a KaniMap {
    type Item = (&'a String, &'a String);
    type IntoIter = std::iter::Map<
        std::slice::Iter<'a, (String, String)>,
        fn(&'a (String, String)) -> (&'a String, &'a String),
    >;

    fn into_iter(self) -> Self::IntoIter {
        fn project(pair: &(String, String)) -> (&String, &String) {
            (&pair.0, &pair.1)
        }
        self.entries.iter().map(project)
    }
}

/// Represents variable bindings in the RETE network.
/// Maps variable names (e.g., "?x") to concrete values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bindings {
    pub bindings: BindingMap,
}

impl Default for Bindings {
    fn default() -> Self {
        Self::new()
    }
}

impl Bindings {
    #[cfg(not(kani))]
    pub fn new() -> Self {
        Bindings {
            bindings: HashMap::new(),
        }
    }

    #[cfg(kani)]
    pub fn new() -> Self {
        Bindings {
            bindings: KaniMap::new(),
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

#[cfg(kani)]
pub mod kani_harness {
    use super::*;

    /// Picks a bounded name symbolically from a fixed, small set of short
    /// literals: two variables (`?x`, `?y`) and one non-variable constant
    /// (`x`). Keeping the alphabet tiny and concrete (rather than fully
    /// symbolic bytes) is what makes CBMC's hashing and string-equality
    /// reasoning terminate: every reachable value is a short, concrete
    /// `String`, so `HashMap` probing and `PartialEq` on `String` stay
    /// bounded regardless of the symbolic choice that selects among them.
    fn any_name() -> String {
        let choice: u8 = kani::any();
        match choice % 3 {
            0 => "?x".to_string(),
            1 => "?y".to_string(),
            _ => "x".to_string(),
        }
    }

    /// Picks a bounded value symbolically from two short literals.
    fn any_value() -> String {
        let choice: u8 = kani::any();
        if choice % 2 == 0 {
            "a".to_string()
        } else {
            "b".to_string()
        }
    }

    /// `add_binding` is total over the bounded domain `{?x, ?y, x} x {a, b}`:
    /// it never panics, and when it returns `Ok` the binding is present in
    /// the map (never silently dropped).
    #[kani::proof]
    #[kani::unwind(6)]
    fn add_binding_totality() {
        let var = any_name();
        let value = any_value();

        let mut bindings = Bindings::new();
        let result = bindings.add_binding(&var, &value);

        if let Ok(()) = result {
            kani::assert(
                bindings.get_binding(&var) == Some(&value),
                "add_binding Ok implies binding is present (not silently dropped)",
            );
        } else {
            // The only reason add_binding can fail is a non-'?' name.
            kani::assert(
                !var.starts_with('?'),
                "add_binding only fails for a non-variable name",
            );
        }
    }
}
