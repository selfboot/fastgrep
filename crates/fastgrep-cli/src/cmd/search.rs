/// `fastgrep search` command implementation.

use std::path::Path;

use anyhow::{Context, Result};

use fastgrep_core::git;
use fastgrep_core::index::builder::{self, BuildOptions};
use fastgrep_core::index::delta::DeltaLayer;
use fastgrep_core::index::reader::IndexReader;
use fastgrep_core::query::execute::{self, SearchOptions};
use fastgrep_core::INDEX_DIR;

use crate::output;

pub fn run(
    root: &Path,
    pattern: &str,
    before_context: usize,
    after_context: usize,
    case_insensitive: bool,
    file_type: Option<String>,
    glob: Option<String>,
    output_format: &str,
    auto_index: bool,
) -> Result<()> {
    // Check if index exists, auto-build if needed
    let index_dir = root.join(INDEX_DIR);
    if !index_dir.exists() {
        if auto_index {
            eprintln!("No index found, building...");
            let opts = BuildOptions::new(root.to_path_buf());
            let stats = builder::build_index(&opts)?;
            eprintln!(
                "Index built: {} files indexed, {} trigrams in {}ms",
                stats.indexed_count, stats.trigram_count, stats.build_time_ms,
            );
        } else {
            anyhow::bail!(
                "No index found at {}. Run `fastgrep index` first.",
                index_dir.display()
            );
        }
    }

    let reader = IndexReader::open(root).context("opening index")?;

    // Check freshness: if HEAD commit changed, full rebuild
    if auto_index && !git::is_index_fresh(root, reader.commit_hash()) {
        eprintln!("Index is stale, rebuilding...");
        let opts = BuildOptions::new(root.to_path_buf());
        let stats = builder::build_index(&opts)?;
        eprintln!(
            "Index rebuilt: {} files indexed, {} trigrams in {}ms",
            stats.indexed_count, stats.trigram_count, stats.build_time_ms,
        );
        let reader = IndexReader::open(root).context("opening fresh index")?;
        // After rebuild, still check for uncommitted changes
        let delta = build_delta_layer(root, &reader);
        return do_search(&reader, root, pattern, before_context, after_context, case_insensitive, file_type, glob, output_format, delta.as_ref());
    }

    // Index matches HEAD, but working tree may have uncommitted changes
    let delta = build_delta_layer(root, &reader);
    do_search(&reader, root, pattern, before_context, after_context, case_insensitive, file_type, glob, output_format, delta.as_ref())
}

/// Build a delta layer from uncommitted working tree changes (git) or
/// mtime-based filesystem changes (non-git).
/// Returns None if no changes detected or detection fails.
fn build_delta_layer(root: &Path, reader: &IndexReader) -> Option<DeltaLayer> {
    if git::is_git_repo(root) {
        // Git repo: use git status for delta detection
        if !git::has_working_tree_changes(root) {
            return None;
        }
        let (modified, deleted) = git::working_tree_changes(root).ok()?;
        if modified.is_empty() && deleted.is_empty() {
            return None;
        }
        let delta = DeltaLayer::from_changed_files(root, &modified, &deleted).ok()?;
        if delta.is_empty() {
            return None;
        }
        eprintln!(
            "[fastgrep] delta: {} modified, {} deleted uncommitted files",
            delta.modified_trigrams.len(),
            delta.deleted_files.len(),
        );
        Some(delta)
    } else {
        // Non-git directory: use mtime-based delta detection
        let build_ts = reader.build_timestamp()?;
        let (modified, deleted) =
            git::detect_fs_changes(root, &reader.meta.files, build_ts).ok()?;
        if modified.is_empty() && deleted.is_empty() {
            return None;
        }
        let delta = DeltaLayer::from_changed_files(root, &modified, &deleted).ok()?;
        if delta.is_empty() {
            return None;
        }
        eprintln!(
            "[fastgrep] delta: {} modified, {} deleted files (mtime-based)",
            delta.modified_trigrams.len(),
            delta.deleted_files.len(),
        );
        Some(delta)
    }
}

fn do_search(
    reader: &IndexReader,
    root: &Path,
    pattern: &str,
    before_context: usize,
    after_context: usize,
    case_insensitive: bool,
    file_type: Option<String>,
    glob: Option<String>,
    output_format: &str,
    delta: Option<&DeltaLayer>,
) -> Result<()> {
    let opts = SearchOptions {
        pattern: pattern.to_string(),
        root: root.to_path_buf(),
        before_context,
        after_context,
        case_insensitive,
        file_type,
        glob,
    };

    let result = execute::execute_search(reader, &opts, delta)?;

    // Print stats to stderr
    let delta_info = if result.delta_files > 0 {
        format!(" + {} delta files", result.delta_files)
    } else {
        String::new()
    };
    eprintln!(
        "[fastgrep] {} matches in {} candidates{} / {} total files (index: {})",
        result.matches.len(),
        result.candidate_count,
        delta_info,
        result.total_files,
        if result.used_index { "used" } else { "full scan" },
    );

    // Print results
    match output_format {
        "json" => output::print_json(&result.matches)?,
        _ => output::print_text(&result.matches),
    }

    Ok(())
}
