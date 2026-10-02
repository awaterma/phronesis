// Main entry point; subcommand bodies arrive with their tasks
use clap::Parser;

#[derive(Parser)]
#[command(name = "phr-bench")]
#[command(about = "A/B benchmark framework for phronesis rules")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Parser)]
enum Commands {
    // Subcommands arrive with their tasks
}

fn main() {
    let _cli = Cli::parse();
    // Implementation arrives with later tasks
}
