//! Fresh-network policy evaluation for the embeddable governance core.
//!
//! This is the pure core of what the MCP hooks do at pre/post time: take a
//! validated [`CompiledPolicy`], an explicit [`Selection`], an explicit
//! [`FactSnapshot`], and classify what fires. Every call builds a **new**
//! [`phr::ReteNetwork`] — there is no session, no persistence, and no
//! implicit state between calls; the persistent network stays an MCP
//! concern behind its adapter.
//!
//! Semantics preserved from the hook baseline:
//!
//! - Facts are plain [`phr::Fact`] values. `Fact.source` is attribution
//!   metadata only; it never authorizes a predicate.
//! - Extension facts (from project predicate providers) may never assert
//!   a caller-reserved predicate, no matter what `source` they claim. All
//!   extension predicates are prevalidated **before any fact is asserted**,
//!   so a rejected snapshot leaves working memory untouched.
//! - Host facts may use reserved predicates — the host owns those
//!   predicates by definition.
//! - Duplicate fact ids are input errors across the whole snapshot (host
//!   and extensions together), stricter than the engine's "identical
//!   re-assert is a no-op" rule, so an extension can never overwrite a
//!   host fact's identity.
//! - Guard (`__script__`) evaluation failures follow the engine's
//!   fail-closed matching: the rule is treated as matched and fires with
//!   the error attached (`payload.guard_error`), which the decision
//!   classifier surfaces as a diagnostic — never an operational error.
//! - Operational failures (rule installation, fact assertion, agenda
//!   update, firing) are typed errors carrying the engine error as
//!   `source`. A policy `Deny` is a successful evaluation, not an error.
//! - An empty selection cannot fire anything: the network is not built
//!   and facts are not asserted. The snapshot is validated first, so a
//!   snapshot that violates a reservation or repeats a fact id is an
//!   error even when the selection is empty — only a VALIDATED empty
//!   selection classifies as `Allow`.
//!
//! No filesystem, environment, discovery, logging, or process side
//! effects. The caller's provenance labels are not authentication.

use std::collections::{BTreeSet, HashSet};

use phr::{Fact, RuleId, ScriptEval};

use crate::decision::{Decision, classify};
use crate::policy::{CompiledPolicy, Selection};

/// The explicit fact input for one evaluation.
///
/// `host` facts come from the embedding application itself; `extensions`
/// come from project-defined predicate providers. The split exists for one
/// rule: extension facts are prevalidated against the caller's reserved
/// predicate set and rejected as a whole snapshot if any of them names a
/// reserved predicate. Host facts are not so restricted.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FactSnapshot {
    /// Facts asserted by the host application.
    pub host: Vec<Fact>,
    /// Facts asserted by extension providers.
    pub extensions: Vec<Fact>,
}

/// Typed operational failures of an evaluation.
///
/// Each engine-backed variant retains the original [`phr::ReteError`] as
/// `source`; the snapshot-validation variants name the offending fact.
/// These are operational errors only — a firing rule that blocks the edit
/// is a successful evaluation whose [`Decision`] carries `Verdict::Deny`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EvaluationError {
    /// An extension fact asserted a predicate the caller reserved. The
    /// whole snapshot is rejected before anything is asserted, regardless
    /// of what `Fact.source` claims.
    #[error(
        "extension fact `{fact_id}` asserts reserved predicate `{predicate}`; \
         extension facts may not use reserved predicates"
    )]
    ReservedExtensionPredicate {
        /// The rejected fact's id.
        fact_id: String,
        /// The reserved predicate it asserted.
        predicate: String,
    },
    /// The same fact id appeared more than once across the snapshot (host
    /// or extension, within one list or across both). Rejected before any
    /// assertion so an extension can never shadow a host fact's identity.
    #[error(
        "duplicate fact id `{fact_id}` across host and extension facts; \
         fact ids must be unique within one snapshot"
    )]
    DuplicateFactId {
        /// The duplicated fact id.
        fact_id: String,
    },
    /// A selected rule failed to install into the fresh network.
    #[error("rule `{rule_id}` failed to install: {source}")]
    RuleInstallation {
        /// The rule that failed to install.
        rule_id: RuleId,
        /// The underlying engine error.
        source: phr::ReteError,
    },
    /// A snapshot fact was rejected by the engine.
    #[error("fact `{fact_id}` failed to assert: {source}")]
    FactAssertion {
        /// The fact that failed to assert.
        fact_id: String,
        /// The underlying engine error.
        source: phr::ReteError,
    },
    /// The agenda could not be recomputed after assertion.
    #[error("agenda update failed: {source}")]
    Agenda {
        /// The underlying engine error.
        source: phr::ReteError,
    },
    /// Fired actions could not be collected.
    #[error("rule execution failed: {source}")]
    Firing {
        /// The underlying engine error.
        source: phr::ReteError,
    },
}

/// Evaluate `policy` for `selection` against `snapshot` in a brand-new
/// engine network and classify the consequences.
///
/// `reserved` is the caller's set of predicates only host code may assert;
/// `demote` names rule ids whose blocks are demoted to warnings for this
/// decision (exact id match, never inferred); `evaluator` backs
/// `__script__` guard conditions. Inject [`builtin_evaluator`] for the
/// dependency-free builtin, or a richer one (e.g. Rhai) — never implicitly.
///
/// Order of operations: extension predicates are validated against
/// `reserved` and fact ids deduplicated **before anything is asserted**;
/// this snapshot validation runs even when the selection is empty, so
/// reserved/duplicate rejections keep priority over the empty-selection
/// `Allow`. Then selected rules are installed, host facts are asserted,
/// extension facts are asserted, the agenda is updated, and all matched
/// rules fire.
/// The raw consequences are classified by [`crate::decision::classify`],
/// which demotes, projects, and retains them unchanged.
pub async fn evaluate(
    policy: &CompiledPolicy,
    selection: Selection,
    snapshot: &FactSnapshot,
    reserved: &HashSet<String>,
    demote: &BTreeSet<RuleId>,
    evaluator: Box<dyn ScriptEval>,
) -> Result<Decision, EvaluationError> {
    validate_snapshot(snapshot, reserved)?;

    let rules = policy.select(selection);
    if rules.is_empty() {
        // Nothing can fire, so there is nothing to install, assert, or run.
        return Ok(classify(&[], demote));
    }

    let network = phr::ReteNetwork::with_script_evaluator(evaluator);
    for rule in &rules {
        let rule_id = RuleId::new(rule.id.clone());
        network
            .add_rule(rule.clone())
            .await
            .map_err(|source| EvaluationError::RuleInstallation { rule_id, source })?;
    }

    for fact in snapshot.host.iter().chain(snapshot.extensions.iter()) {
        let fact_id = fact.id.clone();
        network
            .assert_fact(fact.clone())
            .await
            .map_err(|source| EvaluationError::FactAssertion { fact_id, source })?;
    }

    network
        .update_agenda()
        .await
        .map_err(|source| EvaluationError::Agenda { source })?;

    let consequences = network
        .fire_all_consequences()
        .map_err(|source| EvaluationError::Firing { source })?;

    Ok(classify(&consequences, demote))
}

/// Prevalidate a snapshot: extension predicates against `reserved` first,
/// then fact-id uniqueness across host and extension lists together. No
/// fact is asserted until both checks pass for the entire snapshot.
fn validate_snapshot(
    snapshot: &FactSnapshot,
    reserved: &HashSet<String>,
) -> Result<(), EvaluationError> {
    for fact in &snapshot.extensions {
        if reserved.contains(&fact.predicate) {
            return Err(EvaluationError::ReservedExtensionPredicate {
                fact_id: fact.id.clone(),
                predicate: fact.predicate.clone(),
            });
        }
    }

    let mut seen: HashSet<&str> = HashSet::new();
    for fact in snapshot.host.iter().chain(snapshot.extensions.iter()) {
        if !seen.insert(fact.id.as_str()) {
            return Err(EvaluationError::DuplicateFactId {
                fact_id: fact.id.clone(),
            });
        }
    }
    Ok(())
}

/// Convenience constructor for the dependency-free builtin `__script__`
/// evaluator. Use it when the policy has no guard scripts or only builtin
/// ones; richer evaluators (e.g. Rhai) are always injected explicitly.
pub fn builtin_evaluator() -> Box<dyn ScriptEval> {
    Box::new(phr::BuiltinScriptEvaluator)
}
