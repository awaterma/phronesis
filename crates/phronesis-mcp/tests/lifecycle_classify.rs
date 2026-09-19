use phronesis_mcp::lifecycle::state::*;
use phronesis_mcp::lifecycle::{Host, Mode};

fn root() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}
fn ctx<'a>(host: Host, now: u64) -> PromptContext<'a> {
    PromptContext {
        host,
        now,
        agent_id: None,
        turn_id: None,
        transcript_path: None,
    }
}

#[test]
fn closed_turn_is_fresh() {
    let d = root();
    let c = classify_prompt(d.path(), &ctx(Host::Claude, 100));
    assert_eq!(c.mode, Mode::Fresh);
    assert!(c.interrupt.is_none());
}

#[test]
fn open_turn_no_evidence_is_mid_turn_on_claude_and_codex() {
    for host in [Host::Claude, Host::Codex] {
        let d = root();
        open_turn(d.path(), None, 10);
        let c = classify_prompt(d.path(), &ctx(host, 100));
        assert_eq!(c.mode, Mode::MidTurn, "{host:?}");
        assert!(c.interrupt.is_none());
    }
}

#[test]
fn open_turn_on_gemini_is_interrupt_correction() {
    let d = root();
    open_turn(d.path(), None, 10);
    let c = classify_prompt(d.path(), &ctx(Host::Gemini, 100));
    assert_eq!(c.mode, Mode::Correction);
    assert_eq!(c.interrupt.map(|i| i.as_str()), Some("open_turn"));
}

#[test]
fn live_inflight_in_scope_is_interrupt_and_drops_every_entry() {
    let d = root();
    open_turn(d.path(), None, 10);
    push_inflight(
        d.path(),
        Inflight {
            key: "k".into(),
            tool: "Bash".into(),
            ts: 90,
            agent_id: None,
            head_before: None,
            detection: None,
        },
    );
    push_inflight(
        d.path(),
        Inflight {
            key: "k2".into(),
            tool: "Edit".into(),
            ts: 90,
            agent_id: Some("sub".into()),
            head_before: None,
            detection: None,
        },
    );
    let c = classify_prompt(d.path(), &ctx(Host::Claude, 100));
    assert_eq!(c.mode, Mode::Correction);
    assert_eq!(c.interrupt.map(|i| i.as_str()), Some("inflight"));
    // Every branch that infers an interrupt drops the session's entries, so one
    // Esc cannot yield two interrupts — including the sub-agent's entry, which
    // was not evidence but is just as dead.
    assert!(pop_inflight(d.path(), "k").is_none());
    assert!(pop_inflight(d.path(), "k2").is_none());
}

#[test]
fn stale_inflight_and_foreign_agent_inflight_are_ignored() {
    let d = root();
    open_turn(d.path(), None, 10);
    push_inflight(
        d.path(),
        Inflight {
            key: "stale".into(),
            tool: "Bash".into(),
            ts: 1,
            agent_id: None,
            head_before: None,
            detection: None,
        },
    );
    push_inflight(
        d.path(),
        Inflight {
            key: "child".into(),
            tool: "Edit".into(),
            ts: 1000,
            agent_id: Some("sub".into()),
            head_before: None,
            detection: None,
        },
    );
    let c = classify_prompt(d.path(), &ctx(Host::Claude, 1000));
    assert_eq!(c.mode, Mode::MidTurn);
    // A mid-turn classification leaves the sub-agent's live entry alone: its
    // tool is still running and its post-check must still find it.
    assert!(pop_inflight(d.path(), "child").is_some());
}

/// Spec §Classification step 1: `last_event == "interrupt"` yields `correction`
/// on **every** host, and it is the single interrupt-already-recorded path —
/// `interrupt` is `None`, so the adapter writes no second record.
#[test]
fn last_event_interrupt_is_a_correction_on_every_host() {
    for host in [Host::Claude, Host::Codex, Host::Gemini] {
        let d = root();
        open_turn(d.path(), Some("t1"), 10);
        close_turn(d.path(), "interrupt");
        let c = classify_prompt(d.path(), &ctx(host, 100));
        assert_eq!(c.mode, Mode::Correction, "{host:?}");
        assert!(c.interrupt.is_none(), "{host:?}: the record already exists");
    }
}

/// It is read from `turn`, not from the journal, so compaction cannot erase it
/// and a concurrent `SubagentStop` record cannot hide it.
#[test]
fn last_event_interrupt_survives_a_compacted_journal() {
    use phronesis_mcp::journey::journal;
    let d = root();
    set_session(d.path(), "s-a");
    open_turn(d.path(), Some("t1"), 10);
    close_turn(d.path(), "interrupt");
    // Everything in the journal compacts away; the turn file is untouched.
    journal::append(
        d.path(),
        &journal::JournalRecord {
            v: journal::JOURNAL_V,
            ts: 1,
            sid: "s-a".into(),
            seq: 1,
            tool: journal::LIFECYCLE_TOOL.into(),
            path: String::new(),
            ext: None,
            module: None,
            tags: vec!["lifecycle:interrupt".into()],
            subject: None,
            command_exit: None,
            kind: Some("interrupt".into()),
            mode: None,
            host: Some("codex".into()),
            turn: None,
            agent: None,
            agent_type: None,
            kalpa: None,
        },
    )
    .unwrap();
    assert!(journal::maybe_compact(d.path(), 1, 0).unwrap());
    assert_eq!(
        classify_prompt(d.path(), &ctx(Host::Codex, 100)).mode,
        Mode::Correction
    );
}

/// A prompt delivered inside a sub-agent is not the human speaking: it is
/// `fresh`, it infers nothing, and it touches no state.
#[test]
fn a_sub_agent_prompt_is_fresh_and_reads_no_state() {
    let d = root();
    open_turn(d.path(), Some("t1"), 10);
    push_inflight(
        d.path(),
        Inflight {
            key: "k".into(),
            tool: "Bash".into(),
            ts: 90,
            agent_id: None,
            head_before: None,
            detection: None,
        },
    );
    let mut c = ctx(Host::Claude, 100);
    c.agent_id = Some("sub-1");
    let out = classify_prompt(d.path(), &c);
    assert_eq!(out.mode, Mode::Fresh);
    assert!(out.interrupt.is_none());
    // Nothing was consumed and the parent's turn is untouched.
    assert!(pop_inflight(d.path(), "k").is_some());
    let t = read_turn(d.path());
    assert!(t.open);
    assert_eq!(t.last_prompt_ts, 10);
}

/// A turn left open by a crashed session, and a corrupt turn file, both read as
/// absent — the conservative answer is `fresh`, not a false intervention.
#[test]
fn a_foreign_sid_or_corrupt_turn_yields_fresh() {
    let d = root();
    set_session(d.path(), "s-old");
    open_turn(d.path(), Some("t1"), 10);
    set_session(d.path(), "s-new");
    assert_eq!(
        classify_prompt(d.path(), &ctx(Host::Claude, 100)).mode,
        Mode::Fresh
    );

    std::fs::write(d.path().join(".phronesis/journey/turn"), "{{{").unwrap();
    assert_eq!(
        classify_prompt(d.path(), &ctx(Host::Claude, 100)).mode,
        Mode::Fresh
    );
}
#[test]
fn claude_transcript_marker_after_last_prompt_is_interrupt() {
    let d = root();
    open_turn(d.path(), None, 1_700_000_000);
    let t = d.path().join("t.jsonl");
    std::fs::write(&t, concat!(
        r#"{"type":"user","message":{"role":"user","content":"do it"},"timestamp":"2023-11-14T22:13:00Z"}"#, "\n",
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"working"}]},"timestamp":"2023-11-14T22:13:30Z"}"#, "\n",
        r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"[Request interrupted by user]"}]},"timestamp":"2023-11-14T22:14:00Z"}"#, "\n",
    )).unwrap();
    let mut c = ctx(Host::Claude, 1_700_000_100);
    c.transcript_path = Some(&t);
    let out = classify_prompt(d.path(), &c);
    assert_eq!(out.mode, Mode::Correction);
    assert_eq!(out.interrupt.map(|i| i.as_str()), Some("transcript"));
}

/// Both marker strings the spec names, and the tool-use one is a prefix match.
#[test]
fn both_interrupt_marker_strings_are_recognized() {
    for marker in [
        "[Request interrupted by user]",
        "[Request interrupted by user for tool use]",
        "[Request interrupted by user for tool use: Bash]",
    ] {
        let d = root();
        open_turn(d.path(), None, 1_700_000_000);
        let t = d.path().join("t.jsonl");
        std::fs::write(&t, format!(
            r#"{{"type":"user","message":{{"role":"user","content":"{marker}"}},"timestamp":"2023-11-14T22:14:00Z"}}"#
        )).unwrap();
        let mut c = ctx(Host::Claude, 1_700_000_100);
        c.transcript_path = Some(&t);
        assert_eq!(
            classify_prompt(d.path(), &c).interrupt.map(|i| i.as_str()),
            Some("transcript"),
            "{marker}"
        );
    }
}

/// The transcript is checked **before** `inflight`, because a queued mid-turn
/// message also leaves a live `inflight` entry and only the marker tells the
/// two apart. With the order reversed this test still passes on `mode` — hence
/// the assertion on `inferred_from`, which is the part that carries the
/// confidence a human reads.
#[test]
fn the_transcript_marker_outranks_a_live_inflight_entry() {
    let d = root();
    open_turn(d.path(), None, 1_700_000_000);
    push_inflight(
        d.path(),
        Inflight {
            key: "k".into(),
            tool: "Bash".into(),
            ts: 1_700_000_050,
            agent_id: None,
            head_before: None,
            detection: None,
        },
    );
    let t = d.path().join("t.jsonl");
    std::fs::write(&t, r#"{"type":"user","message":{"role":"user","content":"[Request interrupted by user]"},"timestamp":"2023-11-14T22:14:00Z"}"#).unwrap();
    let mut c = ctx(Host::Claude, 1_700_000_100);
    c.transcript_path = Some(&t);
    assert_eq!(
        classify_prompt(d.path(), &c).interrupt.map(|i| i.as_str()),
        Some("transcript")
    );
}

#[test]
fn claude_transcript_marker_before_last_prompt_is_ignored() {
    let d = root();
    open_turn(d.path(), None, 1_700_000_500);
    let t = d.path().join("t.jsonl");
    std::fs::write(&t, r#"{"type":"user","message":{"role":"user","content":"[Request interrupted by user]"},"timestamp":"2023-11-14T22:13:00Z"}"#).unwrap();
    let mut c = ctx(Host::Claude, 1_700_000_600);
    c.transcript_path = Some(&t);
    assert_eq!(classify_prompt(d.path(), &c).mode, Mode::MidTurn);
}

/// The tail is bounded at 64 KiB, and the first (probably partial) line of a
/// seeked read is discarded. A marker pushed out by a big paste is a documented
/// miss, not a crash: the prompt falls through to `inflight` or `mid_turn`.
#[test]
fn a_marker_pushed_out_of_the_sixty_four_kib_tail_is_missed() {
    let d = root();
    open_turn(d.path(), None, 1_700_000_000);
    let t = d.path().join("t.jsonl");
    let marker = format!(
        "{}\n",
        r#"{"type":"user","message":{"role":"user","content":"[Request interrupted by user]"},"timestamp":"2023-11-14T22:14:00Z"}"#
    );
    let filler = format!(
        "{{\"type\":\"assistant\",\"message\":{{\"role\":\"assistant\",\"content\":\"{}\"}}}}\n",
        "x".repeat(70 * 1024)
    );
    std::fs::write(&t, format!("{marker}{filler}")).unwrap();
    let mut c = ctx(Host::Claude, 1_700_000_100);
    c.transcript_path = Some(&t);
    assert_eq!(
        classify_prompt(d.path(), &c).mode,
        Mode::MidTurn,
        "no inflight, no marker in the tail"
    );

    // The same marker inside the tail is found.
    std::fs::write(&t, format!("{filler}{marker}")).unwrap();
    assert_eq!(
        classify_prompt(d.path(), &c).interrupt.map(|i| i.as_str()),
        Some("transcript")
    );
}

/// An unreadable or absent transcript is not an error; the next branch runs.
#[test]
fn an_unreadable_transcript_falls_through_to_inflight() {
    let d = root();
    open_turn(d.path(), None, 10);
    push_inflight(
        d.path(),
        Inflight {
            key: "k".into(),
            tool: "Bash".into(),
            ts: 90,
            agent_id: None,
            head_before: None,
            detection: None,
        },
    );
    let missing = d.path().join("does-not-exist.jsonl");
    let mut c = ctx(Host::Claude, 100);
    c.transcript_path = Some(&missing);
    assert_eq!(
        classify_prompt(d.path(), &c).interrupt.map(|i| i.as_str()),
        Some("inflight")
    );
}

/// `detect_interrupt` is the same evidence check without the classification, so
/// `SessionEnd` can ask "was this turn aborted?" (spec §"Host adapters /
/// Claude Code": "if `turn` is open, run the same interrupt detection step 2
/// runs and record `interrupt` when there is evidence, otherwise record `stop`").
#[test]
fn detect_interrupt_is_reusable_by_session_end() {
    let d = root();
    open_turn(d.path(), None, 10);
    assert!(detect_interrupt(d.path(), &ctx(Host::Claude, 100)).is_none());
    push_inflight(
        d.path(),
        Inflight {
            key: "k".into(),
            tool: "Bash".into(),
            ts: 90,
            agent_id: None,
            head_before: None,
            detection: None,
        },
    );
    assert_eq!(
        detect_interrupt(d.path(), &ctx(Host::Claude, 100)).map(|i| i.as_str()),
        Some("inflight")
    );
    assert!(
        pop_inflight(d.path(), "k").is_none(),
        "it drops the entries too"
    );
}

/// The marker branch against the transcript line shape Claude Code writes.
/// The committed tail is SYNTHETIC (see `fixtures/transcripts/claude/README.md`):
/// hand-written to the documented shape with placeholder ids and redacted
/// prose. A real, scrubbed capture may replace it later.
#[test]
fn the_committed_transcript_tail_is_recognized_as_an_interrupt() {
    let tail = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/transcripts/claude/interrupted-tail.jsonl");
    let raw = std::fs::read_to_string(&tail).unwrap_or_else(|e| {
        panic!(
            "{}: {e} — see fixtures/transcripts/claude/README.md",
            tail.display()
        )
    });
    assert!(
        raw.contains("[Request interrupted by user"),
        "the committed tail has no marker in it"
    );
    let d = root();
    // The turn opened before the marker's timestamp, so the marker is after it.
    open_turn(d.path(), None, 0);
    let mut c = ctx(Host::Claude, u64::MAX / 2);
    c.transcript_path = Some(&tail);
    let out = classify_prompt(d.path(), &c);
    assert_eq!(out.mode, Mode::Correction);
    assert_eq!(out.interrupt.map(|i| i.as_str()), Some("transcript"));
}
