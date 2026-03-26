/// Correctness tests: verify fastgrep results match grep/rg exactly.
///
/// Strategy: for each test pattern, run both fastgrep (via library) and
/// a naive grep (line-by-line regex scan on all files) on the same corpus,
/// then assert the results are identical.

use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;

use tempfile::TempDir;

use fastgrep_core::index::builder::{self, BuildOptions};
use fastgrep_core::index::reader::IndexReader;
use fastgrep_core::query::execute::{self, SearchMatch, SearchOptions};

// ─── Test Corpus ───────────────────────────────────────────────

fn create_correctness_corpus(dir: &Path) {
    // Rust-like files
    fs::write(dir.join("main.rs"), r#"use std::collections::HashMap;
use std::io::{self, BufRead};

fn main() {
    let mut map: HashMap<String, i32> = HashMap::new();
    map.insert("hello".to_string(), 42);
    println!("Hello, world!");
    // TODO: refactor this
    // FIXME: handle errors properly
}

pub fn helper() -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
}

impl Default for MyStruct {
    fn default() -> Self {
        Self { value: 0 }
    }
}

struct MyStruct {
    value: i32,
}

impl std::fmt::Display for MyStruct {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MyStruct({})", self.value)
    }
}
"#).unwrap();

    fs::write(dir.join("lib.rs"), r#"//! Library root
// SPDX-License-Identifier: MIT

pub mod utils;

pub struct Config {
    pub name: String,
    pub debug: bool,
}

impl Config {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            debug: false,
        }
    }

    pub fn with_debug(mut self) -> Self {
        self.debug = true;
        self
    }
}

impl std::fmt::Display for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Config({})", self.name)
    }
}

// HACK: workaround for upstream bug
fn internal_helper() {
    let _x: HashMap<u32, String> = HashMap::new();
}
"#).unwrap();

    // Python file
    fs::write(dir.join("script.py"), r#"#!/usr/bin/env python3
"""A sample Python script."""

import os
import sys
from collections import defaultdict

# TODO: add proper logging
# FIXME: this is a hack

class DataProcessor:
    """Process data from various sources."""

    def __init__(self, config):
        self.config = config
        self.cache = {}  # HashMap equivalent

    def process(self, items):
        """Process a list of items."""
        result = defaultdict(list)
        for item in items:
            key = item.get("type", "unknown")
            result[key].append(item)
        return dict(result)

    def display(self):
        """Display processor state."""
        print(f"DataProcessor({self.config})")

def main():
    proc = DataProcessor({"debug": True})
    proc.process([])

if __name__ == "__main__":
    main()
"#).unwrap();

    // JSON config
    fs::write(dir.join("config.json"), r#"{
    "name": "fastgrep-test",
    "version": "1.0.0",
    "settings": {
        "HashMap_cache_size": 1024,
        "display_mode": "compact",
        "debug": false
    },
    "tags": ["search", "grep", "fast", "HashMap"]
}
"#).unwrap();

    // Markdown doc
    fs::write(dir.join("README.md"), r#"# Test Project

This project uses `HashMap` for caching.

## Usage

```rust
use std::collections::HashMap;

fn main() {
    let map: HashMap<String, i32> = HashMap::new();
    println!("Hello, world!");
}
```

## TODO

- [ ] Add more tests
- [ ] FIXME: fix the display bug
- [ ] Implement Display trait

## License

SPDX-License-Identifier: MIT
"#).unwrap();

    // File with tricky patterns
    fs::write(dir.join("edge_cases.txt"), r#"Line with special chars: (TODO|FIXME|HACK)
Regular TODO here
Another FIXME there
HACK around the issue
impl Display for Something
impl<T> Clone for Wrapper<T>
fn   spaced_function  (  arg: u32  )
use std::collections::HashMap;
nested::path::HashMap::new()
CamelCaseHashMapUsage
lowercasehashmap
UPPERCASEHASHMAP
MiXeDcAsEhAsHmAp
a]$^special.regex" chars[
empty line above and below:

next line after empty
"#).unwrap();

    // Subdirectory
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/mod.rs"), r#"pub mod inner;

use crate::Config;

pub fn setup() -> Config {
    Config::new("default")
}
"#).unwrap();

    fs::write(dir.join("src/inner.rs"), r#"//! Inner module

use std::collections::HashMap;

pub struct Cache {
    store: HashMap<String, Vec<u8>>,
}

impl Cache {
    pub fn new() -> Self {
        Self {
            store: HashMap::new(),
        }
    }

    pub fn get(&self, key: &str) -> Option<&Vec<u8>> {
        self.store.get(key)
    }
}

impl std::fmt::Display for Cache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Cache({} entries)", self.store.len())
    }
}
"#).unwrap();
}

// ─── Naive grep (ground truth) ─────────────────────────────────

/// Scan all files under `root` with a regex, return sorted (file, line_number, line) tuples.
/// This is the "ground truth" — no index, just brute-force line-by-line matching.
fn naive_grep(root: &Path, pattern: &str, case_insensitive: bool) -> Vec<(String, usize, String)> {
    let regex_pat = if case_insensitive {
        format!("(?i){}", pattern)
    } else {
        pattern.to_string()
    };
    let regex = regex::Regex::new(&regex_pat).unwrap();

    let mut results = Vec::new();
    collect_files(root, root, &regex, &mut results);
    results.sort();
    results
}

fn collect_files(
    base: &Path,
    dir: &Path,
    regex: &regex::Regex,
    results: &mut Vec<(String, usize, String)>,
) {
    let mut entries: Vec<_> = fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();

        // Skip hidden and .fastgrep
        if name.starts_with('.') || name == ".fastgrep" {
            continue;
        }

        if path.is_dir() {
            collect_files(base, &path, regex, results);
        } else if path.is_file() {
            // Skip binary files (same heuristic as fastgrep)
            if let Ok(data) = fs::read(&path) {
                let check_len = data.len().min(8192);
                if data[..check_len].contains(&0) {
                    continue;
                }
            }
            if let Ok(file) = fs::File::open(&path) {
                let reader = BufReader::new(file);
                let rel = path.strip_prefix(base).unwrap().to_string_lossy().to_string();
                for (i, line) in reader.lines().enumerate() {
                    if let Ok(line) = line {
                        if regex.is_match(&line) {
                            results.push((rel.clone(), i + 1, line));
                        }
                    }
                }
            }
        }
    }
}

/// Convert fastgrep SearchMatch vec to the same (file, line_number, line) format, sorted.
fn normalize_results(matches: &[SearchMatch]) -> Vec<(String, usize, String)> {
    let mut results: Vec<(String, usize, String)> = matches
        .iter()
        .map(|m| (m.file.clone(), m.line_number, m.line.clone()))
        .collect();
    results.sort();
    results
}

/// Assert fastgrep results match naive grep exactly. Print detailed diff on failure.
fn assert_results_match(
    pattern: &str,
    fastgrep_results: &[(String, usize, String)],
    naive_results: &[(String, usize, String)],
) {
    if fastgrep_results == naive_results {
        return;
    }

    let fg_set: BTreeSet<_> = fastgrep_results.iter().collect();
    let naive_set: BTreeSet<_> = naive_results.iter().collect();

    let missing: Vec<_> = naive_set.difference(&fg_set).collect();
    let extra: Vec<_> = fg_set.difference(&naive_set).collect();

    let mut msg = format!(
        "Pattern {:?}: fastgrep={} matches, naive grep={} matches\n",
        pattern,
        fastgrep_results.len(),
        naive_results.len(),
    );

    if !missing.is_empty() {
        msg.push_str(&format!("  MISSING from fastgrep ({}):\n", missing.len()));
        for (i, m) in missing.iter().enumerate() {
            if i >= 10 {
                msg.push_str(&format!("  ... and {} more\n", missing.len() - 10));
                break;
            }
            msg.push_str(&format!("    {}:{}:{}\n", m.0, m.1, m.2));
        }
    }

    if !extra.is_empty() {
        msg.push_str(&format!("  EXTRA in fastgrep ({}):\n", extra.len()));
        for (i, m) in extra.iter().enumerate() {
            if i >= 10 {
                msg.push_str(&format!("  ... and {} more\n", extra.len() - 10));
                break;
            }
            msg.push_str(&format!("    {}:{}:{}\n", m.0, m.1, m.2));
        }
    }

    panic!("{}", msg);
}

// ─── Helper ────────────────────────────────────────────────────

fn build_and_open(root: &Path) -> IndexReader {
    let opts = BuildOptions::new(root.to_path_buf());
    builder::build_index(&opts).unwrap();
    IndexReader::open(root).unwrap()
}

fn search(reader: &IndexReader, root: &Path, pattern: &str, case_insensitive: bool) -> Vec<(String, usize, String)> {
    let opts = SearchOptions {
        pattern: pattern.to_string(),
        root: root.to_path_buf(),
        before_context: 0,
        after_context: 0,
        case_insensitive,
        file_type: None,
        glob: None,
    };
    let result = execute::execute_search(reader, &opts, None).unwrap();
    normalize_results(&result.matches)
}

// ─── Tests ─────────────────────────────────────────────────────

/// Test all patterns against naive grep on the same corpus.
#[test]
fn test_correctness_all_patterns() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    create_correctness_corpus(root);

    let reader = build_and_open(root);

    let patterns: &[(&str, bool)] = &[
        // Literals (case-sensitive)
        ("HashMap", false),
        ("SPDX-License-Identifier", false),
        ("pub fn", false),
        ("Hello, world!", false),
        ("TODO", false),
        ("FIXME", false),
        ("impl", false),
        ("Display", false),
        ("fn main", false),
        ("use std", false),

        // Regex patterns
        (r"fn\s+\w+", false),
        (r"impl\s+\w+", false),
        (r"impl\s+\w+\s+for\s+\w+", false),
        (r"pub\s+fn\s+\w+", false),
        (r"use\s+\w+::\w+", false),
        (r"(TODO|FIXME|HACK)", false),

        // Case-insensitive
        ("hashmap", true),
        ("display", true),
        ("config", true),
        ("todo", true),
        ("HashMap", true),

        // Unoptimizable (full scan fallback, but must still be correct)
        (r".*", false),
        (r"\d+", false),
        (r"\w+", false),
    ];

    let mut passed = 0;
    let total = patterns.len();

    for &(pattern, case_insensitive) in patterns {
        let fg_results = search(&reader, root, pattern, case_insensitive);
        let naive_results = naive_grep(root, pattern, case_insensitive);

        assert_results_match(pattern, &fg_results, &naive_results);
        passed += 1;
    }

    eprintln!("Correctness: {}/{} patterns passed", passed, total);
}

/// Test that index doesn't lose matches: for every pattern, the set of files
/// returned by the index must be a superset of files that actually contain matches.
#[test]
fn test_index_no_false_negatives() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    create_correctness_corpus(root);

    let reader = build_and_open(root);

    let patterns = &[
        "HashMap",
        "Display",
        "SPDX-License-Identifier",
        "pub fn new",
        "(TODO|FIXME|HACK)",
        r"impl\s+\w+\s+for\s+\w+",
    ];

    for &pattern in patterns {
        let fg_results = search(&reader, root, pattern, false);
        let naive_results = naive_grep(root, pattern, false);

        // Files in naive grep must all appear in fastgrep results
        let fg_files: BTreeSet<&str> = fg_results.iter().map(|(f, _, _)| f.as_str()).collect();
        let naive_files: BTreeSet<&str> = naive_results.iter().map(|(f, _, _)| f.as_str()).collect();

        let missing: Vec<_> = naive_files.difference(&fg_files).collect();
        assert!(
            missing.is_empty(),
            "Pattern {:?}: index missed files: {:?}",
            pattern, missing,
        );
    }
}

/// Test case-insensitive accuracy: fastgrep -i must find all case variants.
#[test]
fn test_case_insensitive_accuracy() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    create_correctness_corpus(root);

    let reader = build_and_open(root);

    // The corpus has: HashMap, lowercasehashmap, UPPERCASEHASHMAP, MiXeDcAsEhAsHmAp
    let fg = search(&reader, root, "hashmap", true);
    let naive = naive_grep(root, "hashmap", true);
    assert_results_match("hashmap (case-insensitive)", &fg, &naive);

    // All four variants must be found
    let lines: Vec<&str> = fg.iter().map(|(_, _, l)| l.as_str()).collect();
    let joined = lines.join("\n");
    assert!(joined.contains("HashMap"), "should find HashMap");
    assert!(joined.contains("lowercasehashmap"), "should find lowercasehashmap");
    assert!(joined.contains("UPPERCASEHASHMAP"), "should find UPPERCASEHASHMAP");
}

/// Test context lines don't break correctness of match lines.
#[test]
fn test_context_does_not_alter_matches() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    create_correctness_corpus(root);

    let reader = build_and_open(root);

    // Search without context
    let no_ctx = search(&reader, root, "HashMap", false);

    // Search with context
    let opts = SearchOptions {
        pattern: "HashMap".to_string(),
        root: root.to_path_buf(),
        before_context: 2,
        after_context: 2,
        case_insensitive: false,
        file_type: None,
        glob: None,
    };
    let with_ctx = execute::execute_search(&reader, &opts, None).unwrap();
    let with_ctx_normalized = normalize_results(&with_ctx.matches);

    // Every match from no-context must appear in with-context results
    let ctx_set: BTreeSet<_> = with_ctx_normalized.iter().collect();
    for m in &no_ctx {
        assert!(
            ctx_set.contains(m),
            "Match line missing when context enabled: {:?}", m,
        );
    }

    // Context results must be >= no-context results
    assert!(
        with_ctx_normalized.len() >= no_ctx.len(),
        "Context should add lines, not remove: {} vs {}",
        with_ctx_normalized.len(), no_ctx.len(),
    );
}

/// Test file type filter correctness.
#[test]
fn test_file_type_filter_correctness() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    create_correctness_corpus(root);

    let reader = build_and_open(root);

    // Search HashMap in .rs files only
    let opts = SearchOptions {
        pattern: "HashMap".to_string(),
        root: root.to_path_buf(),
        before_context: 0,
        after_context: 0,
        case_insensitive: false,
        file_type: Some("rs".to_string()),
        glob: None,
    };
    let result = execute::execute_search(&reader, &opts, None).unwrap();

    // All results must be .rs files
    for m in &result.matches {
        assert!(m.file.ends_with(".rs"), "non-.rs file in results: {}", m.file);
    }

    // Cross-check: naive grep on only .rs files
    let naive_all = naive_grep(root, "HashMap", false);
    let naive_rs: Vec<_> = naive_all.iter().filter(|(f, _, _)| f.ends_with(".rs")).cloned().collect();
    let fg_normalized = normalize_results(&result.matches);
    assert_results_match("HashMap (type=rs)", &fg_normalized, &naive_rs);
}

/// Stress test with many patterns to catch edge cases.
#[test]
fn test_correctness_edge_cases() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    create_correctness_corpus(root);

    let reader = build_and_open(root);

    let edge_patterns: &[(&str, bool)] = &[
        // Very short pattern (no trigrams, falls back to full scan)
        ("fn", false),
        ("if", false),
        // Pattern that matches nothing
        ("ZZZZZ_NONEXISTENT_PATTERN", false),
        // Special regex chars as literal
        (r"\{", false),
        (r"\(", false),
        // Pattern at line boundaries
        ("^#", false),
        // Empty-line-adjacent matches
        ("next line after empty", false),
        // Multi-word literal
        ("pub fn new", false),
        ("let mut map", false),
        // Nested module path
        ("nested::path", false),
    ];

    for &(pattern, ci) in edge_patterns {
        let fg = search(&reader, root, pattern, ci);
        let naive = naive_grep(root, pattern, ci);
        assert_results_match(pattern, &fg, &naive);
    }
}
