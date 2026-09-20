//! Tests for journey fact derivation — window parsing, rule scan, selector
//! validation, aggregator emission, and the determinism contract.
//!
//! Mirrors PLAN-journey-facts.md Task 3 step 3.6: the integration tests here
//! are the on-disk contract for the v1 aggregator family.

use phr::{Condition, Fact, ReteNetwork, Rule};
use phronesis_mcp::journey::derive::{self, DeriveInput, Window, WindowScope, assert_facts};
use phronesis_mcp::journey::journal::{self, JournalRecord};
use phronesis_mcp::journey::tagger::TaggerConfig;

// ---------- Window parsing (step 3.2) ----------

#[test]
fn window_parses_calls() {
    assert_eq!(Window::parse("5c").unwrap(), Window::Calls(5));
    assert_eq!(Window::parse("100c").unwrap(), Window::Calls(100));
}

#[test]
fn window_parses_time() {
    assert_eq!(Window::parse("30m").unwrap(), Window::Seconds(30 * 60));
    assert_eq!(Window::parse("2h").unwrap(), Window::Seconds(2 * 3600));
    assert_eq!(Window::parse("7d").unwrap(), Window::Seconds(7 * 86_400));
    // `Ns` is seconds; the bare token `s` (tested below) is the session window.
    assert_eq!(Window::parse("60s").unwrap(), Window::Seconds(60));
}

#[test]
fn window_parses_session() {
    assert_eq!(Window::parse("s").unwrap(), Window::Session);
}

#[test]
fn window_repo_is_phase_2() {
    let err = Window::parse("r").unwrap_err();
    let msg = format!("{}", err);
    assert!(msg.contains("phase 2"), "{}", msg);
}

#[test]
fn window_rejects_malformed() {
    assert!(Window::parse("").is_err());
    assert!(Window::parse("5").is_err());
    assert!(Window::parse("5C").is_err());
    assert!(Window::parse("abc").is_err());
}

// ---------- Helpers (step 3.6) ----------

fn derive_input<'a>(
    project_root: &'a std::path::Path,
    rules: &'a [Rule],
    config: &'a TaggerConfig,
    now_ts: u64,
) -> DeriveInput<'a> {
    DeriveInput {
        project_root,
        rules,
        config,
        scope: WindowScope {
            current_sid: "s-now",
            now_ts,
        },
    }
}

fn make_record(
    timing: (u64, u64),
    identity: (&str, &[&str]),
    subject: Option<&str>,
) -> JournalRecord {
    let (seq, ts) = timing;
    let (sid, tags) = identity;
    JournalRecord {
        v: 1,
        ts,
        sid: sid.to_string(),
        seq,
        tool: "Edit".to_string(),
        path: "src/a.rs".to_string(),
        ext: Some("rs".to_string()),
        module: None,
        tags: tags.iter().map(|s| s.to_string()).collect(),
        subject: subject.map(|s| s.to_string()),
        command_exit: None,
        kind: None,
        mode: None,
        host: None,
        turn: None,
        agent: None,
        agent_type: None,
        kalpa: None,
    }
}

fn make_record_with_path(
    timing: (u64, u64),
    identity: (&str, &[&str]),
    path: &str,
) -> JournalRecord {
    let (seq, ts) = timing;
    let (sid, tags) = identity;
    JournalRecord {
        v: 1,
        ts,
        sid: sid.to_string(),
        seq,
        tool: "Edit".to_string(),
        path: path.to_string(),
        ext: Some("rs".to_string()),
        module: None,
        tags: tags.iter().map(|s| s.to_string()).collect(),
        subject: None,
        command_exit: None,
        kind: None,
        mode: None,
        host: None,
        turn: None,
        agent: None,
        agent_type: None,
        kalpa: None,
    }
}

macro_rules! rec {
    ($seq:expr, $ts:expr, $sid:expr, $tags:expr, $subject:expr) => {
        make_record(($seq, $ts), ($sid, $tags), $subject)
    };
}

macro_rules! rec_with_path {
    ($seq:expr, $ts:expr, $sid:expr, $tags:expr, $path:expr) => {
        make_record_with_path(($seq, $ts), ($sid, $tags), $path)
    };
}

fn cfg(json: &str) -> TaggerConfig {
    serde_json::from_str(json).unwrap()
}

fn script_cond(s: &str) -> Condition {
    Condition {
        predicate: "__script__".to_string(),
        args: Vec::new(),
        script: Some(s.to_string()),
    }
}

fn leaf_cond(pred: &str, args: &[&str]) -> Condition {
    Condition {
        predicate: pred.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
        script: None,
    }
}

fn rule_with_script(id: &str, scripts: Vec<&str>) -> Rule {
    Rule {
        id: id.to_string(),
        priority: 10,
        conditions: scripts.into_iter().map(script_cond).collect(),
        actions: Vec::new(),
    }
}

fn rule_with_conds(id: &str, conds: Vec<Condition>) -> Rule {
    Rule {
        id: id.to_string(),
        priority: 10,
        conditions: conds,
        actions: Vec::new(),
    }
}

fn journey_facts(net: &ReteNetwork, predicate: &str) -> Vec<Fact> {
    net.facts_snapshot()
        .unwrap()
        .into_iter()
        .filter(|f| f.predicate == predicate)
        .collect()
}

// ---------- Aggregator emission (step 3.6) ----------

#[tokio::test]
async fn journey_occurrence_count_in_session() {
    let dir = tempfile::tempdir().unwrap();
    for s in 1..=4u64 {
        journal::append(dir.path(), &rec!(s, 1000 + s, "s-now", &["auth"], None)).unwrap();
    }
    // record in a different session — must not contribute
    journal::append(dir.path(), &rec!(5, 1100, "s-OLD", &["auth"], None)).unwrap();

    let c = cfg(r#"{
        "version":1,
        "taggers":[{"tag":"auth","when":[{"file_path_matches":"src/auth/"}]}],
        "modules":[]
    }"#);
    let rules = vec![rule_with_script(
        "auth-churn",
        vec!["facts_count('journey_occurrence', ['auth','s']) >= 3"],
    )];

    let mut net = ReteNetwork::new();
    assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 2_000))
        .await
        .unwrap();
    let occurrences: Vec<Fact> = journey_facts(&net, "journey_occurrence")
        .into_iter()
        .filter(|f| {
            f.args.first().map(String::as_str) == Some("auth")
                && f.args.get(1).map(String::as_str) == Some("s")
        })
        .collect();
    assert_eq!(
        occurrences.len(),
        4,
        "one journey_occurrence per matching record in current session only"
    );
}

#[tokio::test]
async fn journey_count_emits_single_bindable() {
    let dir = tempfile::tempdir().unwrap();
    for s in 1..=4u64 {
        journal::append(dir.path(), &rec!(s, 1000 + s, "s-now", &["auth"], None)).unwrap();
    }
    let c = cfg(r#"{
        "version":1,
        "taggers":[{"tag":"auth","when":[{"file_path_matches":"src/auth/"}]}],
        "modules":[]
    }"#);
    let rules = vec![rule_with_script(
        "report-count",
        vec!["facts_contain('journey_count', ['auth','s','?n'])"],
    )];
    let mut net = ReteNetwork::new();
    assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 2_000))
        .await
        .unwrap();
    let counts = journey_facts(&net, "journey_count");
    assert_eq!(counts.len(), 1);
    assert_eq!(
        counts[0].args,
        vec!["auth".to_string(), "s".to_string(), "4".to_string()]
    );
}

#[tokio::test]
async fn journey_seen_emits_boolean_on_presence() {
    let dir = tempfile::tempdir().unwrap();
    journal::append(dir.path(), &rec!(1, 1000, "s-now", &["sql"], None)).unwrap();
    let c = cfg(r#"{
        "version":1,
        "taggers":[{"tag":"sql","when":[{"new_content_contains":"INSERT INTO"}]}],
        "modules":[]
    }"#);
    // Bare equality form: { "journey_seen": ["sql","5c"] }
    let rules = vec![rule_with_conds(
        "sql-recent",
        vec![leaf_cond("journey_seen", &["sql", "5c"])],
    )];
    let mut net = ReteNetwork::new();
    assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 2_000))
        .await
        .unwrap();
    let seen = journey_facts(&net, "journey_seen");
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].args, vec!["sql".to_string(), "5c".to_string()]);
}

#[tokio::test]
async fn journey_since_ge_ladders_to_max_k() {
    let dir = tempfile::tempdir().unwrap();
    // seq 1 was a build, seq 2..=9 are non-build edits → distance-since = 8
    journal::append(dir.path(), &rec!(1, 1000, "s-now", &["build"], None)).unwrap();
    for s in 2..=9u64 {
        journal::append(dir.path(), &rec!(s, 1000 + s, "s-now", &["auth"], None)).unwrap();
    }
    let c = cfg(r#"{
        "version":1,
        "taggers":[
            {"tag":"build","when":[{"bash_command_matches":"cargo (build|test)"}]},
            {"tag":"auth","when":[{"file_path_matches":"src/auth/"}]}
        ],
        "modules":[]
    }"#);
    let rules = vec![rule_with_script(
        "build-stale",
        vec!["facts_count('journey_since_ge', ['build','8']) >= 1"],
    )];
    let mut net = ReteNetwork::new();
    assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 2_000))
        .await
        .unwrap();
    let since: Vec<Fact> = journey_facts(&net, "journey_since_ge")
        .into_iter()
        .filter(|f| f.args.first().map(String::as_str) == Some("build"))
        .collect();
    assert_eq!(since.len(), 8, "ladder k=1..8 for distance 8");
    let mut ks: Vec<String> = since.iter().map(|f| f.args[1].clone()).collect();
    ks.sort_by_key(|s| s.parse::<u32>().unwrap_or(u32::MAX));
    assert_eq!(
        ks,
        vec!["1", "2", "3", "4", "5", "6", "7", "8"]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn absence_via_zero_count_fires() {
    let dir = tempfile::tempdir().unwrap();
    journal::append(dir.path(), &rec!(1, 1000, "s-now", &["auth"], None)).unwrap();
    journal::append(dir.path(), &rec!(2, 1010, "s-now", &["auth"], None)).unwrap();
    journal::append(dir.path(), &rec!(3, 1020, "s-now", &["auth"], None)).unwrap();
    // No "tests" tag anywhere — absence clause should hold.

    let c = cfg(r#"{
        "version":1,
        "taggers":[
            {"tag":"auth","when":[{"file_path_matches":"src/auth/"}]},
            {"tag":"tests","when":[{"file_path_matches":"tests/"}]}
        ],
        "modules":[]
    }"#);
    // Two-script-clause form — pure-__script__ rules reach the agenda
    // directly as of phr 0.13.0; no anchor leaf needed.
    let rule = Rule {
        id: "auth-without-tests".to_string(),
        priority: 25,
        conditions: vec![
            script_cond("facts_count('journey_occurrence', ['auth','s']) >= 3"),
            script_cond("facts_count('journey_occurrence', ['tests','s']) == 0"),
        ],
        actions: vec![phr::Action {
            action_type: "constraint_warning".to_string(),
            params: vec!["edit auth without tests".to_string()],
            ..Default::default()
        }],
    };

    let mut net = ReteNetwork::new();
    net.add_rule(rule.clone()).await.unwrap();
    let rules = vec![rule];
    assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 2_000))
        .await
        .unwrap();
    net.update_agenda().await.unwrap();
    let consequences = net.fire_all_consequences().unwrap();
    assert!(
        !consequences.is_empty(),
        "auth-without-tests fires on absence"
    );
}

#[tokio::test]
async fn undefined_selector_rejected_at_load() {
    let c = cfg(r#"{"version":1,"taggers":[{"tag":"auth","when":[]}],"modules":[]}"#);
    let rules = vec![rule_with_script(
        "typo",
        vec!["facts_count('journey_occurrence', ['testz','s']) == 0"],
    )];

    let dir = tempfile::tempdir().unwrap();
    let mut net = ReteNetwork::new();
    let err = assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 2_000))
        .await
        .unwrap_err();
    let msg = format!("{}", err);
    assert!(msg.contains("typo"), "missing rule id: {}", msg);
    assert!(msg.contains("testz"), "missing selector: {}", msg);
}

#[tokio::test]
async fn determinism_contract() {
    let dir = tempfile::tempdir().unwrap();
    // build at seq=1, then 3 writes — gives the filtered aggregator a
    // non-empty ladder to chew on alongside the others.
    journal::append(dir.path(), &rec!(1, 1000, "s-now", &["build"], None)).unwrap();
    for s in 2..=4u64 {
        journal::append(dir.path(), &rec!(s, 1000 + s, "s-now", &["write"], None)).unwrap();
    }
    // Plus a couple auth records so journey_occurrence has something to do.
    journal::append(dir.path(), &rec!(5, 1005, "s-now", &["auth"], None)).unwrap();
    journal::append(dir.path(), &rec!(6, 1006, "s-now", &["auth"], None)).unwrap();
    journal::append(dir.path(), &rec!(7, 1007, "s-now", &["auth"], None)).unwrap();

    let c = cfg(r#"{
        "version":1,
        "taggers":[
            {"tag":"auth","when":[]},
            {"tag":"build","when":[]},
            {"tag":"write","when":[]}
        ],
        "modules":[]
    }"#);
    let rules = vec![
        rule_with_script(
            "auth-churn",
            vec!["facts_count('journey_occurrence', ['auth','s']) >= 3"],
        ),
        rule_with_script(
            "build-stale-filtered",
            vec!["facts_count('journey_filtered_since_ge', ['build','write','5']) >= 1"],
        ),
    ];
    let mut a = ReteNetwork::new();
    let mut b = ReteNetwork::new();
    assert_facts(&mut a, derive_input(dir.path(), &rules, &c, 2_000))
        .await
        .unwrap();
    assert_facts(&mut b, derive_input(dir.path(), &rules, &c, 2_000))
        .await
        .unwrap();

    let serialize = |n: &ReteNetwork| -> String {
        let mut facts: Vec<String> = n
            .facts_snapshot()
            .unwrap()
            .into_iter()
            .filter(|f| f.predicate.starts_with("journey_"))
            .map(|f| format!("{}({})", f.predicate, f.args.join(",")))
            .collect();
        facts.sort();
        facts.join("\n")
    };
    let sa = serialize(&a);
    assert!(!sa.is_empty(), "expected some journey_* facts");
    assert!(
        sa.contains("journey_filtered_since_ge(build,write,"),
        "determinism fixture must exercise the filtered aggregator; got:\n{}",
        sa,
    );
    assert_eq!(sa, serialize(&b));
}

// ---------- journey_filtered_since_ge (SPEC-journey-filtered-since) ----------

#[tokio::test]
async fn journey_filtered_since_ge_counts_writes_since_build() {
    let dir = tempfile::tempdir().unwrap();
    // 5 build records, then 3 write records. Distance counted over `write`
    // since the most recent `build` is 3 → ladder k=1,2,3 only.
    for s in 1..=5u64 {
        journal::append(dir.path(), &rec!(s, 1000 + s, "s-now", &["build"], None)).unwrap();
    }
    for s in 6..=8u64 {
        journal::append(dir.path(), &rec!(s, 1000 + s, "s-now", &["write"], None)).unwrap();
    }
    let c = cfg(r#"{
        "version":1,
        "taggers":[
            {"tag":"build","when":[]},
            {"tag":"write","when":[]}
        ],
        "modules":[]
    }"#);
    // Rule references max_k=5; actual filtered distance is 3.
    let rules = vec![rule_with_script(
        "build-stale-filtered",
        vec!["facts_count('journey_filtered_since_ge', ['build','write','5']) >= 1"],
    )];
    let mut net = ReteNetwork::new();
    assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 2_000))
        .await
        .unwrap();
    let facts = journey_facts(&net, "journey_filtered_since_ge");
    let mut ks: Vec<u32> = facts
        .iter()
        .filter(|f| {
            f.args.first().map(String::as_str) == Some("build")
                && f.args.get(1).map(String::as_str) == Some("write")
        })
        .map(|f| f.args[2].parse::<u32>().unwrap())
        .collect();
    ks.sort_unstable();
    assert_eq!(
        ks,
        vec![1, 2, 3],
        "ladder must stop at the real filtered count, not max_k=5"
    );
}

#[tokio::test]
async fn journey_filtered_since_ge_emits_nothing_when_target_absent() {
    let dir = tempfile::tempdir().unwrap();
    // Only write records — no build anywhere.
    for s in 1..=5u64 {
        journal::append(dir.path(), &rec!(s, 1000 + s, "s-now", &["write"], None)).unwrap();
    }
    let c = cfg(r#"{
        "version":1,
        "taggers":[
            {"tag":"build","when":[]},
            {"tag":"write","when":[]}
        ],
        "modules":[]
    }"#);
    let rules = vec![rule_with_script(
        "build-stale-filtered",
        vec!["facts_count('journey_filtered_since_ge', ['build','write','8']) >= 1"],
    )];
    let mut net = ReteNetwork::new();
    assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 2_000))
        .await
        .unwrap();
    let facts = journey_facts(&net, "journey_filtered_since_ge");
    assert!(facts.is_empty(), "no target → no facts");
}

#[tokio::test]
async fn journey_filtered_since_ge_emits_nothing_when_no_counted_records_after_target() {
    let dir = tempfile::tempdir().unwrap();
    // Writes, then a terminal build → "writes after the last build" is zero.
    for s in 1..=3u64 {
        journal::append(dir.path(), &rec!(s, 1000 + s, "s-now", &["write"], None)).unwrap();
    }
    journal::append(dir.path(), &rec!(4, 1004, "s-now", &["build"], None)).unwrap();
    let c = cfg(r#"{
        "version":1,
        "taggers":[
            {"tag":"build","when":[]},
            {"tag":"write","when":[]}
        ],
        "modules":[]
    }"#);
    let rules = vec![rule_with_script(
        "build-stale-filtered",
        vec!["facts_count('journey_filtered_since_ge', ['build','write','8']) >= 1"],
    )];
    let mut net = ReteNetwork::new();
    assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 2_000))
        .await
        .unwrap();
    let facts = journey_facts(&net, "journey_filtered_since_ge");
    assert!(
        facts.is_empty(),
        "target is the last record → zero counted after → no facts"
    );
}

#[tokio::test]
async fn journey_filtered_since_ge_with_target_equals_counted_emits_nothing() {
    let dir = tempfile::tempdir().unwrap();
    for s in 1..=3u64 {
        journal::append(dir.path(), &rec!(s, 1000 + s, "s-now", &["write"], None)).unwrap();
    }
    let c = cfg(r#"{
        "version":1,
        "taggers":[{"tag":"write","when":[]}],
        "modules":[]
    }"#);
    let rules = vec![rule_with_script(
        "self-against-self",
        vec!["facts_count('journey_filtered_since_ge', ['write','write','5']) >= 1"],
    )];
    let mut net = ReteNetwork::new();
    assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 2_000))
        .await
        .unwrap();
    let facts = journey_facts(&net, "journey_filtered_since_ge");
    assert!(
        facts.is_empty(),
        "after the last write there are zero further writes by definition"
    );
}

#[tokio::test]
async fn journey_filtered_since_ge_undefined_selector_rejected_at_load() {
    let dir = tempfile::tempdir().unwrap();
    // `write` is defined; `bogus` is not.
    let c = cfg(r#"{
        "version":1,
        "taggers":[
            {"tag":"build","when":[]},
            {"tag":"write","when":[]}
        ],
        "modules":[]
    }"#);
    let rules = vec![rule_with_script(
        "bad-counted",
        vec!["facts_count('journey_filtered_since_ge', ['build','bogus','5']) >= 1"],
    )];
    let mut net = ReteNetwork::new();
    let err = assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 2_000))
        .await
        .unwrap_err();
    let msg = format!("{}", err);
    assert!(msg.contains("bad-counted"), "missing rule id: {}", msg);
    assert!(msg.contains("bogus"), "missing selector: {}", msg);
}

// ---------- Auxiliary coverage (RuleScan + edge cases) ----------

#[tokio::test]
async fn time_window_filters_by_ts() {
    let dir = tempfile::tempdir().unwrap();
    // Three records spanning > 30 minutes apart.
    journal::append(dir.path(), &rec!(1, 1000, "s-now", &["sql"], None)).unwrap();
    journal::append(dir.path(), &rec!(2, 1000 + 60, "s-now", &["sql"], None)).unwrap();
    journal::append(
        dir.path(),
        &rec!(3, 1000 + 60 * 60, "s-now", &["sql"], None),
    )
    .unwrap();

    let c = cfg(r#"{
        "version":1,
        "taggers":[{"tag":"sql","when":[]}],
        "modules":[]
    }"#);
    let rules = vec![rule_with_script(
        "sql-recent",
        vec!["facts_count('journey_occurrence', ['sql','30m']) >= 1"],
    )];
    let mut net = ReteNetwork::new();
    // now_ts = 1000 + 3600 + 60 → only the third record (ts = 4600) is
    // within 30m (1800s); the second (1060) is > 30m old; the first (1000) too.
    let now = 1000 + 60 * 60 + 60;
    assert_facts(&mut net, derive_input(dir.path(), &rules, &c, now))
        .await
        .unwrap();
    let occs: Vec<Fact> = journey_facts(&net, "journey_occurrence")
        .into_iter()
        .filter(|f| f.args.get(1).map(String::as_str) == Some("30m"))
        .collect();
    assert_eq!(occs.len(), 1, "only the latest record fits 30m");
}

#[tokio::test]
async fn call_window_filters_by_recency() {
    let dir = tempfile::tempdir().unwrap();
    // 10 sql records; only the last 5 should count for 5c window.
    for s in 1..=10u64 {
        journal::append(dir.path(), &rec!(s, 1000 + s, "s-now", &["sql"], None)).unwrap();
    }
    let c = cfg(r#"{
        "version":1,
        "taggers":[{"tag":"sql","when":[]}],
        "modules":[]
    }"#);
    let rules = vec![rule_with_conds(
        "sql-window",
        vec![leaf_cond("journey_seen", &["sql", "5c"])],
    )];
    let mut net = ReteNetwork::new();
    assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 2_000))
        .await
        .unwrap();
    let seen = journey_facts(&net, "journey_seen");
    assert_eq!(seen.len(), 1, "seen is a single boolean");
}

#[tokio::test]
async fn journey_seen_absent_when_no_match() {
    let dir = tempfile::tempdir().unwrap();
    journal::append(dir.path(), &rec!(1, 1000, "s-now", &["auth"], None)).unwrap();
    let c = cfg(r#"{
        "version":1,
        "taggers":[
            {"tag":"sql","when":[]},
            {"tag":"auth","when":[]}
        ],
        "modules":[]
    }"#);
    let rules = vec![rule_with_conds(
        "sql-recent",
        vec![leaf_cond("journey_seen", &["sql", "5c"])],
    )];
    let mut net = ReteNetwork::new();
    assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 2_000))
        .await
        .unwrap();
    let seen = journey_facts(&net, "journey_seen");
    assert!(seen.is_empty(), "no journey_seen without a matching record");
}

#[tokio::test]
async fn journey_since_ge_emits_nothing_when_selector_never_seen() {
    let dir = tempfile::tempdir().unwrap();
    for s in 1..=5u64 {
        journal::append(dir.path(), &rec!(s, 1000 + s, "s-now", &["auth"], None)).unwrap();
    }
    let c = cfg(r#"{
        "version":1,
        "taggers":[{"tag":"build","when":[]},{"tag":"auth","when":[]}],
        "modules":[]
    }"#);
    let rules = vec![rule_with_script(
        "build-stale",
        vec!["facts_count('journey_since_ge', ['build','5']) >= 1"],
    )];
    let mut net = ReteNetwork::new();
    assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 2_000))
        .await
        .unwrap();
    let since = journey_facts(&net, "journey_since_ge");
    assert!(since.is_empty(), "no since_ge when selector not in window");
}

#[tokio::test]
async fn journey_distinct_dedups_paths() {
    let dir = tempfile::tempdir().unwrap();
    journal::append(
        dir.path(),
        &rec_with_path!(1, 1000, "s-now", &["sql"], "src/a.rs"),
    )
    .unwrap();
    journal::append(
        dir.path(),
        &rec_with_path!(2, 1001, "s-now", &["sql"], "src/a.rs"),
    )
    .unwrap();
    journal::append(
        dir.path(),
        &rec_with_path!(3, 1002, "s-now", &["sql"], "src/b.rs"),
    )
    .unwrap();
    journal::append(
        dir.path(),
        &rec_with_path!(4, 1003, "s-now", &["sql"], "src/b.rs"),
    )
    .unwrap();

    let c = cfg(r#"{
        "version":1,
        "taggers":[{"tag":"sql","when":[]}],
        "modules":[]
    }"#);
    let rules = vec![rule_with_script(
        "distinct-paths",
        vec!["facts_contain('journey_distinct', ['path','s','?n'])"],
    )];
    let mut net = ReteNetwork::new();
    assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 2_000))
        .await
        .unwrap();
    let distinct = journey_facts(&net, "journey_distinct");
    assert_eq!(distinct.len(), 1);
    assert_eq!(
        distinct[0].args,
        vec!["path".to_string(), "s".to_string(), "2".to_string()]
    );
}

#[tokio::test]
async fn module_selector_validated_against_modules() {
    let dir = tempfile::tempdir().unwrap();
    let c = cfg(r#"{
        "version":1,
        "taggers":[],
        "modules":[{"name":"payments","paths":["src/payments/**"]}]
    }"#);
    let rules = vec![rule_with_conds(
        "pay-watch",
        vec![leaf_cond("journey_seen", &["module:payments", "5c"])],
    )];
    let mut net = ReteNetwork::new();
    assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 2_000))
        .await
        .unwrap();

    let rules_bad = vec![rule_with_conds(
        "pay-typo",
        vec![leaf_cond("journey_seen", &["module:typo", "5c"])],
    )];
    let mut net2 = ReteNetwork::new();
    let err = assert_facts(&mut net2, derive_input(dir.path(), &rules_bad, &c, 2_000))
        .await
        .unwrap_err();
    let msg = format!("{}", err);
    assert!(msg.contains("module:typo"), "{}", msg);
}

#[tokio::test]
async fn malformed_window_in_rule_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let c = cfg(r#"{"version":1,"taggers":[{"tag":"auth","when":[]}],"modules":[]}"#);
    let rules = vec![rule_with_script(
        "bad-window",
        vec!["facts_count('journey_occurrence', ['auth','5C']) >= 1"],
    )];
    let mut net = ReteNetwork::new();
    let err = assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 2_000))
        .await
        .unwrap_err();
    let msg = format!("{}", err);
    assert!(msg.contains("5C"), "{}", msg);
}

#[test]
fn rule_scan_collects_script_and_bare_forms() {
    // Sanity check: scan_rules surfaces both __script__ and bare-leaf forms.
    let rules = vec![
        rule_with_script(
            "a",
            vec!["facts_count('journey_occurrence', ['x','s']) >= 1"],
        ),
        rule_with_conds("b", vec![leaf_cond("journey_seen", &["y", "5c"])]),
        rule_with_script(
            "c",
            vec!["facts_contain('journey_count', ['x','30m','?n'])"],
        ),
        rule_with_script("d", vec!["facts_count('journey_since_ge', ['z','3']) >= 1"]),
        rule_with_script(
            "e",
            vec!["facts_contain('journey_distinct', ['path','s','?n'])"],
        ),
    ];
    let scan = derive::scan_rules(&rules).unwrap();
    assert!(
        scan.occurrence_pairs
            .iter()
            .any(|(s, w)| s == "x" && w == "s")
    );
    assert!(scan.seen_pairs.iter().any(|(s, w)| s == "y" && w == "5c"));
    assert!(scan.count_pairs.iter().any(|(s, w)| s == "x" && w == "30m"));
    assert_eq!(scan.since_max_k.get("z").copied(), Some(3));
    assert!(
        scan.distinct_pairs
            .iter()
            .any(|(f, w)| f == "path" && w == "s")
    );
}

// ---------- Lifecycle tool projection (Task 2) ----------

/// A lifecycle record, in the same `(seq, ts)` / `(sid, tags)` shape as the
/// file's existing helpers.
fn make_lifecycle(timing: (u64, u64), identity: (&str, &[&str]), kind: &str) -> JournalRecord {
    let (seq, ts) = timing;
    let (sid, tags) = identity;
    JournalRecord {
        v: journal::JOURNAL_V,
        ts,
        sid: sid.to_string(),
        seq,
        tool: journal::LIFECYCLE_TOOL.to_string(),
        path: String::new(),
        ext: None,
        module: None,
        tags: tags.iter().map(|s| s.to_string()).collect(),
        subject: None,
        command_exit: None,
        kind: Some(kind.to_string()),
        mode: None,
        host: Some("claude".to_string()),
        turn: None,
        agent: None,
        agent_type: None,
        kalpa: None,
    }
}

/// Goal 5 of the spec: interleaving lifecycle records changes no existing fact.
#[tokio::test]
async fn tool_projection_keeps_existing_facts_identical() {
    async fn facts_for(
        records: &[JournalRecord],
        rules: &[Rule],
        config: &TaggerConfig,
    ) -> Vec<String> {
        let dir = tempfile::tempdir().unwrap();
        for r in records {
            journal::append(dir.path(), r).unwrap();
        }
        let mut net = ReteNetwork::new();
        assert_facts(&mut net, derive_input(dir.path(), rules, config, 200))
            .await
            .unwrap();
        let mut all: Vec<String> = Vec::new();
        for p in [
            "journey_count",
            "journey_since_ge",
            "journey_filtered_since_ge",
            "journey_distinct",
        ] {
            for f in journey_facts(&net, p) {
                all.push(format!("{}:{}", f.predicate, f.args.join(",")));
            }
        }
        all.sort();
        all
    }

    let c = cfg(r#"{
        "version":1,
        "taggers":[
            {"tag":"edits","when":[{"file_path_matches":"src/"}]},
            {"tag":"tests","when":[{"file_path_matches":"tests/"}]}
        ],
        "modules":[]
    }"#);
    let rules = vec![rule_with_script(
        "r",
        vec![
            "facts_count('journey_count', ['edits','5c']) >= 0",
            "facts_count('journey_since_ge', ['tests', 1]) >= 0",
            "facts_count('journey_filtered_since_ge', ['tests','edits',1]) >= 0",
            "facts_count('journey_distinct', ['path','5c']) >= 0",
        ],
    )];

    let tools: Vec<JournalRecord> = (0..10u64)
        .map(|i| {
            if i == 4 {
                make_record_with_path((i, 100 + i), ("s-now", &["tests"]), "tests/x.rs")
            } else {
                make_record_with_path((i, 100 + i), ("s-now", &["edits"]), &format!("src/f{i}.rs"))
            }
        })
        .collect();
    let mut mixed = Vec::new();
    for (i, t) in tools.iter().enumerate() {
        let i = i as u64;
        mixed.push(t.clone());
        mixed.push(make_lifecycle(
            (100 + i, 100 + i),
            ("s-now", &["lifecycle:prompt", "lifecycle:prompt:fresh"]),
            "prompt",
        ));
    }

    let tools_only = facts_for(&tools, &rules, &c).await;
    // Golden values, so a symmetric off-by-one in both runs cannot pass: the
    // `5c` window is the last 5 tool records (indices 5..9), all `edits`, over
    // 5 distinct paths; the `tests` record at index 4 has 5 tool records after
    // it, all `edits`, so both ladders cap at k = 1.
    assert!(
        tools_only.contains(&"journey_count:edits,5c,5".to_string()),
        "{tools_only:?}"
    );
    assert!(
        tools_only.contains(&"journey_distinct:path,5c,5".to_string()),
        "{tools_only:?}"
    );
    assert!(
        tools_only.contains(&"journey_since_ge:tests,1".to_string()),
        "{tools_only:?}"
    );
    assert!(
        tools_only.contains(&"journey_filtered_since_ge:tests,edits,1".to_string()),
        "{tools_only:?}"
    );
    assert_eq!(tools_only, facts_for(&mixed, &rules, &c).await);
}

#[tokio::test]
async fn lifecycle_selectors_validate_without_journey_config() {
    let c = TaggerConfig::default();
    let rules = vec![rule_with_script(
        "r",
        vec![
            "facts_count('journey_seen', ['lifecycle:interrupt','s']) >= 1",
            "facts_count('journey_count', ['kalpa:demo','s']) >= 1",
            // Spec: a `lifecycle:*` selector with an `Nc` window yields no facts,
            // because positional windows run over the tool projection.
            "facts_count('journey_occurrence', ['lifecycle:interrupt','5c']) >= 1",
        ],
    )];
    let dir = tempfile::tempdir().unwrap();
    journal::append(
        dir.path(),
        &make_lifecycle(
            (1, 5),
            ("s-now", &["lifecycle:interrupt", "kalpa:demo"]),
            "interrupt",
        ),
    )
    .unwrap();
    let mut net = ReteNetwork::new();
    assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 10))
        .await
        .unwrap();
    assert_eq!(journey_facts(&net, "journey_seen").len(), 1);
    assert_eq!(
        journey_facts(&net, "journey_count")[0].args,
        vec!["kalpa:demo", "s", "1"]
    );
    assert!(journey_facts(&net, "journey_occurrence").is_empty());
}

/// The spec's shipped example rule `suggest-name-the-work-item` (§"Work items
/// and governed throughput") mixes a `journey_seen` leaf on
/// `lifecycle:prompt:fresh` with a script counting `lifecycle:unit_start`.
/// Both selectors must be built-in, or the example rule the spec publishes
/// fails validation with `UndefinedSelector` for every reader who pastes it.
#[tokio::test]
async fn spec_example_work_item_rule_validates_with_default_config() {
    let c = TaggerConfig::default();
    let rules = vec![Rule {
        id: "suggest-name-the-work-item".to_string(),
        priority: 10,
        conditions: vec![
            leaf_cond("journey_seen", &["lifecycle:prompt:fresh", "s"]),
            script_cond("facts_count('journey_seen', ['lifecycle:unit_start','s']) == 0"),
        ],
        actions: Vec::new(),
    }];
    let dir = tempfile::tempdir().unwrap();
    journal::append(
        dir.path(),
        &make_lifecycle(
            (1, 5),
            ("s-now", &["lifecycle:prompt", "lifecycle:prompt:fresh"]),
            "prompt",
        ),
    )
    .unwrap();
    journal::append(
        dir.path(),
        &make_lifecycle((2, 6), ("s-now", &["lifecycle:unit_start"]), "unit_start"),
    )
    .unwrap();
    let mut net = ReteNetwork::new();
    assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 10))
        .await
        .expect("the spec's example rule must validate against the built-in selectors");
    let mut seen: Vec<String> = journey_facts(&net, "journey_seen")
        .iter()
        .map(|f| f.args[0].clone())
        .collect();
    seen.sort();
    assert_eq!(seen, vec!["lifecycle:prompt:fresh", "lifecycle:unit_start"]);
}

/// The counterpart boundary: `lifecycle:unit_end` is built in too.
#[tokio::test]
async fn unit_end_selector_is_built_in() {
    let c = TaggerConfig::default();
    let rules = vec![rule_with_conds(
        "r",
        vec![leaf_cond("journey_seen", &["lifecycle:unit_end", "s"])],
    )];
    let dir = tempfile::tempdir().unwrap();
    journal::append(
        dir.path(),
        &make_lifecycle((1, 5), ("s-now", &["lifecycle:unit_end"]), "unit_end"),
    )
    .unwrap();
    let mut net = ReteNetwork::new();
    assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 10))
        .await
        .unwrap();
    assert_eq!(journey_facts(&net, "journey_seen").len(), 1);
}

/// Fail-closed is unchanged for everything outside the two built-in namespaces.
#[tokio::test]
async fn undefined_non_lifecycle_selector_still_fails_closed() {
    let c = cfg(r#"{
        "version":1,
        "taggers":[{"tag":"edits","when":[{"file_path_matches":"src/"}]}],
        "modules":[]
    }"#);
    let rules = vec![rule_with_script(
        "r",
        vec!["facts_count('journey_count', ['nonexistent','s']) >= 1"],
    )];
    let dir = tempfile::tempdir().unwrap();
    let mut net = ReteNetwork::new();
    let err = assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 10))
        .await
        .unwrap_err();
    assert!(
        matches!(err, derive::DeriveError::UndefinedSelector { .. }),
        "{err:?}"
    );
}

#[tokio::test]
async fn since_ge_counts_tool_records_after_lifecycle_target() {
    let c = TaggerConfig::default();
    let rules = vec![rule_with_script(
        "r",
        vec!["facts_count('journey_since_ge', ['lifecycle:interrupt', 3]) >= 0"],
    )];
    let dir = tempfile::tempdir().unwrap();
    journal::append(
        dir.path(),
        &make_lifecycle((1, 1), ("s-now", &["lifecycle:interrupt"]), "interrupt"),
    )
    .unwrap();
    for i in 0..2u64 {
        journal::append(dir.path(), &rec!(2 + i, 2 + i, "s-now", &["edits"], None)).unwrap();
    }
    journal::append(
        dir.path(),
        &make_lifecycle((5, 5), ("s-now", &["lifecycle:stop"]), "stop"),
    )
    .unwrap();
    let mut net = ReteNetwork::new();
    assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 10))
        .await
        .unwrap();
    // Distance is 2 (two *tool* records after the interrupt; the trailing stop
    // does not count), and `emit_since_ge` ladders k = 1..=min(max_k, distance)
    // = 1..=min(3, 2), so exactly "1" and "2" are emitted and "3" is not.
    let mut ks: Vec<String> = journey_facts(&net, "journey_since_ge")
        .iter()
        .map(|f| f.args[1].clone())
        .collect();
    ks.sort_by_key(|s| s.parse::<u32>().unwrap_or(u32::MAX));
    assert_eq!(ks, vec!["1", "2"]);
}

/// Spec §"The journal record, v2" / Read bound: "5 tool records followed by 200
/// lifecycle records under a `Calls(5)` rule (the iterative re-read must still
/// find all five)". A fixed multiple of `n` cannot do this; only doubling can.
#[tokio::test]
async fn calls_window_reads_iteratively_until_it_has_n_tool_records() {
    let c = cfg(r#"{
        "version":1,
        "taggers":[{"tag":"edits","when":[{"file_path_matches":"src/"}]}],
        "modules":[]
    }"#);
    let rules = vec![rule_with_script(
        "r",
        vec!["facts_count('journey_count', ['edits','5c']) >= 0"],
    )];
    let dir = tempfile::tempdir().unwrap();
    for i in 0..5u64 {
        journal::append(dir.path(), &rec!(i, i, "s-now", &["edits"], None)).unwrap();
    }
    for i in 5..205u64 {
        journal::append(
            dir.path(),
            &make_lifecycle((i, i), ("s-now", &["lifecycle:prompt"]), "prompt"),
        )
        .unwrap();
    }
    let mut net = ReteNetwork::new();
    assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 1_000))
        .await
        .unwrap();
    // A single 5-line read would see only lifecycle records and count 0; a
    // 2n+64 read would reach line 74 and still count 0. Doubling reaches 256.
    assert_eq!(journey_facts(&net, "journey_count")[0].args[2], "5");
}

/// The over-read must stop as well as start: a file with fewer tool records
/// than the window asks for terminates rather than looping to the hard cap on
/// every hook.
#[tokio::test]
async fn calls_window_terminates_when_the_file_holds_fewer_tool_records() {
    let c = cfg(r#"{
        "version":1,
        "taggers":[{"tag":"edits","when":[{"file_path_matches":"src/"}]}],
        "modules":[]
    }"#);
    let rules = vec![rule_with_script(
        "r",
        vec!["facts_count('journey_count', ['edits','50c']) >= 0"],
    )];
    let dir = tempfile::tempdir().unwrap();
    for i in 0..2u64 {
        journal::append(dir.path(), &rec!(i, i, "s-now", &["edits"], None)).unwrap();
    }
    let mut net = ReteNetwork::new();
    assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 100))
        .await
        .unwrap();
    assert_eq!(journey_facts(&net, "journey_count")[0].args[2], "2");
}

/// Spec §"The journal record, v2": "a `Seconds` window whose range contains
/// lifecycle records" — time windows enumerate **every** record, so a
/// lifecycle selector matches there.
#[tokio::test]
async fn seconds_window_enumerates_lifecycle_records() {
    let c = TaggerConfig::default();
    let rules = vec![rule_with_script(
        "r",
        vec!["facts_count('journey_count', ['lifecycle:interrupt','60s']) >= 1"],
    )];
    let dir = tempfile::tempdir().unwrap();
    journal::append(
        dir.path(),
        &make_lifecycle((1, 950), ("s-now", &["lifecycle:interrupt"]), "interrupt"),
    )
    .unwrap();
    // Outside the 60 s window: same selector, must not be counted.
    journal::append(
        dir.path(),
        &make_lifecycle((2, 100), ("s-now", &["lifecycle:interrupt"]), "interrupt"),
    )
    .unwrap();
    let mut net = ReteNetwork::new();
    assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 1_000))
        .await
        .unwrap();
    assert_eq!(journey_facts(&net, "journey_count")[0].args[2], "1");
}

/// Spec §"The journal record, v2": "a `since_ge` whose last match sits behind
/// more lifecycle records than a single read would cover". `since_ge` already
/// reads the hard cap, so this pins that the branch selection did not regress
/// into the calls-only path.
#[tokio::test]
async fn since_ge_finds_a_target_behind_two_hundred_lifecycle_records() {
    let c = cfg(r#"{
        "version":1,
        "taggers":[{"tag":"edits","when":[{"file_path_matches":"src/"}]}],
        "modules":[]
    }"#);
    let rules = vec![rule_with_script(
        "r",
        vec!["facts_count('journey_since_ge', ['lifecycle:interrupt', 5]) >= 0"],
    )];
    let dir = tempfile::tempdir().unwrap();
    journal::append(
        dir.path(),
        &make_lifecycle((0, 0), ("s-now", &["lifecycle:interrupt"]), "interrupt"),
    )
    .unwrap();
    for i in 1..201u64 {
        journal::append(
            dir.path(),
            &make_lifecycle((i, i), ("s-now", &["lifecycle:prompt"]), "prompt"),
        )
        .unwrap();
    }
    for i in 201..204u64 {
        journal::append(dir.path(), &rec!(i, i, "s-now", &["edits"], None)).unwrap();
    }
    let mut net = ReteNetwork::new();
    assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 1_000))
        .await
        .unwrap();
    let mut ks: Vec<String> = journey_facts(&net, "journey_since_ge")
        .iter()
        .map(|f| f.args[1].clone())
        .collect();
    ks.sort_by_key(|s| s.parse::<u32>().unwrap_or(u32::MAX));
    assert_eq!(
        ks,
        vec!["1", "2", "3"],
        "three tool records after the interrupt"
    );
}

/// The exemption is a **closed set**: a typo must still fail closed, or a rule
/// that can never fire validates and sits silent forever.
#[tokio::test]
async fn a_misspelled_lifecycle_selector_still_fails_closed() {
    let c = TaggerConfig::default();
    for bad in [
        "lifecycle:prompt:corection",
        "lifecycle:subagent_started",
        "lifecycle:",
        "kalpa:",
    ] {
        let rules = vec![rule_with_script(
            "r",
            vec![&format!("facts_count('journey_count', ['{bad}','s']) >= 1")],
        )];
        let dir = tempfile::tempdir().unwrap();
        let mut net = ReteNetwork::new();
        let err = assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 10))
            .await
            .unwrap_err();
        assert!(
            matches!(err, derive::DeriveError::UndefinedSelector { .. }),
            "{bad}: {err:?}"
        );
    }
}

/// Spec §"The journal record, v2": "No tagger, no modules." A catch-all tagger
/// config must stamp nothing on a lifecycle record, so no user-defined tag can
/// ever land on one and no existing tag selector can match one.
#[tokio::test]
async fn a_catch_all_tagger_stamps_nothing_on_a_lifecycle_record() {
    let c = cfg(r#"{
        "version":1,
        "taggers":[{"tag":"everything","when":[]}],
        "modules":[]
    }"#);
    let rules = vec![rule_with_script(
        "r",
        vec!["facts_count('journey_count', ['everything','s']) >= 0"],
    )];
    let dir = tempfile::tempdir().unwrap();
    journal::append(
        dir.path(),
        &make_lifecycle((1, 5), ("s-now", &["lifecycle:prompt"]), "prompt"),
    )
    .unwrap();
    journal::append(dir.path(), &rec!(2, 6, "s-now", &["everything"], None)).unwrap();
    let mut net = ReteNetwork::new();
    assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 10))
        .await
        .unwrap();
    // One tool record carries the tag; the lifecycle record does not, because
    // the tagger never runs for it (the record is written by `lifecycle::record`,
    // which stamps `tags()` and nothing else).
    assert_eq!(journey_facts(&net, "journey_count")[0].args[2], "1");
}

/// The `lifecycle:` and `kalpa:` namespaces are reserved: a tagger that claims
/// one is rejected at config load, not silently shadowed.
#[test]
fn a_tagger_tag_in_a_reserved_namespace_is_rejected_at_load() {
    for tag in ["lifecycle:prompt", "kalpa:demo"] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".phronesis")).unwrap();
        std::fs::write(
            dir.path().join(".phronesis/journey.json"),
            format!(
                r#"{{"version":1,"taggers":[{{"tag":"{tag}","when":[{{"file_path_matches":"src/"}}]}}],"modules":[]}}"#
            ),
        )
        .unwrap();
        let err = phronesis_mcp::journey::load_config(dir.path()).unwrap_err();
        assert!(
            matches!(err, phronesis_mcp::journey::ConfigError::ReservedTag { .. }),
            "{tag}: {err:?}"
        );
    }
}

/// Pairing a lifecycle selector with an `Nc` window yields no facts by
/// construction, so the rule can never fire. One stderr warning names the rule
/// rather than leaving it silent (spec §"The journal record, v2", tool
/// projection table).
#[tokio::test]
async fn a_lifecycle_selector_with_a_calls_window_warns_and_still_validates() {
    let c = TaggerConfig::default();
    let rules = vec![rule_with_script(
        "never-fires",
        vec!["facts_count('journey_count', ['lifecycle:interrupt','5c']) >= 1"],
    )];
    let scan = derive::scan_rules(&rules).expect("scan");
    let warnings = derive::lifecycle_window_warnings(&rules, &scan);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("never-fires"), "{warnings:?}");
    assert!(warnings[0].contains("lifecycle:interrupt"), "{warnings:?}");
    // It is a warning, not an error: the rule still validates.
    let dir = tempfile::tempdir().unwrap();
    let mut net = ReteNetwork::new();
    assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 10))
        .await
        .unwrap();
}
