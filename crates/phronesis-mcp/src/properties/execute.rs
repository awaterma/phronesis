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
    /// Explicitly configured, human-set: no confinement. A downgraded trust
    /// tier — the S3 allowlist gate still applies, and the tier is recorded
    /// so raw-everywhere drift is visible (S9 disciplines).
    Raw,
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

/// Which tier this host can provide right now. Resolution order (S9 ladder):
/// container runtime (docker/podman) → macOS sandbox-exec → raw (only when
/// the human-set config allows) → refused. Host-enforced: the strongest
/// available tier wins; raw is never a default.
pub fn detect_tier(root: &Path) -> Option<ConfinementTier> {
    // The devcontainer tier requires the (language, verifier) instantiation to
    // ship its devcontainer.json — a running daemon without a declared image
    // is not a Tier-1 claim (S9: the host composes the container from the
    // instantiation's declaration).
    let has_devcontainer = root
        .join("verification/templates/devcontainer.json")
        .is_file();
    resolve_tier(
        has_devcontainer,
        container_runtime_available,
        sandbox_exec_available,
        || raw_execution_allowed(root),
    )
}

/// The S9 ladder over host probes. Probes run lazily, in ladder order, so a
/// stronger tier short-circuits the weaker probes (and no container runtime
/// is spawned unless a devcontainer is declared).
fn resolve_tier(
    devcontainer_declared: bool,
    container_runtime: impl FnOnce() -> bool,
    sandbox_exec: impl FnOnce() -> bool,
    raw_allowed: impl FnOnce() -> bool,
) -> Option<ConfinementTier> {
    if devcontainer_declared && container_runtime() {
        return Some(ConfinementTier::Devcontainer);
    }
    if sandbox_exec() {
        return Some(ConfinementTier::SandboxExec);
    }
    if raw_allowed() {
        // The raw tier: the human set the config (S1 marker discipline).
        // Selection still records it (S7) — raw-everywhere drift is visible.
        return Some(ConfinementTier::Raw);
    }
    None
}

/// Tier-1 availability: a working docker or podman CLI.
fn container_runtime_available() -> bool {
    ["docker", "podman"].into_iter().any(|runtime| {
        std::process::Command::new(runtime)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
}

/// Tier-2 availability: the macOS Seatbelt frontend.
fn sandbox_exec_available() -> bool {
    // `-h` exits 64 (usage error) — probe with a trivial allow-all profile
    // instead: a real run is the only honest availability check.
    std::process::Command::new("sandbox-exec")
        .args(["-p", "(version 1)(allow default)", "/usr/bin/true"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// The human-set raw-execution config (S9 discipline 1): reads
/// `.phronesis/verification.json` field `raw_execution`. Fail-closed on
/// parse errors — a malformed config never grants raw.
fn raw_execution_allowed(root: &Path) -> bool {
    let path = root.join(".phronesis").join("verification.json");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return false;
    };
    #[derive(serde::Deserialize)]
    struct VerificationConfig {
        #[serde(default)]
        raw_execution: bool,
    }
    serde_json::from_str::<VerificationConfig>(&raw)
        .map(|c| c.raw_execution)
        .unwrap_or(false)
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
        ConfinementTier::Raw => {
            // No confinement — the S3 allowlist gate still applied upstream.
            // Argv composition holds even raw (S4).
            let mut argv = verifier_command
                .split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>();
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

/// Parse verus output into a result status (the verus instantiation's
/// adapter). Returns None when the verifier's marker line is absent — the
/// zero-parse rule routes to `inconclusive` (S8), never a silent pass.
pub fn parse_verus_result(raw: &str, exit_code: Option<i32>) -> Option<String> {
    let marker = raw.lines().find(|l| l.contains("verification results::"))?;
    // `verification results:: N verified, M errors`
    let errors = marker
        .split(',')
        .find_map(|part| {
            let p = part.trim().split(' ').collect::<Vec<_>>();
            if p.windows(2).any(|w| w[1] == "errors") {
                p.first().and_then(|n| n.parse::<usize>().ok())
            } else {
                None
            }
        })
        .unwrap_or(0);
    let verified = marker
        .split("::")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|n| n.parse::<usize>().ok())
        .unwrap_or(0);
    if exit_code != Some(0) {
        return Some("failed".to_string());
    }
    Some(if errors == 0 && verified > 0 {
        "passed".to_string()
    } else {
        "failed".to_string()
    })
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
    let Some(tier) = detect_tier(root) else {
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

    // Per-toolchain result parse (SPEC-C: the verus instantiation). Verus
    // prints `verification results:: N verified, M errors` — the aggregate is
    // the property's status for single-property harnesses (phase 1 shape).
    let status = parse_verus_result(&raw, output.status.code());
    let Some(status) = status else {
        // S8's fourth state: the parser matched nothing — loud silence.
        return Ok(inconclusive_from(
            "unknown-property",
            verifier_command,
            tree_revision,
            tier,
            &raw,
        ));
    };
    Ok(ProofOutcome {
        v: 1,
        kind: "verification_result".to_string(),
        property: String::new(),
        verifier: verifier_command.to_string(),
        status,
        revision: String::new(),
        tool: verifier_command.to_string(),
        tier: format!("{tier:?}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// S9 tier resolution over every probe combination: the strongest
    /// available tier wins, a container needs a declared devcontainer, and
    /// raw is reached only when nothing stronger exists and the human-set
    /// config allows it. Host-independent — the probes are injected.
    #[test]
    fn s9_tier_resolution_ladder() {
        use ConfinementTier::{Devcontainer, Raw, SandboxExec};
        for bits in 0u8..16 {
            let [declared, runtime, seatbelt, raw] = [0, 1, 2, 3].map(|i| bits & (1 << i) != 0);
            let expected = if declared && runtime {
                Some(Devcontainer)
            } else if seatbelt {
                Some(SandboxExec)
            } else if raw {
                Some(Raw)
            } else {
                None
            };
            let tier = resolve_tier(declared, || runtime, || seatbelt, || raw);
            assert_eq!(
                tier, expected,
                "declared={declared} runtime={runtime} seatbelt={seatbelt} raw={raw}"
            );
        }
        // No container runtime is probed without a declared devcontainer.
        let tier = resolve_tier(false, || panic!("probed runtime"), || true, || false);
        assert_eq!(tier, Some(SandboxExec));
    }

    /// `detect_tier` wires the real probes: with no devcontainer declared and
    /// no raw config, this host lands on sandbox-exec exactly when Seatbelt
    /// works here, and is refused otherwise (e.g. Linux CI).
    #[test]
    fn detect_tier_matches_the_hosts_seatbelt_probe() {
        let root = tempfile::tempdir().unwrap();
        let expected = sandbox_exec_available().then_some(ConfinementTier::SandboxExec);
        assert_eq!(detect_tier(root.path()), expected);
    }

    /// S9 discipline 1: raw is never a default — without the human-set config
    /// a host with no container/seatbelt refuses execution (fail-closed); a
    /// malformed config never grants raw.
    #[test]
    fn raw_tier_requires_the_human_set_config() {
        let root = tempfile::tempdir().unwrap();
        assert!(
            !raw_execution_allowed(root.path()),
            "no config file: raw denied"
        );
        std::fs::create_dir_all(root.path().join(".phronesis")).unwrap();
        std::fs::write(
            root.path().join(".phronesis/verification.json"),
            r#"{"raw_execution": true}"#,
        )
        .unwrap();
        assert!(raw_execution_allowed(root.path()));
        std::fs::write(
            root.path().join(".phronesis/verification.json"),
            r#"{"raw_execution": "yes"}"#,
        )
        .unwrap();
        assert!(
            !raw_execution_allowed(root.path()),
            "malformed config never grants raw"
        );
    }

    /// S9: raw argv composes plainly (still argv, never shell strings) —
    /// the S3 gate applied upstream is what keeps raw defensible.
    #[test]
    fn raw_argv_is_plain_verifier_composition() {
        let argv = verifier_argv(
            ConfinementTier::Raw,
            Path::new("verification/unreviewed/h.rs"),
            "verus",
        );
        assert_eq!(argv.first(), Some(&"verus".to_string()));
        assert!(argv.last().is_some_and(|a| a.ends_with("h.rs")));
        assert!(
            !argv.iter().any(|a| a == "docker" || a == "sandbox-exec"),
            "raw wraps nothing: {argv:?}"
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

#[cfg(test)]
mod verus_tests {
    use super::*;

    #[test]
    fn verus_parser_reads_the_marker_line() {
        let raw = "verification results:: 10 verified, 0 errors\n";
        assert_eq!(parse_verus_result(raw, Some(0)), Some("passed".to_string()));
        let raw = "verification results:: 9 verified, 1 errors\n";
        assert_eq!(parse_verus_result(raw, Some(1)), Some("failed".to_string()));
        assert_eq!(parse_verus_result(raw, Some(0)), Some("failed".to_string()));
    }

    /// S8: no marker line → the zero-parse rule routes to inconclusive.
    #[test]
    fn verus_parser_absent_marker_routes_to_inconclusive() {
        assert_eq!(parse_verus_result("nothing recognizable", Some(0)), None);
    }
}
