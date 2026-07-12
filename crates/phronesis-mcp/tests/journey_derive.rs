//! Tests for journey fact derivation — window parsing, rule scan, selector
//! validation, aggregator emission, and the determinism contract.
//!
//! Mirrors PLAN-journey-facts.md Task 3 step 3.6: the integration tests here
//! are the on-disk contract for the v1 aggregator family.

use phr::{Condition, Fact, ReteNetwork, Rule};
use phronesis_mcp::journey::derive::{self, Window, assert_facts};
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

fn rec(seq: u64, ts: u64, sid: &str, tags: &[&str], subject: Option<&str>) -> JournalRecord {
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
    }
}

fn rec_with_path(seq: u64, ts: u64, sid: &str, tags: &[&str], path: &str) -> JournalRecord {
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
    }
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
        journal::append(dir.path(), &rec(s, 1000 + s, "s-now", &["auth"], None)).unwrap();
    }
    // record in a different session — must not contribute
    journal::append(dir.path(), &rec(5, 1100, "s-OLD", &["auth"], None)).unwrap();

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
    assert_facts(&mut net, dir.path(), &rules, &c, "s-now", 2_000)
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
        journal::append(dir.path(), &rec(s, 1000 + s, "s-now", &["auth"], None)).unwrap();
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
    assert_facts(&mut net, dir.path(), &rules, &c, "s-now", 2_000)
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
    journal::append(dir.path(), &rec(1, 1000, "s-now", &["sql"], None)).unwrap();
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
    assert_facts(&mut net, dir.path(), &rules, &c, "s-now", 2_000)
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
    journal::append(dir.path(), &rec(1, 1000, "s-now", &["build"], None)).unwrap();
    for s in 2..=9u64 {
        journal::append(dir.path(), &rec(s, 1000 + s, "s-now", &["auth"], None)).unwrap();
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
    assert_facts(&mut net, dir.path(), &rules, &c, "s-now", 2_000)
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
    journal::append(dir.path(), &rec(1, 1000, "s-now", &["auth"], None)).unwrap();
    journal::append(dir.path(), &rec(2, 1010, "s-now", &["auth"], None)).unwrap();
    journal::append(dir.path(), &rec(3, 1020, "s-now", &["auth"], None)).unwrap();
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
        }],
    };

    let mut net = ReteNetwork::new();
    net.add_rule(rule.clone()).await.unwrap();
    let rules = vec![rule];
    assert_facts(&mut net, dir.path(), &rules, &c, "s-now", 2_000)
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
    let err = assert_facts(&mut net, dir.path(), &rules, &c, "s-now", 2_000)
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
    journal::append(dir.path(), &rec(1, 1000, "s-now", &["build"], None)).unwrap();
    for s in 2..=4u64 {
        journal::append(dir.path(), &rec(s, 1000 + s, "s-now", &["write"], None)).unwrap();
    }
    // Plus a couple auth records so journey_occurrence has something to do.
    journal::append(dir.path(), &rec(5, 1005, "s-now", &["auth"], None)).unwrap();
    journal::append(dir.path(), &rec(6, 1006, "s-now", &["auth"], None)).unwrap();
    journal::append(dir.path(), &rec(7, 1007, "s-now", &["auth"], None)).unwrap();

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
    assert_facts(&mut a, dir.path(), &rules, &c, "s-now", 2_000)
        .await
        .unwrap();
    assert_facts(&mut b, dir.path(), &rules, &c, "s-now", 2_000)
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
        journal::append(dir.path(), &rec(s, 1000 + s, "s-now", &["build"], None)).unwrap();
    }
    for s in 6..=8u64 {
        journal::append(dir.path(), &rec(s, 1000 + s, "s-now", &["write"], None)).unwrap();
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
    assert_facts(&mut net, dir.path(), &rules, &c, "s-now", 2_000)
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
        journal::append(dir.path(), &rec(s, 1000 + s, "s-now", &["write"], None)).unwrap();
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
    assert_facts(&mut net, dir.path(), &rules, &c, "s-now", 2_000)
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
        journal::append(dir.path(), &rec(s, 1000 + s, "s-now", &["write"], None)).unwrap();
    }
    journal::append(dir.path(), &rec(4, 1004, "s-now", &["build"], None)).unwrap();
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
    assert_facts(&mut net, dir.path(), &rules, &c, "s-now", 2_000)
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
        journal::append(dir.path(), &rec(s, 1000 + s, "s-now", &["write"], None)).unwrap();
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
    assert_facts(&mut net, dir.path(), &rules, &c, "s-now", 2_000)
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
    let err = assert_facts(&mut net, dir.path(), &rules, &c, "s-now", 2_000)
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
    journal::append(dir.path(), &rec(1, 1000, "s-now", &["sql"], None)).unwrap();
    journal::append(dir.path(), &rec(2, 1000 + 60, "s-now", &["sql"], None)).unwrap();
    journal::append(dir.path(), &rec(3, 1000 + 60 * 60, "s-now", &["sql"], None)).unwrap();

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
    assert_facts(&mut net, dir.path(), &rules, &c, "s-now", now)
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
        journal::append(dir.path(), &rec(s, 1000 + s, "s-now", &["sql"], None)).unwrap();
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
    assert_facts(&mut net, dir.path(), &rules, &c, "s-now", 2_000)
        .await
        .unwrap();
    let seen = journey_facts(&net, "journey_seen");
    assert_eq!(seen.len(), 1, "seen is a single boolean");
}

#[tokio::test]
async fn journey_seen_absent_when_no_match() {
    let dir = tempfile::tempdir().unwrap();
    journal::append(dir.path(), &rec(1, 1000, "s-now", &["auth"], None)).unwrap();
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
    assert_facts(&mut net, dir.path(), &rules, &c, "s-now", 2_000)
        .await
        .unwrap();
    let seen = journey_facts(&net, "journey_seen");
    assert!(seen.is_empty(), "no journey_seen without a matching record");
}

#[tokio::test]
async fn journey_since_ge_emits_nothing_when_selector_never_seen() {
    let dir = tempfile::tempdir().unwrap();
    for s in 1..=5u64 {
        journal::append(dir.path(), &rec(s, 1000 + s, "s-now", &["auth"], None)).unwrap();
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
    assert_facts(&mut net, dir.path(), &rules, &c, "s-now", 2_000)
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
        &rec_with_path(1, 1000, "s-now", &["sql"], "src/a.rs"),
    )
    .unwrap();
    journal::append(
        dir.path(),
        &rec_with_path(2, 1001, "s-now", &["sql"], "src/a.rs"),
    )
    .unwrap();
    journal::append(
        dir.path(),
        &rec_with_path(3, 1002, "s-now", &["sql"], "src/b.rs"),
    )
    .unwrap();
    journal::append(
        dir.path(),
        &rec_with_path(4, 1003, "s-now", &["sql"], "src/b.rs"),
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
    assert_facts(&mut net, dir.path(), &rules, &c, "s-now", 2_000)
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
    assert_facts(&mut net, dir.path(), &rules, &c, "s-now", 2_000)
        .await
        .unwrap();

    let rules_bad = vec![rule_with_conds(
        "pay-typo",
        vec![leaf_cond("journey_seen", &["module:typo", "5c"])],
    )];
    let mut net2 = ReteNetwork::new();
    let err = assert_facts(&mut net2, dir.path(), &rules_bad, &c, "s-now", 2_000)
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
    let err = assert_facts(&mut net, dir.path(), &rules, &c, "s-now", 2_000)
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
