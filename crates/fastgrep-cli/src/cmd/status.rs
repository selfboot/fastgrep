/// `fastgrep status` command implementation.

use std::path::Path;

use anyhow::Result;

use fastgrep_core::git;
use fastgrep_core::index::reader::IndexReader;
use fastgrep_core::INDEX_DIR;

pub fn run(root: &Path) -> Result<()> {
    let index_dir = root.join(INDEX_DIR);

    if !index_dir.exists() {
        println!("No index found at {}", index_dir.display());
        println!("Run `fastgrep index` to build one.");
        return Ok(());
    }

    let reader = IndexReader::open(root)?;

    println!("Index status:");
    println!("  Root:         {}", root.display());
    println!("  Files:        {}", reader.file_count());
    println!(
        "  Trigrams:     {}",
        reader.meta.trigram_count
    );
    println!(
        "  Commit:       {}",
        reader.commit_hash().unwrap_or("(none)")
    );

    // Check freshness
    let fresh = git::is_index_fresh(root, reader.commit_hash());
    println!(
        "  Fresh:        {}",
        if fresh { "yes" } else { "NO — rebuild recommended" }
    );

    // File sizes
    let lookup_size = std::fs::metadata(index_dir.join("index.lookup"))
        .map(|m| m.len())
        .unwrap_or(0);
    let postings_size = std::fs::metadata(index_dir.join("index.postings"))
        .map(|m| m.len())
        .unwrap_or(0);
    let total_size = lookup_size + postings_size;

    println!(
        "  Index size:   {} KB (lookup: {} KB, postings: {} KB)",
        total_size / 1024,
        lookup_size / 1024,
        postings_size / 1024,
    );

    Ok(())
}
