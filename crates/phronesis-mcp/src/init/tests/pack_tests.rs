use crate::init::rules_python::python_rules;
use crate::init::rules_rust::rust_rules;
use crate::init::*;
use serde_json::Value;

#[test]
fn user_config_paths_are_based_on_home() {
    let home = tempfile::tempdir().unwrap();
    let old = std::env::var_os("HOME");
    unsafe {
        std::env::set_var("HOME", home.path());
    }
    assert_eq!(
        user_claude_config_path(),
        Some(home.path().join(".claude.json"))
    );
    assert_eq!(
        user_gemini_config_path(),
        Some(home.path().join(".gemini/settings.json"))
    );
    match old {
        Some(value) => unsafe { std::env::set_var("HOME", value) },
        None => unsafe { std::env::remove_var("HOME") },
    }
}

/// Load-time rule validation fails closed (decision D1), so a starter
/// pack that no longer validates would make every hook in a freshly
/// initialized project block. Every pack, alone and composed, must load.
#[test]
fn every_starter_pack_passes_load_time_validation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.json");
    let mut sets: Vec<(String, Value)> = Pack::ALL
        .iter()
        .map(|p| (p.label().to_string(), p.rules()))
        .collect();
    let composable: Vec<Pack> = Pack::ALL
        .iter()
        .copied()
        .filter(|p| *p != Pack::None)
        .collect();
    sets.push(("all packs composed".to_string(), compose_packs(&composable)));
    for (label, rules) in sets {
        std::fs::write(&path, serde_json::to_vec(&rules).unwrap()).unwrap();
        if let Err(e) = crate::rules_file::read(&path) {
            panic!("pack `{label}` fails load-time validation: {e}");
        }
    }
}

/// This repository's own `.phronesis/rules.json` must load too. The file
/// is not tracked, so the check runs only in a checkout that has one.
#[test]
fn this_repos_rules_pass_load_time_validation() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.phronesis/rules.json");
    if path.exists()
        && let Err(e) = crate::rules_file::read(&path)
    {
        panic!("{} fails load-time validation: {e}", path.display());
    }
}

#[test]
fn pack_parse_accepts_aliases() {
    assert_eq!(Pack::parse("llm").unwrap(), Pack::Llm);
    assert_eq!(Pack::parse("minimal").unwrap(), Pack::Llm); // legacy alias
    assert_eq!(Pack::parse("rust").unwrap(), Pack::Rust);
    assert_eq!(Pack::parse("rs").unwrap(), Pack::Rust);
    assert_eq!(Pack::parse("PYTHON").unwrap(), Pack::Python);
    assert_eq!(Pack::parse("ts").unwrap(), Pack::TypeScript);
    assert_eq!(Pack::parse("js").unwrap(), Pack::TypeScript);
    assert_eq!(Pack::parse("none").unwrap(), Pack::None);
    assert_eq!(Pack::parse("rhai").unwrap(), Pack::Rhai);
    assert_eq!(Pack::parse("RHAI").unwrap(), Pack::Rhai);
}

#[test]
fn parses_swift_pack() {
    assert_eq!(Pack::parse("swift").unwrap(), Pack::Swift);
}

#[test]
fn parses_new_language_packs() {
    assert_eq!(Pack::parse("lua").unwrap(), Pack::Lua);
    assert_eq!(Pack::parse("LUA").unwrap(), Pack::Lua);
    assert_eq!(Pack::parse("cue").unwrap(), Pack::Cue);
    assert_eq!(Pack::parse("json").unwrap(), Pack::Json);
    assert_eq!(Pack::parse("yaml").unwrap(), Pack::Yaml);
    assert_eq!(Pack::parse("yml").unwrap(), Pack::Yaml);
    assert_eq!(Pack::parse("helm3").unwrap(), Pack::Helm3);
    assert_eq!(Pack::parse("helm").unwrap(), Pack::Helm3);
    assert_eq!(Pack::parse("java").unwrap(), Pack::Java);
}

#[test]
fn java_pack_has_no_rules_and_merges_toolchains_without_replacing_user_entries() {
    use crate::init::writers_scaffold::write_java_toolchains;
    let dir = tempfile::tempdir().expect("tempdir");
    let opts = InitOpts {
        project_root: dir.path().into(),
        packs: vec![Pack::Java],
        force: false,
        dry_run: false,
        rules_only: false,
        hooks_only: false,
    };
    let mut report = InitReport::default();
    write_java_toolchains(dir.path(), &opts, &mut report).expect("write defs");
    let path = dir.path().join(".phronesis/toolchains.json");
    let first: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read defs")).expect("json");
    assert!(
        first
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == "mvn")
    );
    assert!(
        first
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == "gradle")
    );
    assert_eq!(Pack::Java.rules()["rules"].as_array().unwrap().len(), 0);

    std::fs::write(&path, r#"[{"id":"mvn","matches":"custom-maven"}]"#).expect("user edit");
    write_java_toolchains(dir.path(), &opts, &mut report).expect("merge defs");
    let merged: Value =
        serde_json::from_slice(&std::fs::read(path).expect("read merged")).expect("json");
    assert_eq!(merged[0]["matches"], "custom-maven");
    assert!(
        merged
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == "gradle")
    );
}

#[test]
fn parses_structural_pack() {
    assert_eq!(
        Pack::parse("structural").expect("structural"),
        Pack::Structural
    );
    assert_eq!(Pack::parse("graph").expect("graph alias"), Pack::Structural);
}

/// The structural pack's rules, as a vector.
fn structural_rules_vec() -> Vec<serde_json::Value> {
    Pack::Structural.rules()["rules"]
        .as_array()
        .expect("rules array")
        .clone()
}

#[test]
fn structural_pack_ships_the_two_measured_rules() {
    let ids: Vec<String> = structural_rules_vec()
        .iter()
        .filter_map(|r| r["id"].as_str().map(str::to_string))
        .collect();
    assert!(ids.contains(&"warn-untested-risky-call".to_string()));
    assert!(ids.contains(&"warn-import-cycle".to_string()));
}

#[test]
fn structural_rules_cover_typescript() {
    // Only the risky-call rule is TypeScript-specific: `!` and its advice
    // differ from Rust's watchlist. Import cycles are language-neutral,
    // so `warn-import-cycle` alone covers TypeScript modules too — a
    // `warn-ts-import-cycle` twin would just double-report every cycle.
    let ids: Vec<String> = structural_rules_vec()
        .iter()
        .filter_map(|r| r["id"].as_str().map(str::to_string))
        .collect();
    assert!(
        ids.contains(&"warn-ts-untested-risky-call".to_string()),
        "{ids:?}"
    );
    assert!(ids.contains(&"warn-import-cycle".to_string()), "{ids:?}");
    assert!(
        !ids.contains(&"warn-ts-import-cycle".to_string()),
        "the cycle rule is language-neutral and must not be duplicated per language: {ids:?}"
    );
}

#[test]
fn structural_rules_only_warn() {
    // Spec §5.3: nothing blocks until a second corpus is measured. A
    // heuristic that has never been measured must not be able to stop work.
    for rule in structural_rules_vec() {
        let then = &rule["then"];
        assert!(
            then.get("block").is_none(),
            "structural rule {} must not block",
            rule["id"]
        );
        assert!(then.get("warn").is_some(), "rule {} must warn", rule["id"]);
    }
}

#[test]
fn structural_rules_opt_into_audit() {
    // Graph rules are invisible to the file-scanning audit loop, but
    // `graph::audit` evaluates them against the whole graph and merges
    // the findings. Opting in is what routes them there — without it a
    // clean audit would mean "never checked", not "nothing found".
    for rule in structural_rules_vec() {
        assert_eq!(
            rule.get("audit").and_then(|v| v.as_bool()),
            Some(true),
            "rule {} must opt into audit",
            rule["id"]
        );
    }
}

#[test]
fn structural_rules_reference_only_graph_relations() {
    // Every condition must name a relation hydration can supply, or the
    // rule is dead weight that never matches. `or` is a DNF wrapper, not
    // a relation itself — recurse into its branches instead of checking
    // the literal key "or" against the relation list.
    fn check(cond: &serde_json::Value, relations: &[&str], rule_id: &str) {
        let key = cond
            .as_object()
            .and_then(|o| o.keys().next().cloned())
            .expect("condition key");
        if key == "or" {
            for branch in cond["or"].as_array().expect("or array") {
                check(branch, relations, rule_id);
            }
            return;
        }
        assert!(
            relations.contains(&key.as_str()),
            "rule {rule_id} uses unknown relation {key}"
        );
    }
    let relations = crate::graph::hydrate::GRAPH_RELATIONS;
    for rule in structural_rules_vec() {
        let id = rule["id"].as_str().unwrap_or("?").to_string();
        for cond in rule["when"].as_array().expect("when array") {
            check(cond, relations, &id);
        }
    }
}

#[test]
fn rust_pack_does_not_carry_structural_rules() {
    let rust_ids: Vec<String> = Pack::Rust.rules()["rules"]
        .as_array()
        .expect("rules")
        .iter()
        .filter_map(|r| r["id"].as_str().map(str::to_string))
        .collect();
    assert!(!rust_ids.contains(&"warn-import-cycle".to_string()));
}

#[test]
fn rhai_pack_carries_rhai_rules_and_rust_does_not() {
    let rhai = Pack::Rhai.rules();
    let rhai_ids: Vec<&str> = rhai["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert!(rhai_ids.contains(&"block-rhai-inline-eval-string"));
    assert!(rhai_ids.contains(&"block-rhai-print-in-script"));

    let rust = Pack::Rust.rules();
    let rust_ids: Vec<&str> = rust["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert!(!rust_ids.contains(&"block-rhai-inline-eval-string"));
    assert!(!rust_ids.contains(&"block-rhai-print-in-script"));
}

/// The Rhai-pack messages should be project-neutral: no references
/// to helper identifiers, file names, or project codenames from
/// any particular host. Pins the "generalize messages" intent of
/// the 0.6.1 split. The forbidden-token list below names some
/// historical leaks the test exists to guard against.
#[test]
fn rhai_pack_messages_are_project_neutral() {
    let v = Pack::Rhai.rules();
    let arr = v["rules"].as_array().unwrap();
    for rule in arr {
        // v2 shape: message is in rule["then"]["block"] or rule["then"]["warn"]
        let then = &rule["then"];
        let msg = then
            .get("block")
            .or_else(|| then.get("warn"))
            .or_else(|| then.get("log"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        for forbidden in ["GameLogicLoader", "save.rhai", "response_append"] {
            assert!(
                !msg.contains(forbidden),
                "rhai pack message for {} contains project-specific reference {:?}",
                rule["id"],
                forbidden
            );
        }
    }
}

#[test]
fn swift_pack_yields_rules() {
    let v = Pack::Swift.rules();
    let arr = v.get("rules").unwrap().as_array().unwrap();
    assert!(!arr.is_empty(), "swift pack should ship at least one rule");
    let ids: Vec<&str> = arr.iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&"warn-swift-force-unwrap"));
    assert!(ids.contains(&"warn-swift-try-bang"));
    assert!(ids.contains(&"warn-swift-force-cast"));
    assert!(ids.contains(&"audit-swift-fatal-error"));
    assert!(ids.contains(&"audit-swift-mutable-singleton"));
    assert!(ids.contains(&"audit-swift-legacy-constructor"));
    assert!(ids.contains(&"audit-swift-legacy-random"));
}

#[test]
fn pack_parse_rejects_unknown() {
    assert!(matches!(
        Pack::parse("haskell"),
        Err(InitError::UnknownPack(_))
    ));
}

#[test]
fn parse_packs_default_is_the_complete_base() {
    assert_eq!(parse_packs("").unwrap(), BASE_PACKS);
}

#[test]
fn parse_packs_handles_comma_separated_list() {
    let p = parse_packs("llm, rust").unwrap();
    let mut expected = BASE_PACKS.to_vec();
    expected.push(Pack::Rust);
    assert_eq!(p, expected);
}

#[test]
fn parse_packs_dedupes_duplicates() {
    let p = parse_packs("rust,llm,rust,llm").unwrap();
    let mut expected = BASE_PACKS.to_vec();
    expected.push(Pack::Rust);
    assert_eq!(p, expected);
}

#[test]
fn parse_packs_rejects_none_combined_with_another_pack() {
    let err = parse_packs("none,rust").expect_err("none must be exclusive");
    assert!(matches!(err, InitError::InvalidPackSelection(_)));
    assert!(err.to_string().contains("cannot be combined"));
}

/// SPEC-C S1: the trust-anchor refusal ships in the default platform,
/// with or without a language pack. The file-tool rules block (the
/// enforced seam); the lexical shell rule warns (the advisory seam).
#[test]
fn default_platform_blocks_trust_anchor_writes() {
    for selection in ["", "rust", "python,typescript"] {
        let v = compose_packs(&parse_packs(selection).unwrap());
        for (id, verb) in [
            ("block-agent-write-to-verification-allowlist", "block"),
            ("block-agent-write-to-verification-optin", "block"),
            ("block-agent-write-to-verification-templates", "block"),
            ("warn-agent-shell-write-to-trust-anchors", "warn"),
        ] {
            let rule = v["rules"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["id"] == id)
                .unwrap_or_else(|| panic!("`{selection}` must install {id}"));
            assert_eq!(rule["phase"], "pre", "{id}");
            assert!(rule["then"][verb].is_string(), "{id} must {verb}");
            assert!(rule.get("audit").is_none(), "{id}: path rules never audit");
        }
    }
}

#[test]
fn llm_pack_is_only_deflection_rules() {
    let v = Pack::Llm.rules();
    let ids: Vec<&str> = v["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"enforce-no-pre-existing-issue"));
    assert!(ids.contains(&"enforce-no-not-from-our-changes"));
    assert!(ids.contains(&"enforce-no-not-caused-by-our"));
    // Should NOT carry language-specific rules
    assert!(!ids.contains(&"enforce-no-unwrap-in-src"));
}

#[test]
fn rust_pack_carries_only_rust_rules() {
    let v = Pack::Rust.rules();
    let ids: Vec<&str> = v["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"enforce-no-unwrap-in-src"));
    assert!(ids.contains(&"enforce-no-result-string-error"));
    assert!(ids.contains(&"warn-dbg-in-src"));
    // Should NOT bundle deflection rules — they're a separate pack
    assert!(!ids.contains(&"enforce-no-pre-existing-issue"));
}

#[test]
fn rust_pack_includes_new_predicate_rules() {
    let v = Pack::Rust.rules();
    let arr = v.get("rules").unwrap().as_array().unwrap();
    let ids: Vec<&str> = arr.iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&"warn-rust-public-fn-takes-string-ref"));
    assert!(ids.contains(&"warn-rust-public-fn-takes-vec-ref"));
    assert!(ids.contains(&"warn-deref-for-non-pointer-type"));
}

#[test]
fn rust_pack_includes_block_pattern_rules() {
    let v = Pack::Rust.rules();
    let ids: Vec<&str> = v["rules"]
        .as_array()
        .expect("rules array")
        .iter()
        .map(|r| r["id"].as_str().expect("rule id is a string"))
        .collect();
    assert!(
        ids.contains(&"audit-rust-let-binding-count-high"),
        "expected audit-rust-let-binding-count-high in rust pack, got {:?}",
        ids
    );
    assert!(
        ids.contains(&"audit-rust-let-mut-count-high"),
        "expected audit-rust-let-mut-count-high in rust pack, got {:?}",
        ids
    );
}

#[test]
fn let_count_audit_rules_are_doc_excepted() {
    let v = Pack::Rust.rules();
    let arr = v.get("rules").unwrap().as_array().unwrap();
    for id in [
        "audit-rust-let-binding-count-high",
        "audit-rust-let-mut-count-high",
    ] {
        let rule = arr
            .iter()
            .find(|r| r["id"] == id)
            .unwrap_or_else(|| panic!("{id} missing from rust pack"));
        assert_eq!(
            rule["doc_excepted"], true,
            "{id} must honor //! phronesis-allow markers"
        );
    }
}

/// All audit-only rules added in 0.4.0 should carry both `phase: "audit"`
/// and `audit: true`. Pins the audit-only convention against accidental
/// regression to a hook phase.
#[test]
fn rust_pack_audit_only_rules_have_consistent_shape() {
    let v = Pack::Rust.rules();
    let arr = v.get("rules").unwrap().as_array().unwrap();
    let audit_only_ids = [
        "audit-manual-err-return",
        "audit-newtype-id-string",
        "audit-newtype-id-u64",
        "audit-if-let-opportunity-none-empty",
        "audit-if-let-opportunity-err-empty",
        "audit-rc-refcell-in-src",
        "audit-string-concat-with-plus",
        "audit-allow-dead-code-in-src",
        "audit-env-set-var-in-src",
    ];
    for id in audit_only_ids {
        let rule = arr
            .iter()
            .find(|r| r["id"] == id)
            .unwrap_or_else(|| panic!("rust pack must include {id}"));
        assert_eq!(rule["phase"], "audit", "{id} must be phase: audit");
        assert_eq!(rule["audit"], true, "{id} must be audit: true");
    }
}

/// Rules added in 0.6.1, sourced from the rust-unofficial/patterns book.
/// Verifies presence and that the block/warn ones use the right severity.
#[test]
fn rust_pack_includes_patterns_book_rules() {
    let v = Pack::Rust.rules();
    let arr = v.get("rules").unwrap().as_array().unwrap();
    let by_id = |id: &str| {
        arr.iter()
            .find(|r| r["id"] == id)
            .unwrap_or_else(|| panic!("rust pack must include {id}"))
    };
    // v2 shape: severity is the key in rule["then"]
    assert!(
        by_id("block-deny-warnings-attribute")["then"]
            .get("block")
            .is_some(),
        "block-deny-warnings-attribute must use 'block' verb"
    );
    assert!(
        by_id("warn-public-fn-takes-box-ref")["then"]
            .get("warn")
            .is_some(),
        "warn-public-fn-takes-box-ref must use 'warn' verb"
    );
    assert!(
        by_id("warn-expect-with-empty-message")["then"]
            .get("warn")
            .is_some(),
        "warn-expect-with-empty-message must use 'warn' verb"
    );
    for id in [
        "audit-rc-refcell-in-src",
        "audit-string-concat-with-plus",
        "audit-allow-dead-code-in-src",
    ] {
        assert_eq!(by_id(id)["phase"], "audit");
    }
}

#[test]
fn rust_pack_includes_tier_1_rules() {
    let rules = rust_rules();
    let arr = rules["rules"].as_array().unwrap();
    let ids: Vec<&str> = arr.iter().map(|r| r["id"].as_str().unwrap()).collect();
    for required in &[
        "warn-cargo-build-without-workspace",
        "warn-clone-heavy",
        "warn-empty-test",
    ] {
        assert!(
            ids.contains(required),
            "rust pack must include {}",
            required
        );
    }
    // The replaced rule is gone.
    assert!(
        !ids.contains(&"warn-rust-clone-count"),
        "warn-rust-clone-count must be removed (replaced by warn-clone-heavy)"
    );
    for retired in [
        "block-await-on-sync-execute-all-agenda-items",
        "block-await-on-sync-fire-all-consequences",
    ] {
        assert!(
            !ids.contains(&retired),
            "repository-specific rule {retired} must not ship in the public Rust pack"
        );
    }
}

#[test]
fn compose_packs_llm_plus_rust_merges_both() {
    let v = compose_packs(&[Pack::Llm, Pack::Rust]);
    let ids: Vec<&str> = v["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"enforce-no-pre-existing-issue"));
    assert!(ids.contains(&"enforce-no-unwrap-in-src"));
}

#[test]
fn rust_pack_includes_runtime_hazard_rules() {
    let rules = rust_rules();
    let ids: Vec<&str> = rules["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|rule| rule["id"].as_str().unwrap())
        .collect();
    for id in [
        "audit-rust-sync-lock-across-await",
        "audit-rust-unsafe-without-safety-comment",
        "warn-rust-blocking-call-in-async",
    ] {
        assert!(ids.contains(&id), "rust pack must include {id}");
    }
}

#[test]
fn rust_pack_includes_panic_in_drop_rule() {
    let rules = rust_rules();
    let arr = rules["rules"].as_array().unwrap();
    let rule = arr
        .iter()
        .find(|r| r["id"] == "block-panic-in-drop-impl")
        .expect("rust pack must include block-panic-in-drop-impl");
    assert!(
        rule["then"].get("block").is_some(),
        "block-panic-in-drop-impl must use 'block' verb"
    );
    assert_eq!(rule["audit"], true, "block-panic-in-drop-impl must audit");
}

#[test]
fn python_pack_includes_patterns_guide_rules() {
    let rules = python_rules();
    let ids: Vec<&str> = rules["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|rule| rule["id"].as_str().unwrap())
        .collect();
    for id in [
        "warn-python-import-time-io",
        "warn-python-is-literal",
        "audit-python-mutated-module-global",
        "audit-python-star-import",
    ] {
        assert!(ids.contains(&id), "python pack must include {id}");
    }
}

#[test]
fn rust_pack_includes_pub_fn_doc_rule() {
    let v = Pack::Rust.rules();
    let ids: Vec<&str> = v["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert!(
        ids.contains(&"warn-pub-fn-missing-doc"),
        "rust pack must include warn-pub-fn-missing-doc, got {:?}",
        ids
    );
}

#[test]
fn swift_pack_includes_throws_force_unwrap_rule() {
    let v = Pack::Swift.rules();
    let ids: Vec<&str> = v["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert!(
        ids.contains(&"warn-swift-throws-with-force-unwrap"),
        "swift pack must include warn-swift-throws-with-force-unwrap, got {:?}",
        ids
    );
}

#[test]
fn typescript_pack_includes_param_count_rule() {
    let v = Pack::TypeScript.rules();
    let ids: Vec<&str> = v["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert!(
        ids.contains(&"warn-ts-function-param-count-high"),
        "typescript pack must include warn-ts-function-param-count-high, got {:?}",
        ids
    );
}

#[test]
fn compose_packs_dedupes_by_rule_id() {
    // Composing the same pack twice doesn't duplicate rules.
    let v = compose_packs(&[Pack::Llm, Pack::Llm]);
    let count = v["rules"].as_array().unwrap().len();
    let single = compose_packs(&[Pack::Llm]);
    assert_eq!(count, single["rules"].as_array().unwrap().len());
}

#[test]
fn none_pack_is_empty() {
    let v = Pack::None.rules();
    assert!(v["rules"].as_array().unwrap().is_empty());
}

#[test]
fn base_expands_to_every_language_agnostic_pack() {
    assert_eq!(parse_packs("base").expect("parse"), BASE_PACKS.to_vec());
    assert_eq!(
        BASE_PACKS,
        &[
            Pack::Llm,
            Pack::Confidence,
            Pack::Journey,
            Pack::Structural,
            Pack::Context,
        ],
        "base must enumerate every language-neutral capability, including graph"
    );
}

#[test]
fn base_contains_no_language_pack() {
    // Language packs match raw substrings gated only by path, so bundling
    // them produces cross-language false positives (the TypeScript `: any`
    // rule fires on Rust's `: anyhow::Error`).
    for language in [
        Pack::Rust,
        Pack::Rhai,
        Pack::Python,
        Pack::TypeScript,
        Pack::Swift,
        Pack::Lua,
        Pack::Cue,
        Pack::Json,
        Pack::Yaml,
        Pack::Helm3,
    ] {
        assert!(
            !BASE_PACKS.contains(&language),
            "{} must be opted into by name",
            language.label()
        );
    }
}

#[test]
fn base_composes_with_a_language_pack() {
    let packs = parse_packs("base,rust").expect("parse");
    assert!(packs.contains(&Pack::Rust));
    for pack in BASE_PACKS {
        assert!(packs.contains(pack), "{} missing", pack.label());
    }
}

#[test]
fn base_is_order_preserving_and_deduped() {
    // Naming a pack `base` already covers must not duplicate it, and the
    // explicit tail must keep its position.
    let packs = parse_packs("base,llm,structural,rust").expect("parse");
    let mut unique = packs.clone();
    unique.sort_by_key(|p| p.label());
    unique.dedup();
    assert_eq!(unique.len(), packs.len(), "duplicate pack in {packs:?}");
    assert_eq!(packs.last(), Some(&Pack::Rust));
}

#[test]
fn base_is_case_insensitive_and_whitespace_tolerant() {
    assert_eq!(
        parse_packs("  BASE , rust ").expect("parse"),
        parse_packs("base,rust").expect("parse")
    );
}

#[test]
fn an_unknown_pack_still_errors_and_names_base() {
    let err = parse_packs("base,nonsense").expect_err("unknown pack must error");
    let message = err.to_string();
    assert!(message.contains("nonsense"));
    assert!(message.contains("base"), "help must list base: {message}");
}
