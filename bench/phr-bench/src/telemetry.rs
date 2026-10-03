use anyhow::{bail, Result};
use serde::Serialize;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct TranscriptStats {
    pub assistant_events: u32,
    pub tool_use_events: u32,
    pub turns: u32,
    pub tokens_in: Option<u64>,
    pub tokens_out: Option<u64>,
    pub duration_ms: Option<u64>,
    pub malformed_events: u32,
}

/// Parse a Claude stream-json transcript, tolerating individual damaged lines.
pub fn parse_transcript(jsonl: &str) -> Result<TranscriptStats> {
    let mut stats = TranscriptStats {
        assistant_events: 0,
        tool_use_events: 0,
        turns: 0,
        tokens_in: None,
        tokens_out: None,
        duration_ms: None,
        malformed_events: 0,
    };
    let mut result_turns = None;

    for line in jsonl.lines().filter(|line| !line.trim().is_empty()) {
        let event: serde_json::Value = match serde_json::from_str(line) {
            Ok(event) => event,
            Err(_) => {
                stats.malformed_events = stats.malformed_events.saturating_add(1);
                continue;
            }
        };
        match event.get("type").and_then(serde_json::Value::as_str) {
            Some("assistant") => {
                stats.assistant_events = stats.assistant_events.saturating_add(1);
                if let Some(content) = event
                    .get("message")
                    .and_then(|message| message.get("content"))
                    .and_then(serde_json::Value::as_array)
                {
                    for item in content {
                        if item.get("type").and_then(serde_json::Value::as_str) == Some("tool_use") {
                            stats.tool_use_events = stats.tool_use_events.saturating_add(1);
                        }
                    }
                }
            }
            Some("result") => {
                let usage = event.get("usage");
                stats.tokens_in = usage
                    .and_then(|value| value.get("input_tokens"))
                    .and_then(serde_json::Value::as_u64);
                stats.tokens_out = usage
                    .and_then(|value| value.get("output_tokens"))
                    .and_then(serde_json::Value::as_u64);
                stats.duration_ms = event
                    .get("duration_ms")
                    .and_then(serde_json::Value::as_u64);
                result_turns = event
                    .get("num_turns")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|turns| u32::try_from(turns).ok());
            }
            _ => {}
        }
    }

    if stats.assistant_events == 0 {
        bail!("transcript has no assistant events");
    }
    stats.turns = result_turns.unwrap_or(stats.assistant_events);
    Ok(stats)
}
