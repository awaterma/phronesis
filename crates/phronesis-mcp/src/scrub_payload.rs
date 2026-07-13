//! Scrub-anonymize captured hook payloads for committing as fixtures.
//!
//! `phr-mcp scrub-payload <path> [--write] [--home DIR] [--project-root DIR]`
//! reads a captured JSONL file (or a single-JSON fixture), rewrites every
//! string value through the [`Scrubber`](crate::payload_scrub::Scrubber),
//! verifies the output for residual leaks, runs the residual-risk detectors,
//! and prints scrubbed JSONL to stdout — or rewrites the file in place under
//! `--write`, backing the original up to `<path>.bak` first.
//!
//! # Safety contract
//!
//! scrub-payload performs deterministic anonymization and detects several
//! common leak classes. It is not a proof that arbitrary source or command
//! content contains no secrets. Review scrubbed fixtures before committing
//! them.
//!
//! Two mechanisms (see [`crate::payload_scrub`]):
//!
//! - **Deterministic anonymization** — project-root paths, other
//!   `$HOME`-rooted paths, session ids, transcript paths, and the username
//!   are rewritten to fixed placeholders.
//! - **Residual-risk detection** — after anonymization, conservative
//!   bounded patterns classify findings. Errors (credential-bearing URLs,
//!   private-key headers, token/secret assignments, secret-suggesting
//!   environment keys, absolute paths outside the placeholder roots) abort
//!   with a nonzero exit. Warnings (email addresses, digit-less possible
//!   tokens, the username as a free-text word) go to stderr and the run
//!   exits 0 — warnings alone must not break re-scrub idempotence; a human
//!   reviews them before committing. Diagnostics truncate matched text;
//!   full suspected secrets are never echoed.
//!
//! Contract:
//! - Output is JSONL, one compact record per line — diffable, promotable
//!   per-line, and re-parseable, so a second run is a fixpoint.
//! - `--write` copies the original to `<path>.bak` before touching it.
//! - Any error — invalid roots, a hard residual leak, a residual-risk
//!   detector error, or a corrupt line — aborts with a nonzero exit BEFORE
//!   the backup is made or the output is modified. The first failing
//!   record stops the run.
//! - Shape handling: input that parses as one JSON value is scrubbed as a
//!   single raw record — verbatim, never wrapped in a capture-record
//!   envelope — otherwise each line must parse as JSON on its own. Raw
//!   JSON is accepted by default; shape recognition is a parsing
//!   convenience, not a safety guarantee.
//! - `--home` defaults to `$HOME`; `--project-root` defaults to the current
//!   directory, so in-project paths are preserved when run from the root.
//!
//! Design: `docs/superpowers/specs/2026-07-06-payload-contract-corpus-design.md`
//! and Tasks 1–2 of
//! `docs/superpowers/specs/2026-07-12-evidence-integrity-hardening.md`.

use std::path::Path;

use anyhow::Context;
use serde_json::Value;

use crate::payload_scrub::{Scrubber, Severity, detect_residual_risks};

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

    // Invalid roots fail here — before the input is read and long before
    // any backup or output write.
    let mut scrubber =
        Scrubber::new(&home, &project_root).context("invalid scrub-payload configuration")?;

    let raw = crate::security::read_file_capped(path)
        .with_context(|| format!("cannot read {}", path.display()))?;

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
/// Nothing is emitted (and under `--write`, nothing is written — not even
/// the backup) unless every record scrubs, verifies clean, AND passes the
/// residual-risk detectors without an error-severity finding.
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

/// Scrub a single record in place, then verify and run residual-risk
/// detection. Hard leaks and error-severity findings abort the run; warnings
/// go to stderr for the human reviewer but do not fail (exit stays 0).
fn scrub_one(scrubber: &mut Scrubber, value: &mut Value, line_no: usize) -> anyhow::Result<()> {
    scrubber.scrub_value(value);
    scrubber
        .verify(value)
        .with_context(|| format!("line {line_no}: residual leak after scrubbing"))?;
    for w in scrubber.warnings(value) {
        eprintln!("phronesis: scrub warning (line {line_no}): {w}");
    }
    let findings = detect_residual_risks(value)?;
    let mut error_count = 0usize;
    for f in &findings {
        match f.severity {
            Severity::Error => {
                error_count += 1;
                eprintln!(
                    "phronesis: scrub error (line {line_no}): suspected {}: {}",
                    f.what, f.hint
                );
            }
            Severity::Warning => {
                eprintln!(
                    "phronesis: scrub warning (line {line_no}): possible {}: {}",
                    f.what, f.hint
                );
            }
        }
    }
    if error_count > 0 {
        anyhow::bail!(
            "line {line_no}: {error_count} residual-risk finding(s) classified as errors; \
             nothing was written — redact or drop the flagged content and re-run"
        );
    }
    Ok(())
}
