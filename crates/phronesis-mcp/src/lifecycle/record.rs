//! `record` — stamp a `LifecycleEvent` and write both projections.

use std::path::Path;

use crate::action_log;
use crate::journey;
use crate::journey::tagger::PromptTextSetting;
use crate::lifecycle::event::{LifecycleEvent, PromptText, Stamped};
use crate::lifecycle::state;

fn unix_secs_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Fails **closed**. `load_config` returns `NotFound` when the project has no
/// `journey.json` at all, which the spec says means `"full"`; every other error
/// — unreadable file, malformed JSON, a `lifecycle` block serde could not
/// parse, a `prompt_text` value that is neither `"full"` nor `"none"` — is a
/// config we do not understand, and the switch must never fail open on one.
pub fn prompt_text_setting(root: &Path) -> PromptText {
    match journey::load_config(root) {
        Ok(cfg) => match cfg.lifecycle.prompt_text {
            PromptTextSetting::Full => PromptText::Full,
            PromptTextSetting::None => PromptText::None,
        },
        Err(journey::ConfigError::NotFound(_)) => PromptText::Full,
        Err(e) => {
            eprintln!("phronesis: lifecycle prompt_text unreadable ({e}); omitting prompt text");
            PromptText::None
        }
    }
}

/// The single accessor for correction text. `phr-mcp journey --corrections`,
/// the `extract_rules` hand-off, and any future MCP surface call this rather
/// than reading `entry.data["prompt"]`, so flipping `prompt_text` to `"none"`
/// hides text already on disk as well as text not yet written.
pub fn correction_text(root: &Path, entry: &crate::action_log::LogEntry) -> Option<String> {
    if prompt_text_setting(root) == PromptText::None {
        return None;
    }
    entry
        .data
        .get("prompt")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

pub fn record(root: &Path, event: LifecycleEvent) -> Option<Stamped> {
    let stamped = Stamped {
        ts: unix_secs_now(),
        sid: journey::current_sid(root),
        seq: crate::hook::seq::next_seq(root),
        kalpa: state::read_kalpa(root).map(|k| k.name),
        subject: crate::outcomes::subject::current(root),
    };
    let journaled = match journey::journal::append(root, &event.to_journal_record(&stamped)) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("phronesis: lifecycle journal append failed: {e}");
            false
        }
    };
    let entry = event.to_log_entry(&stamped, prompt_text_setting(root));
    if let Err(e) = action_log::append(&action_log::default_path(root), &entry) {
        eprintln!("phronesis: lifecycle log append failed: {e}");
    }
    journaled.then_some(stamped)
}
