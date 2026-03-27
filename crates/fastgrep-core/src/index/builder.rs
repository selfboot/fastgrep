/// Index builder: discover files, extract trigrams, build in-memory index, write to disk.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result};
use rayon::prelude::*;

use crate::git;
use crate::index::reader::IndexReader;
use crate::index::writer;
use crate::ngram::extract::extract_trigrams_with_folded;

/// Maximum file size to index (default 1MB). Larger files are skipped.
const MAX_FILE_SIZE: u64 = 1_048_576;

/// Auto-trigger incremental rebuild when delta file count exceeds this.
pub const INCREMENTAL_REBUILD_THRESHOLD: usize = 100;

/// Fall back to full rebuild when changed ratio exceeds this (20%).
const INCREMENTAL_REBUILD_MAX_RATIO: f64 = 0.2;

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

    // 4. Get current commit hash and build timestamp
    let commit_hash = git::get_head_commit(root).ok();
    let build_timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .ok();

    // 5. Write to disk
    eprint!("  Writing index...");
    writer::write_index(&trigram_map, &indexed_files, root, commit_hash, build_timestamp)?;
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

/// Incremental index rebuild: detect changes, re-extract only changed files,
/// rebuild trigram_map from old index + changes, write new index.
///
/// Returns Ok(None) if no changes detected (index is up-to-date).
/// Falls back to full rebuild if change ratio exceeds threshold.
pub fn incremental_rebuild(opts: &BuildOptions) -> Result<Option<BuildStats>> {
    let start = std::time::Instant::now();
    let root = &opts.root;

    // 1. Open old index
    let reader = IndexReader::open(root).context("opening existing index for incremental rebuild")?;
    let build_ts = match reader.build_timestamp() {
        Some(ts) => ts,
        None => {
            eprintln!("[fastgrep] no build_timestamp in index, falling back to full rebuild");
            return Ok(Some(build_index(opts)?));
        }
    };

    // 2. Detect changes
    let (modified, deleted) = if git::is_git_repo(root) {
        // Git repo: use git-based detection
        if let Some(stored) = reader.commit_hash() {
            git::changed_files_since(root, stored)?
        } else {
            // No commit hash stored, fall back to full rebuild
            return Ok(Some(build_index(opts)?));
        }
    } else {
        // Non-git: mtime-based detection
        git::detect_fs_changes(root, &reader.meta.files, build_ts)?
    };

    if modified.is_empty() && deleted.is_empty() {
        eprintln!("[fastgrep] index is up-to-date, no changes detected");
        return Ok(None);
    }

    let change_count = modified.len() + deleted.len();
    let total_files = reader.file_count();
    let change_ratio = change_count as f64 / total_files.max(1) as f64;

    eprintln!(
        "[fastgrep] incremental: {} modified/new, {} deleted ({:.1}% of {} files)",
        modified.len(),
        deleted.len(),
        change_ratio * 100.0,
        total_files,
    );

    // 3. If too many changes, fall back to full rebuild
    if change_ratio > INCREMENTAL_REBUILD_MAX_RATIO {
        eprintln!(
            "[fastgrep] change ratio {:.1}% > {:.0}% threshold, falling back to full rebuild",
            change_ratio * 100.0,
            INCREMENTAL_REBUILD_MAX_RATIO * 100.0,
        );
        return Ok(Some(build_index(opts)?));
    }

    // 4. Discover current files on disk
    eprint!("  Discovering files...");
    let new_files = discover_files(root)?;
    eprintln!(" {} files found", new_files.len());

    // 5. Build path→new_id mapping for current files
    let path_to_new_id: HashMap<&str, u32> = new_files
        .iter()
        .enumerate()
        .map(|(i, p)| (p.as_str(), i as u32))
        .collect();

    // 6. Build old_id→new_id mapping + identify which new_ids need re-extraction
    let old_files = &reader.meta.files;
    let mut old_id_to_new_id: HashMap<u32, u32> = HashMap::new();
    let deleted_set: HashSet<&str> = deleted.iter().map(|s| s.as_str()).collect();
    let modified_set: HashSet<&str> = modified.iter().map(|s| s.as_str()).collect();

    for (old_id, path) in old_files.iter().enumerate() {
        if deleted_set.contains(path.as_str()) {
            continue; // deleted, skip
        }
        if let Some(&new_id) = path_to_new_id.get(path.as_str()) {
            old_id_to_new_id.insert(old_id as u32, new_id);
        }
    }

    // new_ids that need re-extraction: modified files + truly new files (not in old index)
    let old_file_set: HashSet<&str> = old_files.iter().map(|s| s.as_str()).collect();
    let needs_extract: HashSet<u32> = new_files
        .iter()
        .enumerate()
        .filter_map(|(new_id, path)| {
            if modified_set.contains(path.as_str()) || !old_file_set.contains(path.as_str()) {
                Some(new_id as u32)
            } else {
                None
            }
        })
        .collect();

    // Also mark modified files' old IDs as "skip from old index"
    let skip_old_ids: HashSet<u32> = old_files
        .iter()
        .enumerate()
        .filter_map(|(old_id, path)| {
            if modified_set.contains(path.as_str()) {
                Some(old_id as u32)
            } else {
                None
            }
        })
        .collect();

    // 7. Rebuild trigram_map from old index, remapping file IDs
    eprint!("  Rebuilding trigram map from old index...");
    let mut trigram_map: BTreeMap<u64, Vec<u32>> = BTreeMap::new();
    let entry_count = reader.entry_count();

    for i in 0..entry_count {
        let entry = reader.get_lookup_entry(i).unwrap();
        let old_file_ids = reader.decode_posting_list(entry.offset, entry.len);

        let mut new_file_ids: Vec<u32> = Vec::new();
        for old_id in old_file_ids {
            // Skip deleted and modified files (modified will be re-extracted)
            if deleted_set.contains(old_files.get(old_id as usize).map(|s| s.as_str()).unwrap_or(""))
                || skip_old_ids.contains(&old_id)
            {
                continue;
            }
            if let Some(&new_id) = old_id_to_new_id.get(&old_id) {
                new_file_ids.push(new_id);
            }
        }

        if !new_file_ids.is_empty() {
            trigram_map.insert(entry.ngram_hash, new_file_ids);
        }
    }
    eprintln!(" done");

    // 8. Extract trigrams for changed/new files only
    let extract_files: Vec<(u32, &str)> = new_files
        .iter()
        .enumerate()
        .filter_map(|(new_id, path)| {
            if needs_extract.contains(&(new_id as u32)) {
                Some((new_id as u32, path.as_str()))
            } else {
                None
            }
        })
        .collect();

    let skipped_binary = AtomicUsize::new(0);
    let skipped_large = AtomicUsize::new(0);
    let processed = AtomicUsize::new(0);
    let extract_total = extract_files.len();

    eprintln!("  Extracting trigrams for {} changed files...", extract_total);

    let changed_trigrams: Vec<(u32, HashSet<u64>)> = extract_files
        .par_iter()
        .filter_map(|&(new_id, path)| {
            let count = processed.fetch_add(1, Ordering::Relaxed) + 1;
            if count % 100 == 0 || count == extract_total {
                eprint!("\r  Extracting trigrams... {}/{}", count, extract_total);
            }

            let full_path = root.join(path);

            if let Ok(meta) = std::fs::metadata(&full_path) {
                if meta.len() > opts.max_file_size {
                    skipped_large.fetch_add(1, Ordering::Relaxed);
                    return None;
                }
            }

            let data = std::fs::read(&full_path).ok()?;
            let check_len = data.len().min(8192);
            if data[..check_len].contains(&0) {
                skipped_binary.fetch_add(1, Ordering::Relaxed);
                return None;
            }

            let trigrams = extract_trigrams_with_folded(&data);
            if trigrams.is_empty() {
                return None;
            }
            Some((new_id, trigrams))
        })
        .collect();

    if extract_total > 0 {
        eprintln!("\r  Extracting trigrams... {}/{} done", extract_total, extract_total);
    }

    // 9. Merge new trigrams into trigram_map
    for (new_id, trigrams) in &changed_trigrams {
        for &hash in trigrams {
            trigram_map.entry(hash).or_default().push(*new_id);
        }
    }

    // Sort and dedup all posting lists
    for list in trigram_map.values_mut() {
        list.sort_unstable();
        list.dedup();
    }

    // 10. Figure out which files were actually indexed (have trigrams)
    let mut indexed_new_ids: HashSet<u32> = HashSet::new();
    for list in trigram_map.values() {
        for &id in list {
            indexed_new_ids.insert(id);
        }
    }

    // Build final file list: only files that appear in at least one posting list
    // Need to remap AGAIN to get contiguous IDs
    let mut final_id_list: Vec<u32> = indexed_new_ids.into_iter().collect();
    final_id_list.sort_unstable();
    let remap_to_final: HashMap<u32, u32> = final_id_list
        .iter()
        .enumerate()
        .map(|(final_id, &old_new_id)| (old_new_id, final_id as u32))
        .collect();

    let indexed_files: Vec<String> = final_id_list
        .iter()
        .map(|&id| new_files[id as usize].clone())
        .collect();

    // Remap trigram_map to final contiguous IDs
    let mut final_trigram_map: BTreeMap<u64, Vec<u32>> = BTreeMap::new();
    for (hash, ids) in &trigram_map {
        let remapped: Vec<u32> = ids
            .iter()
            .filter_map(|id| remap_to_final.get(id).copied())
            .collect();
        if !remapped.is_empty() {
            final_trigram_map.insert(*hash, remapped);
        }
    }

    let trigram_count = final_trigram_map.len();
    let indexed_count = indexed_files.len();
    eprintln!(
        "  Incremental rebuild: {} trigrams from {} files",
        trigram_count, indexed_count
    );

    // 11. Write new index
    let commit_hash = git::get_head_commit(root).ok();
    let build_timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .ok();

    eprint!("  Writing index...");
    writer::write_index(&final_trigram_map, &indexed_files, root, commit_hash, build_timestamp)?;
    eprintln!(" done");

    let build_time_ms = start.elapsed().as_millis();

    Ok(Some(BuildStats {
        file_count: new_files.len(),
        indexed_count,
        skipped_binary: skipped_binary.load(Ordering::Relaxed),
        skipped_large: skipped_large.load(Ordering::Relaxed),
        trigram_count,
        build_time_ms,
    }))
}
