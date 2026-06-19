//! Adapter layer: command pattern → toolchain output parser → neutral facts.
//!
//! Each adapter knows one toolchain's output format and emits only the neutral
//! `OutcomeFact`s in `facts.rs`. The registry picks the first adapter that
//! recognizes the command. This is the single place that knows toolchain
//! specifics — everything above it is language-neutral, so a new adapter
//! generalizes confidence scoring to a new ecosystem.

use crate::outcomes::cargo::CargoAdapter;
use crate::outcomes::facts::OutcomeFact;

/// Parses one toolchain's command output into neutral outcome facts.
pub trait OutcomeAdapter {
    /// Does this adapter handle the given shell command?
    fn handles(&self, command: &str) -> bool;
    /// Parse `output` (the tool call's stdout/stderr) into neutral facts keyed
    /// by `subject`. `command` is provided so an adapter can distinguish, e.g.,
    /// a build from a test invocation.
    fn parse(&self, subject: &str, command: &str, output: &str) -> Vec<OutcomeFact>;
}

/// The registered adapters, in priority order. Ships cargo only; pytest / tsc /
/// go test land later behind the same trait.
fn registry() -> Vec<Box<dyn OutcomeAdapter>> {
    vec![Box::new(CargoAdapter)]
}

/// Does any registered adapter recognize this command? Lets callers skip
/// opening a work unit for irrelevant commands (e.g. `ls`).
pub fn handles(command: &str) -> bool {
    registry().iter().any(|a| a.handles(command))
}

/// Extract neutral outcome facts from a tool call's `command` + `output`, keyed
/// by `subject`. Returns empty when no adapter recognizes the command — a
/// non-build/test command produces no grounded signal, which is correct.
pub fn extract(subject: &str, command: &str, output: &str) -> Vec<OutcomeFact> {
    for adapter in registry() {
        if adapter.handles(command) {
            return adapter.parse(subject, command, output);
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_command_yields_no_facts() {
        let facts = extract("u", "ls -la", "total 0\n");
        assert!(facts.is_empty());
    }

    #[test]
    fn cargo_command_is_routed_to_an_adapter() {
        let facts = extract("u", "cargo build", "   Finished dev profile in 0.4s\n");
        assert!(
            facts.iter().any(|f| f.predicate == "build_outcome"),
            "a cargo command should produce a build_outcome via the cargo adapter"
        );
    }
}
