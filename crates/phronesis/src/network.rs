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

#[cfg(feature = "embedding-host")]
use serde::{Deserialize, Serialize};

use crate::agenda::{Agenda, AgendaItem};
use crate::alpha_network::AlphaNetwork;
use crate::beta_network::{BetaNetwork, PStateActivation};
use crate::consequence::Consequence;
use crate::engine_types::{Action, Condition, Fact, PerformanceStats, Rule};
use crate::error::ReteError;
use crate::production::{ProductionNetwork, ProductionState};
use crate::script_evaluator::{BuiltinScriptEvaluator, ScriptEval};
use crate::wme::{WmeManager, WorkingMemoryElement};
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::{debug, warn};

/// Scope guard returned by [`ReteNetwork::begin_batch`].
///
/// While a fast-mode guard from this network is open, `assert_fact`
/// and `retract_fact` skip the per-fact/per-retraction agenda work they'd
/// otherwise do (the single- and multi-condition incremental inserts, and
/// `retract_fact`'s trailing full rescan) — alpha/beta propagation and the
/// WME store are untouched, only agenda *visibility* of new matches is
/// deferred. When the last outstanding guard ends (`close` or `Drop`),
/// exactly one full production scan (`ReteNetwork::full_scan_update_agenda`,
/// the same algorithm `update_agenda` has always run) rebuilds the agenda
/// from current alpha/beta state, applying the normal `fired_activations`
/// refraction check. No per-fact bookkeeping is queued or replayed; this is
/// a "defer, then catch up once" strategy, not a transaction log.
///
/// Nesting is reentrant over one shared depth counter per `ReteNetwork`
/// instance — not per guard, not task-local. Concurrent callers holding
/// guards on the *same* `ReteNetwork` share that counter: there is no
/// isolation between logically-independent batches opened concurrently on
/// one instance, and the single eventual full scan serves all of them.
/// Opening guards on different `ReteNetwork` instances is fully
/// independent, same as any other per-instance state in this type.
///
/// # Boundaries and ordering
///
/// This is deferred agenda maintenance, not a transaction: fact mutations
/// remain visible immediately, and an error does not roll them back. Existing
/// agenda items can still fire during a batch; retraction removes obsolete
/// items immediately. Callers requiring a consistent boundary must serialize
/// mutations, firing, and batch opening/closing on the same network. The
/// per-subsystem locks do not make those operations collectively atomic.
///
/// Salience still orders activations, but equal-salience ties may differ from
/// unbatched insertion order because the final scan discovers matches by rule.
/// Dropping a guard (including on early return or unwind) closes its scope;
/// explicitly call [`FactBatch::close`] to receive a scan error. Forgetting a
/// guard with [`std::mem::forget`] leaves its scope open indefinitely.
///
/// # Ordered mode
///
/// `ReteNetwork::begin_ordered_batch` opens the same guard type in
/// `BatchMode::Ordered` instead of `BatchMode::Fast`. While any ordered
/// guard is open, `assert_fact`/`retract_fact` run their normal, non-batched
/// incremental agenda maintenance on every call — the exact same code path
/// as if no batch were open at all — so the resulting `(rule_id,
/// bound_fact_ids)` firing trace, including equal-salience ties, is
/// identical to calling the same `assert_fact`/`retract_fact` sequence with
/// no batch open. This forgoes the fast mode's coalescing: an ordered scope
/// pays the same per-fact/per-retract cost as unbatched calls. Because
/// nothing is ever deferred in ordered mode, closing the outermost ordered
/// guard runs **no** catch-up scan at all (unlike fast mode) — such a scan
/// could rediscover and re-fire an activation whose guard a drain inside the
/// batch found false and un-latched, which an unbatched caller would not see
/// unless it separately called [`ReteNetwork::update_agenda`].
///
/// The mode for a nesting session is fixed by whichever guard is opened
/// first on an otherwise-closed stack (`batch_depth() == 0`); every nested
/// guard opened before that session's depth returns to zero must request
/// the same mode. Nesting is then a plain depth counter, scoped to this
/// `ReteNetwork` instance (not per guard, not task-local — see the
/// concurrency note above), and — like the original fast-only guard — closes
/// in any order, not just reverse-open order. Requesting the other mode
/// while a session is open is rejected with `Err` *before* any state
/// changes (depth is not incremented, mode is not touched), so a rejected
/// `begin_batch`/`begin_ordered_batch` call does not open a guard at all and
/// leaves the existing session exactly as it was.
#[must_use = "dropping immediately ends this batch span; bind it to a name"]
#[derive(Debug)]
pub struct FactBatch<'a> {
    network: &'a ReteNetwork,
    finished: bool,
}

/// Agenda-maintenance strategy fixed for one open batch-nesting session.
/// See the "Ordered mode" section on [`FactBatch`] for the contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(feature = "embedding-host"), allow(dead_code))]
enum BatchMode {
    /// Defer per-fact/per-retract agenda maintenance to the single scan the
    /// outermost guard runs at close (the original, fast-coalescing mode).
    Fast,
    /// Run normal, non-batched incremental agenda maintenance on every call
    /// for the whole nesting session, trading away coalescing for an
    /// unbatched-identical firing trace. The outermost close runs no scan.
    Ordered,
}

/// `batch_depth` and the session's fixed `batch_mode`, guarded by one lock
/// so "reject a mode mismatch before touching either" is a single atomic
/// check-and-update rather than two.
#[derive(Debug, Default)]
struct BatchState {
    depth: usize,
    mode: Option<BatchMode>,
}

impl FactBatch<'_> {
    /// Explicitly end this guard's span now, surfacing the `Result` of the
    /// final-close full scan (or a poisoned-mutex error) instead of
    /// swallowing it as `Drop` does. Idempotent: `self` is consumed, so a
    /// second explicit close is not possible, and `Drop` becomes a no-op
    /// after this runs.
    pub fn close(mut self) -> Result<(), ReteError> {
        self.finished = true;
        self.network.end_batch()
    }
}

impl Drop for FactBatch<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.finished = true;
            if let Err(err) = self.network.end_batch() {
                warn!("FactBatch dropped without close(): {err}");
            }
        }
    }
}

/// Refraction key: one activation of a rule over an ordered WME id list.
///
/// Typed rather than a delimiter-joined string so the key is injective:
/// rule and fact ids are free-form and routinely contain `:` and `,`
/// (`coverage:...`, `provider:x:0`), which a `"rule:w1,w2"` encoding
/// would collide on and mis-parse at retraction time.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ActivationKey {
    rule_id: String,
    wme_ids: Vec<String>,
}

impl ActivationKey {
    fn new<'a>(rule_id: &str, wme_ids: impl IntoIterator<Item = &'a str>) -> Self {
        ActivationKey {
            rule_id: rule_id.to_owned(),
            wme_ids: wme_ids.into_iter().map(str::to_owned).collect(),
        }
    }

    fn references(&self, wme_id: &str) -> bool {
        self.wme_ids.iter().any(|w| w == wme_id)
    }
}

/// Outcome of judging an activation's `__script__` guards.
///
/// Guards are judged when an activation is about to fire, against the
/// working memory as it is then — never when the triggering WME arrives.
/// Refraction latches the *structural* match only, so the verdict does not
/// depend on the order facts were asserted in: a guard reading facts that
/// arrive after the trigger (a predicate provider's, say) sees them.
///
/// - `Pass`: the activation fires.
/// - `Fail`: the activation is dropped and un-latched (its refraction key is
///   cleared), so the next `update_agenda` rediscovers it and judges it
///   against the working memory of that cycle.
/// - `Error`: the guard could not be evaluated. The rule is treated as
///   matched (fail closed) — a broken guard on a block rule blocks — and each
///   consequence carries the error as `payload.guard_error` and in its
///   `message`, so the host can name the rule and the script error. Firing
///   the rule's own action, rather than emitting a separate error
///   consequence, keeps the rule's severity (block vs. warn) and routing
///   intact.
#[derive(Debug)]
enum GuardVerdict {
    Pass,
    Fail,
    Error(String),
}

/// Annotate the consequences of an activation whose guard errored: the raw
/// error goes in `payload.guard_error`; the `message` gains a suffix naming
/// the rule and the error so any host that prints messages surfaces both.
fn annotate_guard_error(consequences: &mut [Consequence], rule_id: &str, error: &str) {
    for consequence in consequences {
        let Some(payload) = consequence.payload.as_object_mut() else {
            continue;
        };
        let message = payload
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or_default();
        let annotated = format!(
            "{message} [rule `{rule_id}`: __script__ guard failed to evaluate ({error}); failing closed]"
        );
        payload.insert("message".to_string(), annotated.into());
        payload.insert("guard_error".to_string(), error.into());
    }
}

/// One activation consumed by
/// [`ReteNetwork::execute_agenda_items_for_fact`]: the agenda item as it was
/// when drained, and the actions its rule produced from it. `actions` are
/// descriptions only — substituted `Action` values the production network
/// built from `item`'s bindings. Nothing here executes or cascades them;
/// that is the embedding host's responsibility once it receives this value.
#[cfg(feature = "embedding-host")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutedActivation {
    pub item: AgendaItem,
    pub actions: Vec<Action>,
}

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
    fired_activations: Arc<Mutex<HashSet<ActivationKey>>>,
    /// Script condition evaluator for `__script__` pseudo-predicate conditions (038-xp-progression).
    /// Defaults to [`BuiltinScriptEvaluator`]; swap via [`ReteNetwork::with_script_evaluator`].
    script_evaluator: Box<dyn ScriptEval>,
    /// Reentrant fact-batch nesting depth and fixed session mode (depth 0 =
    /// not batching). See [`ReteNetwork::begin_batch`] and
    /// `ReteNetwork::begin_ordered_batch`.
    batch_state: Arc<Mutex<BatchState>>,
    /// Count of completed full production/agenda scans
    /// (`full_scan_update_agenda` invocations) — incremented once per scan,
    /// batched or not. Lets tests prove N batched mutations collapse into
    /// exactly one scan at outermost close, without relying on wall clock.
    agenda_pass_count: Arc<AtomicU64>,
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
            batch_state: Arc::new(Mutex::new(BatchState::default())),
            agenda_pass_count: Arc::new(AtomicU64::new(0)),
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

    /// Begin (or nest into) a fact batch: while any guard this returns is
    /// open, `assert_fact` skips its per-fact agenda-insertion work and
    /// `retract_fact` skips its trailing full rescan — alpha/beta
    /// propagation, the WME store, and refraction bookkeeping proceed
    /// exactly as without batching. When the last outstanding guard ends
    /// (explicitly via [`FactBatch::close`] or by drop), exactly one full
    /// production scan (the same algorithm [`ReteNetwork::update_agenda`]
    /// has always run) rebuilds the agenda from current alpha/beta state.
    /// See [`FactBatch`] for nesting and concurrency semantics.
    ///
    /// ```
    /// # use phronesis::{engine_types::Fact, error::ReteError, network::ReteNetwork};
    /// # async fn restore(net: &ReteNetwork, facts: Vec<Fact>) -> Result<(), ReteError> {
    /// let batch = net.begin_batch()?;
    /// for fact in facts {
    ///     net.assert_fact(fact).await?;
    /// }
    /// batch.close()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn begin_batch(&self) -> Result<FactBatch<'_>, ReteError> {
        self.push_batch_mode(BatchMode::Fast)
    }

    /// Begin (or nest into) an ordered fact batch: for the whole nesting
    /// session this guard belongs to, `assert_fact`/`retract_fact` run
    /// their normal, non-batched incremental agenda maintenance on every
    /// call instead of deferring it — the opt-in, trace-preserving
    /// counterpart to [`begin_batch`](Self::begin_batch). See the "Ordered
    /// mode" section on [`FactBatch`] for the exact guarantee and its
    /// same-mode-only nesting rule.
    ///
    /// Requires the `embedding-host` feature; not consumed by the bundled MCP.
    ///
    /// ```
    /// # use phronesis::{engine_types::Fact, error::ReteError, network::ReteNetwork};
    /// # #[cfg(feature = "embedding-host")]
    /// # async fn restore(net: &ReteNetwork, facts: Vec<Fact>) -> Result<(), ReteError> {
    /// let batch = net.begin_ordered_batch()?;
    /// for fact in facts {
    ///     net.assert_fact(fact).await?;
    /// }
    /// batch.close()?;
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "embedding-host")]
    pub fn begin_ordered_batch(&self) -> Result<FactBatch<'_>, ReteError> {
        self.push_batch_mode(BatchMode::Ordered)
    }

    /// Open a guard in `mode`: if no session is open (`depth == 0`), `mode`
    /// becomes the session's fixed mode; if a session is already open,
    /// `mode` must match it. A mismatch is rejected with `Err` before
    /// `depth` or `mode` change, so the existing session is left exactly as
    /// it was and no guard is opened.
    fn push_batch_mode(&self, mode: BatchMode) -> Result<FactBatch<'_>, ReteError> {
        let mut state = self
            .batch_state
            .lock()
            .map_err(|_| ReteError::poisoned("batch_state"))?;
        if let Some(open_mode) = state.mode
            && open_mode != mode
        {
            return Err(ReteError::Internal(format!(
                "FactBatch: cannot open a {mode:?} guard while a {open_mode:?} guard is already \
                 open on this network (depth {}); mixed-mode nesting is not supported — close \
                 every guard in the open session before switching modes",
                state.depth
            )));
        }
        state.mode = Some(mode);
        state.depth += 1;
        Ok(FactBatch {
            network: self,
            finished: false,
        })
    }

    /// Current number of outstanding batch guards (0 = not batching).
    pub fn batch_depth(&self) -> Result<usize, ReteError> {
        Ok(self
            .batch_state
            .lock()
            .map_err(|_| ReteError::poisoned("batch_state"))?
            .depth)
    }

    /// Number of successfully completed full production/agenda scans.
    /// Incremental per-fact insertions and deferred updates do not count.
    /// Useful for observing batch effectiveness without timing assertions.
    pub fn agenda_pass_count(&self) -> u64 {
        self.agenda_pass_count.load(Ordering::SeqCst)
    }

    /// `true` while the open batch-nesting session's fixed mode is
    /// `BatchMode::Fast` — i.e. `assert_fact`/`retract_fact` should defer
    /// their per-fact agenda work. `false` when no session is open, or the
    /// open session's mode is `BatchMode::Ordered`.
    fn is_batching(&self) -> Result<bool, ReteError> {
        Ok(matches!(
            self.batch_state
                .lock()
                .map_err(|_| ReteError::poisoned("batch_state"))?
                .mode,
            Some(BatchMode::Fast)
        ))
    }

    /// Close one guard; when `depth` reaches 0, the session's mode is
    /// cleared and, only for `BatchMode::Fast`, exactly one full
    /// production scan runs to catch the agenda up with whatever alpha/beta
    /// state accumulated while deferred. `BatchMode::Ordered` never
    /// deferred anything, so its outermost close runs no scan at all — a
    /// scan here could rediscover and re-fire an activation some drain
    /// inside the batch found guard-false and un-latched, which would not
    /// match what an unbatched caller sees at the same point. Called by
    /// [`FactBatch::close`] and by its `Drop` impl; since the session's mode
    /// no longer depends on stack order, guards in one session may close in
    /// any order (same as the original fast-only guard). An `end_batch`
    /// without a matching open guard (depth already 0, which should be
    /// unreachable via the public API since only a `FactBatch` created by
    /// `begin_batch`/`begin_ordered_batch` can call it, and each does so at
    /// most once) is reported as an error and leaves the state untouched.
    fn end_batch(&self) -> Result<(), ReteError> {
        let mode = {
            let mut state = self
                .batch_state
                .lock()
                .map_err(|_| ReteError::poisoned("batch_state"))?;
            match state.depth.checked_sub(1) {
                Some(next) => state.depth = next,
                None => {
                    return Err(ReteError::Internal(
                        "FactBatch: end_batch called without a matching begin_batch (depth underflow)"
                            .to_string(),
                    ));
                }
            }
            if state.depth > 0 {
                return Ok(());
            }
            state.mode.take()
        };
        match mode {
            Some(BatchMode::Fast) => self.full_scan_update_agenda(),
            Some(BatchMode::Ordered) | None => Ok(()),
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

        // While a fast-mode FactBatch is open, skip the per-fact agenda-insertion work
        // below entirely: alpha/beta propagation above already ran (beta
        // memories and alpha WME stores are current), and the outermost
        // guard's single full scan will discover every resulting match —
        // including these — against `fired_activations` exactly once. Not
        // skipping just the agenda write (and still touching
        // `fired_activations`/`activation_wmes` per fact) would leave the
        // multi-condition path doing per-fact work for nothing, since the
        // final scan recomputes everything from `beta_memory` anyway.
        if !self.is_batching()? {
            if !p_state_activations.is_empty() {
                self.add_p_state_activations(&p_state_activations)?;
            }

            // Handle single-condition rules (alpha-only path, no beta chain)
            self.update_agenda_for_wme_single_condition(&wme_id).await?;
        }

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
    /// by `activation_wmes`.  `__script__` guards are not judged here —
    /// see `GuardVerdict`.
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
            let activation_key = ActivationKey::new(
                activation.rule_id.as_str(),
                activation.token.wmes.iter().map(|w| w.id.as_str()),
            );

            if fired_activations.contains(&activation_key) {
                debug!(
                    "Skipping already-fired p-state activation: {:?}",
                    activation_key
                );
                continue;
            }

            let rule = production_network
                .find_by_rule_id(activation.rule_id.as_str())
                .map(|pn| pn.rule.clone());

            if let Some(rule) = rule {
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
        // fire again if the same fact is re-asserted later. Compare exact
        // ids, not substrings: retracting "f1" must not clobber "f10".
        {
            let mut fired_activations = self
                .fired_activations
                .lock()
                .map_err(|_| ReteError::poisoned("fired_activations"))?;
            fired_activations.retain(|key| !key.references(wme_id));
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

        // Update agenda after removal. While a fast-mode FactBatch is open this is a
        // no-op (see `update_agenda`) — no per-retract full scan runs;
        // instead the next match-or-not for this rule set is picked up by
        // the single scan the outermost guard runs at close, same as every
        // other mutation made during the batch.
        self.update_agenda().await?;

        Ok(removed_wme)
    }

    /// Add a rule to the RETE network
    pub async fn add_rule(&self, rule: Rule) -> Result<String, ReteError> {
        let unbound = rule.unbound_action_variables();
        if !unbound.is_empty() {
            warn!(
                "rule '{}' actions name variable(s) {:?} that no condition binds; they render literally",
                rule.id, unbound
            );
        }
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
        let terminal_index = state_ids.len().saturating_sub(2);
        for (index, next) in state_ids[1..].iter().enumerate() {
            // Only the terminal join is per rule (isolated when the shared
            // join for the pattern already belongs to another rule); every
            // earlier join stays shared between rules.
            let join = if index == terminal_index {
                self.create_terminal_join(&current, next)?
            } else {
                self.create_join(&current, next)?
            };
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

    fn create_terminal_join(&self, left: &str, right: &str) -> Result<String, ReteError> {
        let mut beta = self
            .beta_network
            .lock()
            .map_err(|_| ReteError::poisoned("beta_network"))?;
        Ok(beta.get_or_create_isolated_terminal(
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
            let activation_key = ActivationKey::new(&entry.rule.id, [wme.id.as_str()]);
            if fired_activations.contains(&activation_key) {
                debug!("Skipping already-fired activation: {:?}", activation_key);
                continue;
            }

            let bindings =
                crate::variable_binding::Bindings::new().can_bind(&entry.condition, &wme.fact)?;

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

    /// Update the agenda based on current network state.
    ///
    /// While a fast-mode [`FactBatch`] is open on this network, this is a no-op: the
    /// outermost guard's close runs the same scan exactly once instead, so
    /// calling `update_agenda` directly mid-batch does not bypass batching
    /// or trigger extra scans.
    ///
    /// NOTE: this does a full scan — prefer the incremental per-fact paths
    /// (`update_agenda_for_wme_single_condition`, beta-driven p-state
    /// activations) for the hot assert path; this exists for callers that
    /// need to force a full rebuild (e.g. after directly mutating rules)
    /// and for `FactBatch`'s outermost-close catch-up.
    pub async fn update_agenda(&self) -> Result<(), ReteError> {
        if self.is_batching()? {
            return Ok(());
        }
        self.full_scan_update_agenda()
    }

    /// The actual full production/agenda scan: for every rule, find its
    /// current matches in alpha (single-condition) or beta (multi-condition)
    /// state and insert any not already in `fired_activations` into the live
    /// agenda. Synchronous (no lock here is held across an `.await`) so it
    /// can run from [`FactBatch`]'s `Drop` impl, which cannot be async.
    /// Increments [`ReteNetwork::agenda_pass_count`] exactly once per call —
    /// the deterministic signal batching tests use to prove N staged
    /// mutations collapse into one scan.
    fn full_scan_update_agenda(&self) -> Result<(), ReteError> {
        // Use the persistent fired_activations set
        let mut fired_activations = self
            .fired_activations
            .lock()
            .map_err(|_| ReteError::poisoned("fired_activations"))?;

        // Get all production rules. `states` is `Vec<Arc<ProductionState>>`,
        // so this clones pointers, not every rule's conditions/actions.
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

        self.agenda_pass_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn update_single_condition_agenda(
        &self,
        state: &ProductionState,
        fired: &mut HashSet<ActivationKey>,
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
            let activation_key = ActivationKey::new(&rule.id, [wme.id.as_str()]);
            if fired.contains(&activation_key) {
                debug!("Skipping duplicate activation: {:?}", activation_key);
                continue;
            }
            let bindings =
                crate::variable_binding::Bindings::new().can_bind(condition, &wme.fact)?;
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
        fired: &mut HashSet<ActivationKey>,
    ) -> Result<(), ReteError> {
        let rule = &state.rule;
        if rule.conditions.is_empty() {
            return Ok(());
        }
        let activation_key = ActivationKey::new(&rule.id, []);
        if fired.contains(&activation_key) {
            debug!(
                "Skipping duplicate pure-script activation: {:?}",
                activation_key
            );
            return Ok(());
        }
        let bindings = crate::variable_binding::Bindings::new();
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
        fired: &mut HashSet<ActivationKey>,
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
            let activation_key =
                ActivationKey::new(&state.rule.id, token.wmes.iter().map(|wme| wme.id.as_str()));
            if fired.contains(&activation_key) {
                debug!("Skipping duplicate activation: {:?}", activation_key);
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

    /// Judge a rule's `__script__` guards against `facts` (the working
    /// memory at fire time) and the activation's `bindings`.
    ///
    /// Every guard is evaluated, so a broken guard is reported even when
    /// another guard on the same rule is false: an evaluation error
    /// dominates and yields [`GuardVerdict::Error`] (fail closed).
    fn judge_guards(
        &self,
        rule: &Rule,
        bindings: &crate::variable_binding::Bindings,
        facts: &[Fact],
    ) -> GuardVerdict {
        let mut verdict = GuardVerdict::Pass;
        for condition in Self::guard_conditions(rule) {
            let Some(script) = condition.script.as_deref() else {
                warn!(
                    "Script condition in rule '{}' has no script text — failing closed",
                    rule.id
                );
                return GuardVerdict::Error("__script__ condition has no script text".to_string());
            };
            match self
                .script_evaluator
                .evaluate(script, facts, &bindings.bindings)
            {
                Ok(true) => {}
                Ok(false) => {
                    debug!("Script condition blocked rule '{}': {}", rule.id, script);
                    verdict = GuardVerdict::Fail;
                }
                Err(e) => {
                    warn!(
                        "Script condition error in rule '{}': {} — failing closed (rule treated as matched)",
                        rule.id, e
                    );
                    return GuardVerdict::Error(e);
                }
            }
        }
        verdict
    }

    /// The `__script__` guard conditions on `rule`.
    fn guard_conditions(rule: &Rule) -> impl Iterator<Item = &Condition> {
        rule.conditions
            .iter()
            .filter(|c| c.predicate == "__script__")
    }

    /// Clone the facts in working memory — the snapshot guards are judged
    /// against. Taken once per drain: firing never changes working memory.
    fn working_memory_facts(&self) -> Result<Vec<Fact>, ReteError> {
        Ok(self
            .wme_manager
            .lock()
            .map_err(|_| ReteError::poisoned("wme_manager"))?
            .get_all()
            .iter()
            .map(|wme| wme.fact.clone())
            .collect())
    }

    /// Judge `item`'s guards, taking the working-memory snapshot on first
    /// need so guard-free drains never clone the fact base.
    fn verdict_for(
        &self,
        item: &AgendaItem,
        facts: &mut Option<Vec<Fact>>,
    ) -> Result<GuardVerdict, ReteError> {
        if Self::guard_conditions(&item.rule).next().is_none() {
            return Ok(GuardVerdict::Pass);
        }
        if facts.is_none() {
            *facts = Some(self.working_memory_facts()?);
        }
        let facts = facts.as_deref().unwrap_or_default();
        Ok(self.judge_guards(&item.rule, &item.bindings, facts))
    }

    /// Judge `item`'s guards and, if they fail, clear its refraction key —
    /// the one piece of guard/refraction handling every drain path
    /// (`pop_firable`, [`execute_agenda_items_for_fact`]) shares. Returns
    /// `None` when the guard failed (dropped and un-latched: the next
    /// [`ReteNetwork::update_agenda`] rediscovers it and judges it
    /// again against the working memory of that cycle); otherwise
    /// `Some(guard_error)`, where `guard_error` is set only when the guard
    /// could not be evaluated (fail-closed: `item` still fires).
    fn judge_and_unlatch(
        &self,
        item: &AgendaItem,
        facts: &mut Option<Vec<Fact>>,
    ) -> Result<Option<Option<String>>, ReteError> {
        match self.verdict_for(item, facts)? {
            GuardVerdict::Pass => Ok(Some(None)),
            GuardVerdict::Error(e) => Ok(Some(Some(e))),
            GuardVerdict::Fail => {
                let key =
                    ActivationKey::new(&item.rule.id, item.wme_list.iter().map(|w| w.id.as_str()));
                self.fired_activations
                    .lock()
                    .map_err(|_| ReteError::poisoned("fired_activations"))?
                    .remove(&key);
                Ok(None)
            }
        }
    }

    /// Pop the next activation whose guards do not currently fail, with the
    /// guard error it must be annotated with, if any. Nothing is retained
    /// between drains, so a long-lived session never re-judges an
    /// ever-growing set of guard-false activations.
    fn pop_firable(
        &self,
        facts: &mut Option<Vec<Fact>>,
    ) -> Result<Option<(AgendaItem, Option<String>)>, ReteError> {
        loop {
            let next = self
                .agenda
                .lock()
                .map_err(|_| ReteError::poisoned("agenda"))?
                .pop_next();
            let Some(item) = next else {
                return Ok(None);
            };
            if let Some(guard_error) = self.judge_and_unlatch(&item, facts)? {
                return Ok(Some((item, guard_error)));
            }
        }
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

    /// Execute the next agenda item whose `__script__` guards do not fail
    /// against current working memory. Internal building block for
    /// [`execute_all_agenda_items`](Self::execute_all_agenda_items); the
    /// public single-step surface is [`execute_next_agenda_item`] behind the
    /// `embedding-host` feature.
    ///
    /// The legacy `Action` shape has no room for an annotation, so an
    /// activation whose guard errored fires its actions unannotated (still
    /// fail closed); use [`fire_all_consequences`](Self::fire_all_consequences)
    /// to see the `guard_error`.
    fn execute_next_agenda_item_inner(
        &self,
        facts: &mut Option<Vec<Fact>>,
    ) -> Result<Vec<Action>, ReteError> {
        match self.pop_firable(facts)? {
            Some((item, _guard_error)) => {
                let production_network = self
                    .production_network
                    .lock()
                    .map_err(|_| ReteError::poisoned("production_network"))?;
                production_network.execute_agenda_item(&item)
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
        self.execute_next_agenda_item_inner(&mut None)
    }

    /// Drain every pending agenda item whose `wme_list` includes `fact_id`,
    /// in normal salience/FIFO firing order, leaving every other pending
    /// item on the agenda untouched — including items of higher salience
    /// that sit ahead of a matching one. Each selected item is judged and
    /// fired exactly as [`execute_next_agenda_item`](Self::execute_next_agenda_item)
    /// would: a false `__script__` guard drops the activation and clears
    /// its refraction key (the next [`ReteNetwork::update_agenda`]
    /// rediscovers it); a guard evaluation error still fires the rule's own
    /// actions, fail-closed, unannotated (the legacy `Action` shape has no
    /// room for the annotation — see `GuardVerdict`). No new error policy
    /// is introduced here; this reuses exactly the guard/refraction behavior
    /// `execute_next_agenda_item`/`execute_all_agenda_items` already have.
    ///
    /// Selection removes one best matching item at a time under the agenda
    /// lock. Unselected items retain their exact sequence and refraction
    /// state; no working-memory mutation is used to skip them.
    ///
    /// # Open `FactBatch` behavior
    ///
    /// While a [`FactBatch`] guard is open on this network in
    /// `BatchMode::Fast`, newly-exposed matches for `fact_id` may not yet
    /// be visible on the agenda, because this call does not itself trigger the batch's
    /// catch-up scan. It only drains activations already present. Call it
    /// after the batch closes (or use an ordered batch) to see matches from
    /// facts asserted during that batch.
    ///
    /// # Partial completion
    ///
    /// As with the ordinary drain, an execution error consumes the failing
    /// item and discards results already accumulated by this call. Items not
    /// yet selected remain pending, with their original sequence, including
    /// other matches for this fact. No rollback or partial-result API is
    /// promised; unselected items are never consumed by this method.
    ///
    /// Requires the `embedding-host` feature; not consumed by the bundled MCP.
    #[cfg(feature = "embedding-host")]
    pub fn execute_agenda_items_for_fact(
        &self,
        fact_id: &str,
    ) -> Result<Vec<ExecutedActivation>, ReteError> {
        let mut executed = Vec::new();
        let mut facts = None;
        loop {
            let item = self
                .agenda
                .lock()
                .map_err(|_| ReteError::poisoned("agenda"))?
                .pop_matching(|item| item.wme_list.iter().any(|w| w.id == fact_id));
            let Some(item) = item else { break };
            if self.judge_and_unlatch(&item, &mut facts)?.is_none() {
                continue;
            }
            let actions = self
                .production_network
                .lock()
                .map_err(|_| ReteError::poisoned("production_network"))?
                .execute_agenda_item(&item)?;
            executed.push(ExecutedActivation { item, actions });
        }

        Ok(executed)
    }

    /// Execute all agenda items
    pub fn execute_all_agenda_items(&self) -> Result<Vec<Action>, ReteError> {
        let start = Instant::now();

        let mut all_actions = Vec::new();
        let mut facts = None;
        loop {
            match self.execute_next_agenda_item_inner(&mut facts) {
                Ok(actions) => all_actions.extend(actions),
                Err(ReteError::EmptyAgenda) => break,
                Err(e) => return Err(e),
            }
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
    ///
    /// `__script__` guards are judged here, against the working memory at
    /// fire time — see `GuardVerdict`. An activation whose guard errored
    /// fires with a `guard_error` string in each consequence payload and
    /// the error appended to its `message`.
    pub fn fire_all_consequences(&self) -> Result<Vec<Consequence>, ReteError> {
        let start = Instant::now();
        let mut all = Vec::new();

        let mut facts = None;
        while let Some((item, guard_error)) = self.pop_firable(&mut facts)? {
            let mut consequences = self
                .production_network
                .lock()
                .map_err(|_| ReteError::poisoned("production_network"))?
                .fire_agenda_item(&item)?;
            if let Some(error) = guard_error {
                annotate_guard_error(&mut consequences, &item.rule.id, &error);
            }
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
    ///
    /// "Pending" means firable now: activations whose `__script__` guard is
    /// false against current working memory are left out (firing would
    /// drop them); a guard that errors is included, because firing fails
    /// closed.
    pub fn agenda_snapshot(&self) -> Result<Vec<AgendaItem>, ReteError> {
        let items: Vec<AgendaItem> = self
            .agenda
            .lock()
            .map_err(|_| ReteError::poisoned("agenda"))?
            .get_all_items()
            .into_iter()
            .cloned()
            .collect();
        let mut facts = None;
        let mut pending = Vec::with_capacity(items.len());
        for item in items {
            if !matches!(self.verdict_for(&item, &mut facts)?, GuardVerdict::Fail) {
                pending.push(item);
            }
        }
        Ok(pending)
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
        drop(production_network);
        // A removed rule leaves nothing behind: its pending activations
        // would fail at fire time (no production state), and its refraction
        // keys would stop a rule re-added under the same id from firing.
        self.agenda
            .lock()
            .map_err(|_| ReteError::poisoned("agenda"))?
            .remove_by_condition(|item| item.rule.id == rule_id);
        self.fired_activations
            .lock()
            .map_err(|_| ReteError::poisoned("fired_activations"))?
            .retain(|key| key.rule_id != rule_id);
        // The beta terminal this rule claimed keeps its p-state marker after
        // removal; a rule re-added under the same id would inherit the stale
        // association (old pattern, old salience) through activation lookup.
        // Retire the marker, leaving shared memories and surviving rules'
        // terminals untouched.
        self.beta_network
            .lock()
            .map_err(|_| ReteError::poisoned("beta_network"))?
            .clear_p_states_for_rule(rule_id);
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
                data: None,
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
                data: None,
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

/// `end_batch` is private — every public-API batch test exercises it only
/// indirectly through `FactBatch::close`/`Drop`, which the static call
/// graph does not resolve through. These call it directly so the function
/// itself has a mapped test, independent of that resolution gap.
#[cfg(test)]
mod end_batch_tests {
    use super::*;

    #[test]
    fn end_batch_runs_the_catch_up_scan_only_once_depth_reaches_zero() -> Result<(), ReteError> {
        let net = ReteNetwork::new();
        let passes_before = net.agenda_pass_count();

        {
            let mut state = net
                .batch_state
                .lock()
                .map_err(|_| ReteError::poisoned("batch_state"))?;
            state.mode = Some(BatchMode::Fast);
            state.depth = 2;
        }
        net.end_batch()?;
        assert_eq!(
            net.agenda_pass_count(),
            passes_before,
            "depth still 1: no scan yet"
        );

        net.end_batch()?;
        assert_eq!(
            net.agenda_pass_count(),
            passes_before + 1,
            "depth now 0: exactly one scan ran"
        );
        Ok(())
    }

    /// Ordered mode's outermost close must not run the scan at all — see
    /// the "Ordered mode" section on `FactBatch`.
    #[test]
    fn end_batch_in_ordered_mode_never_runs_the_scan() -> Result<(), ReteError> {
        let net = ReteNetwork::new();
        let passes_before = net.agenda_pass_count();

        {
            let mut state = net
                .batch_state
                .lock()
                .map_err(|_| ReteError::poisoned("batch_state"))?;
            state.mode = Some(BatchMode::Ordered);
            state.depth = 1;
        }
        net.end_batch()?;
        assert_eq!(net.agenda_pass_count(), passes_before);
        Ok(())
    }

    #[test]
    fn end_batch_without_a_matching_begin_batch_errors_and_leaves_depth_untouched()
    -> Result<(), ReteError> {
        let net = ReteNetwork::new();
        let Err(err) = net.end_batch() else {
            panic!("expected an error from an unmatched end_batch");
        };
        assert!(matches!(err, ReteError::Internal(_)));
        assert_eq!(
            net.batch_state
                .lock()
                .map_err(|_| ReteError::poisoned("batch_state"))?
                .depth,
            0
        );
        Ok(())
    }
}

#[cfg(all(test, feature = "embedding-host"))]
#[path = "network/embedding_tests.rs"]
mod embedding_tests;
