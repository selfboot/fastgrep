use clap::{Parser, Subcommand};

mod cmd;
mod output;

#[derive(Parser)]
#[command(name = "fastgrep", version, about = "Agent-friendly fast regex search with trigram indexing")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Build a trigram index for the current directory
    Index {
        /// Path to index (defaults to current directory)
        #[arg(short, long)]
        path: Option<String>,

        /// Incremental rebuild: only re-process changed files
        #[arg(long)]
        incremental: bool,
    },

    /// Search using the trigram index
    Search {
        /// The regex pattern to search for
        pattern: String,

        /// Path to search in (defaults to current directory)
        #[arg(short, long)]
        path: Option<String>,

        /// Lines of context before each match
        #[arg(short = 'B', long, default_value = "0")]
        before_context: usize,

        /// Lines of context after each match
        #[arg(short = 'A', long, default_value = "0")]
        after_context: usize,

        /// Lines of context before and after each match
        #[arg(short = 'C', long)]
        context: Option<usize>,

        /// Case insensitive search
        #[arg(short = 'i', long)]
        ignore_case: bool,

        /// Filter by file type extension (e.g., rs, py, js)
        #[arg(short = 't', long = "type")]
        file_type: Option<String>,

        /// Filter by glob pattern (e.g., "*.tsx")
        #[arg(short = 'g', long)]
        glob: Option<String>,

        /// Output format
        #[arg(short = 'f', long = "format", default_value = "text")]
        output_format: String,

        /// Skip auto-build/refresh of index
        #[arg(long)]
        no_auto_index: bool,
    },

    /// Show index status and health
    Status {
        /// Path to check (defaults to current directory)
        #[arg(short, long)]
        path: Option<String>,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Index { path, incremental } => {
            let root = resolve_path(path)?;
            cmd::index::run(&root, incremental)
        }
        Commands::Search {
            pattern,
            path,
            before_context,
            after_context,
            context,
            ignore_case,
            file_type,
            glob,
            output_format,
            no_auto_index,
        } => {
            let root = resolve_path(path)?;
            let (before, after) = if let Some(c) = context {
                (c, c)
            } else {
                (before_context, after_context)
            };
            cmd::search::run(
                &root,
                &pattern,
                before,
                after,
                ignore_case,
                file_type,
                glob,
                &output_format,
                !no_auto_index,
            )
        }
        Commands::Status { path } => {
            let root = resolve_path(path)?;
            cmd::status::run(&root)
        }
    }
}

fn resolve_path(path: Option<String>) -> anyhow::Result<std::path::PathBuf> {
    match path {
        Some(p) => Ok(std::path::PathBuf::from(p)),
        None => std::env::current_dir().map_err(|e| anyhow::anyhow!("getting current dir: {}", e)),
    }
}
