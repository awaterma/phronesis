//! Shared consequence classification: one governance `Decision` from a
//! slice of raw engine consequences.
//!
//! Ported in behavior from `phronesis-mcp::hook_logged` (projection,
//! action-type splitting, stale-graph demotion) as pure functions over
//! `phr::Consequence` values. The MCP `LoggedConsequence` wire shape stays
//! in MCP; this module is the embeddable equivalent.
//!
//! Behavior contract, matching the hook:
//!
//! - Classification reads **only** each payload's `action_type` field,
//!   never the message text. `"constraint_violation"` is a block,
//!   `"constraint_warning"` a warning; every other value (log,
//!   emit_capsule, unknown, absent) carries no decision weight.
//! - `Allow` when no warning/block action fired, `Warn` when warnings and
//!   no effective block remain, `Deny` for any effective block — the same
//!   severity order the pre/post hooks apply (violations > warnings >
//!   clean).
//! - Every original consequence is cloned **unchanged** into
//!   [`Decision::consequences`], in input order. Explicit demotion never
//!   mutates a payload: the original keeps its block `action_type`,
//!   message, and `guard_error`; only the projection's effective severity
//!   and a diagnostic record the demotion.
//! - One [`DecisionReason`] per raw consequence — including unknown
//!   actions and non-`RuleFiring` provenance (Lookup, RuleDrivenLookup,
//!   Asserted), which the hook's projection skips but an embeddable host
//!   must still see. Payload, bindings, fact sources, and provenance stay
//!   on the original values, reachable through
//!   [`DecisionReason::index`].
//! - `guard_error` and `unbound_variables` surface as typed
//!   [`DecisionDiagnostic`]s copied from the structured payload fields,
//!   regardless of the action's severity — a guard failure is diagnostic
//!   even on log/capsule rules.
//! - Explicit demotion matches **exact** rule ids only: the id carried by
//!   `Provenance::RuleFiring` (the same projection rule the hook uses) must
//!   be a member of the caller's set verbatim. No base-id inference, no
//!   `#orN`-family expansion, no wildcards.
//!
//! Operational errors are not verdicts here: `classify` consumes only the
//! successful output of an evaluation. Invalid policy, reserved extension
//! predicates, fact-assertion failures, and install/agenda/fire errors are
//! typed errors of the evaluation layer (see the `policy`/`evaluation`
//! modules) and can never flow through this function, so a policy `Deny`
//! is always a successful evaluation, not an operational failure.

use std::collections::BTreeSet;

use phr::{Consequence, Provenance, RuleId};
use serde::{Deserialize, Serialize};

/// The internal action type a rule's `block` verb compiles to.
pub const ACTION_BLOCK: &str = "constraint_violation";

/// The internal action type a rule's `warn` verb compiles to.
pub const ACTION_WARN: &str = "constraint_warning";

/// The outcome a host acts on: allow the edit, warn but proceed, or deny.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// No warning or block action fired.
    Allow,
    /// Warnings fired and no effective block remains.
    Warn,
    /// At least one effective block fired.
    Deny,
}

impl Verdict {
    /// `true` when the host must refuse the action.
    pub fn is_deny(&self) -> bool {
        matches!(self, Verdict::Deny)
    }

    /// `true` when the host may proceed but should surface messages.
    pub fn is_warn(&self) -> bool {
        matches!(self, Verdict::Warn)
    }

    /// `true` when nothing fired at decision weight.
    pub fn is_allow(&self) -> bool {
        matches!(self, Verdict::Allow)
    }
}

/// Decision weight of one consequence's `action_type`, after explicit
/// demotion. `Info` covers every non-severity action: `log`,
/// `emit_capsule`, unrecognized, and absent — all retained, none affect
/// the verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// `action_type == "constraint_violation"`: the host must refuse.
    Block,
    /// `action_type == "constraint_warning"`, or a block whose rule id was
    /// explicitly demoted.
    Warning,
    /// Any other action type: informational only.
    Info,
}

impl Severity {
    /// `true` for [`Severity::Block`].
    pub fn is_block(&self) -> bool {
        matches!(self, Severity::Block)
    }

    /// `true` for [`Severity::Warning`].
    pub fn is_warning(&self) -> bool {
        matches!(self, Severity::Warning)
    }
}

/// Pure action-type → severity classification. Exact string equality on
/// the `action_type` field; message text is never consulted.
pub fn action_severity(action_type: &str) -> Severity {
    match action_type {
        ACTION_BLOCK => Severity::Block,
        ACTION_WARN => Severity::Warning,
        _ => Severity::Info,
    }
}

/// The raw `message` string of a consequence payload (empty when absent),
/// byte-identical to what the hook's projection extracts.
pub fn action_message(payload: &serde_json::Value) -> &str {
    payload
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

/// Structured projection of one raw consequence. Exactly one reason exists
/// per element of [`Decision::consequences`]; payload, bindings, fact
/// sources, and provenance are read through the original consequence at
/// [`DecisionReason::index`], never duplicated here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionReason {
    /// Index into [`Decision::consequences`] (the original slice order).
    pub index: usize,
    /// The rule id when the provenance is `Provenance::RuleFiring` — the
    /// same projection rule the hook applies; `None` for Lookup,
    /// RuleDrivenLookup, Asserted, and any future provenance variant.
    pub rule_id: Option<RuleId>,
    /// Raw `payload.action_type` (empty when absent). Classification reads
    /// only this field.
    pub action_type: String,
    /// Raw `payload.message` (empty when absent).
    pub message: String,
    /// Effective severity: the raw action severity, or `Warning` when this
    /// consequence's rule id was explicitly demoted.
    pub severity: Severity,
    /// `true` when the caller's demote set named this consequence's exact
    /// rule id and the raw action was a block. The original consequence
    /// keeps its block payload untouched.
    pub demoted: bool,
}

/// Typed observations copied from structured consequence payload fields.
/// None of these alter the verdict by themselves: a guard failure fails
/// closed (the rule fired as matched, so its own action carries the
/// verdict), and unbound variables render literally while the action still
/// fires.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DecisionDiagnostic {
    /// The rule's `__script__` guard failed to evaluate; the engine failed
    /// closed and annotated every consequence of the firing with the error
    /// (`payload.guard_error`). Diagnostic even for log/capsule rules.
    GuardError {
        /// Index of the annotated consequence in
        /// [`Decision::consequences`].
        index: usize,
        /// Rule id when the annotated consequence is a `RuleFiring`.
        rule_id: Option<RuleId>,
        /// The evaluator's error text, copied from `payload.guard_error`.
        error: String,
    },
    /// The action fired naming variables its bindings left unbound
    /// (`payload.unbound_variables`); they rendered literally.
    UnboundVariables {
        /// Index of the consequence in [`Decision::consequences`].
        index: usize,
        /// Rule id when the consequence is a `RuleFiring`.
        rule_id: Option<RuleId>,
        /// The unbound variable names, copied in payload order.
        variables: Vec<String>,
    },
    /// The caller's demote set contained this consequence's exact rule id
    /// and the raw action was a block: effective severity is warning, the
    /// original consequence is unchanged, and its `guard_error` (if any)
    /// stays in place. Never inferred from a base id, an `#orN` family, or
    /// a wildcard.
    Demoted {
        /// Index of the demoted consequence in
        /// [`Decision::consequences`].
        index: usize,
        /// The exact rule id that matched the caller's demote set.
        rule_id: RuleId,
    },
}

impl DecisionDiagnostic {
    /// The index of the consequence this diagnostic is about.
    pub fn index(&self) -> usize {
        match self {
            DecisionDiagnostic::GuardError { index, .. }
            | DecisionDiagnostic::UnboundVariables { index, .. }
            | DecisionDiagnostic::Demoted { index, .. } => *index,
        }
    }

    /// The rule id this diagnostic names, when the projected consequence
    /// carried one.
    pub fn rule_id(&self) -> Option<&RuleId> {
        match self {
            DecisionDiagnostic::GuardError { rule_id, .. }
            | DecisionDiagnostic::UnboundVariables { rule_id, .. } => rule_id.as_ref(),
            DecisionDiagnostic::Demoted { rule_id, .. } => Some(rule_id),
        }
    }
}

/// The governance result for one evaluation's successful output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    /// The host-facing outcome.
    pub verdict: Verdict,
    /// Every original consequence, cloned unchanged and in input order —
    /// demotion and diagnostics never mutate payloads, bindings, or
    /// provenance.
    pub consequences: Vec<Consequence>,
    /// Exactly one structured projection per original consequence.
    pub reasons: Vec<DecisionReason>,
    /// Typed guard-error, unbound-variable, and demotion records.
    pub diagnostics: Vec<DecisionDiagnostic>,
}

impl Decision {
    /// Iterate reasons paired with the original consequence each projects.
    pub fn iter_reasons(&self) -> impl Iterator<Item = (&DecisionReason, &Consequence)> {
        self.reasons.iter().map(|reason| {
            let consequence = &self.consequences[reason.index];
            (reason, consequence)
        })
    }

    /// Messages of every effective block, in original order — the deny
    /// lines a hook prints.
    pub fn block_messages(&self) -> Vec<&str> {
        self.reasons
            .iter()
            .filter(|r| r.severity.is_block())
            .map(|r| r.message.as_str())
            .collect()
    }

    /// Messages of every effective warning (including demoted blocks), in
    /// original order — the warning lines a hook prints.
    pub fn warning_messages(&self) -> Vec<&str> {
        self.reasons
            .iter()
            .filter(|r| r.severity.is_warning())
            .map(|r| r.message.as_str())
            .collect()
    }
}

/// Classify a successful evaluation's consequences into a
/// [`Decision`].
///
/// `demote_rule_ids` names rules whose blocks are demoted to warnings for
/// this decision (e.g. stale-graph rules in MCP). Matching is exact
/// equality against the `Provenance::RuleFiring` rule id; every block from
/// a listed id is demoted, including fail-closed guard matches, and
/// nothing else is.
pub fn classify(consequences: &[Consequence], demote_rule_ids: &BTreeSet<RuleId>) -> Decision {
    let mut reasons = Vec::with_capacity(consequences.len());
    let mut diagnostics = Vec::new();
    let mut any_block = false;
    let mut any_warning = false;

    for (index, consequence) in consequences.iter().enumerate() {
        let rule_id = rule_id_of(&consequence.provenance);
        let action_type = consequence
            .payload
            .get("action_type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let message = action_message(&consequence.payload).to_string();

        if let Some(error) = consequence
            .payload
            .get("guard_error")
            .and_then(|v| v.as_str())
        {
            diagnostics.push(DecisionDiagnostic::GuardError {
                index,
                rule_id: rule_id.clone(),
                error: error.to_string(),
            });
        }
        if let Some(variables) = unbound_variables(&consequence.payload) {
            diagnostics.push(DecisionDiagnostic::UnboundVariables {
                index,
                rule_id: rule_id.clone(),
                variables,
            });
        }

        let raw_severity = action_severity(&action_type);
        let demoted = raw_severity == Severity::Block
            && rule_id
                .as_ref()
                .is_some_and(|id| demote_rule_ids.contains(id));
        if demoted {
            let id = rule_id.as_ref().expect("demotion requires a rule id");
            diagnostics.push(DecisionDiagnostic::Demoted {
                index,
                rule_id: id.clone(),
            });
        }
        let severity = if demoted {
            Severity::Warning
        } else {
            raw_severity
        };
        any_block |= severity.is_block();
        any_warning |= severity.is_warning();

        reasons.push(DecisionReason {
            index,
            rule_id,
            action_type,
            message,
            severity,
            demoted,
        });
    }

    let verdict = if any_block {
        Verdict::Deny
    } else if any_warning {
        Verdict::Warn
    } else {
        Verdict::Allow
    };

    Decision {
        verdict,
        consequences: consequences.to_vec(),
        reasons,
        diagnostics,
    }
}

/// The rule id a consequence's provenance carries, following the hook's
/// projection: `RuleFiring` only.
fn rule_id_of(provenance: &Provenance) -> Option<RuleId> {
    match provenance {
        Provenance::RuleFiring { rule_id, .. } => Some(rule_id.clone()),
        _ => None,
    }
}

/// The unbound variable names in a consequence payload, when the field is
/// a non-empty array of strings.
fn unbound_variables(payload: &serde_json::Value) -> Option<Vec<String>> {
    let vars: Vec<String> = payload
        .get("unbound_variables")?
        .as_array()?
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    if vars.is_empty() { None } else { Some(vars) }
}
