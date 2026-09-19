//! Agent lifecycle events: sub-agent start/stop, prompts, interrupts, turn
//! stops, commits, kalpa boundaries. One type, two on-disk projections
//! (journey journal record, action-log entry), five small correlation files.
//! See `docs/specs/SPEC-agent-lifecycle-events.md`.

pub mod event;
pub mod kalpa_cli;
pub mod outcome;
pub mod record;
pub mod scrub;
pub mod state;
pub mod unit_cli;

pub use event::{Host, Kind, LifecycleEvent, Mode, PromptText, Stamped};
