use std::collections::{HashMap, HashSet};

use phr::{Action, Condition, ReteNetwork, Rule};
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn prop_single_file_path_facts_are_scoped_to_that_file(parts in prop::collection::vec("[a-z]{1,8}", 1..5), foreign in "[A-Z]{1,8}") {
        let path = parts.join("/");
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let check = rt.block_on(async {
            let network = ReteNetwork::new();
            crate::hook_facts::assert_common_facts(&network, &path, "Edit", "pre").await.expect("facts");
            let wmes = network.get_all_wmes().await.expect("working memory");
            let actual: HashSet<String> = wmes.iter()
                .filter(|w| w.fact.predicate == "file_path_matches")
                .flat_map(|w| w.fact.args.iter().cloned()).collect();
            Ok::<_, proptest::test_runner::TestCaseError>(actual)
        }).expect("async assertions");
        prop_assert_eq!(&check, &parts.iter().cloned().collect());
        prop_assert!(!check.contains(&foreign));
    }

    // hook_facts.single_file_gating_soundness: a rule whose `when` clause
    // contains a `file_path_matches` literal fires only when the event's
    // file actually matches that literal. "foreign" here is a realistic
    // adversary — a sibling file in the *same directory* — rather than a
    // disjoint alphabet that could never plausibly collide; a naive
    // implementation that matched by directory prefix instead of exact
    // segment equality would fire for the sibling too.
    #[test]
    fn prop_single_file_gating_rule_fires_only_for_matching_path(
        dir_parts in prop::collection::vec("[a-z]{1,4}", 0..3),
        stem in "[a-z]{1,4}",
        sibling_stem in "[a-z]{1,4}",
    ) {
        prop_assume!(stem != sibling_stem);

        let mut real_segments = dir_parts.clone();
        real_segments.push(stem.clone());
        let real_path = real_segments.join("/");

        let mut foreign_segments = dir_parts.clone();
        foreign_segments.push(sibling_stem.clone());
        let foreign_path = foreign_segments.join("/");

        let gate_rule = Rule {
            id: "gate".into(),
            priority: 1,
            conditions: vec![Condition {
                predicate: "file_path_matches".into(),
                args: vec![stem.clone()],
                script: None,
            }],
            actions: vec![Action {
                action_type: "constraint_violation".into(),
                params: vec!["matched".into()],
                data: None,
            }],
        };

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let (fires_for_real, fires_for_foreign) = rt.block_on(async {
            let real_net = ReteNetwork::new();
            real_net.add_rule(gate_rule.clone()).await.expect("add rule");
            crate::hook_facts::assert_common_facts(&real_net, &real_path, "Edit", "pre")
                .await.expect("facts");
            real_net.update_agenda().await.expect("update agenda");
            let real_actions = real_net.execute_all_agenda_items().expect("execute");

            let foreign_net = ReteNetwork::new();
            foreign_net.add_rule(gate_rule).await.expect("add rule");
            crate::hook_facts::assert_common_facts(&foreign_net, &foreign_path, "Edit", "pre")
                .await.expect("facts");
            foreign_net.update_agenda().await.expect("update agenda");
            let foreign_actions = foreign_net.execute_all_agenda_items().expect("execute");

            Ok::<_, proptest::test_runner::TestCaseError>((!real_actions.is_empty(), !foreign_actions.is_empty()))
        }).expect("async gating");

        prop_assert!(fires_for_real);
        prop_assert!(!fires_for_foreign);
    }

    #[test]
    fn prop_coverage_facts_require_a_demanded_relation(mask in 0u16..1024) {
        let relations: HashSet<String> = crate::coverage::hydrate::RELATIONS.iter().enumerate()
            .filter(|(bit, _)| mask & (1 << bit) != 0)
            .map(|(_, relation)| (*relation).to_string()).collect();
        let dir = tempfile::tempdir().expect("tempdir");
        let input = crate::coverage::hydrate::HydrationInput {
            root: dir.path(), rule_relations: relations.clone(), edited: vec![], head_sha: Some("0123456789abcdef".into()),
        };
        let got = crate::coverage::hydrate::hydrate(&input).expect("hydrate");
        let demanded = crate::coverage::hydrate::RELATIONS.iter().any(|r| relations.contains(*r));
        if !demanded { prop_assert!(got.facts.is_empty()); }
        for fact in got.facts { prop_assert!(relations.contains(&fact.predicate)); }
    }

    // `load_rules_file` skips (never overwrites) an in-memory rule whose id
    // already exists: the disk value is ignored, the field values already
    // in memory survive untouched, and the skip is counted rather than
    // silently dropped. Mutating `hydrate_rules` to overwrite on duplicate
    // ids, or to leave `skipped` at 0, fails this property.
    #[test]
    fn prop_load_skips_existing_duplicate_ids(disk_priority in any::<i16>(), memory_priority in any::<i16>()) {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let (rules, loaded, skipped, phases) = rt.block_on(async {
            let network = ReteNetwork::new();
            let (old, _) = crate::rules_file::rule_from_disk(&disk_rule("shared", memory_priority as i32));
            network.add_rule(old).await.expect("seed rule");
            let existing: HashSet<String> = network
                .get_all_rules()
                .expect("rules")
                .into_iter()
                .map(|r| r.id)
                .collect();
            let mut phases = HashMap::from([("shared".to_string(), "post".to_string())]);
            let rules = vec![disk_rule("shared", disk_priority as i32)];
            let (loaded, skipped) = crate::server_persistence::hydrate_rules(
                &network, &mut phases, &rules, &existing,
            ).await.expect("hydrate");
            Ok::<_, proptest::test_runner::TestCaseError>((
                network.get_all_rules().expect("rules"), loaded, skipped, phases,
            ))
        }).expect("async hydrate");
        prop_assert_eq!(rules.len(), 1);
        prop_assert_eq!(loaded, 0);
        prop_assert_eq!(skipped, 1);
        // The in-memory rule is untouched: it still carries the value it
        // was seeded with, not the disk value.
        prop_assert_eq!(rules[0].priority, memory_priority as i32);
        prop_assert_eq!(phases.get("shared").map(String::as_str), Some("post"));
    }

    // After a load error, `EpistemeMcp::reload_from` treats the repaired
    // file as authoritative: it clears every rule the network held (all of
    // which came from disk, since rule-writing tools were refused while the
    // file did not load) and hydrates fresh from the repaired file, so
    // memory ends up matching disk wholesale rather than merging with the
    // stale in-memory set.
    #[test]
    fn prop_reload_from_repaired_file_replaces_wholesale(
        stale_priority in any::<i16>(),
        repaired_priority in any::<i16>(),
        keep_id in any::<bool>(),
    ) {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let rules = rt.block_on(async {
            let mcp = crate::server::EpistemeMcp::new();
            let root = std::path::Path::new("/nonexistent-root-for-property-test");

            let stale = crate::rule_layers::ResolvedRules {
                rules: vec![disk_rule("stale", stale_priority as i32)],
                origins: HashMap::new(),
                overrides: vec![],
                configured: false,
            };
            mcp.reload_from(root, stale).await.expect("seed stale state");

            let repaired_id = if keep_id { "stale" } else { "repaired" };
            let repaired = crate::rule_layers::ResolvedRules {
                rules: vec![disk_rule(repaired_id, repaired_priority as i32)],
                origins: HashMap::new(),
                overrides: vec![],
                configured: false,
            };
            mcp.reload_from(root, repaired).await.expect("reload repaired");

            let network = mcp.network.lock().await;
            Ok::<_, proptest::test_runner::TestCaseError>(network.get_all_rules().expect("rules"))
        }).expect("async reload");

        // Memory matches the repaired file exactly: one rule, at the
        // repaired id and priority. If a stale id was replaced, it is gone.
        prop_assert_eq!(rules.len(), 1);
        prop_assert_eq!(rules[0].id.as_str(), if keep_id { "stale" } else { "repaired" });
        prop_assert_eq!(rules[0].priority, repaired_priority as i32);
    }

    #[test]
    fn prop_audit_opt_in_survives_write_load_and_merge(priority in any::<i16>()) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rules.json");
        let mut disk = disk_rule("shared", priority as i32);
        disk.audit = Some(true);
        let file = crate::rules_file::RulesFile { rules: vec![disk] };
        crate::rules_file::write_atomic(&path, &file).expect("write");
        let loaded = crate::rules_file::read(&path).expect("read");
        prop_assert_eq!(loaded.rules[0].audit, Some(true));
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let (in_memory, phases) = rt.block_on(async {
            let network = ReteNetwork::new();
            let mut phases = HashMap::new();
            crate::server_persistence::hydrate_rules(&network, &mut phases, &loaded.rules, &HashSet::new())
                .await.expect("load disk rule into network");
            Ok::<_, proptest::test_runner::TestCaseError>((network.get_all_rules().expect("rules"), phases))
        }).expect("async load");
        let merged = crate::rules_file::merge(&loaded, &in_memory, &phases, "pre");
        crate::rules_file::write_atomic(&path, &merged.merged).expect("rewrite");
        prop_assert_eq!(crate::rules_file::read(&path).expect("reread").rules[0].audit, Some(true));
    }
}

fn disk_rule(id: &str, priority: i32) -> crate::rules_file::DiskRule {
    crate::rules_file::DiskRule {
        id: id.into(),
        phase: "pre".into(),
        priority,
        conditions: vec![crate::rules_file::DiskCondition {
            predicate: "p".into(),
            args: vec!["x".into()],
            script: None,
        }],
        actions: vec![crate::rules_file::DiskAction {
            action_type: "log".into(),
            params: vec!["x".into()],
            data: None,
        }],
        silent: None,
        audit: None,
        doc_excepted: None,
    }
}
