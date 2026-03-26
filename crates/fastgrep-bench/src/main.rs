mod runner;
mod corpus;
mod report;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "fastgrep-bench", about = "Benchmark tool for fastgrep vs ripgrep")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Prepare a test corpus
    Prepare {
        /// Corpus name (e.g., "small", "medium", "linux-kernel")
        #[arg(long)]
        corpus: String,
        /// Output directory
        #[arg(long, default_value = "./testdata")]
        output: String,
    },
    /// Run benchmarks
    Run {
        /// Path to corpus
        #[arg(long)]
        corpus: String,
        /// Number of iterations per pattern
        #[arg(long, default_value = "10")]
        iterations: usize,
        /// Output CSV file
        #[arg(long, default_value = "results.csv")]
        output: String,
    },
    /// Generate a markdown report from results
    Report {
        /// Input CSV file
        #[arg(long, default_value = "results.csv")]
        input: String,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Prepare { corpus, output } => {
            corpus::prepare(&corpus, &output)
        }
        Commands::Run {
            corpus,
            iterations,
            output,
        } => runner::run(&corpus, iterations, &output),
        Commands::Report { input } => report::generate(&input),
    }
}
