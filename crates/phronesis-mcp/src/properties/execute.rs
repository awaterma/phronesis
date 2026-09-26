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
    /// confined to the per-run scratch directory holding the artifact copy.
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
    #[error(
        "artifact bytes changed since approval: on-disk sha256 {actual} does not match {expected} (S3)"
    )]
    HashMismatch { expected: String, actual: String },
    #[error("devcontainer tier refused: {message}")]
    DevcontainerImage { message: String },
}

/// The artifact's SHA-256 as lowercase hex — the S3 allowlist key.
pub fn artifact_sha256(bytes: &[u8]) -> String {
    crate::graph::ownership::extract::hex(&crate::graph::ownership::extract::sha256(bytes))
}

/// Where the instantiation declares its verifier environment (S9 Tier 1).
fn devcontainer_path(root: &Path) -> std::path::PathBuf {
    root.join("verification/templates/devcontainer.json")
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
    let has_devcontainer = devcontainer_path(root).is_file();
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

/// The per-run directory's two children: the staged artifact lives in its
/// own subdirectory so no artifact file name can collide with `TMPDIR`.
const RUN_ARTIFACT_DIR: &str = "artifact";
const RUN_TMP_DIR: &str = "tmp";

/// The container path the artifact's run directory is mounted at.
const CONTAINER_RUN_DIR: &str = "/verification";

/// The Seatbelt profile (S9 Tier 2). Everything the verifier needs to read
/// stays readable, but network is denied and every write is denied except
/// under the per-run scratch directory. Writes by proxy are writes too:
/// mach-lookup is denied so no daemon (cfprefsd behind `defaults`, the
/// pasteboard server behind `pbcopy`, …) can write on the verifier's behalf,
/// and signals are confined to the verifier itself so it cannot kill or stop
/// the user's other processes. The run directory is passed as the `RUN_DIR` parameter
/// (`sandbox-exec -D`) so no path is ever spliced into the profile text.
/// Seatbelt matches `subpath` against resolved absolute paths, so `RUN_DIR`
/// must be canonical (macOS `/var` is `/private/var`).
const SEATBELT_PROFILE: &str = "(version 1)(allow default)(deny network*)\
     (deny mach-lookup)(deny signal)(allow signal (target self))(deny file-write*)\
     (allow file-write* (subpath (param \"RUN_DIR\")))\
     (allow file-write-data (literal \"/dev/null\"))";

/// What the host decided for one verifier run: the canonical per-run
/// scratch directory (holds the hashed artifact copy and TMPDIR; the only
/// writable path) and, for the devcontainer tier, the pinned image.
#[derive(Debug, Clone)]
pub struct RunConfinement<'a> {
    pub run_dir: &'a Path,
    pub image: Option<&'a str>,
}

/// The devcontainer tier's image, read from the instantiation's declared
/// `devcontainer.json` (`image` field). It must be pinned by digest
/// (`name@sha256:<64 hex>`): a tag is mutable, and a registry pull would let
/// whatever that tag points at today run as the verifier. Refuses when the
/// file, the field, or the pin is absent.
pub fn devcontainer_image(root: &Path) -> Result<String, ExecutionError> {
    let path = devcontainer_path(root);
    let refuse = |message: String| ExecutionError::DevcontainerImage { message };
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| refuse(format!("cannot read {}: {e}", path.display())))?;
    let config: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| refuse(format!("{} is not plain JSON: {e}", path.display())))?;
    let Some(image) = config.get("image").and_then(|v| v.as_str()) else {
        return Err(refuse(format!(
            "{} declares no \"image\" string",
            path.display()
        )));
    };
    let pinned = image.split_once("@sha256:").is_some_and(|(name, digest)| {
        !name.is_empty()
            && digest.len() == 64
            && digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    });
    if !pinned {
        return Err(refuse(format!(
            "image {image:?} in {} is not pinned by digest (expected name@sha256:<64 hex>)",
            path.display()
        )));
    }
    Ok(image.to_string())
}

/// Compose the verifier invocation as argv (S4: never shell strings), forced
/// confinement flags included (S9: the host sets them regardless of any
/// devcontainer file). `tier` decides the wrapper; `artifact` is the host
/// path of the hashed copy inside `confinement.run_dir`.
pub fn verifier_argv(
    tier: ConfinementTier,
    confinement: &RunConfinement<'_>,
    artifact: &Path,
    verifier_command: &str,
) -> Result<Vec<String>, ExecutionError> {
    let artifact_str = artifact.display().to_string();
    let verifier = verifier_command.split_whitespace().map(str::to_string);
    Ok(match tier {
        ConfinementTier::Devcontainer => {
            let Some(image) = confinement.image else {
                return Err(ExecutionError::DevcontainerImage {
                    message: "no pinned image declared for the devcontainer tier".to_string(),
                });
            };
            // The artifact is reachable only through the read-only run-dir
            // mount; its host path means nothing inside the container.
            let relative =
                artifact
                    .strip_prefix(confinement.run_dir)
                    .map_err(|_| ExecutionError::Failed {
                        message: format!(
                            "artifact {} is outside the run directory {}",
                            artifact.display(),
                            confinement.run_dir.display()
                        ),
                    })?;
            let container_artifact = Path::new(CONTAINER_RUN_DIR).join(relative);
            // `--mount` is a comma-separated field list with no escaping a
            // bind source can rely on: a comma would inject mount options.
            let source = confinement.run_dir.display().to_string();
            if source.contains(',') {
                return Err(ExecutionError::Failed {
                    message: format!(
                        "run directory {source} contains a comma, which docker --mount cannot carry"
                    ),
                });
            }
            let mut argv = vec![
                "docker".to_string(),
                "run".to_string(),
                "--rm".to_string(),
                "--pull=never".to_string(),
                "--network=none".to_string(),
                "--read-only".to_string(),
                "--tmpfs=/tmp".to_string(),
                "--cap-drop=ALL".to_string(),
                "--security-opt=no-new-privileges".to_string(),
                "--memory=2g".to_string(),
                "--cpus=2".to_string(),
                "--pids-limit=512".to_string(),
                format!("--mount=type=bind,source={source},target={CONTAINER_RUN_DIR},readonly"),
                format!("--workdir={CONTAINER_RUN_DIR}"),
                // `--` ends docker's options: the image is the next token,
                // never a verifier token.
                "--".to_string(),
                image.to_string(),
            ];
            argv.extend(verifier);
            argv.push(container_artifact.display().to_string());
            argv
        }
        ConfinementTier::SandboxExec => {
            let mut argv = vec![
                "sandbox-exec".to_string(),
                "-D".to_string(),
                format!("RUN_DIR={}", confinement.run_dir.display()),
                "-p".to_string(),
                SEATBELT_PROFILE.to_string(),
            ];
            argv.extend(verifier);
            argv.push(artifact_str);
            argv
        }
        ConfinementTier::Raw => {
            // No confinement — the S3 allowlist gate still applied upstream.
            // Argv composition holds even raw (S4).
            let mut argv = verifier.collect::<Vec<_>>();
            argv.push(artifact_str);
            argv
        }
        ConfinementTier::Refused => {
            // Unreachable via detect_tier — callers refuse before composing.
            vec!["false".to_string()]
        }
    })
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

/// The verus summary line's prefix. Only a line that *starts* with it (after
/// optional whitespace) is the verifier's own summary — a diagnostic that
/// echoes source text carries a `N |` gutter in front of it.
const VERUS_SUMMARY_PREFIX: &str = "verification results::";

/// Parse verus output into a result status (the verus instantiation's
/// adapter). Returns None when the verifier's summary line is absent or its
/// counts don't parse — the zero-parse rule routes to `inconclusive` (S8),
/// never a silent pass.
///
/// The verifier's own summary decides: the line must be anchored at line
/// start and the *last* one wins (verus prints it once, at the end; the
/// artifact interpolates property text into string literals, so an earlier
/// summary-shaped line can come from data). `passed` needs a zero exit, at
/// least one verified condition, and zero errors. A signal-killed run (no
/// exit code) with a summary on the wire reads as `failed` — like every
/// non-pass it never upgrades confidence (S8).
/// The verus status of a finished run. Verus prints its summary on stdout;
/// stderr carries diagnostics, which can quote arbitrary text (a deprecation
/// note, an echoed string literal), so it is never parsed for the summary —
/// otherwise a summary-shaped stderr line would override the real one.
fn verus_status(output: &std::process::Output) -> Option<String> {
    parse_verus_result(
        &String::from_utf8_lossy(&output.stdout),
        output.status.code(),
    )
}

pub fn parse_verus_result(raw: &str, exit_code: Option<i32>) -> Option<String> {
    let summary = raw
        .lines()
        .rev()
        .find_map(|l| l.trim_start().strip_prefix(VERUS_SUMMARY_PREFIX))?;
    let (verified, errors) = parse_verus_counts(summary)?;
    Some(if exit_code == Some(0) && verified > 0 && errors == 0 {
        "passed".to_string()
    } else {
        "failed".to_string()
    })
}

/// ` N verified, M errors` (or the singular `1 error`) → `(N, M)`. Anything
/// else — a missing or non-numeric count, extra clauses — is None.
fn parse_verus_counts(summary: &str) -> Option<(usize, usize)> {
    let (verified, errors) = summary.split_once(',')?;
    let verified = match verified.split_whitespace().collect::<Vec<_>>()[..] {
        [n, "verified"] => n.parse::<usize>().ok()?,
        _ => return None,
    };
    let errors = match errors.split_whitespace().collect::<Vec<_>>()[..] {
        [n, "error" | "errors"] => n.parse::<usize>().ok()?,
        _ => return None,
    };
    Some((verified, errors))
}

fn tracing_like(message: &str) {
    // The outcomes fold-in seam journals via the journey; the raw tail rides
    // the journal payload, not the RETE args (three-state discipline).
    eprintln!("phronesis: verification {message}");
}

/// Execute the verifier for an approved artifact through the confinement
/// tiers. `artifact_sha256` is the hash the caller believes it approved; this
/// layer re-hashes the bytes on disk itself and refuses on mismatch or when
/// the on-disk hash is not allowlisted (S3 condition iii). The verifier runs
/// against a copy of exactly the hashed bytes in a fresh per-run directory,
/// so the file cannot change between the check and the run.
pub fn execute(
    root: &Path,
    artifact: &Path,
    artifact_sha256: &str,
    verifier_command: &str,
    tree_revision: &str,
) -> Result<ProofOutcome, ExecutionError> {
    let failed = |message: String| ExecutionError::Failed { message };
    let bytes = std::fs::read(artifact)
        .map_err(|e| failed(format!("cannot read artifact {}: {e}", artifact.display())))?;
    let actual = self::artifact_sha256(&bytes);
    if actual != artifact_sha256 {
        return Err(ExecutionError::HashMismatch {
            expected: artifact_sha256.to_string(),
            actual,
        });
    }
    if !crate::properties::allowlist::contains(root, &actual).map_err(|e| failed(e.to_string()))? {
        return Err(ExecutionError::NotApproved { hash: actual });
    }
    let Some(tier) = detect_tier(root) else {
        // S9 fail-closed: no confinement available, execution refused.
        tracing_like("execution refused: no confinement tier available");
        return Err(ExecutionError::Failed {
            message: "refused: no confinement tier available (S9 fail-closed)".to_string(),
        });
    };
    let image = match tier {
        ConfinementTier::Devcontainer => Some(devcontainer_image(root)?),
        _ => None,
    };
    let _ = tree_revision; // dedup ledger keyed by (hash, revision); the
    // ledger write lands with the results sidecar (property-results.jsonl).

    // The per-run directory: the only writable path under confinement.
    let run = tempfile::Builder::new()
        .prefix("phr-verify-")
        .tempdir()
        .map_err(|e| failed(format!("cannot create run directory: {e}")))?;
    let run_dir = run
        .path()
        .canonicalize()
        .map_err(|e| failed(format!("cannot resolve run directory: {e}")))?;
    let tmp_dir = run_dir.join(RUN_TMP_DIR);
    std::fs::create_dir(&tmp_dir).map_err(|e| failed(format!("cannot create TMPDIR: {e}")))?;
    let file_name = artifact
        .file_name()
        .ok_or_else(|| failed(format!("artifact {} has no file name", artifact.display())))?;
    let artifact_dir = run_dir.join(RUN_ARTIFACT_DIR);
    std::fs::create_dir(&artifact_dir)
        .map_err(|e| failed(format!("cannot create artifact directory: {e}")))?;
    let run_artifact = artifact_dir.join(file_name);
    std::fs::write(&run_artifact, &bytes)
        .map_err(|e| failed(format!("cannot stage artifact: {e}")))?;

    let confinement = RunConfinement {
        run_dir: &run_dir,
        image: image.as_deref(),
    };
    let argv = verifier_argv(tier, &confinement, &run_artifact, verifier_command)?;
    let output = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(&run_dir)
        .env("TMPDIR", &tmp_dir)
        .output()
        .map_err(|e| failed(e.to_string()))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let raw = format!("{stdout}{stderr}");

    // Per-toolchain result parse (SPEC-C: the verus instantiation). Verus
    // prints `verification results:: N verified, M errors` — the aggregate is
    // the property's status for single-property harnesses (phase 1 shape).
    let status = verus_status(&output);
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

    /// A summary-shaped line on stderr (e.g. a multi-line deprecation note)
    /// must not override the real stdout summary.
    #[cfg(unix)]
    #[test]
    fn verus_status_reads_the_summary_from_stdout_only() {
        use std::os::unix::process::ExitStatusExt;
        let output = std::process::Output {
            status: std::process::ExitStatus::from_raw(0),
            stdout: b"verification results:: 0 verified, 0 errors\n".to_vec(),
            stderr: b"         verification results:: 5 verified, 0 errors\n".to_vec(),
        };
        assert_eq!(verus_status(&output), Some("failed".to_string()));
    }

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
            &RunConfinement {
                run_dir: Path::new("/run"),
                image: None,
            },
            Path::new("/run/h.rs"),
            "verus",
        )
        .unwrap();
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
        let sha = artifact_sha256(b"fn h() {}");
        let result = execute(root.path(), &artifact, &sha, "verus", "r1");
        assert!(matches!(result, Err(ExecutionError::NotApproved { .. })));
    }
}

#[cfg(test)]
mod confinement_tests {
    use super::*;

    const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn run_sandboxed(run_dir: &Path, target: &Path) -> bool {
        let confinement = RunConfinement {
            run_dir,
            image: None,
        };
        let argv = verifier_argv(
            ConfinementTier::SandboxExec,
            &confinement,
            target,
            "/usr/bin/touch",
        )
        .unwrap();
        std::process::Command::new(&argv[0])
            .args(&argv[1..])
            .current_dir(run_dir)
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    /// C12: under the Seatbelt tier a write outside the per-run directory
    /// fails and one inside succeeds. macOS-only: skipped (with a note) on a
    /// host without sandbox-exec, where the tier is never selected.
    #[test]
    fn c12_sandbox_profile_confines_writes_to_the_run_dir() {
        if !sandbox_exec_available() {
            eprintln!("skipping C12: sandbox-exec unavailable (non-macOS host)");
            return;
        }
        let scratch = tempfile::tempdir().unwrap();
        let scratch = scratch.path().canonicalize().unwrap();
        let run_dir = scratch.join("run");
        std::fs::create_dir(&run_dir).unwrap();

        let outside = scratch.join("outside-write-probe");
        assert!(!run_sandboxed(&run_dir, &outside), "outside write escaped");
        assert!(!outside.exists(), "outside write escaped the sandbox");

        let inside = run_dir.join("inside-write-probe");
        assert!(run_sandboxed(&run_dir, &inside), "inside write denied");
        assert!(inside.exists());
    }

    /// The `sandbox-exec -D RUN_DIR=… -p <profile>` prefix `verifier_argv`
    /// composes, followed by an arbitrary command.
    fn sandboxed(run_dir: &Path, command: &[&str]) -> std::process::Command {
        let argv = verifier_argv(
            ConfinementTier::SandboxExec,
            &RunConfinement {
                run_dir,
                image: None,
            },
            &run_dir.join("h.rs"),
            "verifier",
        )
        .unwrap();
        let (program, prefix) = (&argv[0], &argv[1..5]);
        let mut cmd = std::process::Command::new(program);
        cmd.args(prefix)
            .args(command)
            .current_dir(run_dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        cmd
    }

    fn canonical_run_dir() -> (tempfile::TempDir, std::path::PathBuf) {
        let scratch = tempfile::tempdir().unwrap();
        let run_dir = scratch.path().canonicalize().unwrap();
        (scratch, run_dir)
    }

    /// C12: writes by proxy are writes. `defaults` asks cfprefsd (over a
    /// mach service) to write `~/Library/Preferences/<domain>.plist` on the
    /// verifier's behalf; mach-lookup must be denied so the daemon is
    /// unreachable.
    #[test]
    fn c12_sandbox_denies_daemon_proxied_writes_via_defaults() {
        if !sandbox_exec_available() {
            eprintln!("skipping C12 defaults probe: sandbox-exec unavailable (non-macOS host)");
            return;
        }
        let (_scratch, run_dir) = canonical_run_dir();
        let domain = format!("com.phronesis.c12-probe-{}", std::process::id());
        let status = sandboxed(&run_dir, &["/usr/bin/defaults", "write", &domain, "k", "v"])
            .status()
            .unwrap();
        let plist = dirs::home_dir()
            .unwrap()
            .join(format!("Library/Preferences/{domain}.plist"));
        let escaped = status.success() || plist.exists();
        // Clean up outside the sandbox whatever happened.
        let _ = std::process::Command::new("/usr/bin/defaults")
            .args(["delete", &domain])
            .stderr(std::process::Stdio::null())
            .status();
        let _ = std::fs::remove_file(&plist);
        assert!(!escaped, "defaults write reached cfprefsd: {status:?}");
    }

    /// C12: the pasteboard is another daemon-held write surface.
    #[test]
    fn c12_sandbox_denies_clipboard_writes() {
        use std::io::Write;
        if !sandbox_exec_available() {
            eprintln!("skipping C12 pbcopy probe: sandbox-exec unavailable (non-macOS host)");
            return;
        }
        let before = std::process::Command::new("/usr/bin/pbpaste")
            .output()
            .map(|o| o.stdout)
            .unwrap_or_default();
        let (_scratch, run_dir) = canonical_run_dir();
        let marker = format!("c12-pbcopy-probe-{}", std::process::id());
        let mut child = sandboxed(&run_dir, &["/usr/bin/pbcopy"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let _ = child.stdin.take().unwrap().write_all(marker.as_bytes());
        let status = child.wait().unwrap();
        let after = std::process::Command::new("/usr/bin/pbpaste")
            .output()
            .map(|o| o.stdout)
            .unwrap_or_default();
        let escaped = after == marker.as_bytes();
        if escaped {
            // Restore the user's clipboard.
            if let Ok(mut restore) = std::process::Command::new("/usr/bin/pbcopy")
                .stdin(std::process::Stdio::piped())
                .spawn()
            {
                let _ = restore.stdin.take().unwrap().write_all(&before);
                let _ = restore.wait();
            }
        }
        assert!(!escaped, "pbcopy overwrote the clipboard: {status:?}");
    }

    /// C12: the verifier may signal only itself — not a sibling process of
    /// the same user.
    #[test]
    fn c12_sandbox_denies_signalling_other_processes() {
        if !sandbox_exec_available() {
            eprintln!("skipping C12 signal probe: sandbox-exec unavailable (non-macOS host)");
            return;
        }
        let (_scratch, run_dir) = canonical_run_dir();
        let mut sibling = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = sibling.id().to_string();
        let status = sandboxed(&run_dir, &["/bin/kill", "-TERM", &pid])
            .status()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let alive = sibling.try_wait().unwrap().is_none();
        let _ = sibling.kill();
        let _ = sibling.wait();
        assert!(
            !status.success() && alive,
            "sandboxed kill reached a sibling: {status:?}, alive={alive}"
        );
    }

    /// An artifact literally named like the scratch TMPDIR must not collide
    /// with it: staging still succeeds and the verifier runs.
    #[test]
    fn an_artifact_named_tmp_does_not_collide_with_the_run_tmpdir() {
        if !sandbox_exec_available() {
            eprintln!("skipping: sandbox-exec unavailable (non-macOS host)");
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let artifact = root.path().join(RUN_TMP_DIR);
        std::fs::write(&artifact, "fn h() {}").unwrap();
        let sha = approve(root.path(), b"fn h() {}");
        let result = execute(root.path(), &artifact, &sha, "/bin/cat", "r1");
        assert!(
            matches!(&result, Ok(o) if o.status == "inconclusive"),
            "an artifact named {RUN_TMP_DIR:?} must stage and run: {result:?}"
        );
    }

    /// A comma in the run directory would split docker's `--mount` field
    /// list (e.g. inject `,readonly=false`-style options): refuse it.
    #[test]
    fn devcontainer_mount_refuses_a_comma_in_the_run_dir() {
        let run_dir = Path::new("/tmp/a,readonly=false,x");
        let image = format!("verus@sha256:{DIGEST}");
        let result = verifier_argv(
            ConfinementTier::Devcontainer,
            &RunConfinement {
                run_dir,
                image: Some(&image),
            },
            &run_dir.join("h.rs"),
            "verus",
        );
        assert!(
            matches!(result, Err(ExecutionError::Failed { .. })),
            "{result:?}"
        );
    }

    /// C12: the run directory rides a `-D` parameter, never the profile text,
    /// so a hostile path cannot splice Seatbelt syntax into the profile.
    #[test]
    fn c12_run_dir_is_a_profile_parameter_not_profile_text() {
        let hostile = Path::new("/tmp/x\")(allow file-write* (subpath \"/");
        let argv = verifier_argv(
            ConfinementTier::SandboxExec,
            &RunConfinement {
                run_dir: hostile,
                image: None,
            },
            &hostile.join("h.rs"),
            "verus",
        )
        .unwrap();
        assert_eq!(argv[..2], ["sandbox-exec", "-D"]);
        assert_eq!(argv[2], format!("RUN_DIR={}", hostile.display()));
        assert_eq!(argv[3..5], ["-p".to_string(), SEATBELT_PROFILE.to_string()]);
        assert!(SEATBELT_PROFILE.contains("(deny file-write*)"));
        assert!(!SEATBELT_PROFILE.contains("subpath \"verification\""));
    }

    fn devcontainer_root(config: &str) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("verification/templates")).unwrap();
        std::fs::write(devcontainer_path(root.path()), config).unwrap();
        root
    }

    /// C13: the devcontainer argv runs the declared, digest-pinned image with
    /// no pull, mounts only the run dir read-only at a fixed path, rewrites
    /// the artifact to that path, and never lets a verifier token become the
    /// image.
    #[test]
    fn c13_devcontainer_argv_pins_image_and_mounts_artifact() {
        let image = format!("ghcr.io/acme/verus@sha256:{DIGEST}");
        let root = devcontainer_root(&format!(r#"{{"image": "{image}"}}"#));
        let declared = devcontainer_image(root.path()).unwrap();
        assert_eq!(declared, image);

        let run_dir = Path::new("/private/var/folders/x/phr-verify-1");
        let argv = verifier_argv(
            ConfinementTier::Devcontainer,
            &RunConfinement {
                run_dir,
                image: Some(&declared),
            },
            &run_dir.join("h.rs"),
            "verus --crate-type=lib",
        )
        .unwrap();
        for forced in [
            "--rm",
            "--pull=never",
            "--network=none",
            "--read-only",
            "--cap-drop=ALL",
        ] {
            assert!(
                argv.iter().any(|a| a == forced),
                "{forced} missing: {argv:?}"
            );
        }
        assert!(argv.contains(&format!(
            "--mount=type=bind,source={},target=/verification,readonly",
            run_dir.display()
        )));
        let sep = argv
            .iter()
            .position(|a| a == "--")
            .expect("option terminator");
        assert_eq!(argv[sep + 1], image, "image follows the terminator");
        assert_eq!(
            argv[sep + 2..],
            ["verus", "--crate-type=lib", "/verification/h.rs"],
            "verifier tokens after the image; artifact rewritten"
        );
        assert!(
            !argv
                .iter()
                .any(|a| a == &run_dir.join("h.rs").display().to_string()),
            "host artifact path never reaches the container: {argv:?}"
        );
    }

    /// C13: no declared image, or one not pinned by digest, refuses the
    /// devcontainer tier with a clear error.
    #[test]
    fn c13_devcontainer_image_must_be_declared_and_digest_pinned() {
        let refused = |config: &str| {
            matches!(
                devcontainer_image(devcontainer_root(config).path()),
                Err(ExecutionError::DevcontainerImage { .. })
            )
        };
        assert!(refused(r#"{"name": "no image"}"#));
        assert!(refused(r#"{"image": "verus:latest"}"#));
        assert!(refused(r#"{"image": "verus@sha256:abc"}"#));
        assert!(refused(&format!(r#"{{"image": "@sha256:{DIGEST}"}}"#)));
        assert!(refused("// jsonc comment\n{}"));
        let missing = tempfile::tempdir().unwrap();
        assert!(matches!(
            devcontainer_image(missing.path()),
            Err(ExecutionError::DevcontainerImage { .. })
        ));
        // And argv composition itself refuses a devcontainer run without one.
        let run_dir = Path::new("/run");
        assert!(matches!(
            verifier_argv(
                ConfinementTier::Devcontainer,
                &RunConfinement {
                    run_dir,
                    image: None
                },
                &run_dir.join("h.rs"),
                "verus",
            ),
            Err(ExecutionError::DevcontainerImage { .. })
        ));
    }

    /// The allowlist key is real SHA-256 (published test vector).
    #[test]
    fn artifact_sha256_is_real_sha256() {
        assert_eq!(
            artifact_sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    fn approve(root: &Path, bytes: &[u8]) -> String {
        let sha = artifact_sha256(bytes);
        crate::properties::allowlist::record(
            root,
            crate::properties::allowlist::AllowlistEntry {
                artifact_sha256: sha.clone(),
                template_sha256: "t".into(),
                property_id: "p".into(),
                property_revision: "r1".into(),
                approver_principal: "human".into(),
                date: "2026-09-26".into(),
            },
        )
        .unwrap();
        sha
    }

    /// C14: a tampered artifact is refused even when the caller passes the
    /// approved hash — execute re-hashes the bytes on disk itself.
    #[test]
    fn c14_execute_rehashes_the_artifact_on_disk() {
        let root = tempfile::tempdir().unwrap();
        let artifact = root.path().join("h.rs");
        let approved = approve(root.path(), b"fn approved() {}");
        std::fs::write(&artifact, "fn tampered() {}").unwrap();
        let result = execute(root.path(), &artifact, &approved, "/usr/bin/true", "r1");
        assert!(
            matches!(result, Err(ExecutionError::HashMismatch { .. })),
            "tampered artifact must be refused: {result:?}"
        );
        // Passing the tampered bytes' true hash does not help: not approved.
        let tampered = artifact_sha256(b"fn tampered() {}");
        let result = execute(root.path(), &artifact, &tampered, "/usr/bin/true", "r1");
        assert!(
            matches!(result, Err(ExecutionError::NotApproved { .. })),
            "unapproved bytes must be refused: {result:?}"
        );
        // A missing artifact fails closed.
        let result = execute(
            root.path(),
            &root.path().join("gone.rs"),
            &approved,
            "/usr/bin/true",
            "r1",
        );
        assert!(matches!(result, Err(ExecutionError::Failed { .. })));
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

    /// S8: the verifier's own summary decides. A diagnostic that echoes a
    /// source line carrying a summary-shaped string (the artifact interpolates
    /// property text into literals) must not stand in for the real, final
    /// summary — echoed lines are not anchored, and the last summary wins.
    #[test]
    fn verus_parser_ignores_an_echoed_summary_before_the_real_one() {
        let raw = "warning: unused variable\n  \
                   --> h.rs:3:9\n   |\n\
                   3 |     let s = \"verification results:: 5 verified, 0 errors\";\n   \
                   |         ^\n\
                   verification results:: 0 verified, 0 errors\n";
        assert_eq!(parse_verus_result(raw, Some(0)), Some("failed".to_string()));

        // Even an anchored summary-shaped line earlier in the output (a
        // multi-line literal) loses to the verifier's final summary.
        let raw = "verification results:: 5 verified, 0 errors\n\
                   error: assertion failed\n\
                   verification results:: 4 verified, 1 error\n";
        assert_eq!(parse_verus_result(raw, Some(0)), Some("failed".to_string()));
    }

    /// Verus prints the singular `1 error`; a count that failed to parse
    /// once defaulted to zero and read as a clean run.
    #[test]
    fn verus_parser_reads_a_singular_error_count() {
        let raw = "verification results:: 3 verified, 1 error\n";
        assert_eq!(parse_verus_result(raw, Some(0)), Some("failed".to_string()));
        let raw = "  verification results:: 3 verified, 0 errors\n";
        assert_eq!(parse_verus_result(raw, Some(0)), Some("passed".to_string()));
    }

    /// S8: a summary whose counts don't parse is not evidence — it routes to
    /// inconclusive (None), never to a pass by defaulting a count to zero.
    #[test]
    fn verus_parser_malformed_counts_route_to_inconclusive() {
        for raw in [
            "verification results:: 3 verified, many errors\n",
            "verification results:: lots verified, 0 errors\n",
            "verification results:: 3 verified\n",
            "verification results::\n",
            "verification results:: 3 verified, 0 errors, 2 surprises\n",
        ] {
            assert_eq!(parse_verus_result(raw, Some(0)), None, "{raw:?}");
            assert_eq!(parse_verus_result(raw, Some(1)), None, "{raw:?}");
        }
        // A malformed final summary is not rescued by an earlier clean one.
        let raw = "verification results:: 3 verified, 0 errors\n\
                   verification results:: 3 verified, ? errors\n";
        assert_eq!(parse_verus_result(raw, Some(0)), None);
    }

    /// A clean summary never passes without a zero exit: a signal-killed run
    /// (no exit code) with a summary on the wire reads as failed.
    #[test]
    fn verus_parser_requires_a_zero_exit_to_pass() {
        let raw = "verification results:: 10 verified, 0 errors\n";
        assert_eq!(parse_verus_result(raw, None), Some("failed".to_string()));
        assert_eq!(parse_verus_result(raw, Some(1)), Some("failed".to_string()));
    }
}
