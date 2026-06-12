//! Typed engine errors.
//!
//! Every fallible engine operation returns [`ReteError`] so hosts can
//! discriminate failures (`FactNotFound` is retryable/ignorable where
//! `LockPoisoned` is not) instead of string-matching. `Display` output
//! preserves the legacy `Result<_, String>` message text, and
//! `From<ReteError> for String` keeps `?`-based hosts that still carry
//! string errors compiling through the transition.

/// Errors produced by the RETE engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReteError {
    /// A `Mutex` guarding engine state was poisoned by a panicking thread.
    LockPoisoned { lock: &'static str },
    /// `assert_fact` was given an id already held by a fact with
    /// different content. (Identical re-asserts are a no-op, not an error.)
    DuplicateFactId(String),
    /// No fact with this id is in working memory.
    FactNotFound(String),
    /// No rule with this id is loaded.
    RuleNotFound(String),
    /// Internal production-network state id lookup failed.
    ProductionStateNotFound(String),
    /// A name passed as a variable did not start with `?`.
    InvalidVariable(String),
    /// A variable would be bound to a value conflicting with its
    /// existing binding.
    BindingConflict {
        variable: String,
        existing: String,
        attempted: String,
    },
    /// A condition structurally cannot match a fact (predicate, arity,
    /// or constant-argument mismatch). Matching callers treat this as
    /// "no match".
    ConditionMismatch(String),
    /// A `__script__` condition was present without a script body.
    ScriptMissing { rule_id: String },
    /// A script expression failed to parse or evaluate.
    ScriptEval(String),
    /// `execute_next_agenda_item` was called on an empty agenda.
    EmptyAgenda,
    /// Invariant violation that callers cannot act on programmatically.
    Internal(String),
}

impl ReteError {
    /// Shorthand for the lock-poisoned case.
    pub fn poisoned(lock: &'static str) -> Self {
        ReteError::LockPoisoned { lock }
    }
}

impl std::fmt::Display for ReteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReteError::LockPoisoned { lock } => {
                write!(f, "engine lock '{}' poisoned by a panicking thread", lock)
            }
            ReteError::DuplicateFactId(id) => write!(
                f,
                "duplicate fact id '{}' with different content — retract it first or use a distinct id",
                id
            ),
            ReteError::FactNotFound(id) => write!(f, "WME with ID {} not found", id),
            ReteError::RuleNotFound(id) => write!(f, "Rule '{}' not found", id),
            ReteError::ProductionStateNotFound(id) => {
                write!(f, "Production state with ID {} not found", id)
            }
            ReteError::InvalidVariable(name) => {
                write!(f, "'{}' is not a variable (must start with '?')", name)
            }
            ReteError::BindingConflict {
                variable,
                existing,
                attempted,
            } => write!(
                f,
                "Variable '{}' already bound to '{}' but trying to bind to '{}'",
                variable, existing, attempted
            ),
            ReteError::ConditionMismatch(what) => write!(f, "{}", what),
            ReteError::ScriptMissing { rule_id } => {
                write!(f, "Script condition missing script in rule '{}'", rule_id)
            }
            ReteError::ScriptEval(msg) => write!(f, "{}", msg),
            ReteError::EmptyAgenda => write!(f, "No items in agenda"),
            ReteError::Internal(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for ReteError {}

/// Transition shim: hosts that still carry `Result<_, String>` can keep
/// using `?` on engine calls.
impl From<ReteError> for String {
    fn from(e: ReteError) -> Self {
        e.to_string()
    }
}
