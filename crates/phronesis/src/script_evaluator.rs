//! Script-like condition evaluator for rete rules.
//!
//! Despite the historical "Rhai" name, this is a hand-rolled parser
//! for a small DSL — no rhai dependency. It supports:
//! - `facts_contain('predicate', ['arg1', 'arg2', ...])`
//! - `facts_count('predicate', ['arg1', '*']) >= N`
//! - `!` negation prefix
//! - `?variable` substitution from bindings
//!
//! Kept here as the minimal "escape hatch" for rules that can't be
//! expressed purely in predicate/argument terms. A future richer
//! scripting layer (real rhai, wasm, etc.) would plug in behind a
//! trait rather than replacing this.

use crate::engine_types::Fact;
use crate::error::ReteError;
use std::collections::HashMap;

/// Evaluates Rhai script conditions against the current RETE working memory.
#[derive(Debug)]
pub struct ScriptEvaluator;

impl ScriptEvaluator {
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
        // Parse: "facts_count('predicate', ['arg1', '*']) >= 5"
        let fc_start = expr
            .find("facts_count(")
            .ok_or_else(|| ReteError::ScriptEval(format!("No facts_count in: {}", expr)))?;

        // Find matching closing paren
        let inner_start = fc_start + "facts_count(".len();
        let mut depth = 1;
        let mut close_pos = inner_start;
        for (i, ch) in expr[inner_start..].char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        close_pos = inner_start + i;
                        break;
                    }
                }
                _ => {}
            }
        }

        let inner = &expr[inner_start..close_pos];
        let remainder = expr[close_pos + 1..].trim();

        // Parse the comparison: ">= 5", "> 10", "== 3", etc.
        let (op, threshold) = self.parse_comparison(remainder)?;

        // Count matching facts
        let (predicate_part, args_part) = self.split_predicate_and_args(inner)?;
        let predicate = predicate_part.trim().trim_matches('\'').trim_matches('"');
        let args = self.parse_args_array(args_part.trim())?;

        let count = facts
            .iter()
            .filter(|f| {
                f.predicate == predicate
                    && f.args.len() >= args.len()
                    && f.args
                        .iter()
                        .zip(args.iter())
                        .all(|(actual, pattern)| pattern == "*" || actual == pattern)
            })
            .count();

        let result = match op {
            ">=" => count >= threshold,
            ">" => count > threshold,
            "==" => count == threshold,
            "<=" => count <= threshold,
            "<" => count < threshold,
            _ => Err(ReteError::ScriptEval(format!("Unknown operator: {}", op)))?,
        };

        Ok(result)
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

impl Default for ScriptEvaluator {
    fn default() -> Self {
        Self::new()
    }
}
