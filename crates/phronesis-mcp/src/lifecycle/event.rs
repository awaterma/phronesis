use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::action_log::LogEntry;
use crate::journey::journal::{JOURNAL_V, JournalRecord, LIFECYCLE_TOOL};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    SubagentStart,
    SubagentStop,
    Prompt,
    Interrupt,
    Stop,
    Commit,
    KalpaStart,
    KalpaEnd,
    /// A human named a work item: `phr-mcp unit start`.
    UnitStart,
    /// The work item closed: `phr-mcp unit end`, or the next `unit start`.
    UnitEnd,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::SubagentStart => "subagent_start",
            Kind::SubagentStop => "subagent_stop",
            Kind::Prompt => "prompt",
            Kind::Interrupt => "interrupt",
            Kind::Stop => "stop",
            Kind::Commit => "commit",
            Kind::KalpaStart => "kalpa_start",
            Kind::KalpaEnd => "kalpa_end",
            Kind::UnitStart => "unit_start",
            Kind::UnitEnd => "unit_end",
        }
    }
    pub fn tag(self) -> String {
        format!("lifecycle:{}", self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Fresh,
    MidTurn,
    Correction,
}
impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Fresh => "fresh",
            Mode::MidTurn => "mid_turn",
            Mode::Correction => "correction",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Host {
    Claude,
    Codex,
    Gemini,
    Cli,
}
impl Host {
    pub fn as_str(self) -> &'static str {
        match self {
            Host::Claude => "claude",
            Host::Codex => "codex",
            Host::Gemini => "gemini",
            Host::Cli => "cli",
        }
    }
}

/// Whether the action-log projection carries prompt text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptText {
    Full,
    None,
}

/// The closed vocabulary of `extra` keys (spec §"Host adapters / Shared
/// module"). `extra` never carries message or transcript content:
/// `last_assistant_message`, `prompt_response`, `transcript_path` and
/// `agent_transcript_path` are read for decisions and dropped at the adapter
/// boundary, and `tests/hook_integration.rs` asserts no log entry contains them.
pub const EXTRA_KEYS: [&str; 15] = [
    "inferred_from",
    "stop_hook_active",
    "matched_start",
    "duration_secs",
    "sha",
    "head_before",
    "confidence_band",
    "tool_use_id",
    // Why commit detection was skipped for a shell call: "timeout" or
    // "no_exit_code" (spec §"Success signal: commit").
    "detection",
    // Work items (spec §"Work items and governed throughput" / Storage). The
    // spec pointer is a repo-relative path, not a hash: if the spec changes
    // after the unit starts, the report shows the path only.
    "spec",
    // The work-unit id, duplicated out of `subject` so a log reader does not
    // have to know that `subject` and the unit id are the same thing.
    "unit_id",
    // `true` on a `unit_end` for a unit that was never explicitly started.
    "implicit",
    // A unit named from `.phronesis/bugs.json` (spec §"Where the name comes
    // from"): the registry's cargo test name and the id it was looked up by.
    "test",
    "bug_id",
    // A `subagent_stop` compensating for a `subagent_start` whose tool call was
    // blocked before it ran: the pair is closed, but nothing ever executed.
    "blocked",
];

/// Lowercase, then keep only if the result matches `[a-z0-9][a-z0-9_.:-]{0,63}` —
/// the same treatment the kalpa name gets, and for the same reason: on Gemini
/// `agent_type` is `tool_input.agent_name`, model-generated free text that
/// reaches a journal tag (`lifecycle:agent:<agent_type>`, hence a RETE fact),
/// the action log, and the `agents` file. Anything else is stored as absent and
/// the `lifecycle:agent:*` tag is dropped (spec §"Correlation state").
pub fn sanitize_agent_type(raw: &str) -> Option<String> {
    let lowered = raw.to_ascii_lowercase();
    let b = lowered.as_bytes();
    if !(1..=64).contains(&b.len()) {
        return None;
    }
    if !(b[0].is_ascii_lowercase() || b[0].is_ascii_digit()) {
        return None;
    }
    // Wider than the kalpa pattern on purpose: Gemini built-ins are snake_case
    // and Claude plugin agents are colon-qualified; both must survive as tags.
    b.iter()
        .all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(*c, b'-' | b'_' | b'.' | b':')
        })
        .then_some(lowered)
}

/// Values stamped at write time by `record::record`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamped {
    pub ts: u64,
    pub sid: String,
    pub seq: u64,
    pub kalpa: Option<String>,
    pub subject: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleEvent {
    pub kind: Kind,
    pub host: Host,
    pub mode: Option<Mode>,
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    pub agent_id: Option<String>,
    pub agent_type: Option<String>,
    /// Already scrubbed by `lifecycle::scrub::scrub_prompt`. Never journaled.
    pub prompt: Option<String>,
    pub extra: Map<String, Value>,
}

impl LifecycleEvent {
    pub fn new(kind: Kind, host: Host) -> Self {
        Self {
            kind,
            host,
            mode: None,
            session_id: None,
            turn_id: None,
            agent_id: None,
            agent_type: None,
            prompt: None,
            extra: Map::new(),
        }
    }
    pub fn with_mode(mut self, m: Mode) -> Self {
        self.mode = Some(m);
        self
    }
    pub fn with_session(mut self, id: impl Into<String>) -> Self {
        self.session_id = Some(id.into());
        self
    }
    pub fn with_turn(mut self, id: impl Into<String>) -> Self {
        self.turn_id = Some(id.into());
        self
    }
    /// Sanitizes `agent_type` here, once, so no adapter can forget: on Gemini
    /// it is `tool_input.agent_name`, model-generated free text that reaches a
    /// journal tag and therefore a RETE fact.
    pub fn with_agent(mut self, id: impl Into<String>, agent_type: Option<String>) -> Self {
        self.agent_id = Some(id.into());
        self.agent_type = agent_type.as_deref().and_then(sanitize_agent_type);
        self
    }
    pub fn with_prompt(mut self, scrubbed: impl Into<String>) -> Self {
        self.prompt = Some(scrubbed.into());
        self
    }
    /// `key` must come from `EXTRA_KEYS`. The debug assertion is the guard: a
    /// typo or a new field lands in the vocabulary deliberately, in this file,
    /// rather than appearing in the action log by accident.
    pub fn with_extra(mut self, key: &str, v: impl Into<Value>) -> Self {
        debug_assert!(
            EXTRA_KEYS.contains(&key),
            "extra key `{key}` is outside the closed vocabulary"
        );
        self.extra.insert(key.to_string(), v.into());
        self
    }

    pub fn tags(&self, kalpa: Option<&str>) -> Vec<String> {
        let mut t = vec![self.kind.tag()];
        if let Some(m) = self.mode
            && matches!(self.kind, Kind::Prompt)
        {
            t.push(format!("lifecycle:prompt:{}", m.as_str()));
            // The autonomy signal: the human changed the plan (steered mid-turn
            // or corrected after an interrupt), as opposed to replying. Only at
            // top level — a prompt carrying an `agent_id` was delivered inside a
            // sub-agent, so the human did not speak and it is not an
            // intervention (spec §"Event model", Intervention).
            if matches!(m, Mode::MidTurn | Mode::Correction) && self.agent_id.is_none() {
                t.push("lifecycle:intervention".to_string());
            }
        }
        if let Some(at) = self.agent_type.as_deref().filter(|s| !s.is_empty())
            && matches!(self.kind, Kind::SubagentStart | Kind::SubagentStop)
        {
            t.push(format!("lifecycle:agent:{at}"));
        }
        if let Some(k) = kalpa {
            t.push(format!("kalpa:{k}"));
        }
        t
    }

    pub fn to_journal_record(&self, s: &Stamped) -> JournalRecord {
        JournalRecord {
            v: JOURNAL_V,
            ts: s.ts,
            sid: s.sid.clone(),
            seq: s.seq,
            tool: LIFECYCLE_TOOL.to_string(),
            path: String::new(),
            ext: None,
            module: None,
            tags: self.tags(s.kalpa.as_deref()),
            subject: s.subject.clone(),
            command_exit: None,
            kind: Some(self.kind.as_str().to_string()),
            mode: self.mode.map(|m| m.as_str().to_string()),
            host: Some(self.host.as_str().to_string()),
            turn: self.turn_id.clone(),
            agent: self.agent_id.clone(),
            agent_type: self.agent_type.clone(),
            kalpa: s.kalpa.clone(),
        }
    }

    pub fn to_log_entry(&self, s: &Stamped, text: PromptText) -> LogEntry {
        let mut e = LogEntry::new("lifecycle", self.kind.as_str())
            .with("host", self.host.as_str())
            .with("sid", s.sid.clone())
            .with("seq", s.seq);
        e.ts = s.ts;
        if let Some(m) = self.mode {
            e = e.with("mode", m.as_str());
        }
        if let Some(v) = &self.session_id {
            e = e.with("session_id", v.clone());
        }
        if let Some(v) = &self.turn_id {
            e = e.with("turn_id", v.clone());
        }
        if let Some(v) = &self.agent_id {
            e = e.with("agent_id", v.clone());
        }
        if let Some(v) = &self.agent_type {
            e = e.with("agent_type", v.clone());
        }
        if let Some(v) = &s.kalpa {
            e = e.with("kalpa", v.clone());
        }
        if let Some(v) = &s.subject {
            e = e.with("subject", v.clone());
        }
        if let Some(p) = &self.prompt {
            e = e.with("prompt_bytes", p.len() as u64);
            if text == PromptText::Full {
                e = e.with("prompt", p.clone());
            }
        }
        for (k, v) in &self.extra {
            e.data.insert(k.clone(), v.clone());
        }
        e
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_for_correction_prompt_with_kalpa() {
        let e = LifecycleEvent::new(Kind::Prompt, Host::Claude).with_mode(Mode::Correction);
        assert_eq!(
            e.tags(Some("demo")),
            vec![
                "lifecycle:prompt",
                "lifecycle:prompt:correction",
                "lifecycle:intervention",
                "kalpa:demo"
            ]
        );
    }
    #[test]
    fn intervention_tag_only_on_mid_turn_and_correction() {
        let fresh = LifecycleEvent::new(Kind::Prompt, Host::Claude).with_mode(Mode::Fresh);
        assert_eq!(
            fresh.tags(None),
            vec!["lifecycle:prompt", "lifecycle:prompt:fresh"]
        );
        let mid = LifecycleEvent::new(Kind::Prompt, Host::Codex).with_mode(Mode::MidTurn);
        assert!(
            mid.tags(None)
                .contains(&"lifecycle:intervention".to_string())
        );
        let stop = LifecycleEvent::new(Kind::Stop, Host::Claude);
        assert!(!stop.tags(None).iter().any(|t| t.contains("intervention")));
    }
    #[test]
    fn tags_for_subagent_with_type() {
        let e = LifecycleEvent::new(Kind::SubagentStop, Host::Codex)
            .with_agent("a1", Some("reviewer".into()));
        assert_eq!(
            e.tags(None),
            vec!["lifecycle:subagent_stop", "lifecycle:agent:reviewer"]
        );
    }
    /// `agent_type` is model-supplied free text on Gemini (`tool_input.agent_name`)
    /// and reaches a journal tag, hence a RETE fact. It gets the kalpa name's
    /// treatment: lowercase, then keep only `[a-z0-9][a-z0-9_.:-]{0,63}`. Anything
    /// else is stored as absent and the tag is dropped entirely.
    #[test]
    fn agent_type_is_sanitized_before_it_becomes_a_tag() {
        assert_eq!(sanitize_agent_type("Explore").as_deref(), Some("explore"));
        assert_eq!(
            sanitize_agent_type("code-reviewer-2").as_deref(),
            Some("code-reviewer-2")
        );
        assert_eq!(
            sanitize_agent_type("codebase_investigator").as_deref(),
            Some("codebase_investigator"),
            "Gemini snake_case survives"
        );
        assert_eq!(
            sanitize_agent_type("code-simplifier:code-simplifier").as_deref(),
            Some("code-simplifier:code-simplifier"),
            "Claude plugin agents survive"
        );
        assert_eq!(sanitize_agent_type("../../etc/passwd"), None);
        assert_eq!(
            sanitize_agent_type("kalpa:evil").as_deref(),
            Some("kalpa:evil"),
            "colons are allowed; the tag is lifecycle:agent:kalpa:evil, which no kalpa:* selector matches"
        );
        assert_eq!(sanitize_agent_type("spaces here"), None);
        assert_eq!(sanitize_agent_type(""), None);
        assert_eq!(sanitize_agent_type(&"a".repeat(65)), None);
        assert_eq!(sanitize_agent_type("-lead"), None);

        let e = LifecycleEvent::new(Kind::SubagentStart, Host::Claude)
            .with_agent("a1", Some("Explore".into()));
        assert_eq!(
            e.agent_type.as_deref(),
            Some("explore"),
            "stored sanitized, not just tagged"
        );
        assert_eq!(
            e.tags(None),
            vec!["lifecycle:subagent_start", "lifecycle:agent:explore"]
        );

        let hostile = LifecycleEvent::new(Kind::SubagentStart, Host::Gemini)
            .with_agent("a2", Some("hack me; rm -rf /".into()));
        assert_eq!(hostile.agent_type, None);
        assert_eq!(hostile.tags(None), vec!["lifecycle:subagent_start"]);
    }
    /// Spec §"Event model", Intervention: a prompt delivered *inside* a sub-agent
    /// is never an intervention, because the human did not speak.
    #[test]
    fn a_sub_agent_prompt_is_never_tagged_as_an_intervention() {
        let inside = LifecycleEvent::new(Kind::Prompt, Host::Claude)
            .with_mode(Mode::Correction)
            .with_agent("sub-1", None);
        assert_eq!(
            inside.tags(None),
            vec!["lifecycle:prompt", "lifecycle:prompt:correction"],
            "no lifecycle:intervention on a sub-agent's prompt"
        );
        // The same mode at top level is an intervention.
        let top = LifecycleEvent::new(Kind::Prompt, Host::Claude).with_mode(Mode::Correction);
        assert!(top.tags(None).iter().any(|t| t == "lifecycle:intervention"));
    }
    /// The `extra` map is a closed vocabulary; nothing outside it, and never
    /// message or transcript content.
    #[test]
    fn extra_keys_are_a_closed_vocabulary() {
        for k in EXTRA_KEYS {
            let e = LifecycleEvent::new(Kind::Stop, Host::Claude).with_extra(k, "x");
            assert!(e.extra.contains_key(k));
        }
        for forbidden in [
            "prompt_response",
            "last_assistant_message",
            "transcript_path",
            "agent_transcript_path",
        ] {
            assert!(!EXTRA_KEYS.contains(&forbidden), "{forbidden}");
        }
    }
    #[test]
    fn journal_record_never_carries_prompt_text() {
        let e = LifecycleEvent::new(Kind::Prompt, Host::Claude)
            .with_mode(Mode::Fresh)
            .with_prompt("SECRET TEXT");
        let s = Stamped {
            ts: 1,
            sid: "s-a".into(),
            seq: 2,
            kalpa: None,
            subject: None,
        };
        let rec = e.to_journal_record(&s);
        assert_eq!(rec.tool, "__lifecycle");
        assert_eq!(rec.path, "");
        assert_eq!(rec.v, crate::journey::journal::JOURNAL_V);
        assert!(!serde_json::to_string(&rec).unwrap().contains("SECRET"));
        assert_eq!(rec.kind.as_deref(), Some("prompt"));
        assert_eq!(rec.mode.as_deref(), Some("fresh"));
    }
    #[test]
    fn log_entry_carries_prompt_only_when_full() {
        let e = LifecycleEvent::new(Kind::Prompt, Host::Claude)
            .with_mode(Mode::Fresh)
            .with_prompt("hello");
        let s = Stamped {
            ts: 1,
            sid: "s-a".into(),
            seq: 2,
            kalpa: Some("k".into()),
            subject: None,
        };
        let full = serde_json::to_value(e.to_log_entry(&s, PromptText::Full)).unwrap();
        assert_eq!(full["kind"], "lifecycle");
        assert_eq!(full["event"], "prompt");
        assert_eq!(full["prompt"], "hello");
        assert_eq!(full["prompt_bytes"], 5);
        assert_eq!(full["kalpa"], "k");
        assert_eq!(full["sid"], "s-a");
        assert_eq!(full["seq"], 2);
        let none = serde_json::to_value(e.to_log_entry(&s, PromptText::None)).unwrap();
        assert!(none.get("prompt").is_none());
        assert_eq!(none["prompt_bytes"], 5);
    }
    #[test]
    fn extra_fields_flatten_into_log_entry() {
        let e = LifecycleEvent::new(Kind::SubagentStop, Host::Codex)
            .with_extra("duration_secs", 12u64)
            .with_extra("matched_start", true);
        let s = Stamped {
            ts: 1,
            sid: "s".into(),
            seq: 1,
            kalpa: None,
            subject: None,
        };
        let v = serde_json::to_value(e.to_log_entry(&s, PromptText::Full)).unwrap();
        assert_eq!(v["duration_secs"], 12);
        assert_eq!(v["matched_start"], true);
    }

    /// The two selectors spec §"Event model" lists for work items. They are in the
    /// closed set `validate_selectors` exempts, so a rule may scope to them.
    #[test]
    fn unit_kinds_have_their_spec_names_and_tags() {
        assert_eq!(Kind::UnitStart.as_str(), "unit_start");
        assert_eq!(Kind::UnitEnd.as_str(), "unit_end");
        assert_eq!(Kind::UnitStart.tag(), "lifecycle:unit_start");
        assert_eq!(Kind::UnitEnd.tag(), "lifecycle:unit_end");
        // The claim above, checked: both tags are in the closed set, so a rule
        // scoped to them validates instead of failing as `UndefinedSelector`.
        for tag in [Kind::UnitStart.tag(), Kind::UnitEnd.tag()] {
            assert!(
                crate::journey::derive::LIFECYCLE_SELECTORS.contains(&tag.as_str()),
                "{tag} must be a built-in selector"
            );
        }
        let e = LifecycleEvent::new(Kind::UnitStart, Host::Cli);
        assert_eq!(
            e.tags(Some("demo")),
            vec!["lifecycle:unit_start", "kalpa:demo"]
        );
        // A unit boundary is not a prompt, so it can never be an intervention.
        assert!(!e.tags(None).iter().any(|t| t.contains("intervention")));
    }

    /// `extra` gains exactly five keys and no more (spec §"Work items / Storage"
    /// plus §"Where the name comes from", which adds `test` and `bug_id`).
    #[test]
    fn extra_vocabulary_gains_spec_unit_id_and_implicit() {
        for k in ["spec", "unit_id", "implicit", "test", "bug_id", "blocked"] {
            assert!(
                EXTRA_KEYS.contains(&k),
                "{k} must be in the closed vocabulary"
            );
        }
        assert_eq!(EXTRA_KEYS.len(), 15, "six added, nothing else");
        for forbidden in [
            "prompt_response",
            "last_assistant_message",
            "transcript_path",
            "spec_hash",
            "title",
        ] {
            assert!(!EXTRA_KEYS.contains(&forbidden), "{forbidden}");
        }
    }

    /// The `unit_start` projection: `unit_id` and `spec` flatten into the action
    /// log, and `subject` is the same id, which is what the report joins on.
    #[test]
    fn unit_start_log_entry_carries_unit_id_spec_and_subject() {
        let e = LifecycleEvent::new(Kind::UnitStart, Host::Cli)
            .with_extra("unit_id", "unit-42")
            .with_extra("spec", "docs/specs/SPEC-agent-lifecycle-events.md");
        let s = Stamped {
            ts: 7,
            sid: "s-a".into(),
            seq: 3,
            kalpa: Some("k".into()),
            subject: Some("unit-42".into()),
        };
        let v = serde_json::to_value(e.to_log_entry(&s, PromptText::Full)).unwrap();
        assert_eq!(v["kind"], "lifecycle");
        assert_eq!(v["event"], "unit_start");
        assert_eq!(v["unit_id"], "unit-42");
        assert_eq!(v["spec"], "docs/specs/SPEC-agent-lifecycle-events.md");
        assert_eq!(v["subject"], "unit-42");
        let rec = e.to_journal_record(&s);
        assert_eq!(rec.kind.as_deref(), Some("unit_start"));
        assert_eq!(rec.subject.as_deref(), Some("unit-42"));
        assert_eq!(rec.tool, "__lifecycle");
    }

    /// `implicit` marks a unit that was never explicitly started — the flag that
    /// keeps the `explicit` / `implicit` split in `kalpa show` honest.
    #[test]
    fn unit_end_can_be_marked_implicit() {
        let e = LifecycleEvent::new(Kind::UnitEnd, Host::Cli)
            .with_extra("unit_id", "unit-9")
            .with_extra("implicit", true);
        let s = Stamped {
            ts: 1,
            sid: "s".into(),
            seq: 1,
            kalpa: None,
            subject: Some("unit-9".into()),
        };
        let v = serde_json::to_value(e.to_log_entry(&s, PromptText::Full)).unwrap();
        assert_eq!(v["implicit"], true);
        assert_eq!(v["event"], "unit_end");
    }
}
