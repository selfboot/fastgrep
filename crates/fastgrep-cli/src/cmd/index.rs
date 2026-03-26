/// `fastgrep index` command implementation.

use std::path::Path;

use anyhow::Result;

use fastgrep_core::index::builder::{self, BuildOptions};

pub fn run(root: &Path) -> Result<()> {
    eprintln!("Building index for {}...", root.display());

    let opts = BuildOptions::new(root.to_path_buf());

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

    Ok(())
}
