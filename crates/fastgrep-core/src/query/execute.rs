/// Query execution: lookup → intersect → verify with full regex.

use std::collections::HashSet;
use std::path::Path;

use anyhow::Result;
use memmap2::Mmap;
use rayon::prelude::*;
use regex::bytes::Regex as BytesRegex;

use crate::index::delta::DeltaLayer;
use crate::index::posting;
use crate::index::reader::IndexReader;
use crate::query::decompose;
use crate::query::plan::{self, QueryPlan};

/// A single match result.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchMatch {
    pub file: String,
    pub line_number: usize,
    pub line: String,
}

/// Search options.
pub struct SearchOptions {
    pub pattern: String,
    pub root: std::path::PathBuf,
    /// Lines of context before match.
    pub before_context: usize,
    /// Lines of context after match.
    pub after_context: usize,
    /// Case insensitive search.
    pub case_insensitive: bool,
    /// File type filter (e.g., "rs", "py").
    pub file_type: Option<String>,
    /// Glob pattern filter.
    pub glob: Option<String>,
}

/// Result with stats about the search.
#[derive(Debug)]
pub struct SearchResult {
    pub matches: Vec<SearchMatch>,
    pub candidate_count: usize,
    pub total_files: usize,
    pub used_index: bool,
    /// Number of extra files checked via delta layer.
    pub delta_files: usize,
}

/// Execute a search using the trigram index, with an optional delta layer
/// that covers uncommitted changes.
pub fn execute_search(
    reader: &IndexReader,
    opts: &SearchOptions,
    delta: Option<&DeltaLayer>,
) -> Result<SearchResult> {
    let total_files = reader.file_count();

    // Build bytes-mode regex (works on &[u8] from mmap, no UTF-8 decode needed)
    let regex_pattern = if opts.case_insensitive {
        format!("(?i){}", &opts.pattern)
    } else {
        opts.pattern.clone()
    };
    let regex = BytesRegex::new(&regex_pattern)?;

    // Decompose the pattern into trigrams
    let decomposed = decompose::decompose(&opts.pattern, opts.case_insensitive);

    // Get candidate file IDs from the main index
    let (candidate_ids, used_index) = if decomposed.optimizable {
        let plan = plan::plan_query(&decomposed.must_match, &decomposed.alternatives, reader);
        let candidates = execute_plan(&plan, reader);
        (candidates, true)
    } else {
        // Fallback: scan all files
        let all_ids: Vec<u32> = (0..total_files as u32).collect();
        (all_ids, false)
    };

    let candidate_count = candidate_ids.len();

    // Pre-compile glob matcher once (not per-file)
    let glob_matcher = opts.glob.as_ref().and_then(|g| {
        globset::Glob::new(g)
            .ok()
            .map(|glob| glob.compile_matcher())
    });

    // Collect paths of deleted files from delta (to exclude from main index results)
    let deleted_files: HashSet<&str> = match delta {
        Some(d) => d.deleted_files.iter().map(|s| s.as_str()).collect(),
        None => HashSet::new(),
    };

    // Build list of (rel_path, full_path) for candidate files, applying all filters
    let candidate_paths: Vec<(&str, std::path::PathBuf)> = candidate_ids
        .iter()
        .filter_map(|&file_id| {
            let rel_path = reader.file_path(file_id)?;
            // Skip deleted files
            if deleted_files.contains(rel_path) {
                return None;
            }
            // File type filter
            if let Some(ref ft) = opts.file_type {
                let ext = Path::new(rel_path)
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("");
                if ext != ft.as_str() {
                    return None;
                }
            }
            // Glob filter
            if let Some(ref matcher) = glob_matcher {
                if !matcher.is_match(rel_path) {
                    return None;
                }
            }
            let full_path = opts.root.join(rel_path);
            Some((rel_path, full_path))
        })
        .collect();

    // Parallel file verification with mmap
    let before_ctx = opts.before_context;
    let after_ctx = opts.after_context;

    let matches: Vec<SearchMatch> = candidate_paths
        .par_iter()
        .flat_map(|(rel_path, full_path)| {
            search_file_mmap(full_path, rel_path, &regex, before_ctx, after_ctx)
                .unwrap_or_default()
        })
        .collect();

    // Track searched files for delta dedup
    let searched_files: HashSet<&str> = candidate_paths.iter().map(|(p, _)| *p).collect();

    // Delta layer: search additional modified/new files
    let mut delta_file_count = 0;
    let mut delta_matches = Vec::new();
    if let Some(delta) = delta {
        for path in delta.modified_trigrams.keys() {
            if searched_files.contains(path.as_str()) {
                continue;
            }
            if !matches_filter(path, &opts.file_type, &glob_matcher) {
                continue;
            }
            let full_path = opts.root.join(path);
            if let Ok(file_matches) = search_file_mmap(&full_path, path, &regex, before_ctx, after_ctx) {
                delta_matches.extend(file_matches);
            }
            delta_file_count += 1;
        }
    }

    let mut all_matches = matches;
    all_matches.extend(delta_matches);

    Ok(SearchResult {
        matches: all_matches,
        candidate_count,
        total_files,
        used_index,
        delta_files: delta_file_count,
    })
}

/// Execute the query plan against the index to get candidate file IDs.
fn execute_plan(plan: &QueryPlan, reader: &IndexReader) -> Vec<u32> {
    if !plan.uses_index {
        return (0..reader.file_count() as u32).collect();
    }

    let mut result: Option<Vec<u32>> = None;

    // Process must-match trigrams (intersection)
    for &hash in &plan.ordered_trigrams {
        let posting_list = match reader.lookup(hash) {
            Some(list) => list,
            None => return Vec::new(), // Trigram not in index → no matches
        };

        result = Some(match result {
            Some(current) => {
                let intersection = posting::intersect(&current, &posting_list);
                if intersection.is_empty() {
                    return Vec::new(); // Early termination
                }
                intersection
            }
            None => posting_list,
        });
    }

    // Process alternative groups (each group: intersection within, union across groups)
    if !plan.alternative_groups.is_empty() {
        let mut alt_union: Option<Vec<u32>> = None;

        for group in &plan.alternative_groups {
            let mut group_result: Option<Vec<u32>> = None;
            for &hash in group {
                let posting_list = match reader.lookup(hash) {
                    Some(list) => list,
                    None => {
                        group_result = Some(Vec::new());
                        break;
                    }
                };
                group_result = Some(match group_result {
                    Some(current) => posting::intersect(&current, &posting_list),
                    None => posting_list,
                });
            }

            if let Some(group_ids) = group_result {
                alt_union = Some(match alt_union {
                    Some(current) => posting::union(&current, &group_ids),
                    None => group_ids,
                });
            }
        }

        if let Some(alt_ids) = alt_union {
            result = Some(match result {
                Some(current) => posting::intersect(&current, &alt_ids),
                None => alt_ids,
            });
        }
    }

    result.unwrap_or_default()
}

/// Check if a file path passes the type/glob filters.
fn matches_filter(
    path: &str,
    file_type: &Option<String>,
    glob_matcher: &Option<globset::GlobMatcher>,
) -> bool {
    if let Some(ref ft) = file_type {
        let ext = Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        if ext != ft.as_str() {
            return false;
        }
    }
    if let Some(ref matcher) = glob_matcher {
        if !matcher.is_match(path) {
            return false;
        }
    }
    true
}

/// Search a single file using mmap (zero-copy) + bytes regex.
/// Much faster than BufReader line-by-line for large files.
fn search_file_mmap(
    path: &Path,
    rel_path: &str,
    regex: &BytesRegex,
    before_ctx: usize,
    after_ctx: usize,
) -> Result<Vec<SearchMatch>> {
    let file = std::fs::File::open(path)?;
    let metadata = file.metadata()?;

    // For empty files, return immediately
    if metadata.len() == 0 {
        return Ok(Vec::new());
    }

    // Mmap the file for zero-copy access
    let mmap = unsafe { Mmap::map(&file)? };
    let data = &mmap[..];

    // Find all line start offsets (with sentinel at end)
    let line_starts = find_line_starts(data);
    // Actual line count is len - 1 (last entry is sentinel)
    let line_count = line_starts.len() - 1;

    // Skip empty files (only sentinel)
    if line_count == 0 {
        return Ok(Vec::new());
    }

    // Helper: get line bytes (excluding \n and \r\n)
    let line_bytes = |idx: usize| -> &[u8] {
        let start = line_starts[idx];
        let mut end = line_starts[idx + 1];
        // Strip trailing \n
        if end > start && data[end - 1] == b'\n' {
            end -= 1;
        }
        // Strip trailing \r
        if end > start && data[end - 1] == b'\r' {
            end -= 1;
        }
        &data[start..end]
    };

    // Find which lines match the regex
    let mut matching_lines: Vec<usize> = Vec::new();
    for line_idx in 0..line_count {
        if regex.is_match(line_bytes(line_idx)) {
            matching_lines.push(line_idx);
        }
    }

    if matching_lines.is_empty() {
        return Ok(Vec::new());
    }

    // Collect output lines (matches + context), deduped
    let mut output_line_indices: Vec<usize> = Vec::new();
    let mut seen = HashSet::new();

    for &match_idx in &matching_lines {
        let ctx_start = match_idx.saturating_sub(before_ctx);
        let ctx_end = (match_idx + after_ctx + 1).min(line_count);

        for idx in ctx_start..ctx_end {
            if seen.insert(idx) {
                output_line_indices.push(idx);
            }
        }
    }
    output_line_indices.sort_unstable();

    // Build SearchMatch results
    let file_str = rel_path.to_string();
    let matches: Vec<SearchMatch> = output_line_indices
        .iter()
        .map(|&line_idx| {
            let line_text = String::from_utf8_lossy(line_bytes(line_idx)).into_owned();
            SearchMatch {
                file: file_str.clone(),
                line_number: line_idx + 1,
                line: line_text,
            }
        })
        .collect();

    Ok(matches)
}

/// Find the byte offset of every line start in the data, plus a sentinel at data.len().
/// Returns vec where vec[i] = byte offset of line i, vec[last] = data.len().
#[inline]
fn find_line_starts(data: &[u8]) -> Vec<usize> {
    let mut starts = Vec::with_capacity(data.len() / 40 + 2);
    starts.push(0);
    for (i, &byte) in data.iter().enumerate() {
        if byte == b'\n' {
            starts.push(i + 1);
        }
    }
    // Sentinel: makes line-end calculation uniform
    if starts.last() != Some(&data.len()) {
        starts.push(data.len());
    }
    starts
}
