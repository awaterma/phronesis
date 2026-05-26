//! Per-section profiler for `audit::run`.
//!
//! Loads the project's `.phronesis/rules.json` (or a path passed as argv[1])
//! and runs the audit against the current repo with per-section `Instant`
//! timers. Mirrors the methodology used in
//! `crates/phronesis/examples/profile_assert_fact.rs`.
//!
//! Run: `cargo run --example profile_audit --release`

use std::path::PathBuf;

use phronesis_mcp::audit::{AuditOpts, run_profiled};
use phronesis_mcp::rules_file;

fn pp(ns: std::time::Duration) -> String {
    let n = ns.as_nanos();
    if n < 1_000 {
        format!("{n}ns")
    } else if n < 1_000_000 {
        format!("{:.2}µs", n as f64 / 1_000.0)
    } else if n < 1_000_000_000 {
        format!("{:.2}ms", n as f64 / 1_000_000.0)
    } else {
        format!("{:.2}s", n as f64 / 1_000_000_000.0)
    }
}

fn pct(part: std::time::Duration, whole: std::time::Duration) -> f64 {
    if whole.as_nanos() == 0 {
        0.0
    } else {
        100.0 * (part.as_nanos() as f64) / (whole.as_nanos() as f64)
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let project_root: PathBuf = args
        .get(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let rules_path = project_root.join(".phronesis/rules.json");

    eprintln!("project: {}", project_root.display());
    eprintln!("rules:   {}", rules_path.display());

    let rules = match rules_file::read(&rules_path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("cannot read rules: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("total rules in file: {}", rules.rules.len());

    let opts = AuditOpts {
        project_root: project_root.clone(),
        scan_root: project_root,
        rule_filter: None,
    };

    // Warm-up run (priming filesystem cache + JIT-style branch warmup).
    let _ = run_profiled(&rules, &opts);

    // Measurement run.
    let (report, times) = run_profiled(&rules, &opts);

    let total = times.total;
    let inner_sum =
        times.discover + times.read_files + times.keep_mask + times.match_loop + times.report_build;
    let unaccounted = total.saturating_sub(inner_sum);

    println!();
    println!("=== audit profile ===");
    println!("  files_scanned       {}", times.files_scanned);
    println!("  audit_rules         {}", times.audit_rules);
    println!(
        "  line_matches_evaluated  {} ({:.2}M)",
        times.line_matches_evaluated,
        times.line_matches_evaluated as f64 / 1_000_000.0
    );
    println!();
    println!(
        "  discover            {:>9}  ({:>5.1}%)",
        pp(times.discover),
        pct(times.discover, total)
    );
    println!(
        "  read_files          {:>9}  ({:>5.1}%)",
        pp(times.read_files),
        pct(times.read_files, total)
    );
    println!(
        "  keep_mask           {:>9}  ({:>5.1}%)",
        pp(times.keep_mask),
        pct(times.keep_mask, total)
    );
    println!(
        "  match_loop          {:>9}  ({:>5.1}%)",
        pp(times.match_loop),
        pct(times.match_loop, total)
    );
    println!(
        "  report_build        {:>9}  ({:>5.1}%)",
        pp(times.report_build),
        pct(times.report_build, total)
    );
    println!(
        "  unaccounted         {:>9}  ({:>5.1}%)",
        pp(unaccounted),
        pct(unaccounted, total)
    );
    println!("  TOTAL               {:>9}", pp(total));
    println!();
    println!(
        "  hits across all rules: {}",
        report.per_rule.iter().map(|r| r.hits).sum::<u32>()
    );
}
