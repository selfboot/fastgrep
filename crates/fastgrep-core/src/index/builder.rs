/// Index builder: discover files, extract trigrams, build in-memory index, write to disk.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result};
use rayon::prelude::*;

use crate::git;
use crate::index::writer;
use crate::ngram::extract::extract_trigrams_with_folded;

/// Maximum file size to index (default 1MB). Larger files are skipped.
const MAX_FILE_SIZE: u64 = 1_048_576;

/// Build options for the index.
pub struct BuildOptions {
    /// Root directory to index.
    pub root: std::path::PathBuf,
    /// Maximum file size in bytes. Files larger than this are skipped.
    pub max_file_size: u64,
}

impl BuildOptions {
    pub fn new(root: std::path::PathBuf) -> Self {
        Self {
            root,
            max_file_size: MAX_FILE_SIZE,
        }
    }
}

/// Statistics from the build process.
#[derive(Debug)]
pub struct BuildStats {
    pub file_count: usize,
    pub indexed_count: usize,
    pub skipped_binary: usize,
    pub skipped_large: usize,
    pub trigram_count: usize,
    pub build_time_ms: u128,
}

/// Build a trigram index for the given directory.
pub fn build_index(opts: &BuildOptions) -> Result<BuildStats> {
    let start = std::time::Instant::now();
    let root = &opts.root;

    // 1. Discover files using the `ignore` crate (respects .gitignore)
    eprint!("  Discovering files...");
    let files = discover_files(root)?;
    eprintln!(" {} files found", files.len());

    // 2. Extract trigrams from all files in parallel
    let skipped_binary = AtomicUsize::new(0);
    let skipped_large = AtomicUsize::new(0);
    let processed = AtomicUsize::new(0);
    let total = files.len();

    let per_file_trigrams: Vec<(usize, std::collections::HashSet<u64>)> = files
        .par_iter()
        .enumerate()
        .filter_map(|(file_id, path)| {
            let count = processed.fetch_add(1, Ordering::Relaxed) + 1;
            if count % 500 == 0 || count == total {
                eprint!("\r  Extracting trigrams... {}/{}", count, total);
            }

            let full_path = root.join(path);

            // Skip large files
            if let Ok(meta) = std::fs::metadata(&full_path) {
                if meta.len() > opts.max_file_size {
                    skipped_large.fetch_add(1, Ordering::Relaxed);
                    return None;
                }
            }

            let data = std::fs::read(&full_path).ok()?;

            // Skip binary files (check for null bytes in first 8KB)
            let check_len = data.len().min(8192);
            if data[..check_len].contains(&0) {
                skipped_binary.fetch_add(1, Ordering::Relaxed);
                return None;
            }

            let trigrams = extract_trigrams_with_folded(&data);
            if trigrams.is_empty() {
                return None;
            }
            Some((file_id, trigrams))
        })
        .collect();

    eprintln!("\r  Extracting trigrams... {}/{} done", total, total);

    let skipped_binary = skipped_binary.load(Ordering::Relaxed);
    let skipped_large = skipped_large.load(Ordering::Relaxed);

    // Collect actually-indexed file paths (only those with trigrams)
    let mut indexed_file_ids: Vec<usize> = per_file_trigrams
        .iter()
        .map(|(id, _)| *id)
        .collect();
    indexed_file_ids.sort_unstable();

    // Build file ID remapping: old_id → new_id (contiguous)
    let mut id_remap: Vec<Option<u32>> = vec![None; files.len()];
    let mut indexed_files: Vec<String> = Vec::with_capacity(indexed_file_ids.len());
    for (new_id, &old_id) in indexed_file_ids.iter().enumerate() {
        id_remap[old_id] = Some(new_id as u32);
        indexed_files.push(files[old_id].clone());
    }

    // 3. Build inverted index: ngram_hash → sorted file IDs (remapped)
    eprint!("  Building inverted index...");
    let mut trigram_map: BTreeMap<u64, Vec<u32>> = BTreeMap::new();
    for (file_id, trigrams) in &per_file_trigrams {
        let new_id = match id_remap[*file_id] {
            Some(id) => id,
            None => continue,
        };
        for &hash in trigrams {
            trigram_map.entry(hash).or_default().push(new_id);
        }
    }

    // Sort each posting list
    for list in trigram_map.values_mut() {
        list.sort_unstable();
        list.dedup();
    }

    let trigram_count = trigram_map.len();
    let indexed_count = indexed_files.len();
    eprintln!(" {} trigrams from {} files", trigram_count, indexed_count);

    // 4. Get current commit hash
    let commit_hash = git::get_head_commit(root).ok();

    // 5. Write to disk
    eprint!("  Writing index...");
    writer::write_index(&trigram_map, &indexed_files, root, commit_hash)?;
    eprintln!(" done");

    let build_time_ms = start.elapsed().as_millis();

    Ok(BuildStats {
        file_count: files.len(),
        indexed_count,
        skipped_binary,
        skipped_large,
        trigram_count,
        build_time_ms,
    })
}

/// Discover all files in a directory, respecting .gitignore.
/// Returns relative paths as strings.
fn discover_files(root: &Path) -> Result<Vec<String>> {
    let mut files = Vec::new();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(true) // skip hidden files
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .filter_entry(|entry| {
            // Skip .fastgrep directory and .git directory
            let name = entry.file_name().to_string_lossy();
            name != ".fastgrep" && name != ".git"
        })
        .build();

    for result in walker {
        let entry = result.context("walking directory")?;
        if entry.file_type().map_or(false, |ft| ft.is_file()) {
            if let Ok(rel) = entry.path().strip_prefix(root) {
                files.push(rel.to_string_lossy().into_owned());
            }
        }
    }
    files.sort();
    Ok(files)
}
