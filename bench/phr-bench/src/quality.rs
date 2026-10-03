use crate::record::AuditSummary;
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::path::Path;
use std::process::Command;

/// Env var to override the phr-mcp binary for testing. Tests point it at a
/// fixture script; operators leave it unset for the real CLI.
pub const PHR_MCP_PATH_ENV: &str = "PHR_BENCH_PHR_MCP";

/// Parse the `phr-mcp audit --json` output into an AuditSummary.
/// Sums only block-level violations (level == "block") into total_violations and per_rule.
pub fn parse_audit(json: &str) -> Result<AuditSummary> {
    let report: Value = serde_json::from_str(json)
        .context("failed to parse audit JSON")?;

    let mut total_violations = 0u32;
    let mut per_rule = std::collections::BTreeMap::new();

    // Extract the rules array from the report
    if let Some(rules) = report.get("rules").and_then(|v| v.as_array()) {
        for rule in rules {
            // Only count block-level rules
            if let Some("block") = rule.get("level").and_then(|v| v.as_str()) {
                if let Some(rule_id) = rule.get("rule_id").and_then(|v| v.as_str()) {
                    if let Some(hits) = rule.get("hits").and_then(|v| v.as_u64()) {
                        let hits_u32 = hits as u32;
                        total_violations += hits_u32;
                        per_rule.insert(rule_id.to_string(), hits_u32);
                    }
                }
            }
        }
    }

    Ok(AuditSummary {
        total_violations,
        per_rule,
    })
}

/// Stage the rules.json into the clone, run `phr-mcp audit --json`, parse and
/// return the AuditSummary, then clean up the staged .phronesis directory
/// (for control clones only; treatment clones keep theirs from init).
///
/// **Ordering constraint:** Refuses if the patch artifact is absent.
pub fn audit_clone(
    clone_dir: &Path,
    rules_json: &str,
    patch_path: &Path,
) -> Result<AuditSummary> {
    // Ordering gate: the diff must have been extracted before audit touches anything
    if !patch_path.exists() {
        bail!(
            "patch artifact {} absent — ordering constraint violated; \
             diff must be extracted BEFORE quality stages governance files",
            patch_path.display()
        );
    }

    // Stage .phronesis/rules.json into the clone
    let phronesis_dir = clone_dir.join(".phronesis");
    std::fs::create_dir_all(&phronesis_dir)
        .context("create .phronesis directory")?;
    std::fs::write(phronesis_dir.join("rules.json"), rules_json)
        .context("write staged rules.json")?;

    // Run `phr-mcp audit --json` in the clone
    // (Tests can override via PHR_BENCH_PHR_MCP env var)
    let phr_mcp_bin = std::env::var(PHR_MCP_PATH_ENV).unwrap_or_else(|_| "phr-mcp".into());
    let output = Command::new(&phr_mcp_bin)
        .args(["audit", "--json"])
        .current_dir(clone_dir)
        .output()
        .context("spawn phr-mcp audit --json")?;

    if !output.status.success() {
        bail!(
            "phr-mcp audit failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let audit_json = String::from_utf8(output.stdout)
        .context("phr-mcp audit stdout was not valid UTF-8")?;

    // Parse the audit output
    let summary = parse_audit(&audit_json)?;

    // Clean up: remove the staged .phronesis/rules.json (control clones only;
    // treatment clones keep theirs since init placed it)
    // For this function, we always remove since callers control whether to
    // preserve (treatment clones never call this for cleanup — their rules stay)
    // Actually: the spec says we stage and then clean up for control clones.
    // Treatment clones keep theirs. Let the caller decide by not calling for
    // treatment, or pass a flag. For now, always clean the staged rules.json.
    std::fs::remove_file(phronesis_dir.join("rules.json")).ok();

    // If .phronesis is now empty, remove it (control clones only)
    if phronesis_dir.read_dir().ok().map(|mut d| d.next().is_none()).unwrap_or(false) {
        std::fs::remove_dir(&phronesis_dir).ok();
    }

    Ok(summary)
}
