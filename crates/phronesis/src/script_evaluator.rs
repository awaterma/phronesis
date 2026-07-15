//! Script-like condition evaluator for rete rules.
//!
//! [`BuiltinScriptEvaluator`] is a hand-rolled parser for a small,
//! dependency-free DSL — the default `__script__` evaluator. It supports:
//! - `facts_contain('predicate', ['arg1', 'arg2', ...])`
//! - `facts_count('predicate', ['arg1', '*']) >= N`
//! - `!` negation prefix
//! - `?variable` substitution from bindings
//!
//! It is the minimal "escape hatch" for rules that can't be expressed
//! purely in predicate/argument terms. A richer scripting layer plugs in
//! behind the [`ScriptEval`] trait rather than replacing the builtin —
//! see the `phronesis-rhai` crate for a full Rhai implementation with
//! numeric comparisons and boolean combinators over fact arguments.

use crate::engine_types::Fact;
use crate::error::ReteError;
use std::collections::HashMap;

/// Evaluates a `__script__` condition against the current RETE working
/// memory, returning whether the guard passes.
///
/// The [`ReteNetwork`](crate::network::ReteNetwork) holds one
/// `Box<dyn ScriptEval>`; it defaults to [`BuiltinScriptEvaluator`] and
/// can be swapped for an alternative implementation (e.g. `phronesis-rhai`)
/// via [`ReteNetwork::with_script_evaluator`](crate::network::ReteNetwork::with_script_evaluator).
///
/// A returned `Err` is treated by the network as a *blocked* condition
/// (safe default): a broken guard never silently passes.
pub trait ScriptEval: Send + Sync + std::fmt::Debug {
    /// Evaluate `script` against `facts` and the rule's variable `bindings`.
    fn evaluate(
        &self,
        script: &str,
        facts: &[Fact],
        bindings: &HashMap<String, String>,
    ) -> Result<bool, String>;
}

/// The default, dependency-free `__script__` evaluator.
///
/// Supports `facts_contain(...)`, `facts_count(...) <op> N`, a leading `!`
/// negation, and `?variable` substitution from bindings. For richer guard
/// expressions, wire in the `phronesis-rhai` evaluator instead.
#[derive(Debug)]
pub struct BuiltinScriptEvaluator;

/// Backwards-compatible alias for [`BuiltinScriptEvaluator`], the original
/// name of the builtin evaluator before the [`ScriptEval`] trait split.
pub type ScriptEvaluator = BuiltinScriptEvaluator;

impl ScriptEval for BuiltinScriptEvaluator {
    fn evaluate(
        &self,
        script: &str,
        facts: &[Fact],
        bindings: &HashMap<String, String>,
    ) -> Result<bool, String> {
        // Delegate to the inherent method, mapping the typed engine error
        // into the trait's string contract.
        BuiltinScriptEvaluator::evaluate(self, script, facts, bindings).map_err(|e| e.to_string())
    }
}

impl BuiltinScriptEvaluator {
    pub fn new() -> Self {
        Self
    }

    /// Evaluate a script expression against the current facts.
    /// Supports:
    /// - `facts_contain('predicate', ['arg1', 'arg2', ...])` — true if matching fact exists
    /// - `facts_count('predicate', ['arg1', '*']) >= N` — count matching facts with wildcard support
    /// - `!` prefix — negation
    /// - `?variable` substitution from bindings
    pub fn evaluate(
        &self,
        script: &str,
        facts: &[Fact],
        bindings: &HashMap<String, String>,
    ) -> Result<bool, ReteError> {
        let resolved = self.substitute_variables(script, bindings);
        let (negated, expression) = if let Some(rest) = resolved.strip_prefix('!') {
            (true, rest.trim())
        } else {
            (false, resolved.as_str())
        };

        let result = if expression.starts_with("facts_contain(") {
            self.evaluate_facts_contain(expression, facts)?
        } else if expression.contains("facts_count(") {
            self.evaluate_facts_count_comparison(expression, facts)?
        } else {
            return Err(ReteError::ScriptEval(format!(
                "Unknown script expression: {}",
                expression
            )));
        };

        Ok(if negated { !result } else { result })
    }

    fn substitute_variables(&self, script: &str, bindings: &HashMap<String, String>) -> String {
        let mut result = script.to_string();
        for (var, value) in bindings {
            result = result.replace(var, value);
        }
        result
    }

    fn evaluate_facts_contain(&self, expr: &str, facts: &[Fact]) -> Result<bool, ReteError> {
        let inner = expr
            .strip_prefix("facts_contain(")
            .and_then(|s| s.strip_suffix(')'))
            .ok_or_else(|| ReteError::ScriptEval(format!("Malformed facts_contain: {}", expr)))?;

        let (predicate_part, args_part) = self.split_predicate_and_args(inner)?;

        let predicate = predicate_part.trim().trim_matches('\'').trim_matches('"');
        let args = self.parse_args_array(args_part.trim())?;

        Ok(facts.iter().any(|f| {
            f.predicate == predicate
                && f.args.len() == args.len()
                && f.args
                    .iter()
                    .zip(args.iter())
                    .all(|(actual, pattern)| pattern == "*" || actual == pattern)
        }))
    }

    fn split_predicate_and_args<'a>(
        &self,
        inner: &'a str,
    ) -> Result<(&'a str, &'a str), ReteError> {
        if let Some(bracket_start) = inner.find('[') {
            let comma_pos = inner[..bracket_start].rfind(',').ok_or_else(|| {
                ReteError::ScriptEval(format!("No comma separator in: {}", inner))
            })?;
            Ok((&inner[..comma_pos], &inner[comma_pos + 1..]))
        } else {
            Err(ReteError::ScriptEval(format!(
                "No args array found in: {}",
                inner
            )))
        }
    }

    fn parse_args_array(&self, s: &str) -> Result<Vec<String>, ReteError> {
        let inner = s
            .trim()
            .strip_prefix('[')
            .and_then(|s| s.strip_suffix(']'))
            .ok_or_else(|| ReteError::ScriptEval(format!("Malformed args array: {}", s)))?;

        if inner.trim().is_empty() {
            return Ok(vec![]);
        }

        Ok(inner
            .split(',')
            .map(|arg| arg.trim().trim_matches('\'').trim_matches('"').to_string())
            .collect())
    }

    /// Evaluate a `facts_count('predicate', ['arg1', '*']) >= N` expression.
    /// Counts facts matching the predicate and args pattern (with `*` wildcard),
    /// then applies the comparison operator against the threshold.
    fn evaluate_facts_count_comparison(
        &self,
        expr: &str,
        facts: &[Fact],
    ) -> Result<bool, ReteError> {
        let (inner, remainder) = self.split_count_expression(expr)?;
        let (op, threshold) = self.parse_comparison(remainder)?;
        let (predicate_part, args_part) = self.split_predicate_and_args(inner)?;
        let predicate = predicate_part.trim().trim_matches('\'').trim_matches('"');
        let args = self.parse_args_array(args_part.trim())?;
        let count = Self::count_matching_facts(facts, predicate, &args);

        match op {
            ">=" => Ok(count >= threshold),
            ">" => Ok(count > threshold),
            "==" => Ok(count == threshold),
            "<=" => Ok(count <= threshold),
            "<" => Ok(count < threshold),
            _ => Err(ReteError::ScriptEval(format!("Unknown operator: {}", op))),
        }
    }

    fn split_count_expression<'a>(&self, expr: &'a str) -> Result<(&'a str, &'a str), ReteError> {
        let fc_start = expr
            .find("facts_count(")
            .ok_or_else(|| ReteError::ScriptEval(format!("No facts_count in: {}", expr)))?;
        let inner_start = fc_start + "facts_count(".len();
        let mut depth = 1;
        let close_pos = expr[inner_start..]
            .char_indices()
            .find_map(|(index, ch)| {
                match ch {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
                (depth == 0).then_some(inner_start + index)
            })
            .ok_or_else(|| ReteError::ScriptEval(format!("Unclosed facts_count in: {expr}")))?;
        Ok((&expr[inner_start..close_pos], expr[close_pos + 1..].trim()))
    }

    fn count_matching_facts(facts: &[Fact], predicate: &str, args: &[String]) -> usize {
        facts
            .iter()
            .filter(|fact| {
                fact.predicate == predicate
                    && fact.args.len() >= args.len()
                    && fact
                        .args
                        .iter()
                        .zip(args)
                        .all(|(actual, pattern)| pattern == "*" || actual == pattern)
            })
            .count()
    }

    /// Parse a comparison operator and threshold from a string like ">= 5" or "< 10".
    fn parse_comparison<'a>(&self, s: &'a str) -> Result<(&'a str, usize), ReteError> {
        let s = s.trim();
        let (op, rest) = if let Some(r) = s.strip_prefix(">=") {
            (">=", r)
        } else if let Some(r) = s.strip_prefix("<=") {
            ("<=", r)
        } else if let Some(r) = s.strip_prefix("==") {
            ("==", r)
        } else if let Some(r) = s.strip_prefix('>') {
            (">", r)
        } else if let Some(r) = s.strip_prefix('<') {
            ("<", r)
        } else {
            return Err(ReteError::ScriptEval(format!(
                "No comparison operator found in: {}",
                s
            )));
        };

        let threshold: usize = rest
            .trim()
            .parse()
            .map_err(|_| ReteError::ScriptEval(format!("Invalid threshold number in: {}", s)))?;

        Ok((op, threshold))
    }
}

impl Default for BuiltinScriptEvaluator {
    fn default() -> Self {
        Self::new()
    }
}
