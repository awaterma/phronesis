//! Network construction seam.
//!
//! All rule-evaluating sites build their [`phr::ReteNetwork`] through
//! [`build_network`] so the `__script__` evaluator is chosen in one place.
//!
//! With the `rhai` cargo feature **off** (the default), the network uses
//! the engine's dependency-free `BuiltinScriptEvaluator` — behavior is
//! unchanged. With `--features rhai`, it uses the *composite* evaluator
//! from `phronesis-rhai`: the builtin-DSL forms (`facts_contain`/
//! `facts_count`) that the shipped confidence and journey packs rely on
//! still route to the builtin evaluator, while any other `__script__`
//! condition is evaluated as sandboxed Rhai. That lets an expressive Rhai
//! guard coexist with the bundled packs in one `rules.json`.

/// Build a `ReteNetwork` with the configured `__script__` evaluator.
///
/// The evaluator is selected at compile time by the `rhai` feature; every
/// runtime construction site (server, pre/post hooks) routes through here so
/// the choice is made once.
pub fn build_network() -> phr::ReteNetwork {
    #[cfg(feature = "rhai")]
    {
        phr::ReteNetwork::with_script_evaluator(Box::new(
            phronesis_rhai::CompositeScriptEvaluator::new(),
        ))
    }
    #[cfg(not(feature = "rhai"))]
    {
        phr::ReteNetwork::new()
    }
}
