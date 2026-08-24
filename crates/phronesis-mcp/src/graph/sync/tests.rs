use super::*;
use crate::graph::model::Edge;
use crate::graph::store;
use std::collections::BTreeSet;
use tempfile::TempDir;

fn project() -> TempDir {
    let d = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(d.path().join("src")).expect("mkdir src");
    d
}

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(p, body).expect("write");
}

fn edges(root: &Path) -> Vec<Edge> {
    store::load(&store::graph_path(root)).expect("load")
}

#[test]
fn path_included_tests_use_the_compiled_module_identity_and_resolve_super_globs() {
    let dir = project();
    write(dir.path(), "src/lib.rs", "mod foo;");
    write(
        dir.path(),
        "src/foo.rs",
        "pub mod implementation { pub fn production() {} }\npub use implementation::{production};\n#[cfg(test)]\n#[path = \"../tests/unit/foo_tests.rs\"]\nmod tests;\n",
    );
    write(
        dir.path(),
        "tests/unit/foo_tests.rs",
        "use super::*;\nfn helper() {}\n#[test]\nfn works() { production(); helper(); }\n",
    );

    rebuild(dir.path()).expect("rebuild");
    let graph = edges(dir.path());
    let test = "rust:crate::foo::tests::works";
    assert!(
        graph.iter().any(|edge| {
            edge.p == "defines_test" && edge.a.get(1).map(String::as_str) == Some(test)
        }),
        "defines_test: {:?}",
        graph
            .iter()
            .filter(|edge| edge.p == "defines_test")
            .map(|edge| &edge.a)
            .collect::<Vec<_>>()
    );
    assert!(graph.iter().any(|edge| {
        edge.p == "tested_by" && edge.a == ["rust:crate::foo::implementation::production", test]
    }));
    assert!(!graph.iter().any(|edge| {
        edge.p == "tested_by"
            && edge
                .a
                .first()
                .is_some_and(|target| target.ends_with("::helper"))
    }));
}

fn has(root: &Path, p: &str) -> bool {
    edges(root).iter().any(|e| e.p == p)
}

fn write_binding_rule(root: &Path) {
    write(
        root,
        ".phronesis/rules.json",
        r#"{"rules":[{"id":"tracks-foo","phase":"pre","when":[{"new_content_contains":"foo("}],"then":{"block":"foo contract"}}]}"#,
    );
}

// ─── hashing ────────────────────────────────────────────────────

#[test]
fn identical_content_hashes_identically() {
    assert_eq!(hash_content("fn f() {}"), hash_content("fn f() {}"));
}

#[test]
fn different_content_hashes_differently() {
    assert_ne!(hash_content("fn f() {}"), hash_content("fn g() {}"));
}

#[test]
fn graph_sync_reconciles_rule_bindings_in_the_same_generation() {
    let d = project();
    write_binding_rule(d.path());
    write(d.path(), "src/lib.rs", "pub fn foo() {}\n");
    rebuild(d.path()).expect("rebuild");

    let first = super::super::bindings::load(&super::super::bindings::bindings_path(d.path()))
        .expect("load")
        .expect("binding set");
    let index = load_index(&index_path(d.path())).expect("index");
    assert_eq!(first.generation, index.generation);
    assert_eq!(first.bindings.len(), 1);
    assert_eq!(
        first.bindings[0].state,
        super::super::bindings::BindingState::Bound
    );

    let changed = "pub fn replacement() {}\n";
    write(d.path(), "src/lib.rs", changed);
    on_save(d.path(), "src/lib.rs", changed).expect("save");
    let second = super::super::bindings::load(&super::super::bindings::bindings_path(d.path()))
        .expect("load")
        .expect("binding set");
    assert_eq!(
        second.bindings[0].state,
        super::super::bindings::BindingState::Stale
    );
    assert!(second.bindings[0].stale_at.is_some());
}

#[test]
fn writing_rules_reconciles_without_advancing_the_graph() {
    let d = project();
    write(d.path(), "src/lib.rs", "pub fn late_rule_target() {}\n");
    rebuild(d.path()).expect("rebuild");
    let generation = load_index(&index_path(d.path())).expect("index").generation;

    let source: crate::rules_file::SourceRule = serde_json::from_value(serde_json::json!({
        "id": "late-rule",
        "phase": "pre",
        "when": [{"new_content_contains": "late_rule_target("}],
        "then": {"block": "target contract"}
    }))
    .expect("rule");
    crate::rules_file::write_source(&crate::rules_file::default_path(d.path()), &[source])
        .expect("rules write");

    let set = super::super::bindings::load(&super::super::bindings::bindings_path(d.path()))
        .expect("load")
        .expect("bindings");
    assert_eq!(set.generation, generation);
    assert_eq!(set.bindings.len(), 1);
    assert_eq!(set.bindings[0].symbol, "late_rule_target");
}

#[test]
fn graph_sensor_reconciles_a_direct_rules_file_edit() {
    let d = project();
    write(d.path(), "src/lib.rs", "pub fn direct_edit_target() {}\n");
    rebuild(d.path()).expect("rebuild");
    write(
        d.path(),
        ".phronesis/rules.json",
        r#"{"rules":[{"id":"direct-rule","phase":"pre","when":[{"new_content_contains":"direct_edit_target("}],"then":{"block":"target contract"}}]}"#,
    );

    record_from_disk(d.path(), ".phronesis/rules.json");

    let set = super::super::bindings::load(&super::super::bindings::bindings_path(d.path()))
        .expect("load")
        .expect("bindings");
    assert_eq!(set.bindings.len(), 1);
    assert_eq!(set.bindings[0].symbol, "direct_edit_target");
}

// ─── index round trip ───────────────────────────────────────────

#[test]
fn a_missing_index_loads_as_empty() {
    let d = project();
    assert_eq!(
        load_index(&index_path(d.path())).expect("load"),
        Index::default()
    );
}

#[test]
fn the_index_survives_a_save_load_round_trip() {
    let d = project();
    let mut idx = Index::default();
    idx.entries.insert("src/a.rs".into(), 42);
    let p = index_path(d.path());
    save_index(&p, &idx).expect("save");
    assert_eq!(load_index(&p).expect("load").entries, idx.entries);
}

#[test]
fn a_written_index_is_stamped_with_the_current_format() {
    let d = project();
    let p = index_path(d.path());
    save_index(&p, &Index::default()).expect("save");
    assert_eq!(load_index(&p).expect("load").format, GRAPH_FORMAT);
}

// Pins decision D17. `.phronesis/graph.toml` exists in every project that
// opts into data contracts *or* ownership enrichment; if its mere presence
// routes a Rust save into `rebuild`, enabling either feature converts every
// edit in the repository into a whole-repo rescan. The unindexed sibling
// file is the probe: only a full rebuild would pick it up.
#[test]
fn a_rust_save_with_graph_toml_present_takes_the_incremental_path() {
    let d = project();
    write(d.path(), "src/a.rs", "pub fn a() {}\n");
    write(
        d.path(),
        ".phronesis/graph.toml",
        "[[generated_artifacts]]\nproducer = \"rust:app::build::emit\"\nartifact = \"config/out.json\"\n\n[ownership.rust]\nenabled = true\nprovider = \"ast\"\n",
    );
    rebuild(d.path()).expect("rebuild");

    write(d.path(), "src/b.rs", "pub fn b() {}\n");
    let body = "pub fn a() -> u8 { 1 }\n";
    write(d.path(), "src/a.rs", body);
    on_save(d.path(), "src/a.rs", body).expect("save");

    let index = load_index(&index_path(d.path())).expect("index");
    assert!(
        !index.entries.contains_key("src/b.rs"),
        "a .rs save must not rescan the repository merely because graph.toml exists"
    );
    assert!(
        index.entries.contains_key("src/a.rs"),
        "the edited file is still recorded"
    );
}

#[test]
fn hook_sensor_rebuilds_the_complete_graph_on_every_save() {
    let d = project();
    write(d.path(), "src/a.rs", "pub fn alpha() {}\n");
    write(d.path(), "src/b.rs", "pub fn beta() {}\n");
    rebuild(d.path()).expect("initial rebuild");

    write(d.path(), "src/b.rs", "pub fn beta_changed() {}\n");
    record_from_disk(d.path(), "README.md");

    let index = load_index(&index_path(d.path())).expect("index");
    assert_eq!(
        index.entries.get("src/b.rs"),
        Some(&hash_content("pub fn beta_changed() {}\n")),
        "the save-triggered full rebuild must refresh sibling files"
    );
}

// The companion to the test above: narrowing the trigger must not stop a
// declared artifact's own edit from recomputing the data-contract edges,
// which are attributed to graph.toml and so survive compaction unchanged.
#[test]
fn editing_a_declared_artifact_still_forces_a_full_rebuild() {
    let d = project();
    write(d.path(), "src/a.rs", "pub fn a() {}\n");
    write(d.path(), "config/out.json", "{\"name\": \"demo\"}");
    write(
        d.path(),
        ".phronesis/graph.toml",
        "[[generated_artifacts]]\nproducer = \"rust:app::build::emit\"\nartifact = \"config/out.json\"\n",
    );
    rebuild(d.path()).expect("rebuild");

    write(d.path(), "src/b.rs", "pub fn b() {}\n");
    let body = "{\"name\": \"demo\", \"extra\": 1}";
    write(d.path(), "config/out.json", body);
    on_save(d.path(), "config/out.json", body).expect("save");

    let index = load_index(&index_path(d.path())).expect("index");
    assert!(
        index.entries.contains_key("src/b.rs"),
        "editing a declared artifact must still rebuild the whole graph"
    );
}

#[test]
fn a_complete_rebuild_records_intentionally_skipped_content_as_observed() {
    let d = project();
    write(d.path(), "config/broken.json", "{not valid json");

    let outcome = rebuild(d.path()).expect("rebuild");
    assert!(outcome.skipped > 0);
    let index = load_index(&index_path(d.path())).expect("index");
    assert_eq!(check_freshness(d.path(), &index), Freshness::Fresh);

    write(d.path(), "config/broken.json", "{still not valid json");
    assert!(matches!(
        check_freshness(d.path(), &index),
        Freshness::Stale { .. }
    ));
}

#[test]
fn rebuild_migrates_deprecated_graph_predicates_without_losing_rule_metadata() {
    let d = project();
    write(d.path(), "src/a.rs", "fn risky() { panic!(); }");
    write(
        d.path(),
        ".phronesis/rules.json",
        r#"{
          "rules": [{
            "id": "legacy-graph-rule",
            "phase": "audit",
            "priority": 17,
            "audit": true,
            "silent": true,
            "doc_excepted": true,
            "binds": false,
            "when": [
              {"untested": ["?func"]},
              {"or": [
                {"untested": ["?other"]},
                {"calls_api": ["?func", "panic"]}
              ]}
            ],
            "then": {"warn": "legacy relation"}
          }]
        }"#,
    );
    assert_eq!(
        deprecated_graph_rule_predicates(d.path()).expect("predicate drift"),
        vec!["untested"]
    );

    let outcome = rebuild(d.path()).expect("rebuild");
    assert_eq!(outcome.migrated_rules, 1);
    let migrated =
        std::fs::read_to_string(d.path().join(".phronesis/rules.json")).expect("migrated rules");
    assert!(!migrated.contains("\"untested\""));
    assert_eq!(migrated.matches("\"no_direct_test\"").count(), 2);
    for metadata in [
        "\"audit\": true",
        "\"silent\": true",
        "\"doc_excepted\": true",
        "\"binds\": false",
    ] {
        assert!(
            migrated.contains(metadata),
            "missing {metadata}: {migrated}"
        );
    }
    let index = load_index(&index_path(d.path())).expect("index");
    assert_eq!(check_freshness(d.path(), &index), Freshness::Fresh);
    assert!(
        deprecated_graph_rule_predicates(d.path())
            .expect("resolved drift")
            .is_empty()
    );

    let second = rebuild(d.path()).expect("idempotent rebuild");
    assert_eq!(second.migrated_rules, 0);
}

#[test]
fn rebuild_migrates_dynamic_boundary_relations_and_literal_callable_ids() {
    let d = project();
    write(
        d.path(),
        ".phronesis/rules.json",
        r#"{"rules":[{"id":"legacy-rhai","phase":"audit","when":[{"rhai_exposes_fn":["?host","state_get_hp"]},{"calls_rhai_fn":["?script","state_*"]}],"then":{"warn":"legacy"}}]}"#,
    );
    let outcome = rebuild(d.path()).expect("rebuild");
    assert_eq!(outcome.migrated_rules, 1);
    let migrated =
        std::fs::read_to_string(d.path().join(".phronesis/rules.json")).expect("migrated rules");
    assert!(migrated.contains("\"exposes\""));
    assert!(migrated.contains("\"calls\""));
    assert!(migrated.contains("rhai:callable::state_get_hp"));
    assert!(migrated.contains("rhai:callable::state_*"));
}

#[test]
fn rebuild_refuses_a_non_equivalent_resolution_rule_migration() {
    let d = project();
    write(
        d.path(),
        ".phronesis/rules.json",
        r#"{"rules":[{"id":"legacy-resolution","phase":"audit","when":[{"rhai_call_resolves_to":["?script","?target"]}],"then":{"warn":"legacy"}}]}"#,
    );
    let error = rebuild(d.path()).expect_err("manual migration required");
    assert!(error.to_string().contains("manual rule migration"));
    let unchanged = std::fs::read_to_string(d.path().join(".phronesis/rules.json"))
        .expect("rules remain readable");
    assert!(unchanged.contains("rhai_call_resolves_to"));
}

#[test]
fn an_index_without_a_format_header_reads_as_format_zero() {
    // The shape written before identity versioning existed.
    let d = project();
    let p = index_path(d.path());
    write(d.path(), INDEX_REL_PATH, "42 src/a.rs\n");
    let idx = load_index(&p).expect("load");
    assert_eq!(idx.format, 0);
    assert_eq!(idx.entries.get("src/a.rs"), Some(&42));
}

// ─── identity-format migration ──────────────────────────────────

#[test]
fn a_graph_built_under_an_older_identity_format_is_not_fresh() {
    // Content hashes match exactly — nothing on disk changed. Only the
    // format header betrays that every edge carries the old naming.
    let d = project();
    write(d.path(), "src/a.rs", "fn f() {}");
    let index = Index {
        format: 0,
        generation: 0,
        entries: BTreeMap::from([("src/a.rs".to_string(), hash_content("fn f() {}"))]),
    };
    assert_eq!(
        check_freshness(d.path(), &index),
        Freshness::Outdated {
            found: 0,
            expected: GRAPH_FORMAT,
        }
    );
}

#[test]
fn an_index_that_describes_nothing_is_never_reported_as_outdated() {
    // A project that has never built a graph has format 0 too; calling
    // that a migration would demand a rebuild of an empty graph.
    let d = project();
    assert_ne!(
        check_freshness(d.path(), &Index::default()),
        Freshness::Outdated {
            found: 0,
            expected: GRAPH_FORMAT,
        }
    );
}

#[test]
fn saving_into_an_older_format_graph_rebuilds_every_file() {
    let d = project();
    write(d.path(), "src/a.rs", "fn alpha() {}");
    write(d.path(), "src/b.rs", "fn beta() {}");
    // A graph in the pre-versioning naming, with an index that says it is
    // current for both files.
    store::write_atomic(
        &store::graph_path(d.path()),
        &[
            Edge::base("defines_fn", &["src/a.rs", "crate::a::alpha"], "src/a.rs"),
            Edge::base("defines_fn", &["src/b.rs", "crate::b::beta"], "src/b.rs"),
        ],
    )
    .expect("seed graph");
    save_index(
        &index_path(d.path()),
        &Index {
            format: 0,
            generation: 0,
            entries: BTreeMap::from([
                ("src/a.rs".to_string(), hash_content("fn alpha() {}")),
                ("src/b.rs".to_string(), hash_content("fn beta() {}")),
            ]),
        },
    )
    .expect("seed index");
    // Written by `save_index`, which always stamps the current format;
    // force the legacy shape back onto disk.
    std::fs::write(index_path(d.path()), "0 src/a.rs\n0 src/b.rs\n").expect("legacy index");

    on_save(d.path(), "src/a.rs", "fn alpha() {}").expect("save");

    let names: Vec<String> = edges(d.path())
        .into_iter()
        .filter(|e| e.p == "defines_fn")
        .filter_map(|e| e.a.get(1).cloned())
        .collect();
    assert!(
        names.iter().all(|n| n.starts_with("rust:")),
        "the unedited file must be migrated too, not left in the old naming: {names:?}"
    );
    assert!(names.iter().any(|n| n.ends_with("::beta")), "{names:?}");
    assert_eq!(
        load_index(&index_path(d.path())).expect("load").format,
        GRAPH_FORMAT,
        "a migrated graph must stop reporting itself outdated"
    );
}

// ─── the per-save pipeline ──────────────────────────────────────

#[test]
fn saving_a_file_writes_its_base_edges() {
    let d = project();
    write(d.path(), "src/a.rs", "fn f() {}");
    on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
    assert!(has(d.path(), "defines_fn"));
}

#[test]
fn saving_a_file_derives_untested_in_the_same_pass() {
    let d = project();
    write(d.path(), "src/a.rs", "fn f() {}");
    on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
    assert!(
        has(d.path(), "no_direct_test"),
        "derived facts must be current after every save, not only after rebuild"
    );
}

#[test]
fn a_test_added_later_clears_untested_without_a_rebuild() {
    let d = project();
    write(d.path(), "src/a.rs", "fn fire() {}");
    on_save(d.path(), "src/a.rs", "fn fire() {}").expect("save");
    assert!(has(d.path(), "no_direct_test"));

    let test_src = "#[test]\nfn t() { fire(); }";
    write(d.path(), "tests/a.rs", test_src);
    on_save(d.path(), "tests/a.rs", test_src).expect("save");
    assert!(
        !has(d.path(), "no_direct_test"),
        "coverage is whole-repo: a test elsewhere must clear it"
    );
}

#[test]
fn re_saving_a_file_replaces_its_edges_rather_than_duplicating_them() {
    let d = project();
    for _ in 0..3 {
        on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
    }
    let defines: Vec<_> = edges(d.path())
        .into_iter()
        .filter(|e| e.p == "defines_fn")
        .collect();
    assert_eq!(defines.len(), 1);
}

#[test]
fn removing_a_function_removes_its_edges() {
    let d = project();
    on_save(d.path(), "src/a.rs", "fn f() {}\nfn g() {}").expect("save");
    on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
    let names: Vec<String> = edges(d.path())
        .into_iter()
        .filter(|e| e.p == "defines_fn")
        .filter_map(|e| e.a.get(1).cloned())
        .collect();
    assert_eq!(names, vec!["rust:crate::a::f".to_string()]);
}

#[test]
fn derived_edges_do_not_accumulate_across_saves() {
    let d = project();
    on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
    on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
    let untested: Vec<_> = edges(d.path())
        .into_iter()
        .filter(|e| e.p == "no_direct_test")
        .collect();
    assert_eq!(untested.len(), 1);
}

#[test]
fn saving_records_the_files_hash_in_the_index() {
    let d = project();
    on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
    let idx = load_index(&index_path(d.path())).expect("load");
    assert_eq!(
        idx.entries.get("src/a.rs"),
        Some(&hash_content("fn f() {}"))
    );
}

#[test]
fn a_non_rust_file_is_ignored() {
    let d = project();
    on_save(d.path(), "README.md", "# hi").expect("save");
    assert!(edges(d.path()).is_empty());
}

#[test]
fn saving_a_typescript_file_writes_its_base_edges() {
    let d = project();
    write(d.path(), "package.json", r#"{"name": "myapp"}"#);
    write(
        d.path(),
        "src/billing.ts",
        "export function charge() { return 1 }\n",
    );
    rebuild(d.path()).expect("opt the project into the graph");
    let names: Vec<String> = edges(d.path())
        .into_iter()
        .filter(|e| e.p == "defines_fn")
        .filter_map(|e| e.a.get(1).cloned())
        .collect();
    // No tsconfig.json exists, so there is no baseUrl: `src/` stays part
    // of the module path (see graph/resolve.rs's `strip_module_base` and
    // `graph/unit.rs`'s `context_for`, which only strips a unit's
    // `baseUrl` from the front of a file's path).
    assert!(
        names.contains(&"typescript:myapp::src::billing::charge".to_string()),
        "{names:?}"
    );
}

// ─── the hook entry point ───────────────────────────────────────

#[test]
fn a_project_that_never_opted_into_the_graph_gets_no_graph() {
    // The sensor runs before rules load, so it cannot key off rule
    // phases. Without an explicit gate it builds a graph in every
    // phronesis project on the first edit — imposing a per-save tree walk
    // and an unasked-for file on users who only wanted the `llm` pack.
    let d = project();
    write(d.path(), "src/a.rs", "fn f() {}");
    record_from_disk(d.path(), "src/a.rs");
    assert!(
        !store::graph_path(d.path()).exists(),
        "no graph existed, so none should be created"
    );
    assert!(!index_path(d.path()).exists());
}

#[test]
fn an_existing_graph_is_still_kept_current() {
    // `init --packs structural` builds the graph; its presence is the
    // opt-in signal the sensor keys off.
    let d = project();
    write(d.path(), "src/a.rs", "fn f() {}");
    rebuild(d.path()).expect("rebuild opts the project in");
    write(d.path(), "src/a.rs", "fn f() {}\nfn added() {}");
    record_from_disk(d.path(), "src/a.rs");
    let names: Vec<String> = edges(d.path())
        .into_iter()
        .filter(|e| e.p == "defines_fn")
        .filter_map(|e| e.a.get(1).cloned())
        .collect();
    assert!(names.iter().any(|n| n.ends_with("::added")), "{names:?}");
}

#[test]
fn recording_from_disk_accepts_the_absolute_path_a_host_sends() {
    // Every real host sends an absolute path. A guard that rejected them
    // silently disabled the sensor everywhere, and because the sensor is
    // best-effort by design nothing reported it.
    let d = project();
    write(d.path(), "src/a.rs", "fn f() {}");
    rebuild(d.path()).expect("opt the project into the graph");
    write(d.path(), "src/a.rs", "fn f() {}\nfn added() {}");
    let absolute = d.path().join("src/a.rs");
    record_from_disk(d.path(), absolute.to_str().expect("utf8"));
    let names: Vec<String> = edges(d.path())
        .into_iter()
        .filter(|e| e.p == "defines_fn")
        .filter_map(|e| e.a.get(1).cloned())
        .collect();
    assert!(
        names.iter().any(|n| n.ends_with("::added")),
        "sensor must record the edit: {names:?}"
    );
}

#[test]
fn a_parse_failure_preserves_the_files_existing_edges() {
    // A malformed mid-edit save must not be read as "this file now
    // defines nothing". Compacting an empty extraction erases every
    // function, risky call and import the file had, and recording the
    // malformed content's hash makes the graph report itself fresh — so
    // the harness keeps enforcing on evidence it silently destroyed.
    let d = project();
    write(d.path(), "src/a.rs", "fn important() {}");
    on_save(d.path(), "src/a.rs", "fn important() {}").expect("save");
    assert!(has(d.path(), "defines_fn"), "precondition");

    let broken = "fn important( { ((( ";
    write(d.path(), "src/a.rs", broken);
    on_save(d.path(), "src/a.rs", broken).expect("save");

    let names: Vec<String> = edges(d.path())
        .into_iter()
        .filter(|e| e.p == "defines_fn")
        .filter_map(|e| e.a.get(1).cloned())
        .collect();
    assert!(
        names.iter().any(|n| n.ends_with("::important")),
        "existing evidence must survive a parse failure: {names:?}"
    );
}

#[test]
fn a_parse_failure_leaves_the_file_reported_as_stale() {
    // Staleness is the honest state: the graph no longer reflects what is
    // on disk, and structural rules must demote to warnings until it does.
    let d = project();
    write(d.path(), "src/a.rs", "fn important() {}");
    on_save(d.path(), "src/a.rs", "fn important() {}").expect("save");

    let broken = "fn important( { ((( ";
    write(d.path(), "src/a.rs", broken);
    on_save(d.path(), "src/a.rs", broken).expect("save");

    let index = load_index(&index_path(d.path())).expect("load index");
    assert_eq!(
        check_freshness(d.path(), &index),
        Freshness::Stale(vec!["src/a.rs".to_string()]),
        "an unparseable file must not be recorded as successfully indexed"
    );
}

#[test]
fn deleting_a_file_removes_its_edges_and_its_index_entry() {
    // A `Delete File` patch block routes through the sensor. Leaving the
    // edges and hash behind makes every later freshness check report
    // drift, demoting structural rules to warnings until a manual
    // rebuild — the exact failure the sensor exists to prevent.
    let d = project();
    write(d.path(), "src/a.rs", "fn alpha() {}");
    write(d.path(), "src/b.rs", "fn beta() {}");
    on_save(d.path(), "src/a.rs", "fn alpha() {}").expect("save a");
    on_save(d.path(), "src/b.rs", "fn beta() {}").expect("save b");

    std::fs::remove_file(d.path().join("src/a.rs")).expect("delete");
    record_from_disk(d.path(), "src/a.rs");

    let names: Vec<String> = edges(d.path())
        .into_iter()
        .filter(|e| e.p == "defines_fn")
        .filter_map(|e| e.a.get(1).cloned())
        .collect();
    assert!(
        names.iter().all(|n| !n.ends_with("::alpha")),
        "the deleted file's edges must go: {names:?}"
    );
    assert!(names.iter().any(|n| n.ends_with("::beta")), "{names:?}");
    assert!(
        !load_index(&index_path(d.path()))
            .expect("load index")
            .entries
            .contains_key("src/a.rs"),
        "a hash for a path that no longer exists is permanent drift"
    );
    assert_eq!(
        check_freshness(d.path(), &load_index(&index_path(d.path())).expect("index")),
        Freshness::Fresh,
        "a recorded deletion must leave the graph fresh"
    );
}

#[test]
fn recording_from_disk_ignores_a_path_outside_the_project() {
    let d = project();
    let outside = TempDir::new().expect("tempdir");
    write(outside.path(), "evil.rs", "fn f() {}");
    let absolute = outside.path().join("evil.rs");
    record_from_disk(d.path(), absolute.to_str().expect("utf8"));
    assert!(
        edges(d.path()).is_empty(),
        "a file outside the project has no graph identity"
    );
}

#[test]
fn recording_from_disk_reads_the_current_file_content() {
    let d = project();
    write(d.path(), "src/a.rs", "fn f() {}");
    rebuild(d.path()).expect("opt the project into the graph");
    write(d.path(), "src/a.rs", "fn f() {}\nfn later() {}");
    record_from_disk(d.path(), "src/a.rs");
    let names: Vec<String> = edges(d.path())
        .into_iter()
        .filter(|e| e.p == "defines_fn")
        .filter_map(|e| e.a.get(1).cloned())
        .collect();
    assert!(names.iter().any(|n| n.ends_with("::later")), "{names:?}");
}

#[test]
fn recording_a_file_outside_the_project_is_a_no_op() {
    // Path traversal must not let the sensor read arbitrary files.
    let d = project();
    record_from_disk(d.path(), "../../etc/hosts.rs");
    assert!(edges(d.path()).is_empty());
}

#[test]
fn recording_a_missing_file_is_a_no_op_rather_than_an_error() {
    let d = project();
    record_from_disk(d.path(), "src/gone.rs");
    assert!(edges(d.path()).is_empty());
}

#[test]
fn recording_a_node_modules_file_is_a_no_op() {
    // `tracked_files` (and thus `rebuild`/`check_freshness`) prunes
    // `node_modules`. If the sensor recorded an index entry for a path
    // under it anyway, that hash could never be matched by the walk and
    // the file would report as drift forever — the exact permanent-
    // demotion failure the sensor exists to prevent, entering through
    // `on_save` instead of `rebuild`. `patch-package`, `prisma
    // generate`, or an agent edit under `node_modules` can all arrive
    // here through `PostToolUse`.
    let d = project();
    write(d.path(), "package.json", r#"{"name": "myapp"}"#);
    write(
        d.path(),
        "src/billing.ts",
        "export function charge() { return 1 }\n",
    );
    rebuild(d.path()).expect("opt the project into the graph");

    write(
        d.path(),
        "node_modules/dep/index.ts",
        "export function vendored() { return 1 }\n",
    );
    record_from_disk(d.path(), "node_modules/dep/index.ts");

    let idx = load_index(&index_path(d.path())).expect("load index");
    assert!(
        !idx.entries.contains_key("node_modules/dep/index.ts"),
        "node_modules must never gain an index entry: {:?}",
        idx.entries.keys().collect::<Vec<_>>()
    );
    let names: Vec<String> = edges(d.path())
        .into_iter()
        .filter(|e| e.p == "defines_fn")
        .filter_map(|e| e.a.get(1).cloned())
        .collect();
    assert!(
        names.iter().all(|n| !n.contains("vendored")),
        "node_modules must not be extracted: {names:?}"
    );
    assert_eq!(
        check_freshness(d.path(), &idx),
        Freshness::Fresh,
        "a node_modules edit must not be reported as drift"
    );
}

// ─── staleness ──────────────────────────────────────────────────

#[test]
fn an_untouched_project_is_fresh() {
    let d = project();
    write(d.path(), "src/a.rs", "fn f() {}");
    on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
    let idx = load_index(&index_path(d.path())).expect("load");
    assert_eq!(check_freshness(d.path(), &idx), Freshness::Fresh);
}

#[test]
fn an_edit_outside_the_hook_path_is_detected_as_stale() {
    let d = project();
    write(d.path(), "src/a.rs", "fn f() {}");
    on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
    // Simulates git checkout / shell edit: content changes, hook never runs.
    write(d.path(), "src/a.rs", "fn f() {}\nfn sneaky() {}");
    let idx = load_index(&index_path(d.path())).expect("load");
    assert_eq!(
        check_freshness(d.path(), &idx),
        Freshness::Stale(vec!["src/a.rs".to_string()])
    );
}

#[test]
fn a_deleted_file_is_detected_as_stale() {
    let d = project();
    write(d.path(), "src/a.rs", "fn f() {}");
    on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
    std::fs::remove_file(d.path().join("src/a.rs")).expect("rm");
    let idx = load_index(&index_path(d.path())).expect("load");
    assert!(matches!(
        check_freshness(d.path(), &idx),
        Freshness::Stale(_)
    ));
}

#[test]
fn an_untracked_new_file_is_detected_as_stale() {
    let d = project();
    write(d.path(), "src/a.rs", "fn f() {}");
    on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
    write(d.path(), "src/b.rs", "fn g() {}");
    let idx = load_index(&index_path(d.path())).expect("load");
    assert!(matches!(
        check_freshness(d.path(), &idx),
        Freshness::Stale(_)
    ));
}

// ─── rebuild ────────────────────────────────────────────────────

#[test]
fn rebuild_indexes_every_rust_file_in_the_project() {
    let d = project();
    write(d.path(), "src/a.rs", "fn f() {}");
    write(d.path(), "src/b.rs", "fn g() {}");
    rebuild(d.path()).expect("rebuild");
    let names: Vec<String> = edges(d.path())
        .into_iter()
        .filter(|e| e.p == "defines_fn")
        .filter_map(|e| e.a.get(1).cloned())
        .collect();
    assert_eq!(names.len(), 2, "both files must be scanned");
}

#[test]
fn rebuild_composes_multilingual_modules_and_cross_language_schema_imports() {
    let d = project();
    write(d.path(), "src/lib.rs", "pub fn run() {}");
    write(
        d.path(),
        "scripts/run.lua",
        "function run() return true end",
    );
    write(d.path(), "config/model.cue", "#Model: { name: string }");
    write(
        d.path(),
        "schemas/base.yaml",
        "$schema: https://json-schema.org/draft/2020-12/schema\n$anchor: User\n",
    );
    write(
        d.path(),
        "schemas/user.json",
        r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","$ref":"base.yaml#User"}"#,
    );
    write(
        d.path(),
        "charts/app/Chart.yaml",
        "name: app\nversion: 0.1.0\n",
    );
    write(
        d.path(),
        "charts/app/templates/_helpers.tpl",
        "{{ define \"app.name\" }}app{{ end }}",
    );
    write(
        d.path(),
        "charts/app/templates/deployment.yaml",
        "apiVersion: apps/v1\nmetadata:\n  name: {{ include \"app.name\" . }}\n",
    );

    rebuild(d.path()).expect("rebuild multilingual graph");
    let graph = edges(d.path());

    for prefix in ["rust:", "lua:", "cue:", "json:", "yaml:", "helm3:"] {
        assert!(
            graph.iter().any(|edge| {
                edge.p == "declares_module"
                    && edge
                        .a
                        .get(1)
                        .is_some_and(|module| module.starts_with(prefix))
            }),
            "missing language-qualified {prefix} module"
        );
    }
    assert!(graph.iter().any(|edge| {
        edge.p == "imports"
            && edge.a
                == [
                    "json:project::schemas::user".to_string(),
                    "yaml:project::schemas::base::doc:0".to_string(),
                ]
    }));
    assert!(
        graph.iter().any(|edge| {
            edge.p == "declares_module"
                && edge.a
                    == [
                        "charts/app/templates/deployment.yaml".to_string(),
                        "helm3:charts/app::templates::deployment".to_string(),
                    ]
        }),
        "templated YAML ownership: {:?}",
        graph
            .iter()
            .filter(|edge| edge.src.contains("deployment.yaml"))
            .collect::<Vec<_>>()
    );
    assert!(
        graph
            .iter()
            .filter(|edge| edge.p == "graph_definition")
            .all(|edge| { edge.a.len() == 1 && edge.a[0].contains(':') })
    );

    let modules = graph
        .iter()
        .filter(|edge| edge.p == "declares_module")
        .filter_map(|edge| edge.a.get(1))
        .collect::<BTreeSet<_>>();
    for edge in graph.iter().filter(|edge| edge.p == "imports") {
        let Some(target) = edge.a.get(1) else {
            continue;
        };
        if ["cue:", "lua:", "json:", "yaml:", "helm3:"]
            .iter()
            .any(|prefix| target.starts_with(prefix))
        {
            assert!(
                modules.contains(target),
                "repository-local dependency target is not a graph node: {edge:?}"
            );
        }
    }
    for prefix in ["cue:", "lua:", "json:", "yaml:", "helm3:"] {
        let definitions = graph
            .iter()
            .filter(|edge| edge.p == "graph_definition")
            .filter(|edge| edge.a[0].starts_with(prefix))
            .count();
        assert!(
            definitions <= 8,
            "representative {prefix} fixture emitted excessive definitions: {definitions}"
        );
    }
}

#[test]
fn rebuild_restores_freshness_after_drift() {
    let d = project();
    write(d.path(), "src/a.rs", "fn f() {}");
    on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
    write(d.path(), "src/a.rs", "fn f() {}\nfn sneaky() {}");
    rebuild(d.path()).expect("rebuild");
    let idx = load_index(&index_path(d.path())).expect("load");
    assert_eq!(check_freshness(d.path(), &idx), Freshness::Fresh);
}

#[test]
fn rebuild_excludes_node_modules_from_typescript_tracking() {
    // `tracked_files` drives both `rebuild` and the freshness check. If
    // `node_modules` is not pruned there too — discovery already prunes
    // it, but that is a separate walk — `rebuild` would extract from
    // every dependency's TypeScript, and every one of those files would
    // then show as drift on every subsequent freshness check.
    let d = project();
    write(d.path(), "package.json", r#"{"name": "myapp"}"#);
    write(
        d.path(),
        "src/billing.ts",
        "export function charge() { return 1 }\n",
    );
    write(
        d.path(),
        "node_modules/dep/index.ts",
        "export function vendored() { return 1 }\n",
    );
    rebuild(d.path()).expect("rebuild");

    let names: Vec<String> = edges(d.path())
        .into_iter()
        .filter(|e| e.p == "defines_fn")
        .filter_map(|e| e.a.get(1).cloned())
        .collect();
    assert!(
        names.iter().all(|n| !n.contains("vendored")),
        "node_modules must not be extracted: {names:?}"
    );

    let idx = load_index(&index_path(d.path())).expect("load index");
    assert!(
        !idx.entries.keys().any(|k| k.contains("node_modules")),
        "node_modules must not be indexed: {:?}",
        idx.entries.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        check_freshness(d.path(), &idx),
        Freshness::Fresh,
        "an untracked node_modules file must not be reported as drift"
    );
}

#[test]
fn a_typescript_project_is_fresh_immediately_after_rebuild() {
    // Adding four new tracked extensions means files that were
    // previously invisible to `tracked_files` are now tracked. A
    // TypeScript project must report Fresh right after `rebuild`, not
    // immediately show drift.
    let d = project();
    write(d.path(), "package.json", r#"{"name": "myapp"}"#);
    write(
        d.path(),
        "src/billing.ts",
        "export function charge() { return 1 }\n",
    );
    rebuild(d.path()).expect("rebuild");
    let idx = load_index(&index_path(d.path())).expect("load index");
    assert_eq!(check_freshness(d.path(), &idx), Freshness::Fresh);
}

#[test]
fn rebuild_tracks_and_indexes_rhai_scripts() {
    let d = project();
    write(
        d.path(),
        "scripts/combat.rhai",
        "state_attempt_stunning_strike(actor, target);\n",
    );
    rebuild(d.path()).expect("rebuild");
    assert!(
        edges(d.path())
            .iter()
            .any(|edge| { edge.p == "calls" && edge.src == "scripts/combat.rhai" })
    );
    let index = load_index(&index_path(d.path())).expect("index");
    assert!(index.entries.contains_key("scripts/combat.rhai"));
}

#[test]
fn rebuild_drops_edges_for_files_that_no_longer_exist() {
    let d = project();
    write(d.path(), "src/a.rs", "fn f() {}");
    on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
    std::fs::remove_file(d.path().join("src/a.rs")).expect("rm");
    rebuild(d.path()).expect("rebuild");
    assert!(edges(d.path()).is_empty());
}
