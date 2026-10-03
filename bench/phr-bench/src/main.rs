// Main entry point; subcommand bodies arrive with their tasks
use anyhow::{bail, Context, Result};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use phr_bench::corpus::{build_manifest, Slice};
use phr_bench::manifest::{DatasetRef, Manifest};
use phr_bench::record::Arm;
use phr_bench::{arms, quality, runner};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Dataset id/revision pinned in `bench/scripts/SPIKE-FINDINGS.md` (Task 2).
const DATASET_ID: &str = "SWE-bench/SWE-bench_Multilingual";
const DATASET_REVISION: &str = "846e647b9f33c0b51b739d005d13d85493c9af09";

#[derive(Parser)]
#[command(name = "phr-bench")]
#[command(about = "A/B benchmark framework for phronesis rules")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Load the pinned SWE-bench dataset, slice it, and write a manifest.
    Corpus {
        #[arg(long, value_enum)]
        slice: SliceArg,
        #[arg(long)]
        seed: u64,
        #[arg(long)]
        out: PathBuf,
    },
    /// Clone and prepare both arms' repositories for a run.
    Arms {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        run_id: String,
    },
    /// Prepare clones and drive claude headless for every manifest task, one arm at a time.
    ///
    /// Source bench/run-env.sh first: the ANTHROPIC_* vars it exports are
    /// injected into each claude child process. Artifacts land under
    /// bench/results/<run-id>/ (clones/ and runs/<instance>/<arm>/), relative
    /// to the current directory.
    Run {
        /// Path to the manifest JSON (dataset ref, seed, caps, tasks).
        #[arg(long)]
        manifest: PathBuf,
        /// Benchmark run id; artifacts land under bench/results/<run-id>/.
        #[arg(long)]
        run_id: String,
        /// Which arm to prepare and run.
        #[arg(long, value_enum)]
        arm: ArmArg,
    },
    /// Run symmetric audit on both arms' clones for every task.
    ///
    /// For each task: treatment clones use their own init rules;
    /// control clones receive the paired treatment clone's rules.json.
    /// Audit results are written to bench/results/<run-id>/quality.json.
    Quality {
        /// Benchmark run id; reads from bench/results/<run-id>/.
        #[arg(long)]
        run_id: String,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum SliceArg {
    Pilot,
    Full,
}

impl From<SliceArg> for Slice {
    fn from(value: SliceArg) -> Self {
        match value {
            SliceArg::Pilot => Slice::Pilot,
            SliceArg::Full => Slice::Full,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ArmArg {
    Control,
    Treatment,
}

impl From<ArmArg> for Arm {
    fn from(value: ArmArg) -> Self {
        match value {
            ArmArg::Control => Arm::Control,
            ArmArg::Treatment => Arm::Treatment,
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Corpus { slice, seed, out }) => run_corpus(slice.into(), seed, &out),
        Some(Commands::Arms { manifest, run_id }) => run_arms(&manifest, &run_id),
        Some(Commands::Run {
            manifest,
            run_id,
            arm,
        }) => run_cmd(&manifest, &run_id, arm.into()),
        Some(Commands::Quality { run_id }) => run_quality(&run_id),
        None => {
            let _ = Cli::command().print_help();
            Ok(())
        }
    }
}

fn run_corpus(slice: Slice, seed: u64, out: &Path) -> Result<()> {
    let dataset_jsonl = load_dataset_jsonl()?;
    let dataset = DatasetRef {
        id: DATASET_ID.to_owned(),
        revision: DATASET_REVISION.to_owned(),
    };
    let manifest = build_manifest(&dataset_jsonl, slice, seed, dataset)?;
    let encoded = serde_json::to_vec_pretty(&manifest).context("serialize manifest")?;
    if let Some(parent) = out.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create manifest directory {}", parent.display()))?;
        }
    }
    std::fs::write(out, encoded).with_context(|| format!("write manifest to {}", out.display()))?;
    println!(
        "wrote manifest for {} tasks to {}",
        manifest.tasks.len(),
        out.display()
    );
    Ok(())
}

fn run_arms(manifest_path: &Path, run_id: &str) -> Result<()> {
    validate_run_id(run_id)?;
    let encoded = std::fs::read(manifest_path)
        .with_context(|| format!("read manifest {}", manifest_path.display()))?;
    let manifest: Manifest = serde_json::from_slice(&encoded)
        .with_context(|| format!("parse manifest {}", manifest_path.display()))?;
    let root = project_root()?;
    let clones_dir = root.join("bench/results").join(run_id).join("clones");
    for task in &manifest.tasks {
        for arm in [Arm::Control, Arm::Treatment] {
            let clone = arms::prep(task, arm, &clones_dir)
                .with_context(|| format!("prepare {} arm for {}", arm.as_str(), task.instance_id))?;
            println!(
                "prepared {} arm for {} at {}",
                arm.as_str(),
                task.instance_id,
                clone.display()
            );
        }
    }
    Ok(())
}

fn validate_run_id(run_id: &str) -> Result<()> {
    if run_id.is_empty()
        || run_id == "."
        || run_id == ".."
        || run_id.contains(['/', '\\'])
        || run_id.contains('\0')
    {
        bail!("invalid run id {run_id:?}: must be a single path segment");
    }
    Ok(())
}

/// Invoke the project's pinned Python venv (`bench/.venv`, see
/// `SPIKE-FINDINGS.md`) with a small inline shim that loads the pinned
/// dataset revision and writes it to stdout as JSONL, one instance per line,
/// matching the real SWE-bench_Multilingual columns. Not exercised by
/// tests: tests feed fixture JSONL straight to `build_manifest`.
fn load_dataset_jsonl() -> Result<String> {
    let root = project_root()?;
    let python = root.join("bench/.venv/bin/python");
    let shim = format!(
        "import json\n\
         from datasets import load_dataset\n\
         ds = load_dataset(\"{DATASET_ID}\", split=\"test\", revision=\"{DATASET_REVISION}\")\n\
         for row in ds:\n    print(json.dumps(row))\n"
    );
    let output = Command::new(&python)
        .arg("-c")
        .arg(&shim)
        .output()
        .with_context(|| format!("spawn {}", python.display()))?;
    if !output.status.success() {
        bail!(
            "dataset load shim failed (exit {:?}): {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    String::from_utf8(output.stdout).context("dataset shim stdout was not valid UTF-8")
}

fn project_root() -> Result<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("spawn git rev-parse --show-toplevel")?;
    if !output.status.success() {
        bail!("git rev-parse --show-toplevel failed");
    }
    let path = String::from_utf8(output.stdout).context("git rev-parse output was not UTF-8")?;
    Ok(PathBuf::from(path.trim()))
}

fn run_cmd(manifest_path: &Path, run_id: &str, arm: Arm) -> Result<()> {
    let manifest_src = std::fs::read_to_string(manifest_path)
        .with_context(|| format!("reading manifest {}", manifest_path.display()))?;
    let manifest: Manifest = serde_json::from_str(&manifest_src).context("parsing manifest JSON")?;
    let results_root = Path::new("bench/results").join(run_id);
    let clones_root = results_root.join("clones");
    preflight(&manifest, arm, &clones_root)?;
    println!(
        "run {run_id}: arm={} tasks={} max_turns={} max_wall_clock_secs={}",
        arm.as_str(),
        manifest.tasks.len(),
        manifest.caps.max_turns,
        manifest.caps.max_wall_clock_secs
    );
    let runs_root = results_root.join("runs");
    let mut failures = 0u32;
    for task in &manifest.tasks {
        let clone_dir = match arms::prep(task, arm, &clones_root) {
            Ok(dir) => dir,
            Err(err) => {
                failures += 1;
                eprintln!("prep failed for {}: {err:#}", task.instance_id);
                continue;
            }
        };
        let run_dir = runs_root.join(&task.instance_id).join(arm.as_str());
        match runner::run(task, arm, &clone_dir, &run_dir, &manifest.caps) {
            Ok(rec) => println!(
                "{} [{}] exit={} turns={} wall_clock={}s diff_bytes={} governance={}",
                task.instance_id,
                arm.as_str(),
                rec.exit,
                rec.turns,
                rec.wall_clock_secs,
                rec.diff_bytes,
                rec.governance
                    .as_ref()
                    .map(|g| {
                        format!(
                            "blocks={} warns={} fail_closed={}",
                            g.blocks.len(),
                            g.warns.len(),
                            g.fail_closed
                        )
                    })
                    .unwrap_or_else(|| "none".into())
            ),
            Err(err) => {
                failures += 1;
                eprintln!("run failed for {}: {err:#}", task.instance_id);
            }
        }
    }
    if failures > 0 {
        bail!("{failures} task(s) failed to produce a record");
    }
    Ok(())
}

/// Refuse to mix a fresh run into leftover state: prep clones into fixed
/// paths, so an existing clone dir means the run-id is stale.
fn preflight(manifest: &Manifest, arm: Arm, clones_root: &Path) -> Result<()> {
    for task in &manifest.tasks {
        let dir = clones_root.join(&task.instance_id).join(arm.as_str());
        if dir.exists() {
            bail!(
                "clone dir {} already exists; use a fresh --run-id or remove bench/results first",
                dir.display()
            );
        }
    }
    Ok(())
}

fn run_quality(run_id: &str) -> Result<()> {
    validate_run_id(run_id)?;
    let results_root = Path::new("bench/results").join(run_id);
    let clones_root = results_root.join("clones");
    let runs_root = results_root.join("runs");

    // Iterate over all tasks and both arms, collecting audit summaries
    let mut quality_results = std::collections::BTreeMap::new();

    for entry in std::fs::read_dir(&clones_root)
        .with_context(|| format!("read clones directory {}", clones_root.display()))?
    {
        let entry = entry.context("read clone entry")?;
        let instance_id = entry.file_name();
        let instance_str = instance_id
            .to_str()
            .context("instance_id is not valid UTF-8")?
            .to_owned();

        for arm in [Arm::Control, Arm::Treatment] {
            let clone_dir = entry.path().join(arm.as_str());
            let runs_dir = runs_root.join(&instance_str).join(arm.as_str());
            let patch_path = runs_dir.join("patch.diff");

            // Skip if patch artifact is absent (shouldn't happen if runs succeeded, but be defensive)
            if !patch_path.exists() {
                eprintln!(
                    "warning: patch artifact {} absent for {} [{}], skipping quality audit",
                    patch_path.display(),
                    instance_str,
                    arm.as_str()
                );
                continue;
            }

            // For control clones: read the treatment clone's rules.json (symmetry requirement)
            let rules_json = match arm {
                Arm::Treatment => {
                    // Treatment clones have their own rules from init
                    let rules_path = clone_dir.join(".phronesis/rules.json");
                    std::fs::read_to_string(&rules_path).with_context(|| {
                        format!("read treatment rules from {}", rules_path.display())
                    })?
                }
                Arm::Control => {
                    // Control clones get the paired treatment clone's rules
                    let treatment_rules_path = clones_root
                        .join(&instance_str)
                        .join(Arm::Treatment.as_str())
                        .join(".phronesis/rules.json");
                    std::fs::read_to_string(&treatment_rules_path).with_context(|| {
                        format!(
                            "read paired treatment rules from {} for control arm symmetry",
                            treatment_rules_path.display()
                        )
                    })?
                }
            };

            match quality::audit_clone(&clone_dir, &rules_json, &patch_path) {
                Ok(summary) => {
                    let key = format!("{}/{}", instance_str, arm.as_str());
                    quality_results.insert(key.clone(), summary.clone());
                    println!(
                        "{} [{}] total_violations={} rules={}",
                        instance_str,
                        arm.as_str(),
                        summary.total_violations,
                        summary.per_rule.len()
                    );
                }
                Err(err) => {
                    eprintln!(
                        "quality audit failed for {} [{}]: {err:#}",
                        instance_str,
                        arm.as_str()
                    );
                }
            }
        }
    }

    // Write results to quality.json
    let quality_json = results_root.join("quality.json");
    let encoded = serde_json::to_vec_pretty(&quality_results)
        .context("serialize quality results")?;
    std::fs::write(&quality_json, encoded)
        .with_context(|| format!("write quality results to {}", quality_json.display()))?;
    println!(
        "wrote quality audit results for {} instance(s) to {}",
        quality_results.len() / 2,
        quality_json.display()
    );
    Ok(())
}