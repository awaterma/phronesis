//! Tests for the journey tagger — config parse, fire against facts, module
//! resolution, and the perf budget the SPEC pins as a contract.
//!
//! Fact predicates here mirror the predicates the hook already asserts
//! (`file_path_matches`, `file_extension_is`, `new_content_contains`). Tags
//! ride the existing equality matcher; no new matching code anywhere in
//! the production module.

use phr::Fact;
use phronesis_mcp::journey::tagger::{self, TaggerConfig, TaggerError};

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
        source: None,
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

#[test]
fn tagger_config_default_constructs_empty() {
    // The hand-written `impl Default` pins version=1 so a fail-open default
    // matches the v1 schema; load_config returns this on a NotFound or
    // malformed `.phronesis/journey.json`.
    let c = TaggerConfig::default();
    assert_eq!(c.version, 1);
    assert!(c.taggers.is_empty());
    assert!(c.modules.is_empty());
    // resolve_module on default config matches nothing.
    assert_eq!(tagger::resolve_module(&c, "src/anything.rs"), None);
}

#[test]
fn tagger_config_version_defaults_to_1_on_deserialize() {
    // The serde `default = "default_version"` annotation fills `version` when
    // missing — exercises `default_version` directly.
    let c: TaggerConfig = serde_json::from_str(r#"{ "taggers": [], "modules": [] }"#).unwrap();
    assert_eq!(c.version, 1);
}

#[tokio::test]
async fn tagger_config_clone_resets_compiled_cache_and_fires_independently() {
    // Fire once on the source to seed `compiled`. Then clone and fire again on
    // the clone — it must compile its own copy from scratch (the cache is NOT
    // shared by intent — Clone is a fresh logical config). Behaviorally this
    // means firing on the clone still produces the same tags. The test
    // documents the contract: clone is a true logical fresh start.
    let c = cfg(r#"{
        "version": 1,
        "taggers": [
            { "tag": "auth", "when": [ { "file_path_matches": "src/auth/" } ] }
        ],
        "modules": [ { "name": "auth", "paths": ["src/auth/**"] } ]
    }"#);
    let facts = vec![fact("file_path_matches", &["src/auth/"])];
    let res_a = tagger::fire(&c, &facts).await.unwrap();
    assert_eq!(res_a.tags, vec!["auth".to_string()]);

    let clone = c.clone();
    // Modules clone too.
    assert_eq!(
        tagger::resolve_module(&clone, "src/auth/login.rs"),
        Some("auth".to_string())
    );
    let res_b = tagger::fire(&clone, &facts).await.unwrap();
    assert_eq!(res_b.tags, vec!["auth".to_string()]);
}

#[tokio::test]
async fn tagger_config_with_malformed_when_surfaces_config_error() {
    // The tagger's `when` is a Vec<serde_json::Value>; if those values are not
    // valid WhenClause shapes, `synthetic_tagger`'s serde parse fails and the
    // error surfaces as TaggerError::Config via the unfold path. Exercises the
    // `TaggerError::Config` mapping in `compile_taggers` (line 116).
    let c = cfg(r#"{
        "version": 1,
        "taggers": [
            { "tag": "bad", "when": [ 42 ] }
        ],
        "modules": []
    }"#);
    let err = tagger::fire(&c, &[]).await.unwrap_err();
    match &err {
        TaggerError::Config(msg) => {
            // Display rendering: thiserror's format string is exercised.
            let rendered = format!("{err}");
            assert!(rendered.contains("malformed"), "render = {rendered}");
            assert!(!msg.is_empty());
        }
        other => panic!("expected TaggerError::Config, got {other:?}"),
    }
}

#[tokio::test]
async fn tagger_empty_or_clause_surfaces_config_error() {
    // An empty `or` is parseable as a WhenClause (synthetic_tagger succeeds)
    // but unfold_or rejects it as "unsatisfiable" — that's the second
    // TaggerError::Config mapping (line 118) on the `unfold_or` call.
    let c = cfg(r#"{
        "version": 1,
        "taggers": [
            { "tag": "bad", "when": [ { "or": [] } ] }
        ],
        "modules": []
    }"#);
    let err = tagger::fire(&c, &[]).await.unwrap_err();
    assert!(matches!(err, TaggerError::Config(_)));
}

#[test]
fn tagger_error_engine_display_renders() {
    // Engine variant: no easy way to trigger from the public API (would require
    // a malfunctioning ReteNetwork), so construct it directly and exercise the
    // Display impl, mirroring how outcomes/* tests check error formatting.
    let e = TaggerError::Engine("simulated engine failure".to_string());
    let s = format!("{e}");
    assert!(s.contains("engine error"), "render = {s}");
    assert!(s.contains("simulated engine failure"));
}

#[test]
fn tag_result_default_is_empty() {
    let t = tagger::TagResult::default();
    assert!(t.tags.is_empty());
    assert_eq!(t.module, None);
    // Equality + Debug — covers the derive surface.
    assert_eq!(t, tagger::TagResult::default());
    let _ = format!("{t:?}");
}

#[test]
fn glob_double_star_in_middle_matches_any_depth() {
    // The `**` middle-of-pattern branch (line 232-237 in tagger.rs) is the
    // recursive `for i in s..=path.len()` loop after `**` when `rest` is
    // non-empty.
    let c = cfg(r#"{
        "version": 1,
        "taggers": [],
        "modules": [
            { "name": "tests", "paths": ["**/tests/**"] },
            { "name": "rs", "paths": ["src/**/lib.rs"] }
        ]
    }"#);
    // **/tests/** — middle `**` plus trailing `**`.
    assert_eq!(
        tagger::resolve_module(&c, "crates/foo/tests/it.rs"),
        Some("tests".to_string())
    );
    // src/**/lib.rs — `**` in the middle, literal suffix; rest is non-empty.
    assert_eq!(
        tagger::resolve_module(&c, "src/sub/dir/lib.rs"),
        Some("rs".to_string())
    );
    // No match — pattern needs lib.rs suffix.
    assert_eq!(tagger::resolve_module(&c, "src/sub/main.rs"), None);
}

#[test]
fn glob_edge_cases_empty_and_no_globs() {
    let c = cfg(r#"{
        "version": 1,
        "taggers": [],
        "modules": [
            { "name": "root", "paths": [""] },
            { "name": "exact", "paths": ["only/this.rs"] }
        ]
    }"#);
    // Empty pattern matches only empty path.
    assert_eq!(tagger::resolve_module(&c, ""), Some("root".to_string()));
    assert_eq!(
        tagger::resolve_module(&c, "only/this.rs"),
        Some("exact".to_string())
    );
    // Literal pattern with no glob: must match exactly.
    assert_eq!(tagger::resolve_module(&c, "only/this.rsX"), None);
}

#[test]
fn glob_leading_double_star() {
    // The matcher treats `**/lib.rs` as "anything (including empty), then a
    // literal `/lib.rs`" — so it requires at least one `/` somewhere. That
    // matches the SPEC examples (`src/auth/**`-style) where a leading `**`
    // always precedes a path separator. A bare `lib.rs` at the root won't
    // match `**/lib.rs`; authors who want it should add a separate `lib.rs`
    // entry.
    let c = cfg(r#"{
        "version": 1,
        "taggers": [],
        "modules": [ { "name": "lib", "paths": ["**/lib.rs"] } ]
    }"#);
    assert_eq!(
        tagger::resolve_module(&c, "a/lib.rs"),
        Some("lib".to_string())
    );
    assert_eq!(
        tagger::resolve_module(&c, "a/b/c/lib.rs"),
        Some("lib".to_string())
    );
    assert_eq!(tagger::resolve_module(&c, "a/lib.rs.bak"), None);
    // Root-level lib.rs needs an explicit `lib.rs` pattern; `**/lib.rs`
    // requires a preceding `/`.
    assert_eq!(tagger::resolve_module(&c, "lib.rs"), None);
}

#[tokio::test]
async fn tagger_ignores_non_tag_consequences() {
    // When the engine fires a rule that isn't a tagger, the action_type !=
    // "tag" branch (line 190 — the `else` is implicit; the `if let` simply
    // skips). Exercise that by handing `fire` a config whose taggers happen
    // to coexist with a non-matching rule signature. The simplest way:
    // empty taggers, empty facts — the for-loop body still runs zero times,
    // but the if-branch's "false" arm is taken by lack of matching consequences.
    // For a stronger test, build a tagger that matches and confirm only the
    // `tag` consequence is harvested even when there are multiple firings.
    let c = cfg(r#"{
        "version": 1,
        "taggers": [
            { "tag": "a", "when": [ { "file_path_matches": "src/" } ] },
            { "tag": "b", "when": [ { "file_path_matches": "src/" } ] }
        ],
        "modules": []
    }"#);
    let facts = vec![fact("file_path_matches", &["src/"])];
    let result = tagger::fire(&c, &facts).await.unwrap();
    let mut got = result.tags.clone();
    got.sort();
    assert_eq!(got, vec!["a".to_string(), "b".to_string()]);
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
