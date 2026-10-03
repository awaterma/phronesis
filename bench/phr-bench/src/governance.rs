use crate::record::GovernanceSummary;
use serde_json::Value;

/// Error type for governance telemetry parsing.
#[derive(Debug, Clone)]
pub enum GovernanceError {
    /// No hook entries found in log (empty, only lifecycle, or missing file).
    NotWired,
    /// Malformed JSON or unexpected structure.
    Malformed(String),
}

impl std::fmt::Display for GovernanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GovernanceError::NotWired => write!(f, "governance not wired (no hook entries)"),
            GovernanceError::Malformed(msg) => write!(f, "governance malformed: {}", msg),
        }
    }
}

impl std::error::Error for GovernanceError {}

/// Parse a `.phronesis/log.jsonl` string and summarize governance events.
///
/// Returns `GovernanceError::NotWired` if the log contains no hook entries,
/// signaling that the treatment run's governance telemetry was not collected.
/// This distinction is critical: a NotWired treatment arm is an error condition
/// that must surface at the runner layer as `RunExit::Error { reason: "governance_not_wired" }`.
pub fn summarize(log_jsonl: &str) -> Result<GovernanceSummary, GovernanceError> {
    let mut summary = GovernanceSummary::default();
    let mut found_hook_entry = false;

    for line in log_jsonl.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let entry = match serde_json::from_str::<Value>(trimmed) {
            Ok(v) => v,
            Err(e) => return Err(GovernanceError::Malformed(format!("JSON parse error: {}", e))),
        };

        // Only process "hook" kind entries.
        if entry.get("kind").and_then(|v| v.as_str()) != Some("hook") {
            continue;
        }

        found_hook_entry = true;

        let event = entry.get("event").and_then(|v| v.as_str()).unwrap_or("");
        let exit = entry.get("exit").and_then(|v| v.as_i64()).unwrap_or(-1);

        match event {
            "pre_check" if exit == 2 => {
                // pre_check with exit=2 (block): count blocked_by entries.
                if let Some(blocked_by) = entry.get("blocked_by").and_then(|v| v.as_array()) {
                    for block_entry in blocked_by {
                        let block_kind = block_entry.get("kind").and_then(|v| v.as_str()).unwrap_or("");
                        match block_kind {
                            "rule" => {
                                if let Some(rule_id) = block_entry.get("rule").and_then(|v| v.as_str()) {
                                    *summary.blocks.entry(rule_id.to_string()).or_insert(0) += 1;
                                }
                            }
                            "fail_closed" => {
                                summary.fail_closed += 1;
                            }
                            _ => {}
                        }
                    }
                }
            }
            "post_check" if exit == 1 => {
                // post_check with exit=1 (warn): count consequences[].rule_id entries.
                if let Some(consequences) = entry.get("consequences").and_then(|v| v.as_array()) {
                    for consequence in consequences {
                        if let Some(rule_id) = consequence.get("rule_id").and_then(|v| v.as_str()) {
                            *summary.warns.entry(rule_id.to_string()).or_insert(0) += 1;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if !found_hook_entry {
        Err(GovernanceError::NotWired)
    } else {
        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_parsing_works() {
        let log = r#"{"ts":1,"kind":"hook","event":"pre_check","exit":2,"blocked_by":[{"kind":"rule","rule":"test-rule"}]}"#;
        let summary = summarize(log).unwrap();
        assert_eq!(summary.blocks.get("test-rule"), Some(&1));
    }
}
