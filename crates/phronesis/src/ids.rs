//! Strongly-typed identifiers for entities the engine manages.
//!
//! Each newtype wraps a `String` but is incompatible with the others at
//! the type level — a [`RuleId`] cannot be passed where a [`StateId`] is
//! expected and vice versa. Serialization is transparent (`#[serde(
//! transparent)]`), so the on-wire JSON form is unchanged: any consumer
//! reading or writing the previous `String`-shaped contract continues to
//! work without modification.
//!
//! Construction is deliberately ergonomic. `From<String>` and
//! `From<&str>` accept the common cases at call sites, and the inherent
//! `new()` constructor accepts anything `Into<String>`. The point
//! of the newtypes is to catch mixups between *kinds* of IDs — not to
//! make routine construction painful.
//!
//! The newtypes deliberately do **not** offer cross-conversions
//! (`From<StateId> for RuleId`, etc.) — that would defeat the purpose.
//! Converting between kinds requires going through `into_inner()`.

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! id_newtype {
    ($name:ident, $doc:expr) => {
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            /// Construct from anything convertible to `String`.
            pub fn new(s: impl Into<String>) -> Self {
                Self(s.into())
            }

            /// Borrow the inner string.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Unwrap to the inner `String`. Use sparingly — it discards
            /// the type tag the newtype exists to enforce.
            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                // Use `pad` (not `write_str`) so format specs like `{:<35}`
                // are honored — width, alignment, fill all flow through.
                // `write_str` writes the bytes raw and discards the
                // formatter state; that quietly broke alignment in every
                // `phr-mcp stats`-style table that contained a `RuleId`.
                f.pad(&self.0)
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_string())
            }
        }

        impl From<&String> for $name {
            fn from(s: &String) -> Self {
                Self(s.clone())
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl PartialEq<str> for $name {
            fn eq(&self, other: &str) -> bool {
                self.0 == other
            }
        }

        impl PartialEq<&str> for $name {
            fn eq(&self, other: &&str) -> bool {
                self.0 == *other
            }
        }

        impl PartialEq<String> for $name {
            fn eq(&self, other: &String) -> bool {
                self.0 == *other
            }
        }
    };
}

id_newtype!(
    RuleId,
    "Identifier for a rule. Distinct from [`StateId`] at the type level.

A `RuleId` is the stable human-readable string a rule author gives the
rule (e.g. `\"enforce-no-unwrap-in-src\"`). It moves with consequences
in the [`crate::Provenance::RuleFiring`] variant and is what audit logs,
hook decisions, and trend reports group by."
);

id_newtype!(
    StateId,
    "Identifier for a RETE network state — alpha or beta.

State IDs are UUID-shaped strings minted at network-construction time.
They are dense, internal, and never escape the engine surface in the
public API; the newtype exists to keep them from being confused with
rule IDs or fact IDs inside the network's own bookkeeping (left_input /
right_input / children pointers between states)."
);

id_newtype!(
    FactId,
    "Identifier for a fact (a [`crate::wme::WorkingMemoryElement`]).

Fact IDs are typically host-minted strings carried with the fact through
the engine and surfaced in [`crate::Provenance::RuleFiring::bound_facts`]
when an actor wants to trace 'why did this rule fire?' back to its
inputs. The newtype keeps fact IDs distinct from rule IDs and state IDs
at the type level."
);

id_newtype!(
    StableId,
    "Identifier for one packable unit of host-assembled context — `kernel:0`,
`activity:<ts>:<rule>:<n>`, `nudge:<capsule>`, `state:subject`, and so on.

Minted by a host that packs context for a model under a byte and token
budget, and used to say which units were selected and which were omitted.
A [`RuleId`] is a *component* of some stable ids and never a whole one, so
the newtype keeps the two from being interchanged where a packer expects a
whole stable id. Serialization is transparent, so the wire shape is the
bare string."
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_id_serializes_transparently() {
        let id = RuleId::new("my-rule");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"my-rule\"");
    }

    #[test]
    fn rule_id_deserializes_from_plain_string() {
        let id: RuleId = serde_json::from_str("\"my-rule\"").unwrap();
        assert_eq!(id, RuleId::new("my-rule"));
    }

    #[test]
    fn state_id_serializes_transparently() {
        let id = StateId::new("state-abc");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"state-abc\"");
    }

    #[test]
    fn from_str_constructs() {
        let id: RuleId = "rule-x".into();
        assert_eq!(id.as_str(), "rule-x");
    }

    #[test]
    fn from_string_constructs() {
        let s = String::from("rule-y");
        let id: RuleId = s.into();
        assert_eq!(id.as_str(), "rule-y");
    }

    #[test]
    fn partial_eq_with_str_works() {
        let id = RuleId::new("foo");
        assert!(id == "foo");
        // Pre-bound so the owned-String PartialEq impl is exercised without
        // tripping clippy::cmp_owned on an inline temporary.
        let owned = String::from("foo");
        assert!(id == owned);
    }

    #[test]
    fn display_writes_inner_string() {
        let id = StateId::new("n-1");
        assert_eq!(format!("{}", id), "n-1");
    }
}
