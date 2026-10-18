//! Sandboxed Rhai evaluator for language-pack adapter scripts
//! (`docs/superpowers/specs/2026-10-17-go-and-language-packs-design.md`
//! §"Record and runtime contract").
//!
//! An adapter script receives one read-only constant per top-level key of
//! its JSON input object — a language pack's `adapter.rhai` sees `context`
//! (the [`FileContext`](../../phronesis-mcp/src/language_packs/records.rs)
//! for the file being analyzed) and `matches` (the query captures for that
//! file) — and must return an array of records as its final expression. This
//! crate does not depend on `phronesis-mcp`; it only knows JSON in, JSON
//! array out. The host (phronesis-mcp's `language_packs::extract`) validates
//! the returned JSON against the `PackRecord` schema.
//!
//! The script cannot mutate what it was given (scope constants), call
//! `emit_fact` (not registered in this engine — that is the separate
//! fact-provider sandbox in `lib.rs`), touch the filesystem, or `eval`.

use std::sync::OnceLock;

use regex::Regex;
use rhai::packages::{Package, StandardPackage};
use rhai::{Array, Dynamic, Engine, ImmutableString, Map, Scope};
use thiserror::Error;

/// Adapter operation budget (SPEC limit): 2,000,000 Rhai operations.
const MAX_OPERATIONS: u64 = 2_000_000;
/// Adapter call-depth budget (SPEC limit).
const MAX_CALL_LEVELS: usize = 16;
/// Bounded parse-depth budget, independent of Rhai's debug/release defaults.
const MAX_EXPR_DEPTH: usize = 128;
/// Fixed string budget on top of injected input (bytes).
const MAX_STRING_SIZE: usize = 64 * 1024;
/// Fixed array-size budget on top of injected input (elements).
const MAX_ARRAY_SIZE: usize = 16_384;
/// Fixed map-size budget on top of injected input (entries).
const MAX_MAP_SIZE: usize = 16_384;
/// Extra operations granted per injected array element or map entry, so a
/// linear pass over a large `matches` array (the SPEC's "bounded allowance
/// for injected captures") is not mistaken for a runaway script.
const OPS_PER_HOST_ELEMENT: u64 = 32;

/// Symbols disabled in the adapter engine. `eval` is a keyword rather than a
/// package function (see `phronesis_rhai::sandbox_engine`'s doc comment for
/// why disabling the symbol, not omitting a function, is what closes every
/// spelling).
const DISABLED_SYMBOLS: &[&str] = &["eval"];

/// Errors from compiling or running a language-pack adapter script.
#[derive(Debug, Error)]
pub enum AdapterError {
    #[error("adapter script failed to compile: {0}")]
    Compile(String),
    #[error("adapter script error: {0}")]
    Eval(String),
    #[error("adapter script must return an array of records, got {0}")]
    NotAnArray(&'static str),
    #[error("adapter input must be a JSON object, got {0}")]
    InputNotAnObject(&'static str),
    #[error("adapter script output is not representable as JSON: {0}")]
    UnsupportedOutput(String),
    #[error("adapter output array exceeds the record limit of {0}")]
    OutputLimit(usize),
}

fn is_unicode_lowercase(ch: char) -> bool {
    static LOWERCASE_LETTER: OnceLock<Regex> = OnceLock::new();
    LOWERCASE_LETTER
        .get_or_init(|| Regex::new(r"^\p{Ll}$").expect("fixed Unicode category regex"))
        .is_match(&ch.to_string())
}

fn sandbox_engine() -> Engine {
    let mut engine = Engine::new_raw();
    let package = StandardPackage::new();
    package.register_into_engine(&mut engine);
    // Go test names use Unicode category Ll, which is narrower than Rust's
    // `char::is_lowercase` derived property (e.g. U+00AA).
    engine.register_fn("is_unicode_lowercase", is_unicode_lowercase);
    engine.set_max_call_levels(MAX_CALL_LEVELS);
    engine.set_max_expr_depths(MAX_EXPR_DEPTH, MAX_EXPR_DEPTH);
    // Rhai's default `Simple` optimizer constant-folds field access on a
    // scope constant (`context.a` becomes the literal it holds). An
    // assignment target that gets folded into a literal before it is
    // evaluated hits an `unreachable!()` inside Rhai itself instead of the
    // ordinary "cannot assign to a constant" error. Disabling optimization
    // keeps `context.a = 2` on the normal dot-assignment path, which Rhai
    // rejects cleanly.
    engine.set_optimization_level(rhai::OptimizationLevel::None);
    for symbol in DISABLED_SYMBOLS {
        engine.disable_symbol(*symbol);
    }
    engine
}

/// Rhai's own size accounting (arrays/maps/strings) of a JSON value, used to
/// extend the sandbox budget by exactly what the host injected — mirroring
/// `HostDataSize` in `lib.rs`. See that type's doc comment for why injected
/// data must never trip a limit by itself.
#[derive(Debug, Default, Clone, Copy)]
struct HostDataSize {
    arrays: usize,
    maps: usize,
    strings: usize,
}

fn json_host_size(value: &serde_json::Value) -> HostDataSize {
    let mut size = HostDataSize::default();
    accumulate_host_size(value, &mut size);
    size
}

fn accumulate_host_size(value: &serde_json::Value, size: &mut HostDataSize) {
    match value {
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
        serde_json::Value::String(s) => size.strings += s.len(),
        serde_json::Value::Array(items) => {
            size.arrays += items.len();
            for item in items {
                accumulate_host_size(item, size);
            }
        }
        serde_json::Value::Object(map) => {
            size.maps += map.len();
            for (key, value) in map {
                size.strings += key.len();
                accumulate_host_size(value, size);
            }
        }
    }
}

fn set_adapter_budget(engine: &mut Engine, host: HostDataSize) {
    let elements = (host.arrays + host.maps) as u64;
    engine.set_max_operations(MAX_OPERATIONS + OPS_PER_HOST_ELEMENT * elements);
    engine.set_max_string_size(host.strings + MAX_STRING_SIZE);
    engine.set_max_array_size(host.arrays + MAX_ARRAY_SIZE);
    engine.set_max_map_size(host.maps + MAX_MAP_SIZE);
}

/// Convert a JSON value into a Rhai `Dynamic`. Numbers that fit in `i64`
/// become Rhai integers; every other JSON number (fractional, or out of
/// `i64` range) becomes a Rhai float.
fn json_to_dynamic(value: &serde_json::Value) -> Dynamic {
    match value {
        serde_json::Value::Null => Dynamic::UNIT,
        serde_json::Value::Bool(b) => Dynamic::from(*b),
        serde_json::Value::Number(n) => n
            .as_i64()
            .map(Dynamic::from)
            .or_else(|| n.as_f64().map(Dynamic::from))
            .unwrap_or(Dynamic::UNIT),
        serde_json::Value::String(s) => Dynamic::from(s.clone()),
        serde_json::Value::Array(items) => {
            let array: Array = items.iter().map(json_to_dynamic).collect();
            Dynamic::from(array)
        }
        serde_json::Value::Object(map) => {
            let out: Map = map
                .iter()
                .map(|(k, v)| (k.into(), json_to_dynamic(v)))
                .collect();
            Dynamic::from(out)
        }
    }
}

/// Convert a Rhai `Dynamic` produced by a script back into JSON. Any type
/// not representable in the record schema (a function pointer, a custom
/// type, `char`, timestamps, …) is `UnsupportedOutput` rather than silently
/// dropped or stringified.
fn dynamic_to_json(value: &Dynamic) -> Result<serde_json::Value, AdapterError> {
    if value.is_unit() {
        return Ok(serde_json::Value::Null);
    }
    if value.is_bool() {
        return Ok(serde_json::Value::Bool(value.as_bool().unwrap_or(false)));
    }
    if let Some(i) = value.clone().try_cast::<i64>() {
        return Ok(serde_json::Value::from(i));
    }
    if let Some(f) = value.clone().try_cast::<f64>() {
        return serde_json::Number::from_f64(f)
            .map(serde_json::Value::Number)
            .ok_or_else(|| AdapterError::UnsupportedOutput("non-finite float".to_string()));
    }
    if let Some(s) = value.clone().try_cast::<ImmutableString>() {
        return Ok(serde_json::Value::String(s.to_string()));
    }
    if let Some(arr) = value.clone().try_cast::<Array>() {
        let mut out = Vec::with_capacity(arr.len());
        for item in arr {
            out.push(dynamic_to_json(&item)?);
        }
        return Ok(serde_json::Value::Array(out));
    }
    if let Some(map) = value.clone().try_cast::<Map>() {
        let mut out = serde_json::Map::with_capacity(map.len());
        for (key, item) in map {
            out.insert(key.to_string(), dynamic_to_json(&item)?);
        }
        return Ok(serde_json::Value::Object(out));
    }
    Err(AdapterError::UnsupportedOutput(
        value.type_name().to_string(),
    ))
}

/// Compile `script` without running it. Used to validate a pack's
/// `adapter.rhai` at install/validate time, independent of any file's
/// `context`/`matches` input.
pub fn validate_adapter(script: &str) -> Result<(), AdapterError> {
    sandbox_engine()
        .compile(script)
        .map(|_| ())
        .map_err(|e| AdapterError::Compile(e.to_string()))
}

/// Run `script` with one read-only scope constant per top-level key of
/// `input` (a JSON object — typically `{"context": ..., "matches": ...}`),
/// and return its final expression value as JSON. The final value must be a
/// Rhai array; anything else is `NotAnArray`.
pub fn evaluate_adapter(
    script: &str,
    input: &serde_json::Value,
) -> Result<serde_json::Value, AdapterError> {
    evaluate_adapter_with_output_limit(script, input, MAX_ARRAY_SIZE)
}

/// Run an adapter with a host-selected maximum number of top-level output
/// records. The array is checked before recursively converting it to JSON.
pub fn evaluate_adapter_with_output_limit(
    script: &str,
    input: &serde_json::Value,
    max_output_records: usize,
) -> Result<serde_json::Value, AdapterError> {
    let fields = input
        .as_object()
        .ok_or_else(|| AdapterError::InputNotAnObject(json_type_name(input)))?;

    let mut engine = sandbox_engine();
    set_adapter_budget(&mut engine, json_host_size(input));

    let mut scope = Scope::new();
    for (key, value) in fields {
        scope.push_constant(key.as_str(), json_to_dynamic(value));
    }

    // Untrusted contributor scripts run on every file of a host project;
    // an internal Rhai panic (the optimizer bug above is one instance) must
    // become a script error, never take down the calling process.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        engine.eval_with_scope::<Dynamic>(&mut scope, script)
    }));
    let result = match outcome {
        Ok(Ok(value)) => value,
        Ok(Err(e)) => return Err(AdapterError::Eval(e.to_string())),
        Err(panic) => {
            let message = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "adapter script panicked".to_string());
            return Err(AdapterError::Eval(message));
        }
    };

    let Some(output) = result.clone().try_cast::<Array>() else {
        return Err(AdapterError::NotAnArray(result.type_name()));
    };
    if output.len() > max_output_records {
        return Err(AdapterError::OutputLimit(max_output_records));
    }
    dynamic_to_json(&result)
}

fn json_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_finite_floats_are_rejected_instead_of_becoming_null() {
        let value = Dynamic::from(f64::NAN);
        let err = dynamic_to_json(&value).unwrap_err();
        assert!(matches!(err, AdapterError::UnsupportedOutput(_)), "{err:?}");
    }
}
