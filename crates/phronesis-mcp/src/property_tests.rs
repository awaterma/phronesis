use std::collections::{HashMap, HashSet};

use phr::ReteNetwork;
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

    #[test]
    fn prop_load_reconciliation_converges_duplicate_ids(disk_priority in any::<i16>(), memory_priority in any::<i16>()) {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let (loaded, phase) = rt.block_on(async {
            let network = ReteNetwork::new();
            let (old, _) = crate::rules_file::rule_from_disk(&disk_rule(memory_priority as i32));
            network.add_rule(old).await.expect("seed rule");
            let mut phases = HashMap::from([("shared".to_string(), "post".to_string())]);
            let rules = vec![disk_rule(disk_priority as i32)];
            crate::server_persistence::reconcile_rules(&network, &mut phases, &rules).await.expect("reload");
            Ok::<_, proptest::test_runner::TestCaseError>((network.get_all_rules().expect("rules"), phases))
        }).expect("async reload");
        prop_assert_eq!(loaded.len(), 1);
        prop_assert_eq!(loaded[0].priority, disk_priority as i32);
        prop_assert_eq!(phase.get("shared").map(String::as_str), Some("pre"));
    }

    #[test]
    fn prop_audit_opt_in_survives_write_load_and_merge(priority in any::<i16>()) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rules.json");
        let mut disk = disk_rule(priority as i32);
        disk.audit = Some(true);
        let file = crate::rules_file::RulesFile { rules: vec![disk] };
        crate::rules_file::write_atomic(&path, &file).expect("write");
        let loaded = crate::rules_file::read(&path).expect("read");
        prop_assert_eq!(loaded.rules[0].audit, Some(true));
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let (in_memory, phases) = rt.block_on(async {
            let network = ReteNetwork::new();
            let mut phases = HashMap::new();
            crate::server_persistence::reconcile_rules(&network, &mut phases, &loaded.rules)
                .await.expect("load disk rule into network");
            Ok::<_, proptest::test_runner::TestCaseError>((network.get_all_rules().expect("rules"), phases))
        }).expect("async load");
        let merged = crate::rules_file::merge(&loaded, &in_memory, &phases, "pre");
        crate::rules_file::write_atomic(&path, &merged.merged).expect("rewrite");
        prop_assert_eq!(crate::rules_file::read(&path).expect("reread").rules[0].audit, Some(true));
    }
}

fn disk_rule(priority: i32) -> crate::rules_file::DiskRule {
    crate::rules_file::DiskRule {
        id: "shared".into(),
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
