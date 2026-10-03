use anyhow::{bail, Context, Result};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use phr_bench::manifest::Manifest;
use phr_bench::record::Arm;
use phr_bench::{arms, runner};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "phr-bench")]
#[command(about = "A/B benchmark framework for phronesis rules")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
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

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Run {
            manifest,
            run_id,
            arm,
        }) => {
            if let Err(err) = run_cmd(&manifest, &run_id, arm.into()) {
                eprintln!("phr-bench run failed: {err:#}");
                std::process::exit(1);
            }
        }
        None => {
            let _ = Cli::command().print_help();
        }
    }
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