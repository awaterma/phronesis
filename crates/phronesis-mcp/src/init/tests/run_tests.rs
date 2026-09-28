use crate::init::*;
use serde_json::Value;

    /// The Rhai pack carries the two formerly-rust-bundled rules with
    /// generalized (non-project-specific) messages, and the rust pack no
    /// longer ships them. This pins the 0.6.1 pack split against regression.
    fn structural_opts(root: &Path, dry_run: bool) -> InitOpts {
        InitOpts {
            project_root: root.to_path_buf(),
            packs: vec![Pack::Structural],
            force: false,
            dry_run,
            rules_only: false,
            hooks_only: false,
        }
    }

    /// A project with one flaggable file, so the graph has something to find.
    fn structural_project() -> tempfile::TempDir {
        let d = tempfile::TempDir::new().expect("tempdir");
        std::fs::create_dir_all(d.path().join("src")).expect("mkdir");
        std::fs::write(
            d.path().join("src/risky.rs"),
            "pub fn danger(v: Vec<u32>) -> u32 { *v.first().expect(\"empty\") }\n",
        )
        .expect("write");
        d
    }

    #[test]
    fn init_builds_the_graph_when_the_structural_pack_is_selected() {
        // Without this the pack installs silent: the rules load, find no
        // edges, and read as broken rather than as "not built yet".
        let d = structural_project();
        run(structural_opts(d.path(), false)).expect("init");
        let graph = d.path().join(".phronesis/graph.jsonl");
        assert!(graph.exists(), "init must leave a usable graph behind");
        let body = std::fs::read_to_string(&graph).expect("read graph");
        assert!(body.contains("defines_fn"), "graph should hold real edges");
        assert!(
            body.contains("no_direct_test"),
            "derived facts must be present"
        );
    }

    #[test]
    fn init_reports_the_graph_build() {
        let d = structural_project();
        let report = run(structural_opts(d.path(), false)).expect("init");
        assert!(
            report.steps.iter().any(|s| s.contains("graph.jsonl")),
            "steps: {:?}",
            report.steps
        );
    }

    #[test]
    fn graph_is_built_even_when_structural_rules_are_not_selected() {
        let d = structural_project();
        let opts = InitOpts {
            project_root: d.path().to_path_buf(),
            packs: vec![Pack::Llm],
            force: false,
            dry_run: false,
            rules_only: false,
            hooks_only: false,
        };
        run(opts).expect("init");
        assert!(d.path().join(".phronesis/graph.jsonl").exists());
    }

    #[test]
    fn dry_run_builds_no_graph() {
        let d = structural_project();
        let report = run(structural_opts(d.path(), true)).expect("init");
        assert!(!d.path().join(".phronesis/graph.jsonl").exists());
        assert!(
            report.steps.iter().any(|s| s.contains("would")),
            "dry run must still say what it would do: {:?}",
            report.steps
        );
    }

    #[test]
    fn a_failed_graph_build_warns_rather_than_failing_init() {
        // Config writing is init's real job. A graph that cannot be built is
        // recoverable with `phr-mcp graph rebuild`; a failed init is not.
        let d = structural_project();
        // Occupy the graph path with a directory so the write cannot succeed.
        std::fs::create_dir_all(d.path().join(".phronesis/graph.jsonl")).expect("mkdir");
        let report = run(structural_opts(d.path(), false)).expect("init must still succeed");
        assert!(
            report.warnings.iter().any(|w| w.contains("graph")),
            "warnings: {:?}",
            report.warnings
        );
    }

    #[test]
    fn run_dry_run_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let opts = InitOpts {
            project_root: dir.path().to_path_buf(),
            packs: vec![Pack::Llm, Pack::Rust],
            force: false,
            dry_run: true,
            rules_only: false,
            hooks_only: false,
        };
        let report = run(opts).unwrap();
        assert!(!dir.path().join(".claude/settings.local.json").exists());
        assert!(!dir.path().join(".mcp.json").exists());
        assert!(!dir.path().join(".phronesis/rules.json").exists());
        assert!(!dir.path().join(".gitignore").exists());
        assert!(!dir.path().join(".gemini/settings.json").exists());
        // But the report should describe planned steps
        assert!(!report.steps.is_empty());
    }

    #[test]
    fn run_hooks_only_skips_rules_and_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        let opts = InitOpts {
            project_root: dir.path().to_path_buf(),
            packs: vec![Pack::Llm],
            force: false,
            dry_run: false,
            rules_only: false,
            hooks_only: true,
        };
        run(opts).unwrap();
        assert!(
            dir.path().join(".claude/settings.local.json").exists(),
            "hooks-only must still write claude settings"
        );
        assert!(
            !dir.path().join(".phronesis/rules.json").exists(),
            "hooks-only must skip rules.json"
        );
        assert!(
            !dir.path().join(".gitignore").exists(),
            "hooks-only must skip .gitignore"
        );
    }

    #[test]
    fn run_dry_run_does_not_write_gemini_settings() {
        let dir = tempfile::tempdir().unwrap();
        let opts = InitOpts {
            project_root: dir.path().to_path_buf(),
            packs: vec![Pack::Llm],
            force: false,
            dry_run: true,
            rules_only: false,
            hooks_only: false,
        };
        let report = run(opts).unwrap();
        assert!(!dir.path().join(".gemini/settings.json").exists());
        // Report should still mention that .gemini/settings.json would be written
        assert!(
            report
                .steps
                .iter()
                .any(|s| s.contains(".gemini/settings.json")),
            "dry-run report should mention .gemini/settings.json"
        );
    }

    #[test]
    fn run_writes_all_four_files_on_fresh_project() {
        let dir = tempfile::tempdir().unwrap();
        let opts = InitOpts {
            project_root: dir.path().to_path_buf(),
            packs: vec![Pack::Llm, Pack::Rust],
            force: false,
            dry_run: false,
            rules_only: false,
            hooks_only: false,
        };
        let report = run(opts).unwrap();
        assert!(dir.path().join(".claude/settings.local.json").exists());
        assert!(dir.path().join(".mcp.json").exists());
        assert!(dir.path().join(".phronesis/rules.json").exists());
        assert!(dir.path().join(".gitignore").exists());
        // No warnings about missing binary in this test (PATH may or may not have it)
        // Just sanity-check that we got progress steps
        assert!(
            report
                .steps
                .iter()
                .any(|s| s.contains("settings.local.json"))
        );
        assert!(report.steps.iter().any(|s| s.contains("rules.json")));
    }

    #[test]
    fn run_preserves_existing_permissions_in_settings() {
        let dir = tempfile::tempdir().unwrap();
        let settings_path = dir.path().join(".claude/settings.local.json");
        std::fs::create_dir_all(settings_path.parent().unwrap()).unwrap();
        // Pre-existing settings with permissions block (no hooks yet)
        std::fs::write(
            &settings_path,
            r#"{"permissions":{"allow":["Bash(ls:*)"]}}"#,
        )
        .unwrap();

        let opts = InitOpts {
            project_root: dir.path().to_path_buf(),
            packs: vec![Pack::Llm],
            force: false,
            dry_run: false,
            rules_only: false,
            hooks_only: false,
        };
        run(opts).unwrap();

        let content: Value =
            serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
        assert_eq!(content["permissions"]["allow"][0], "Bash(ls:*)");
        // Hooks block was added
        assert!(content["hooks"]["PreToolUse"].is_array());
    }

    #[test]
    fn run_does_not_overwrite_rules_without_force() {
        let dir = tempfile::tempdir().unwrap();
        let rules_path = dir.path().join(".phronesis/rules.json");
        std::fs::create_dir_all(rules_path.parent().unwrap()).unwrap();
        std::fs::write(
            &rules_path,
            r#"{"rules":[{"id":"mine","phase":"pre","priority":1,"conditions":[],"actions":[]}]}"#,
        )
        .unwrap();

        let opts = InitOpts {
            project_root: dir.path().to_path_buf(),
            packs: vec![Pack::Llm, Pack::Rust],
            force: false,
            dry_run: false,
            rules_only: false,
            hooks_only: false,
        };
        run(opts).unwrap();

        let content: Value =
            serde_json::from_str(&std::fs::read_to_string(&rules_path).unwrap()).unwrap();
        assert_eq!(content["rules"][0]["id"], "mine");
    }

    #[test]
    fn run_overwrites_rules_with_force() {
        let dir = tempfile::tempdir().unwrap();
        let rules_path = dir.path().join(".phronesis/rules.json");
        std::fs::create_dir_all(rules_path.parent().unwrap()).unwrap();
        std::fs::write(
            &rules_path,
            r#"{"rules":[{"id":"old","phase":"pre","priority":1,"conditions":[],"actions":[]}]}"#,
        )
        .unwrap();

        let opts = InitOpts {
            project_root: dir.path().to_path_buf(),
            packs: vec![Pack::Llm, Pack::Rust],
            force: true,
            dry_run: false,
            rules_only: false,
            hooks_only: false,
        };
        run(opts).unwrap();

        let content: Value =
            serde_json::from_str(&std::fs::read_to_string(&rules_path).unwrap()).unwrap();
        let ids: Vec<&str> = content["rules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap())
            .collect();
        // Old rule gone; new rust pack present
        assert!(!ids.contains(&"old"));
        assert!(ids.contains(&"enforce-no-unwrap-in-src"));
        // And a .bak of the old rules was created
        let bak_path = dir.path().join(".phronesis/rules.json.bak");
        assert!(
            bak_path.exists(),
            "force should back up the prior rules file"
        );
    }

    #[test]
    fn init_with_base_scaffolds_every_subsystem() {
        let d = tempfile::TempDir::new().expect("tempdir");
        run(InitOpts {
            project_root: d.path().to_path_buf(),
            packs: parse_packs("base").expect("parse"),
            force: false,
            dry_run: false,
            rules_only: false,
            hooks_only: false,
        })
        .expect("init");
        let ep = d.path().join(".phronesis");
        for file in [
            "rules.json",
            "durable.md",
            "confidence.json",
            "bugs.json",
            "toolchains.json",
            "journey.json",
            "context.json",
            "nudges/README.md",
        ] {
            assert!(ep.join(file).exists(), "base must scaffold {file}");
        }
    }

    #[test]
    fn base_ships_the_compact_kernel_within_its_own_ceiling() {
        // `base` includes the context pack, so a fresh project gets a
        // `kernel.md` alongside its `durable.md` — and the kernel has to fit
        // the ceiling the same pack configures.
        let d = tempfile::TempDir::new().expect("tempdir");
        run(InitOpts {
            project_root: d.path().to_path_buf(),
            packs: parse_packs("base").expect("parse"),
            force: false,
            dry_run: false,
            rules_only: false,
            hooks_only: false,
        })
        .expect("init");
        let kernel =
            std::fs::read_to_string(d.path().join(".phronesis/kernel.md")).expect("read kernel.md");
        let config = crate::context::config::ContextConfig::default();
        assert!(
            kernel.len() <= config.interaction.kernel_max_bytes,
            "kernel is {} bytes, over the {} byte ceiling",
            kernel.len(),
            config.interaction.kernel_max_bytes
        );
    }

    fn context_opts(root: &Path, dry_run: bool) -> InitOpts {
        InitOpts {
            project_root: root.to_path_buf(),
            packs: vec![Pack::Context],
            force: false,
            dry_run,
            rules_only: false,
            hooks_only: false,
        }
    }

    #[test]
    fn context_pack_scaffolds_config_kernel_and_readme() {
        let d = tempfile::TempDir::new().expect("tempdir");
        run(context_opts(d.path(), false)).expect("init");

        let config = d.path().join(".phronesis/context.json");
        let parsed: crate::context::config::ContextConfig =
            serde_json::from_str(&std::fs::read_to_string(&config).expect("read context.json"))
                .expect("scaffolded config must be valid");
        assert_eq!(parsed, crate::context::config::ContextConfig::default());

        let kernel =
            std::fs::read_to_string(d.path().join(".phronesis/kernel.md")).expect("read kernel.md");
        assert!(
            kernel.len() <= parsed.session.kernel_max_bytes,
            "the scaffolded kernel must fit its own ceiling: {} bytes",
            kernel.len()
        );

        let readme = std::fs::read_to_string(d.path().join(".phronesis/nudges/README.md"))
            .expect("read nudges README");
        assert!(readme.contains("---json"));
        assert!(readme.contains("context predicates"));
        assert!(readme.contains("max_bytes"));
    }

    #[test]
    fn context_pack_leaves_an_existing_durable_file_as_the_session_document() {
        // Opting in must not repurpose, rewrite, or shrink a durable file the
        // project already wrote. It stays the session-level document; the
        // always-on core arrives as a new, separate file.
        let d = tempfile::TempDir::new().expect("tempdir");
        let ep = d.path().join(".phronesis");
        std::fs::create_dir_all(&ep).expect("mkdir");
        let existing = "# House rules\n\n## Review\n\nAlways review before merging.\n";
        std::fs::write(ep.join("durable.md"), existing).expect("write durable");

        run(context_opts(d.path(), false)).expect("init");

        assert_eq!(
            std::fs::read_to_string(ep.join("durable.md")).expect("read"),
            existing,
            "the project's own document must survive byte-for-byte"
        );
        assert!(
            ep.join("kernel.md").exists(),
            "and the always-on core arrives separately"
        );
    }

    #[test]
    fn context_pack_dry_run_writes_nothing() {
        let d = tempfile::TempDir::new().expect("tempdir");
        let report = run(context_opts(d.path(), true)).expect("init");
        assert!(!d.path().join(".phronesis/context.json").exists());
        assert!(!d.path().join(".phronesis/nudges").exists());
        assert!(
            report
                .steps
                .iter()
                .any(|s| s.contains("would write .phronesis/context.json")),
            "the dry run must still say what it would do: {:?}",
            report.steps
        );
    }

    #[test]
    fn context_pack_preserves_existing_files() {
        let d = tempfile::TempDir::new().expect("tempdir");
        let ep = d.path().join(".phronesis");
        std::fs::create_dir_all(ep.join("nudges")).expect("mkdir");
        std::fs::write(ep.join("context.json"), "{\"mine\":true}").expect("write config");
        std::fs::write(ep.join("durable.md"), "My own kernel.").expect("write durable");
        std::fs::write(ep.join("nudges/README.md"), "my notes").expect("write readme");

        run(context_opts(d.path(), false)).expect("init");

        assert_eq!(
            std::fs::read_to_string(ep.join("context.json")).expect("read"),
            "{\"mine\":true}"
        );
        assert_eq!(
            std::fs::read_to_string(ep.join("durable.md")).expect("read"),
            "My own kernel."
        );
        assert_eq!(
            std::fs::read_to_string(ep.join("nudges/README.md")).expect("read"),
            "my notes"
        );
    }

    #[test]
    fn context_pack_is_idempotent() {
        let d = tempfile::TempDir::new().expect("tempdir");
        run(context_opts(d.path(), false)).expect("first init");
        let config =
            std::fs::read_to_string(d.path().join(".phronesis/context.json")).expect("read config");
        let gitignore =
            std::fs::read_to_string(d.path().join(".gitignore")).expect("read gitignore");

        let report = run(context_opts(d.path(), false)).expect("second init");
        assert_eq!(
            std::fs::read_to_string(d.path().join(".phronesis/context.json")).expect("read"),
            config
        );
        assert_eq!(
            std::fs::read_to_string(d.path().join(".gitignore")).expect("read"),
            gitignore,
            "a second run must not append duplicate ignore lines"
        );
        assert!(
            report
                .steps
                .iter()
                .any(|s| s.contains("context.json already exists")),
            "and must say it left the file alone: {:?}",
            report.steps
        );
    }

    #[test]
    fn context_pack_carves_its_files_out_of_the_broad_gitignore() {
        // `.phronesis/*` would otherwise hide the config and capsules that
        // the pack just wrote and that are meant to be versioned.
        let d = tempfile::TempDir::new().expect("tempdir");
        run(context_opts(d.path(), false)).expect("init");
        let gitignore =
            std::fs::read_to_string(d.path().join(".gitignore")).expect("read gitignore");
        let lines: Vec<&str> = gitignore.lines().collect();
        for carveout in [
            "!.phronesis/context.json",
            "!.phronesis/nudges/",
            "!.phronesis/nudges/**",
        ] {
            assert!(lines.contains(&carveout), "missing carveout {carveout}");
        }
        let broad = lines
            .iter()
            .position(|l| *l == ".phronesis/*")
            .expect("broad ignore present");
        let carved = lines
            .iter()
            .position(|l| *l == "!.phronesis/context.json")
            .expect("carveout present");
        assert!(
            broad < carved,
            "an un-ignore before the broad ignore is inert"
        );
    }

    #[test]
    fn without_the_context_pack_no_context_files_or_carveouts_appear() {
        let d = tempfile::TempDir::new().expect("tempdir");
        run(InitOpts {
            project_root: d.path().to_path_buf(),
            packs: vec![Pack::Llm],
            force: false,
            dry_run: false,
            rules_only: false,
            hooks_only: false,
        })
        .expect("init");
        assert!(!d.path().join(".phronesis/context.json").exists());
        assert!(!d.path().join(".phronesis/nudges").exists());
        let gitignore =
            std::fs::read_to_string(d.path().join(".gitignore")).expect("read gitignore");
        assert!(!gitignore.contains("!.phronesis/context.json"));
    }

    #[test]
    fn gitignore_appends_only_missing_entries() {
        let dir = tempfile::tempdir().unwrap();
        let gi_path = dir.path().join(".gitignore");
        std::fs::write(
            &gi_path,
            "/target\n.phronesis/log.jsonl\n", // one of our entries already there
        )
        .unwrap();

        let opts = InitOpts {
            project_root: dir.path().to_path_buf(),
            packs: vec![Pack::None],
            force: false,
            dry_run: false,
            rules_only: false,
            hooks_only: false,
        };
        run(opts).unwrap();

        let content = std::fs::read_to_string(&gi_path).unwrap();
        // Original target preserved
        assert!(content.contains("/target"));
        // Existing entry not duplicated
        assert_eq!(content.matches(".phronesis/log.jsonl\n").count(), 1);
        // New entries appended
        assert!(content.contains(".phronesis/log.jsonl.1"));
        assert!(content.contains(".phronesis/rules.json.bak"));
    }

    #[test]
    fn second_run_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let opts1 = InitOpts {
            project_root: dir.path().to_path_buf(),
            packs: vec![Pack::Llm, Pack::Rust],
            force: false,
            dry_run: false,
            rules_only: false,
            hooks_only: false,
        };
        run(opts1).unwrap();
        let first =
            std::fs::read_to_string(dir.path().join(".claude/settings.local.json")).unwrap();
        let rules_first =
            std::fs::read_to_string(dir.path().join(".phronesis/rules.json")).unwrap();

        let opts2 = InitOpts {
            project_root: dir.path().to_path_buf(),
            packs: vec![Pack::Llm, Pack::Rust],
            force: false,
            dry_run: false,
            rules_only: false,
            hooks_only: false,
        };
        run(opts2).unwrap();
        let second =
            std::fs::read_to_string(dir.path().join(".claude/settings.local.json")).unwrap();
        let rules_second =
            std::fs::read_to_string(dir.path().join(".phronesis/rules.json")).unwrap();

        assert_eq!(first, second, "settings should be unchanged on rerun");
        assert_eq!(
            rules_first, rules_second,
            "rules should not be touched on rerun"
        );
    }

    #[test]
    fn errors_when_path_missing() {
        let opts = InitOpts {
            project_root: PathBuf::from("/totally/nonexistent/path/xyz12345"),
            packs: vec![Pack::Llm],
            force: false,
            dry_run: false,
            rules_only: false,
            hooks_only: false,
        };
        assert!(matches!(run(opts), Err(InitError::NoSuchPath(_))));
    }

    #[test]
    fn run_writes_gemini_settings_with_mcp_and_hooks() {
        let dir = tempfile::tempdir().unwrap();
        let opts = InitOpts {
            project_root: dir.path().to_path_buf(),
            packs: vec![Pack::Llm],
            force: false,
            dry_run: false,
            rules_only: false,
            hooks_only: false,
        };
        let report = run(opts).unwrap();
        let gemini_path = dir.path().join(".gemini/settings.json");
        assert!(gemini_path.exists(), "should create .gemini/settings.json");
        let content: Value =
            serde_json::from_str(&std::fs::read_to_string(&gemini_path).unwrap()).unwrap();
        // MCP server registered
        assert_eq!(content["mcpServers"]["phronesis"]["command"], "phr-mcp");
        // Hooks wired
        let before = content["hooks"]["BeforeTool"].as_array().unwrap();
        assert!(!before.is_empty());
        assert!(before[0]["matcher"].as_str().unwrap().contains("replace"));
        let after = content["hooks"]["AfterTool"].as_array().unwrap();
        assert!(!after.is_empty());
        assert!(after[0]["matcher"].as_str().unwrap().contains("write_file"));
        // Report mentions gemini
        assert!(
            report
                .steps
                .iter()
                .any(|s| s.to_lowercase().contains("gemini"))
        );
    }

    #[test]
    fn write_settings_includes_session_start_and_user_prompt_submit_hooks() {
        let dir = tempfile::tempdir().unwrap();
        let opts = InitOpts {
            project_root: dir.path().to_path_buf(),
            packs: vec![Pack::Llm],
            force: false,
            dry_run: false,
            rules_only: false,
            hooks_only: false,
        };
        run(opts).unwrap();

        let path = dir.path().join(".claude/settings.local.json");
        let content: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();

        let session = content["hooks"]["SessionStart"].as_array().unwrap();
        assert!(!session.is_empty(), "SessionStart must be wired");
        let session_cmd = session[0]["hooks"][0]["command"].as_str().unwrap();
        assert_eq!(session_cmd, "phr-mcp claude-hook SessionStart");

        let prompt = content["hooks"]["UserPromptSubmit"].as_array().unwrap();
        assert!(!prompt.is_empty(), "UserPromptSubmit must be wired");
        let prompt_cmd = prompt[0]["hooks"][0]["command"].as_str().unwrap();
        assert_eq!(prompt_cmd, "phr-mcp claude-hook UserPromptSubmit");
    }

    #[test]
    fn write_gemini_settings_includes_session_and_before_agent_hooks() {
        let dir = tempfile::tempdir().expect("tempdir failed");
        let opts = InitOpts {
            project_root: dir.path().to_path_buf(),
            packs: vec![Pack::Llm],
            force: false,
            dry_run: false,
            rules_only: false,
            hooks_only: false,
        };
        run(opts).expect("run failed");

        let path = dir.path().join(".gemini/settings.json");
        let content: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read settings failed"))
                .expect("parse json failed");

        let session = content["hooks"]["SessionStart"]
            .as_array()
            .expect("SessionStart not array");
        let cmd = session[0]["hooks"][0]["command"]
            .as_str()
            .expect("command not string");
        assert_eq!(cmd, "phr-mcp claude-hook SessionStart");

        let before = content["hooks"]["BeforeAgent"]
            .as_array()
            .expect("BeforeAgent not array");
        let cmd = before[0]["hooks"][0]["command"]
            .as_str()
            .expect("command not string");
        assert_eq!(cmd, "phr-mcp claude-hook BeforeAgent");
    }

