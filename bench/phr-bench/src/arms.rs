use crate::manifest::TaskSpec;
use crate::record::Arm;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Prepare an arm (clone, checkout, and optionally init treatment).
/// Returns the path to the cloned repository.
/// Layout: workdir/<instance_id>/<arm>/
pub fn prep(task: &TaskSpec, arm: Arm, workdir: &Path) -> Result<PathBuf> {
    let arm_dir = workdir.join(&task.instance_id).join(arm.as_str());
    std::fs::create_dir_all(&arm_dir)?;

    // Clone the repository
    let clone_dir = arm_dir.clone();
    let status = Command::new("git")
        .args(["clone", "--quiet", &task.repo, clone_dir.to_str().unwrap_or("")])
        .status()
        .context("failed to spawn git clone")?;

    if !status.success() {
        anyhow::bail!("git clone failed for {}", task.repo);
    }

    // Checkout the base commit
    let status = Command::new("git")
        .args([
            "-C",
            clone_dir.to_str().unwrap_or(""),
            "checkout",
            "--quiet",
            &task.base_commit,
        ])
        .status()
        .context("failed to spawn git checkout")?;

    if !status.success() {
        anyhow::bail!("git checkout failed for commit {}", task.base_commit);
    }

    // For treatment arm, run phr-mcp init with the appropriate packs
    if arm == Arm::Treatment {
        let packs_arg = task.packs.join(",");
        let status = Command::new("phr-mcp")
            .args(["init", "--packs", &packs_arg])
            .current_dir(&clone_dir)
            .status()
            .context("failed to spawn phr-mcp init")?;

        if !status.success() {
            anyhow::bail!("phr-mcp init failed with exit code: {}", status);
        }

        // Verify init artifacts exist
        if !clone_dir.join(".phronesis/rules.json").exists() {
            anyhow::bail!(".phronesis/rules.json not created by phr-mcp init");
        }

        let settings_local = clone_dir.join(".claude/settings.local.json");
        let settings = clone_dir.join(".claude/settings.json");
        if !settings_local.exists() && !settings.exists() {
            anyhow::bail!(".claude/settings.* not created by phr-mcp init");
        }
    } else {
        // Control arm: verify no governance files exist
        if clone_dir.join(".phronesis").exists() {
            anyhow::bail!("control arm should not have .phronesis directory");
        }
        if clone_dir.join(".claude").exists() {
            anyhow::bail!("control arm should not have .claude directory");
        }
    }

    Ok(clone_dir)
}
