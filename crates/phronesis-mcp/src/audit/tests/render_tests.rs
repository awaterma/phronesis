//! Tests for audit renderers. Split from the original `audit.rs`
//! `mod tests`.

use crate::audit::*;
use std::path::PathBuf;

    fn make_report() -> AuditReport {
        AuditReport {
            generated_at: 1_700_000_000,
            scan_duration_ms: 412,
            files_scanned: 312,
            per_rule: vec![
                RuleAudit {
                    rule_id: "no-unwrap-in-src".into(),
                    level: Level::Block,
                    hits: 10,
                    files: vec![
                        FileAudit {
                            path: PathBuf::from("src/engine.rs"),
                            lines: vec![42, 91, 240],
                            details: vec![],
                        },
                        FileAudit {
                            path: PathBuf::from("src/parser.rs"),
                            lines: vec![18],
                            details: vec![],
                        },
                    ],
                },
                RuleAudit {
                    rule_id: "warn-clone-heavy".into(),
                    level: Level::Warn,
                    hits: 5,
                    files: vec![FileAudit {
                        path: PathBuf::from("src/parser.rs"),
                        lines: vec![3, 4, 5, 6, 7],
                        details: vec![],
                    }],
                },
            ],
        }
    }

    #[test]
    fn render_table_summary_sorts_blocks_above_warns() {
        let out = render_table(&make_report(), false);
        let block_idx = out.find("no-unwrap-in-src").expect("block row present");
        let warn_idx = out.find("warn-clone-heavy").expect("warn row present");
        assert!(
            block_idx < warn_idx,
            "block should sort above warn:\n{}",
            out
        );
        assert!(out.contains("Total: 10 blocked, 5 warned"));
        assert!(out.contains("312 files"));
    }

    #[test]
    fn render_table_expand_shows_files_and_lines() {
        let out = render_table(&make_report(), true);
        assert!(out.contains("src/engine.rs"));
        assert!(out.contains("42"));
        assert!(out.contains("91"));
        assert!(out.contains("240"));
    }

    #[test]
    fn render_table_expand_shows_named_ast_details() {
        // AST hits carry a per-function detail (e.g. "add_rule (12 let
        // bindings)") and a meaningless placeholder line of `1` per hit.
        // The expanded table must surface the names and drop the
        // placeholder "lines: 1, 1" — otherwise the user sees
        // `server.rs — lines: 1, 1, 1` and has to grep for the offenders.
        let report = AuditReport {
            generated_at: 0,
            scan_duration_ms: 0,
            files_scanned: 1,
            per_rule: vec![RuleAudit {
                rule_id: "audit-rust-let-binding-count-high".into(),
                level: Level::Warn,
                hits: 2,
                files: vec![FileAudit {
                    path: PathBuf::from("src/server.rs"),
                    lines: vec![1, 1],
                    details: vec![
                        "add_rule (12 let bindings)".to_string(),
                        "run (9 let bindings)".to_string(),
                    ],
                }],
            }],
        };
        let out = render_table(&report, true);
        assert!(
            out.contains("add_rule (12 let bindings)"),
            "expanded table must name the function, got:\n{out}"
        );
        assert!(
            out.contains("run (9 let bindings)"),
            "expanded table must name the second function, got:\n{out}"
        );
        assert!(
            !out.contains("lines: 1, 1"),
            "expanded table must drop placeholder lines when details are present, got:\n{out}"
        );
    }

    #[test]
    fn render_table_handles_empty_report() {
        let empty = AuditReport {
            generated_at: 1_700_000_000,
            scan_duration_ms: 5,
            files_scanned: 100,
            per_rule: vec![],
        };
        let out = render_table(&empty, false);
        assert!(out.contains("no audit violations found"));
    }

    fn make_trend() -> DebtTrend {
        DebtTrend {
            generated_at: 1_701_000_000,
            snapshots_considered: 3,
            first_snapshot_ts: 1_700_000_000,
            last_snapshot_ts: 1_701_000_000,
            rules: vec![
                RuleTrend {
                    rule_id: "no-unwrap".into(),
                    level: Level::Block,
                    history: vec![
                        TrendPoint {
                            ts: 1_700_000_000,
                            hits: Some(18),
                        },
                        TrendPoint {
                            ts: 1_700_500_000,
                            hits: Some(14),
                        },
                        TrendPoint {
                            ts: 1_701_000_000,
                            hits: Some(10),
                        },
                    ],
                    first_hits: 18,
                    last_hits: 10,
                    net_change: -8,
                },
                RuleTrend {
                    rule_id: "warn-clone-heavy".into(),
                    level: Level::Warn,
                    history: vec![
                        TrendPoint {
                            ts: 1_700_000_000,
                            hits: None,
                        },
                        TrendPoint {
                            ts: 1_700_500_000,
                            hits: Some(5),
                        },
                        TrendPoint {
                            ts: 1_701_000_000,
                            hits: Some(7),
                        },
                    ],
                    first_hits: 5,
                    last_hits: 7,
                    net_change: 2,
                },
            ],
        }
    }

    #[test]
    fn render_trend_table_shows_columns_and_deltas() {
        let out = render_trend_table(&make_trend());
        assert!(out.contains("no-unwrap"));
        assert!(out.contains("warn-clone-heavy"));
        assert!(out.contains("-8"));
        assert!(out.contains("+2"));
    }

    #[test]
    fn render_trend_table_shows_dash_for_missing_snapshot() {
        let out = render_trend_table(&make_trend());
        // warn-clone-heavy has no value in the first snapshot → '–' (em-dash)
        assert!(
            out.contains("–"),
            "missing value should render as em-dash:\n{}",
            out
        );
    }

    #[test]
    fn render_trend_table_handles_no_snapshots() {
        let trend = DebtTrend {
            generated_at: 1_701_000_000,
            snapshots_considered: 0,
            first_snapshot_ts: 0,
            last_snapshot_ts: 0,
            rules: Vec::new(),
        };
        let out = render_trend_table(&trend);
        assert!(out.contains("no audit snapshots recorded yet"));
    }

    #[test]
    fn render_trend_table_handles_single_snapshot() {
        let trend = DebtTrend {
            generated_at: 1_701_000_000,
            snapshots_considered: 1,
            first_snapshot_ts: 1_701_000_000,
            last_snapshot_ts: 1_701_000_000,
            rules: vec![RuleTrend {
                rule_id: "r".into(),
                level: Level::Block,
                history: vec![TrendPoint {
                    ts: 1_701_000_000,
                    hits: Some(5),
                }],
                first_hits: 5,
                last_hits: 5,
                net_change: 0,
            }],
        };
        let out = render_trend_table(&trend);
        assert!(out.contains("need at least 2 snapshots to compute trend"));
        assert!(out.contains("r"));
        assert!(out.contains("5"));
    }

    #[test]
    fn render_trend_json_shape() {
        let out = render_trend_json(&make_trend());
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["snapshots_considered"], 3);
        assert_eq!(v["first_snapshot_ts"], 1_700_000_000);
        assert_eq!(v["last_snapshot_ts"], 1_701_000_000);
        let rules = v["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0]["rule_id"], "no-unwrap");
        assert_eq!(rules[0]["level"], "block");
        assert_eq!(rules[0]["first_hits"], 18);
        assert_eq!(rules[0]["last_hits"], 10);
        assert_eq!(rules[0]["net_change"], -8);
        let history = rules[0]["history"].as_array().unwrap();
        assert_eq!(history.len(), 3);
        // Second rule has a null hits value at index 0.
        assert!(rules[1]["history"][0]["hits"].is_null());
    }

    #[test]
    fn render_json_shape() {
        let out = render_json(&make_report());
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["files_scanned"], 312);
        assert_eq!(v["totals"]["blocked"], 10);
        assert_eq!(v["totals"]["warned"], 5);
        assert_eq!(v["totals"]["rules"], 2);
        let rules = v["rules"].as_array().unwrap();
        assert_eq!(rules[0]["rule_id"], "no-unwrap-in-src");
        assert_eq!(rules[0]["level"], "block");
        assert_eq!(rules[0]["hits"], 10);
        let files = rules[0]["files"].as_array().unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0]["path"], "src/engine.rs");
        let lines = files[0]["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn render_json_includes_per_hit_details() {
        // AST hits must serialize their per-function detail so machine
        // consumers (trend tooling, CI dashboards) can name offenders
        // without re-parsing the tree.
        let report = AuditReport {
            generated_at: 0,
            scan_duration_ms: 0,
            files_scanned: 1,
            per_rule: vec![RuleAudit {
                rule_id: "audit-rust-let-binding-count-high".into(),
                level: Level::Warn,
                hits: 1,
                files: vec![FileAudit {
                    path: PathBuf::from("src/a.rs"),
                    lines: vec![1],
                    details: vec!["ladder (9 let bindings)".to_string()],
                }],
            }],
        };
        let out = render_json(&report);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let files = v["rules"][0]["files"].as_array().unwrap();
        let details = files[0]["details"].as_array().unwrap();
        assert_eq!(details.len(), 1);
        assert_eq!(details[0], "ladder (9 let bindings)");
    }

