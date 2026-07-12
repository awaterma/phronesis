//! Scrub-anonymize captured hook payloads for committing as fixtures.
//!
//! `phr-mcp scrub-payload <file> [--write] [--home DIR] [--project DIR]`
//! reads a captured JSONL file (or a single fixture JSON), rewrites every
//! string value through the [`Scrubber`](crate::payload_scrub::Scrubber),
//! verifies the output for residuals, and prints the result (or rewrites
//! in place with `--write`).
//!
//! Design: `docs/superpowers/specs/2026-07-06-payload-contract-corpus-design.md`.

use std::path::PathBuf;

use serde::Serialize;
use serde_json::Value;

use crate::payload_scrub::Scrubber;

/// A single scrubbed record written back to the output file.
#[derive(Debug, Serialize)]
struct ScrubRecord {
    ts: u64,
    phase: String,
    raw: Value,
}

/// Run the scrub-payload subcommand: anonymize captured payloads for
/// committing as fixtures.
pub fn run(
    file: PathBuf,
    write: bool,
    home: Option<String>,
    project: Option<String>,
) -> std::process::ExitCode {
    let raw = std::fs::read_to_string(&file).map_err(|e| {
        eprintln!("error: cannot read {}: {e}", file.display());
        std::process::exit(1);
    });
    let raw = raw.unwrap_or_default();

    let home =
        home.unwrap_or_else(|| std::env::var("HOME").unwrap_or_else(|_| "/home/dev".to_string()));
    let project = project.unwrap_or_else(|| format!("{}/project", home));

    let mut scrubber = Scrubber::new(&home, &project);

    // Detect JSONL (one object per line) vs single JSON.
    let records: Vec<Value> = raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect();

    let mut output: Vec<ScrubRecord> = Vec::with_capacity(records.len());
    let mut has_errors = false;

    for record in &records {
        // Try to treat it as a capture record; if it lacks "raw", scrub the
        // whole object.
        let raw_val = record.get("raw").unwrap_or(record);
        let mut scrubbed = raw_val.clone();
        scrubber.scrub_value(&mut scrubbed);

        // Verify after scrubbing.
        if let Err(e) = scrubber.verify(&scrubbed) {
            eprintln!("scrub error: {e}");
            has_errors = true;
        }

        // Emit warnings for human review.
        for warn in scrubber.warnings(&scrubbed) {
            eprintln!("warning: {warn}");
        }

        output.push(ScrubRecord {
            ts: record.get("ts").and_then(|v| v.as_u64()).unwrap_or(0),
            phase: record
                .get("phase")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string(),
            raw: scrubbed,
        });
    }

    let serialized = serde_json::to_string_pretty(&output).expect("serialize output");

    if write {
        std::fs::write(&file, format!("{serialized}\n")).map_err(|e| {
            eprintln!("error: cannot write {}: {e}", file.display());
            std::process::exit(1);
        });
        println!(
            "wrote scrubbed {} record(s) to {}",
            output.len(),
            file.display()
        );
    } else {
        println!("{}", serialized);
    }

    if has_errors {
        std::process::exit(1);
    }
    std::process::exit(0)
}
