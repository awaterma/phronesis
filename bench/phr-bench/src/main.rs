// Main entry point; subcommand bodies arrive with their tasks
use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use phr_bench::arms::prep;
use phr_bench::corpus::{build_manifest, Slice};
use phr_bench::manifest::{DatasetRef, Manifest};
use phr_bench::record::Arm;
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

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Corpus { slice, seed, out }) => run_corpus(slice.into(), seed, &out),
        Some(Commands::Arms { manifest, run_id }) => run_arms(&manifest, &run_id),
        None => Ok(()),
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
            let clone = prep(task, arm, &clones_dir)
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
