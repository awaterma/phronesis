use phronesis_mcp::lifecycle::state::*;

fn root() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

#[test]
fn agents_pop_by_id_then_lifo() {
    let d = root();
    push_agent(
        d.path(),
        OpenAgent {
            agent_id: "a".into(),
            agent_type: Some("x".into()),
            ts: 1,
            seq: 1,
        },
    );
    push_agent(
        d.path(),
        OpenAgent {
            agent_id: "b".into(),
            agent_type: None,
            ts: 2,
            seq: 2,
        },
    );
    push_agent(
        d.path(),
        OpenAgent {
            agent_id: "c".into(),
            agent_type: None,
            ts: 3,
            seq: 3,
        },
    );
    assert_eq!(pop_agent(d.path(), Some("a")).unwrap().agent_id, "a");
    assert_eq!(pop_agent(d.path(), None).unwrap().agent_id, "c");
    assert_eq!(pop_agent(d.path(), Some("zzz")), None);
    assert_eq!(pop_agent(d.path(), None).unwrap().agent_id, "b");
    assert_eq!(pop_agent(d.path(), None), None);
}

fn inflight(key: &str, ts: u64, agent: Option<&str>) -> Inflight {
    Inflight {
        key: key.into(),
        tool: "Bash".into(),
        ts,
        agent_id: agent.map(str::to_string),
        head_before: None,
        detection: None,
    }
}

#[test]
fn inflight_ttl_applies_to_classification_only() {
    let d = root();
    push_inflight(d.path(), inflight("old", 100, None));
    let mut parent = inflight("parent", 1000, None);
    parent.head_before = Some("abc".into());
    push_inflight(d.path(), parent);
    push_inflight(d.path(), inflight("child", 1000, Some("sub1")));
    let now = 100 + INFLIGHT_TTL_SECS + 1;

    let seen_parent = live_inflight(d.path(), now, None);
    assert_eq!(
        seen_parent
            .iter()
            .map(|e| e.key.as_str())
            .collect::<Vec<_>>(),
        vec!["parent"]
    );
    let seen_child = live_inflight(d.path(), now, Some("sub1"));
    assert_eq!(
        seen_child
            .iter()
            .map(|e| e.key.as_str())
            .collect::<Vec<_>>(),
        vec!["parent", "child"]
    );
    // `live_inflight` is a classification pass, so it rewrote the file without
    // the expired entry.
    assert!(pop_inflight(d.path(), "old").is_none());
    assert_eq!(
        pop_inflight(d.path(), "parent")
            .unwrap()
            .head_before
            .as_deref(),
        Some("abc")
    );
    let taken = take_inflight_for_scope(d.path(), now, Some("sub1"));
    assert_eq!(taken.len(), 1);
    assert!(live_inflight(d.path(), now, Some("sub1")).is_empty());
}

/// The TTL must not reach `pop_inflight`, or every command that runs longer
/// than 15 minutes silently loses its commit detection — the exact case the
/// feature exists to catch (a long `git rebase`, a slow release script).
#[test]
fn pop_inflight_ignores_the_ttl() {
    let d = root();
    let mut e = inflight("slow-build", 0, None);
    e.head_before = Some("deadbeef".into());
    push_inflight(d.path(), e);
    let popped = pop_inflight(d.path(), "slow-build").expect("a twenty-minute build still pops");
    assert_eq!(popped.head_before.as_deref(), Some("deadbeef"));
}

/// A keyed multiset: two concurrent calls sharing a key push two lines and pop
/// two lines. With a clobbering push the second pre-check would erase the
/// first's `head_before` and the first post-check would find nothing.
#[test]
fn inflight_is_a_multiset_keyed_by_key() {
    let d = root();
    let mut first = inflight("same", 10, None);
    first.head_before = Some("aaa".into());
    let mut second = inflight("same", 20, None);
    second.head_before = Some("bbb".into());
    push_inflight(d.path(), first);
    push_inflight(d.path(), second);
    // Pop removes the LAST matching line.
    assert_eq!(
        pop_inflight(d.path(), "same")
            .unwrap()
            .head_before
            .as_deref(),
        Some("bbb")
    );
    assert_eq!(
        pop_inflight(d.path(), "same")
            .unwrap()
            .head_before
            .as_deref(),
        Some("aaa")
    );
    assert!(pop_inflight(d.path(), "same").is_none());
}

#[test]
fn clear_inflight_drops_every_entry() {
    let d = root();
    push_inflight(d.path(), inflight("a", 10, None));
    push_inflight(d.path(), inflight("b", 10, Some("sub")));
    clear_inflight(d.path());
    assert!(pop_inflight(d.path(), "a").is_none());
    assert!(pop_inflight(d.path(), "b").is_none());
}

#[test]
fn inflight_key_prefers_tool_use_id_then_hashes_input() {
    let input = serde_json::json!({"b": 1, "a": [1, 2]});
    assert_eq!(inflight_key_for(Some("tu-1"), "Bash", &input), "tu-1");
    let k1 = inflight_key_for(None, "run_shell_command", &input);
    let k2 = inflight_key_for(
        None,
        "run_shell_command",
        &serde_json::json!({"a": [1, 2], "b": 1}),
    );
    assert_eq!(k1, k2, "key order must not change the key");
    assert_ne!(k1, inflight_key_for(None, "replace", &input));
}

/// The hash is PINNED, not `std::hash::DefaultHasher` whose algorithm is
/// explicitly unspecified across Rust releases. A pre/post pair split across a
/// rebuild must still match, so this golden value is part of the on-disk
/// contract: if it changes, in-flight entries from the previous binary leak.
///
/// Computed by hand from the FNV-1a 64-bit definition over the bytes
/// `run_shell_command` followed by `{"command":"ls"}`.
#[test]
fn inflight_key_hash_is_pinned_fnv1a() {
    fn fnv1a(bytes: &[u8]) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in bytes {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    }
    let expected = {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for chunk in [
            b"run_shell_command".as_slice(),
            br#"{"command":"ls"}"#.as_slice(),
        ] {
            for b in chunk {
                h ^= u64::from(*b);
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        h
    };
    assert_eq!(expected, fnv1a(br#"run_shell_command{"command":"ls"}"#));
    assert_eq!(
        inflight_key_for(
            None,
            "run_shell_command",
            &serde_json::json!({"command": "ls"})
        ),
        format!("h{expected:016x}")
    );
}

#[test]
fn turn_transitions_and_session_reset() {
    let d = root();
    set_session(d.path(), "s-a");
    assert!(!read_turn(d.path()).open);
    open_turn(d.path(), Some("t1"), 50);
    let t = read_turn(d.path());
    assert!(t.open);
    assert_eq!(
        t.sid, "s-a",
        "the turn is stamped with the session that opened it"
    );
    assert_eq!(t.turn_id.as_deref(), Some("t1"));
    assert_eq!(t.last_prompt_ts, 50);
    assert_eq!(t.last_event, "prompt");
    assert!(close_turn(d.path(), "interrupt"));
    assert!(!read_turn(d.path()).open);
    assert_eq!(read_turn(d.path()).last_event, "interrupt");

    push_agent(
        d.path(),
        OpenAgent {
            agent_id: "a".into(),
            agent_type: None,
            ts: 1,
            seq: 1,
        },
    );
    push_inflight(d.path(), inflight("k", 1, None));
    open_turn(d.path(), None, 60);
    reset_for_session_start(d.path());
    assert!(pop_agent(d.path(), None).is_none());
    assert!(live_inflight(d.path(), 2, None).is_empty());
    assert!(!read_turn(d.path()).open);
}

/// A turn left open by a crashed session must not leak into the next one, and a
/// corrupt file must not either. Both read as absent, i.e. closed, i.e. the
/// next prompt is `fresh` — the conservative answer (spec §Correlation state).
#[test]
fn a_foreign_or_corrupt_turn_file_reads_as_absent() {
    let d = root();
    set_session(d.path(), "s-old");
    open_turn(d.path(), Some("t1"), 50);
    assert!(read_turn(d.path()).open);

    set_session(d.path(), "s-new");
    let t = read_turn(d.path());
    assert!(!t.open, "another session's open turn is not ours");
    assert_eq!(t.last_event, "", "nor is its last_event");

    std::fs::write(d.path().join(".phronesis/journey/turn"), "{not json").unwrap();
    assert!(!read_turn(d.path()).open);
}

/// Spec §Correlation state: the `session` write is an atomic replace, so a
/// lock-free `current_sid` reader never observes an empty file and never mints
/// a phantom sid. A truncate-then-write would expose exactly that window.
#[test]
fn session_write_is_an_atomic_replace_and_is_never_truncated() {
    let d = root();
    set_session(d.path(), "host-sid-1");
    assert_eq!(phronesis_mcp::journey::current_sid(d.path()), "host-sid-1");
    set_session(d.path(), "host-sid-2");
    assert_eq!(phronesis_mcp::journey::current_sid(d.path()), "host-sid-2");

    // A concurrent reader never sees an empty file: 200 overwrites while a
    // reader spins. `current_sid` mints a fresh `s-…` id when the file is empty,
    // so a phantom sid is observable as a value that is neither of the two.
    let path = d.path().to_path_buf();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reader_stop = stop.clone();
    let reader = std::thread::spawn(move || {
        let mut seen: Vec<String> = Vec::new();
        while !reader_stop.load(std::sync::atomic::Ordering::Relaxed) {
            let sid = phronesis_mcp::journey::current_sid(&path);
            if !seen.contains(&sid) {
                seen.push(sid);
            }
        }
        seen
    });
    for i in 0..200 {
        set_session(
            d.path(),
            if i % 2 == 0 {
                "host-sid-1"
            } else {
                "host-sid-2"
            },
        );
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let seen = reader.join().unwrap();
    assert!(
        seen.iter().all(|s| s == "host-sid-1" || s == "host-sid-2"),
        "a reader observed a phantom sid: {seen:?}"
    );

    // And there is no way to truncate it: SessionEnd must not.
    reset_for_session_start(d.path());
    assert_eq!(
        std::fs::read_to_string(d.path().join(".phronesis/journey/session"))
            .unwrap()
            .trim(),
        "host-sid-2",
        "reset_for_session_start truncates agents/inflight/turn, never session"
    );
}

#[test]
fn session_begin_sources_are_startup_resume_and_clear() {
    for begin in ["startup", "resume", "clear"] {
        assert!(is_session_begin(Some(begin)), "{begin}");
    }
    for keep in ["compact", "fork"] {
        assert!(!is_session_begin(Some(keep)), "{keep}");
    }
    // A host that sends no source means a new session; that is what every host
    // predating the field meant.
    assert!(is_session_begin(None));
    assert!(is_session_begin(Some("")));
    // An unknown source is treated as a begin, the same conservative default.
    assert!(is_session_begin(Some("something-new")));
}

#[test]
fn kalpa_round_trip_and_validation() {
    let d = root();
    assert!(read_kalpa(d.path()).is_none());
    write_kalpa(
        d.path(),
        &Kalpa {
            name: "demo-1".into(),
            started_ts: 9,
        },
    );
    assert_eq!(read_kalpa(d.path()).unwrap().name, "demo-1");
    clear_kalpa(d.path());
    assert!(read_kalpa(d.path()).is_none());
    assert!(valid_kalpa_name("lifecycle-events"));
    assert!(!valid_kalpa_name("Bad Name"));
    assert!(!valid_kalpa_name("-lead"));
    assert!(!valid_kalpa_name(&"a".repeat(65)));
}

/// "The pattern is a property of the value, not of the CLI": a hand-edited or
/// stale file whose name fails validation reads as absent, so it can never
/// reach a journal tag, a log field, or the model-visible context header.
#[test]
fn a_kalpa_file_with_an_invalid_name_reads_as_absent() {
    let d = root();
    std::fs::create_dir_all(d.path().join(".phronesis/journey")).unwrap();
    for bad in [
        r#"{"name":"Bad Name","started_ts":1}"#,
        r#"{"name":"../../etc","started_ts":1}"#,
    ] {
        std::fs::write(d.path().join(".phronesis/journey/kalpa"), bad).unwrap();
        assert!(read_kalpa(d.path()).is_none(), "{bad}");
    }
}

/// Every state file corrupted still exits 0 and reads as absent: a torn write
/// or a hand edit must degrade to "unknown", never to a failed hook.
#[test]
fn corrupt_state_files_degrade_to_absent() {
    let d = root();
    std::fs::create_dir_all(d.path().join(".phronesis/journey")).unwrap();
    for (name, body) in [
        (
            "agents",
            r#"{not json
{"agent_id":"ok","agent_type":null,"ts":1,"seq":1}
{"trailing"#,
        ),
        (
            "inflight",
            r#"garbage
{"key":"k","tool":"Bash","ts":1,"agent_id":null,"head_before":null,"detection":null}
{"partial"#,
        ),
        ("turn", "}{"),
        ("kalpa", "["),
    ] {
        std::fs::write(d.path().join(".phronesis/journey").join(name), body).unwrap();
    }
    // Well-formed lines survive; malformed ones are skipped.
    assert_eq!(pop_agent(d.path(), None).unwrap().agent_id, "ok");
    assert_eq!(pop_inflight(d.path(), "k").unwrap().tool, "Bash");
    assert!(!read_turn(d.path()).open);
    assert!(read_kalpa(d.path()).is_none());
}

/// A read-only `.phronesis/journey` must not fail a hook: every writer swallows
/// its error. Skipped when running as root, which ignores the mode bits.
#[test]
fn a_read_only_state_directory_never_panics() {
    let d = root();
    let dir = d.path().join(".phronesis/journey");
    std::fs::create_dir_all(&dir).unwrap();
    let mut perms = std::fs::metadata(&dir).unwrap().permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o500);
        std::fs::set_permissions(&dir, perms.clone()).unwrap();
        if std::fs::write(dir.join("probe"), "x").is_ok() {
            // Running as root: the mode bits do not apply, so there is nothing
            // to test here.
            let _ = std::fs::remove_file(dir.join("probe"));
            return;
        }
        push_agent(
            d.path(),
            OpenAgent {
                agent_id: "a".into(),
                agent_type: None,
                ts: 1,
                seq: 1,
            },
        );
        push_inflight(d.path(), inflight("k", 1, None));
        open_turn(d.path(), None, 1);
        close_turn(d.path(), "stop");
        set_session(d.path(), "s-x");
        write_kalpa(
            d.path(),
            &Kalpa {
                name: "demo".into(),
                started_ts: 1,
            },
        );
        assert!(pop_agent(d.path(), None).is_none());
        perms.set_mode(0o700);
        std::fs::set_permissions(&dir, perms).unwrap();
    }
}

#[test]
fn with_locked_serializes_sixteen_writers() {
    let d = root();
    let p = d.path().to_path_buf();
    let handles: Vec<_> = (0..16)
        .map(|_| {
            let p = p.clone();
            std::thread::spawn(move || {
                for _ in 0..50 {
                    with_locked(&p, "counter", |cur| {
                        let n: u64 = cur.trim().parse().unwrap_or(0);
                        (Some((n + 1).to_string()), ())
                    })
                    .unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let final_v = std::fs::read_to_string(p.join(".phronesis/journey/counter")).unwrap();
    assert_eq!(final_v.trim(), "800");
}

// ---------- record() and prompt_text config (Task 7) ----------

#[test]
fn record_writes_journal_and_log_with_kalpa_and_no_text_in_journal() {
    use phronesis_mcp::lifecycle::{Host, Kind, LifecycleEvent, Mode, record::record};
    let d = root();
    write_kalpa(
        d.path(),
        &Kalpa {
            name: "demo".into(),
            started_ts: 1,
        },
    );
    let ev = LifecycleEvent::new(Kind::Prompt, Host::Claude)
        .with_mode(Mode::Fresh)
        .with_prompt("hello world");
    let stamped = record(d.path(), ev).unwrap();
    assert_eq!(stamped.kalpa.as_deref(), Some("demo"));
    let journal =
        std::fs::read_to_string(d.path().join(".phronesis/journey/events.jsonl")).unwrap();
    assert!(journal.contains(r#""kind":"prompt""#));
    assert!(journal.contains(r#""kalpa":"demo""#));
    assert!(!journal.contains("hello world"));
    let log = std::fs::read_to_string(d.path().join(".phronesis/log.jsonl")).unwrap();
    assert!(log.contains(r#""kind":"lifecycle""#));
    assert!(log.contains(r#""prompt":"hello world""#));
}

#[test]
fn record_honors_prompt_text_none() {
    use phronesis_mcp::lifecycle::{Host, Kind, LifecycleEvent, Mode, record::record};
    let d = root();
    std::fs::create_dir_all(d.path().join(".phronesis")).unwrap();
    std::fs::write(
        d.path().join(".phronesis/journey.json"),
        r#"{"version":1,"taggers":[],"modules":[],"lifecycle":{"prompt_text":"none"}}"#,
    )
    .unwrap();
    record(
        d.path(),
        LifecycleEvent::new(Kind::Prompt, Host::Codex)
            .with_mode(Mode::Fresh)
            .with_prompt("hidden"),
    )
    .unwrap();
    let log = std::fs::read_to_string(d.path().join(".phronesis/log.jsonl")).unwrap();
    assert!(!log.contains("hidden"));
    assert!(log.contains(r#""prompt_bytes":6"#));
}

/// The switch fails **closed**: a `journey.json` we cannot parse must omit the
/// text, never print it. Failing open here would leak prompts from exactly the
/// projects whose config says not to.
#[test]
fn malformed_journey_config_omits_prompt_text() {
    use phronesis_mcp::lifecycle::{Host, Kind, LifecycleEvent, Mode, record::record};
    let d = root();
    std::fs::create_dir_all(d.path().join(".phronesis")).unwrap();
    for bad in [
        r#"{"version":1,"taggers":[],"modules":[],"lifecycle":{"prompt_text":"maybe"}}"#,
        r#"{"version":1,"taggers":[],"modules":[],"lifecycle":"full"}"#,
        r#"{not json at all"#,
    ] {
        let _ = std::fs::remove_file(d.path().join(".phronesis/log.jsonl"));
        std::fs::write(d.path().join(".phronesis/journey.json"), bad).unwrap();
        record(
            d.path(),
            LifecycleEvent::new(Kind::Prompt, Host::Claude)
                .with_mode(Mode::Fresh)
                .with_prompt("hidden"),
        );
        let log = std::fs::read_to_string(d.path().join(".phronesis/log.jsonl")).unwrap();
        assert!(!log.contains("hidden"), "{bad}: {log}");
        assert!(log.contains(r#""prompt_bytes":6"#), "{bad}: {log}");
    }
    // No config file at all is the documented default, and it is `full`.
    std::fs::remove_file(d.path().join(".phronesis/journey.json")).unwrap();
    let _ = std::fs::remove_file(d.path().join(".phronesis/log.jsonl"));
    record(
        d.path(),
        LifecycleEvent::new(Kind::Prompt, Host::Claude)
            .with_mode(Mode::Fresh)
            .with_prompt("hidden"),
    );
    assert!(
        std::fs::read_to_string(d.path().join(".phronesis/log.jsonl"))
            .unwrap()
            .contains("hidden")
    );
}

/// `correction_text` is the one accessor, and it consults the *current* value,
/// so flipping the switch hides text already written under `"full"`.
#[test]
fn correction_text_hides_already_written_prompts_when_the_switch_flips() {
    use phronesis_mcp::action_log::LogEntry;
    use phronesis_mcp::lifecycle::record::correction_text;
    let d = root();
    let entry = LogEntry::new("lifecycle", "prompt")
        .with("mode", "correction")
        .with("prompt", "no, the other thing");
    assert_eq!(
        correction_text(d.path(), &entry).as_deref(),
        Some("no, the other thing")
    );

    std::fs::create_dir_all(d.path().join(".phronesis")).unwrap();
    std::fs::write(
        d.path().join(".phronesis/journey.json"),
        r#"{"version":1,"taggers":[],"modules":[],"lifecycle":{"prompt_text":"none"}}"#,
    )
    .unwrap();
    assert_eq!(
        correction_text(d.path(), &entry),
        None,
        "the switch is enforced at read time"
    );
}

/// The journal append fails (the path is a directory), but the log entry —
/// which is where the prompt text and `prompt_bytes` live — is still written,
/// and `record` reports the failure by returning `None`. Spec: lifecycle
/// writes are fail-open and never fail a hook.
#[test]
fn record_still_logs_when_the_journal_append_fails() {
    use phronesis_mcp::lifecycle::{Host, Kind, LifecycleEvent, Mode, record::record};
    let d = root();
    std::fs::create_dir_all(d.path().join(".phronesis/journey/events.jsonl")).unwrap();
    let out = record(
        d.path(),
        LifecycleEvent::new(Kind::Prompt, Host::Claude)
            .with_mode(Mode::Fresh)
            .with_prompt("abc"),
    );
    assert!(out.is_none(), "a failed journal append must report itself");
    let log = std::fs::read_to_string(d.path().join(".phronesis/log.jsonl")).unwrap();
    assert!(log.contains(r#""prompt_bytes":3"#), "{log}");
}

/// The join key between the two files (spec §"Action log").
#[test]
fn journal_record_and_log_entry_share_sid_and_seq() {
    use phronesis_mcp::lifecycle::{Host, Kind, LifecycleEvent, record::record};
    let d = root();
    record(d.path(), LifecycleEvent::new(Kind::Stop, Host::Claude)).unwrap();
    let journal: serde_json::Value = serde_json::from_str(
        std::fs::read_to_string(d.path().join(".phronesis/journey/events.jsonl"))
            .unwrap()
            .lines()
            .next_back()
            .unwrap(),
    )
    .unwrap();
    let log: serde_json::Value = serde_json::from_str(
        std::fs::read_to_string(d.path().join(".phronesis/log.jsonl"))
            .unwrap()
            .lines()
            .next_back()
            .unwrap(),
    )
    .unwrap();
    assert!(!journal["sid"].is_null());
    assert!(!journal["seq"].is_null());
    assert_eq!(journal["sid"], log["sid"]);
    assert_eq!(journal["seq"], log["seq"]);
}
