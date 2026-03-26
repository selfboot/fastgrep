/// Query execution: lookup → intersect → verify with full regex.

use std::collections::HashSet;
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::Result;
use regex::Regex;

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

    // Build regex
    let regex_pattern = if opts.case_insensitive {
        format!("(?i){}", &opts.pattern)
    } else {
        opts.pattern.clone()
    };
    let regex = Regex::new(&regex_pattern)?;

    // Decompose the pattern into trigrams
    // For case-insensitive, decompose with folded trigrams (index has them too)
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

    // Filter by file type/glob if specified
    let candidate_ids = filter_candidates(&candidate_ids, reader, &opts.file_type, &opts.glob);

    // Collect paths of deleted files from delta (to exclude from main index results)
    let deleted_files: HashSet<&str> = match delta {
        Some(d) => d.deleted_files.iter().map(|s| s.as_str()).collect(),
        None => HashSet::new(),
    };

    // Verify: run full regex on candidate files from main index
    let mut matches = Vec::new();
    // Track files we already searched (to avoid duplicates with delta)
    let mut searched_files: HashSet<String> = HashSet::new();

    for &file_id in &candidate_ids {
        let rel_path = match reader.file_path(file_id) {
            Some(p) => p,
            None => continue,
        };
        // Skip files that were deleted in working tree
        if deleted_files.contains(rel_path) {
            continue;
        }
        let full_path = opts.root.join(rel_path);
        // Always read from disk (not from index) — this way modified files
        // get their current content even without delta layer
        if let Ok(file_matches) =
            search_file(&full_path, rel_path, &regex, opts.before_context, opts.after_context)
        {
            matches.extend(file_matches);
        }
        searched_files.insert(rel_path.to_string());
    }

    // Delta layer: search additional modified/new files not covered by index candidates
    let mut delta_file_count = 0;
    if let Some(delta) = delta {
        for path in delta.modified_trigrams.keys() {
            // Skip if already searched via main index
            if searched_files.contains(path.as_str()) {
                continue;
            }
            // Apply file type / glob filter
            if !matches_filter(path, &opts.file_type, &opts.glob) {
                continue;
            }
            let full_path = opts.root.join(path);
            if let Ok(file_matches) = search_file(
                &full_path,
                path,
                &regex,
                opts.before_context,
                opts.after_context,
            ) {
                matches.extend(file_matches);
            }
            delta_file_count += 1;
        }
    }

    Ok(SearchResult {
        matches,
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
            // Each alternative group: intersect its trigrams
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

/// Filter candidate IDs by file type and glob pattern.
fn filter_candidates(
    candidates: &[u32],
    reader: &IndexReader,
    file_type: &Option<String>,
    glob_pattern: &Option<String>,
) -> Vec<u32> {
    let glob_matcher = glob_pattern.as_ref().and_then(|g| {
        globset::Glob::new(g)
            .ok()
            .map(|glob| glob.compile_matcher())
    });

    candidates
        .iter()
        .copied()
        .filter(|&id| {
            let path = match reader.file_path(id) {
                Some(p) => p,
                None => return false,
            };

            // File type filter
            if let Some(ref ft) = file_type {
                let ext = Path::new(path)
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("");
                if ext != ft.as_str() {
                    return false;
                }
            }

            // Glob filter
            if let Some(ref matcher) = glob_matcher {
                if !matcher.is_match(path) {
                    return false;
                }
            }

            true
        })
        .collect()
}

/// Check if a file path passes the type/glob filters (for delta layer files).
fn matches_filter(path: &str, file_type: &Option<String>, glob_pattern: &Option<String>) -> bool {
    if let Some(ref ft) = file_type {
        let ext = Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        if ext != ft.as_str() {
            return false;
        }
    }
    if let Some(ref g) = glob_pattern {
        if let Ok(glob) = globset::Glob::new(g) {
            let matcher = glob.compile_matcher();
            if !matcher.is_match(path) {
                return false;
            }
        }
    }
    true
}

/// Search a single file with the regex, returning matches with context.
fn search_file(
    path: &Path,
    rel_path: &str,
    regex: &Regex,
    before_ctx: usize,
    after_ctx: usize,
) -> Result<Vec<SearchMatch>> {
    let file = std::fs::File::open(path)?;
    let reader = BufReader::new(file);
    let lines: Vec<String> = reader.lines().filter_map(|l| l.ok()).collect();

    let mut matches = Vec::new();
    let mut context_lines_added = std::collections::HashSet::new();

    for (i, line) in lines.iter().enumerate() {
        if regex.is_match(line) {
            // Add before-context lines
            let start = i.saturating_sub(before_ctx);
            for ctx_i in start..i {
                if context_lines_added.insert(ctx_i) {
                    matches.push(SearchMatch {
                        file: rel_path.to_string(),
                        line_number: ctx_i + 1,
                        line: lines[ctx_i].clone(),
                    });
                }
            }

            // Add the match line itself
            if context_lines_added.insert(i) {
                matches.push(SearchMatch {
                    file: rel_path.to_string(),
                    line_number: i + 1,
                    line: line.clone(),
                });
            }

            // Add after-context lines
            let end = (i + after_ctx + 1).min(lines.len());
            for ctx_i in (i + 1)..end {
                if context_lines_added.insert(ctx_i) {
                    matches.push(SearchMatch {
                        file: rel_path.to_string(),
                        line_number: ctx_i + 1,
                        line: lines[ctx_i].clone(),
                    });
                }
            }
        }
    }

    Ok(matches)
}
