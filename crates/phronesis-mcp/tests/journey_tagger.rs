//! Tests for the journey tagger — config parse, fire against facts, module
//! resolution, and the perf budget the SPEC pins as a contract.
//!
//! Fact predicates here mirror the predicates the hook already asserts
//! (`file_path_matches`, `file_extension_is`, `new_content_contains`). Tags
//! ride the existing equality matcher; no new matching code anywhere in
//! the production module.

use phr::Fact;
use phronesis_mcp::journey::tagger::{self, TaggerConfig};

fn cfg(json: &str) -> TaggerConfig {
    serde_json::from_str(json).expect("valid tagger config")
}

fn fact(pred: &str, args: &[&str]) -> Fact {
    let id = format!("{}_{}", pred, args.join("_").replace(['/', ' ', ':'], "_"));
    Fact {
        id,
        predicate: pred.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
        timestamp: 0,
    }
}

#[tokio::test]
async fn tagger_attaches_tag_on_path_match() {
    let c = cfg(r#"{
        "version": 1,
        "taggers": [
            { "tag": "auth", "when": [ { "file_path_matches": "src/auth/" } ] }
        ],
        "modules": []
    }"#);
    let facts = vec![fact("file_path_matches", &["src/auth/"])];
    let result = tagger::fire(&c, &facts).await.unwrap();
    assert_eq!(result.tags, vec!["auth".to_string()]);
    assert_eq!(result.module, None);
}

#[tokio::test]
async fn tagger_attaches_multiple_tags() {
    let c = cfg(r#"{
        "version": 1,
        "taggers": [
            { "tag": "auth",  "when": [ { "file_path_matches": "src/auth/" } ] },
            { "tag": "rust",  "when": [ { "file_extension_is": "rs" } ] }
        ],
        "modules": []
    }"#);
    let facts = vec![
        fact("file_path_matches", &["src/auth/"]),
        fact("file_extension_is", &["rs"]),
    ];
    let result = tagger::fire(&c, &facts).await.unwrap();
    let mut got = result.tags.clone();
    got.sort();
    assert_eq!(got, vec!["auth".to_string(), "rust".to_string()]);
}

#[tokio::test]
async fn tagger_no_match_no_tag() {
    let c = cfg(r#"{
        "version": 1,
        "taggers": [
            { "tag": "auth", "when": [ { "file_path_matches": "src/auth/" } ] }
        ],
        "modules": []
    }"#);
    let facts = vec![fact("file_path_matches", &["src/payments/"])];
    let result = tagger::fire(&c, &facts).await.unwrap();
    assert!(result.tags.is_empty());
}

#[test]
fn module_resolves_from_globs() {
    let c = cfg(r#"{
        "version": 1,
        "taggers": [],
        "modules": [
            { "name": "auth", "paths": ["src/auth/**"] },
            { "name": "payments", "paths": ["src/payments/**", "crates/pay/**"] }
        ]
    }"#);
    assert_eq!(
        tagger::resolve_module(&c, "src/auth/login.rs"),
        Some("auth".to_string())
    );
    assert_eq!(
        tagger::resolve_module(&c, "crates/pay/lib.rs"),
        Some("payments".to_string())
    );
    assert_eq!(tagger::resolve_module(&c, "src/util/x.rs"), None);
}

#[tokio::test]
async fn or_dnf_expansion_fires_for_either_branch() {
    let c = cfg(r#"{
        "version": 1,
        "taggers": [
            { "tag": "sql", "when": [
                { "or": [
                    { "new_content_contains": "INSERT INTO" },
                    { "new_content_contains": "DELETE FROM" }
                ] }
            ] }
        ],
        "modules": []
    }"#);
    let facts_a = vec![fact("new_content_contains", &["INSERT INTO"])];
    let facts_b = vec![fact("new_content_contains", &["DELETE FROM"])];
    let facts_c = vec![fact("new_content_contains", &["SELECT *"])];
    assert_eq!(
        tagger::fire(&c, &facts_a).await.unwrap().tags,
        vec!["sql".to_string()]
    );
    assert_eq!(
        tagger::fire(&c, &facts_b).await.unwrap().tags,
        vec!["sql".to_string()]
    );
    assert!(tagger::fire(&c, &facts_c).await.unwrap().tags.is_empty());
}

// The 2 ms p95 budget is meaningful only against optimized code (the SPEC
// names a release-build budget). Skip in debug so `cargo test` stays green
// for everyday loops; CI runs `cargo test --release --test journey_tagger
// perf_smoke` to enforce the gate.
#[cfg(not(debug_assertions))]
#[tokio::test]
async fn perf_smoke_20_taggers_100_facts() {
    let mut json = String::from(r#"{"version":1,"taggers":["#);
    for i in 0..20 {
        if i > 0 {
            json.push(',');
        }
        json.push_str(&format!(
            r#"{{"tag":"t{}","when":[{{"file_path_matches":"src/m{}/"}}]}}"#,
            i, i
        ));
    }
    json.push_str(r#"],"modules":[]}"#);
    let c = cfg(&json);

    let mut facts = Vec::new();
    for i in 0..100 {
        facts.push(fact("file_path_matches", &[&format!("src/m{}/", i % 20)]));
    }

    // Warm
    let _ = tagger::fire(&c, &facts).await.unwrap();

    let mut samples = Vec::new();
    for _ in 0..50 {
        let t = std::time::Instant::now();
        let _ = tagger::fire(&c, &facts).await.unwrap();
        samples.push(t.elapsed());
    }
    samples.sort();
    let p95 = samples[(samples.len() * 95) / 100];
    assert!(
        p95 <= std::time::Duration::from_millis(2),
        "tagger p95 {:?} exceeds 2ms budget (samples sorted: first {:?}, last {:?})",
        p95,
        samples.first(),
        samples.last()
    );
}
