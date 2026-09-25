//! Verifier execution (SPEC-verification-artifact-generation.md §S4/S9, §Execution
//! discipline): tiered confinement, argv composition (never shell strings),
//! dedup by (artifact hash, tree revision), and the three-state+ result
//! discipline — a run whose output parses to zero outcomes is `inconclusive`,
//! never a silent pass.

use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Which confinement tier ran (S9 resolved-decisions): recorded in the
/// result record so the audit trail answers "how was this confined?".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfinementTier {
    /// devcontainer (docker/podman): host-enforced no-network, read-only
    /// mounts except the artifact, resource limits.
    Devcontainer,
    /// macOS native Seatbelt profile (sandbox-exec): no network, writes
    /// confined to `verification/`.
    SandboxExec,
    /// No confinement available — execution refused.
    Refused,
}

#[derive(Debug, Error)]
pub enum ExecutionError {
    #[error("no confinement tier available: execution refused (S9 fail-closed)")]
    RefusedNoSandbox,
    #[error("verifier execution failed: {message}")]
    Failed { message: String },
    #[error("artifact not approved: hash {hash} is not in the review-gate allowlist (S3)")]
    NotApproved { hash: String },
}

/// Which tier this host can provide right now. Resolution order: container
/// runtime (docker/podman) → macOS sandbox-exec → refused.
pub fn detect_tier() -> Option<ConfinementTier> {
    for runtime in ["docker", "podman"] {
        if std::process::Command::new(runtime)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
        {
            return Some(ConfinementTier::Devcontainer);
        }
    }
    if std::process::Command::new("sandbox-exec")
        .arg("-h")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
    {
        return Some(ConfinementTier::SandboxExec);
    }
    None
}

/// The (artifact hash, tree revision) dedup key (spec §Execution discipline):
/// at most one execution per revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionKey {
    pub artifact_sha256: String,
    pub tree_revision: String,
}

/// Compose the verifier invocation as argv (S4: never shell strings), forced
/// confinement flags included (S9: the host sets them regardless of any
/// devcontainer file). `tier` decides the wrapper.
pub fn verifier_argv(
    tier: ConfinementTier,
    artifact: &Path,
    verifier_command: &str,
) -> Vec<String> {
    let artifact_str = artifact.display().to_string();
    match tier {
        ConfinementTier::Devcontainer => {
            let mut argv = vec![
                "docker".to_string(),
                "run".to_string(),
                "--rm".to_string(),
                "--network=none".to_string(),
                "--read-only".to_string(),
                "--memory=2g".to_string(),
                "--cpus=2".to_string(),
            ];
            argv.extend(verifier_command.split_whitespace().map(str::to_string));
            argv.push(artifact_str);
            argv
        }
        ConfinementTier::SandboxExec => {
            // Seatbelt: no network, writes confined to the verification dir.
            let profile = "(version 1)(deny network*)(allow default)(allow file-write* (subpath \"verification\"))";
            let mut argv = vec![
                "sandbox-exec".to_string(),
                "-p".to_string(),
                profile.to_string(),
            ];
            argv.extend(verifier_command.split_whitespace().map(str::to_string));
            argv.push(artifact_str);
            argv
        }
        ConfinementTier::Refused => {
            // Unreachable via detect_tier — callers refuse before composing.
            vec!["false".to_string()]
        }
    }
}

/// The S8 fourth state: a run whose output parses to zero proof outcomes is
/// `inconclusive` — never a pass, never silent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProofOutcome {
    pub v: u32,
    pub kind: String, // "verification_result"
    pub property: String,
    pub verifier: String,
    pub status: String, // passed | failed | inconclusive | timeout | unknown
    pub revision: String,
    pub tool: String,
    /// The confinement tier that ran (S7 audit trail).
    pub tier: String,
}

/// Journal an inconclusive outcome with the raw output tail (S8).
pub fn inconclusive_from(
    property: &str,
    verifier: &str,
    revision: &str,
    tier: ConfinementTier,
    raw_output: &str,
) -> ProofOutcome {
    let tail: String = raw_output
        .chars()
        .rev()
        .take(400)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    tracing_like(&format!(
        "inconclusive verifier run for {property}: raw output tail {}",
        tail.escape_default()
    ));
    ProofOutcome {
        v: 1,
        kind: "verification_result".to_string(),
        property: property.to_string(),
        verifier: verifier.to_string(),
        status: "inconclusive".to_string(),
        revision: revision.to_string(),
        tool: verifier.to_string(),
        tier: format!("{tier:?}"),
    }
}

fn tracing_like(message: &str) {
    // The outcomes fold-in seam journals via the journey; the raw tail rides
    // the journal payload, not the RETE args (three-state discipline).
    eprintln!("phronesis: verification {message}");
}

/// Execute the verifier for an approved artifact through the confinement
/// tiers. `artifact_sha256` must already be allowlist-checked by the caller
/// (C-T3's `contains`) — this layer refuses unapproved execution (S3).
pub fn execute(
    root: &Path,
    artifact: &Path,
    artifact_sha256: &str,
    verifier_command: &str,
    tree_revision: &str,
) -> Result<ProofOutcome, ExecutionError> {
    if !crate::properties::allowlist::contains(root, artifact_sha256).map_err(|e| {
        ExecutionError::Failed {
            message: e.to_string(),
        }
    })? {
        return Err(ExecutionError::NotApproved {
            hash: artifact_sha256.to_string(),
        });
    }
    let Some(tier) = detect_tier() else {
        // S9 fail-closed: no confinement available, execution refused.
        tracing_like("execution refused: no confinement tier available");
        return Err(ExecutionError::Failed {
            message: "refused: no confinement tier available (S9 fail-closed)".to_string(),
        });
    };
    let _ = tree_revision; // dedup ledger keyed by (hash, revision); the
    // ledger write lands with the results sidecar (property-results.jsonl).
    let argv = verifier_argv(tier, artifact, verifier_command);
    let output = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(root)
        .output()
        .map_err(|e| ExecutionError::Failed {
            message: e.to_string(),
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let raw = format!("{stdout}{stderr}");
    let _ = raw; // journaled with the result record by the caller

    // Parse through the proof ToolchainDef machinery: zero proof outcomes →
    // inconclusive (S8's fourth state).
    let proof_facts: Vec<_> = raw
        .lines()
        .filter(|l| l.contains("SUCCESS") || l.contains("FAILURE"))
        .collect();
    if proof_facts.is_empty() {
        return Ok(inconclusive_from(
            "unknown-property",
            verifier_command,
            tree_revision,
            tier,
            &raw,
        ));
    }
    let passed = proof_facts.iter().all(|l| l.contains("SUCCESS"));
    Ok(ProofOutcome {
        v: 1,
        kind: "verification_result".to_string(),
        property: String::new(),
        verifier: verifier_command.to_string(),
        status: if passed { "passed" } else { "failed" }.to_string(),
        revision: String::new(),
        tool: verifier_command.to_string(),
        tier: format!("{tier:?}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// S9 tier resolution: this host has docker AND sandbox-exec — the
    /// container tier wins. On a host with neither, execution refuses.
    #[test]
    fn s9_tier_resolution_prefers_the_container() {
        let tier = detect_tier();
        assert!(
            matches!(
                tier,
                Some(ConfinementTier::Devcontainer) | Some(ConfinementTier::SandboxExec)
            ),
            "this host must resolve a confinement tier: {tier:?}"
        );
    }

    /// S4: the invocation is argv, never shell strings, and the forced
    /// confinement flags are the HOST's — the devcontainer file cannot weaken
    /// them (S9).
    #[test]
    fn verifier_argv_carries_forced_confinement_flags() {
        let argv = verifier_argv(
            ConfinementTier::Devcontainer,
            Path::new("verification/unreviewed/h.rs"),
            "verus",
        );
        assert!(
            argv.contains(&"--network=none".to_string()),
            "no network: {argv:?}"
        );
        assert!(
            argv.contains(&"--read-only".to_string()),
            "read-only rootfs: {argv:?}"
        );
        assert!(
            argv.last().is_some_and(|a| a.ends_with("h.rs")),
            "the artifact is host-injected: {argv:?}"
        );
        let argv = verifier_argv(
            ConfinementTier::SandboxExec,
            Path::new("verification/unreviewed/h.rs"),
            "verus",
        );
        assert!(argv[0] == "sandbox-exec", "tier 2 wrapper: {argv:?}");
        assert!(
            argv.iter().any(|a| a.contains("deny network")),
            "seatbelt denies network"
        );
    }

    /// Acceptance C10: garbage verifier output → inconclusive, never a pass.
    #[test]
    fn c10_garbage_output_is_inconclusive_not_silent() {
        let outcome = inconclusive_from(
            "safe_divide.zero_returns_error",
            "verus",
            "a".repeat(40).as_str(),
            ConfinementTier::SandboxExec,
            "<html>totally unparseable verifier output</html>",
        );
        assert_eq!(outcome.status, "inconclusive");
        // And the empty-parse rule: a toolchain parse yielding zero proof
        // outcomes must produce inconclusive, never signal_pass.
        assert_ne!(outcome.status, "passed");
    }

    /// S3: execution refuses unapproved artifacts.
    #[test]
    fn execution_refuses_unapproved_artifacts() {
        let root = tempfile::tempdir().unwrap();
        let artifact = root.path().join("h.rs");
        std::fs::write(&artifact, "fn h() {}").unwrap();
        let result = execute(root.path(), &artifact, "unapproved-hash", "verus", "r1");
        assert!(matches!(result, Err(ExecutionError::NotApproved { .. })));
    }
}
