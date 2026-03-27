/// `fastgrep index` command implementation.

use std::path::Path;

use anyhow::Result;

use fastgrep_core::index::builder::{self, BuildOptions};

pub fn run(root: &Path, incremental: bool) -> Result<()> {
    let opts = BuildOptions::new(root.to_path_buf());

    if incremental {
        eprintln!("Incrementally rebuilding index for {}...", root.display());
        match builder::incremental_rebuild(&opts)? {
            Some(stats) => {
                eprintln!(
                    "Index rebuilt: {} files indexed ({} discovered, {} binary skipped, {} large skipped), {} trigrams in {}ms",
                    stats.indexed_count,
                    stats.file_count,
                    stats.skipped_binary,
                    stats.skipped_large,
                    stats.trigram_count,
                    stats.build_time_ms,
                );
            }
            None => {
                eprintln!("Index is already up-to-date.");
            }
        }
    } else {
        eprintln!("Building index for {}...", root.display());
        let stats = builder::build_index(&opts)?;
        eprintln!(
            "Index built: {} files indexed ({} discovered, {} binary skipped, {} large skipped), {} trigrams in {}ms",
            stats.indexed_count,
            stats.file_count,
            stats.skipped_binary,
            stats.skipped_large,
            stats.trigram_count,
            stats.build_time_ms,
        );
    }

    Ok(())
}
