//! Scrub-anonymize captured hook payloads for committing as fixtures.
//!
//! `phr-mcp scrub-payload <path> [--write] [--home DIR] [--project-root DIR]`
//! reads a captured JSONL file (or a single-JSON fixture), rewrites every
//! string value through the [`Scrubber`](crate::payload_scrub::Scrubber),
//! verifies the output for residual leaks, and prints scrubbed JSONL to
//! stdout — or rewrites the file in place under `--write`, backing the
//! original up to `<path>.bak` first.
//!
//! Contract (plan Task 3):
//! - Output is JSONL, one compact record per line — diffable, promotable
//!   per-line, and re-parseable, so a second run is a fixpoint.
//! - `--write` copies the original to `<path>.bak` before touching it.
//! - A hard residual leak ($HOME path, username as a path component) aborts
//!   the run with a nonzero exit BEFORE anything is written.
//! - A non-JSON line aborts with `line {n}: not JSON: {e}` and nonzero exit.
//! - Soft residuals (username as a free-text token) are warnings on stderr;
//!   the run still succeeds, keeping scrubbing idempotent.
//! - `--home` defaults to `$HOME`; `--project-root` defaults to the current
//!   directory, so in-project paths are preserved when run from the root.
//!
//! Design: `docs/superpowers/specs/2026-07-06-payload-contract-corpus-design.md`.

use std::path::Path;

use anyhow::Context;
use serde_json::Value;

use crate::payload_scrub::Scrubber;

/// Run the scrub-payload subcommand. Errors bubble to `main.rs`'s
/// `anyhow::Result` handler; this module never exits the process.
pub fn run(
    path: &Path,
    write: bool,
    home: Option<String>,
    project_root: Option<String>,
) -> anyhow::Result<()> {
    let home = match home {
        Some(h) => h,
        None => std::env::var("HOME").context("--home not given and $HOME is unset")?,
    };
    let project_root = match project_root {
        Some(p) => p,
        None => std::env::current_dir()
            .context("--project-root not given and the current directory is unreadable")?
            .display()
            .to_string(),
    };

    let raw = crate::security::read_file_capped(path)
        .with_context(|| format!("cannot read {}", path.display()))?;

    let mut scrubber = Scrubber::new(&home, &project_root);
    let out_lines = scrub_lines(&mut scrubber, &raw)?;

    let rendered = out_lines.join("\n") + "\n";
    if write {
        let backup = format!("{}.bak", path.display());
        std::fs::copy(path, &backup).with_context(|| format!("cannot back up to {backup}"))?;
        std::fs::write(path, &rendered)
            .with_context(|| format!("cannot write {}", path.display()))?;
        eprintln!(
            "scrubbed {} line(s) in place; original at {backup}",
            out_lines.len()
        );
    } else {
        print!("{rendered}");
    }
    Ok(())
}

/// Scrub every record in `raw` to compact JSON lines. `raw` is either JSONL
/// (one object per line) or a single-JSON fixture, possibly pretty-printed:
/// if the whole input parses as one JSON value it is scrubbed as a single
/// record — verbatim, never wrapped in a capture-record envelope — otherwise
/// each line must parse on its own and a bad line aborts with its number.
/// Nothing is emitted (and under `--write`, nothing is written) unless every
/// record scrubs AND verifies clean.
fn scrub_lines(scrubber: &mut Scrubber, raw: &str) -> anyhow::Result<Vec<String>> {
    if let Ok(mut value) = serde_json::from_str::<Value>(raw) {
        scrub_one(scrubber, &mut value, 1)?;
        return Ok(vec![value.to_string()]);
    }
    let mut out_lines = Vec::new();
    for (idx, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let mut value: Value = serde_json::from_str(line)
            .map_err(|e| anyhow::anyhow!("line {}: not JSON: {}", idx + 1, e))?;
        scrub_one(scrubber, &mut value, idx + 1)?;
        out_lines.push(value.to_string());
    }
    Ok(out_lines)
}

/// Scrub a single record in place, then verify. Hard leaks abort the run;
/// soft residuals go to stderr for the human reviewer but do not fail.
fn scrub_one(scrubber: &mut Scrubber, value: &mut Value, line_no: usize) -> anyhow::Result<()> {
    scrubber.scrub_value(value);
    scrubber
        .verify(value)
        .with_context(|| format!("line {line_no}: residual leak after scrubbing"))?;
    for w in scrubber.warnings(value) {
        eprintln!("phronesis: scrub warning (line {line_no}): {w}");
    }
    Ok(())
}
