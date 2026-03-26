/// Integration test: build index, search, verify results.

use std::fs;
use tempfile::TempDir;

use fastgrep_core::index::builder::{self, BuildOptions};
use fastgrep_core::index::reader::IndexReader;
use fastgrep_core::query::execute::{self, SearchOptions};

fn create_test_corpus(dir: &std::path::Path) {
    // Create some test files
    fs::write(
        dir.join("hello.rs"),
        r#"use std::collections::HashMap;

pub fn hello() {
    let map: HashMap<String, i32> = HashMap::new();
    println!("Hello, world!");
}
"#,
    )
    .unwrap();

    fs::write(
        dir.join("lib.rs"),
        r#"pub mod hello;

pub struct MyStruct {
    pub name: String,
}

impl std::fmt::Display for MyStruct {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MyStruct({})", self.name)
    }
}
"#,
    )
    .unwrap();

    fs::write(
        dir.join("utils.py"),
        r#"# Utility functions
# TODO: add more utils
# FIXME: this is broken

def find_items(query):
    """Find items matching query."""
    return [item for item in items if query in item]

class HashMap:
    """Python HashMap implementation."""
    pass
"#,
    )
    .unwrap();

    fs::write(
        dir.join("notes.txt"),
        "This is a plain text file.\nIt contains some text.\nHashMap is mentioned here too.\n",
    )
    .unwrap();
}

#[test]
fn test_build_and_search_literal() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    create_test_corpus(root);

    // Build index
    let opts = BuildOptions::new(root.to_path_buf());
    let stats = builder::build_index(&opts).unwrap();
    assert!(stats.file_count >= 4);
    assert!(stats.trigram_count > 0);

    // Open index
    let reader = IndexReader::open(root).unwrap();
    assert_eq!(reader.file_count(), stats.file_count);

    // Search for "HashMap"
    let search_opts = SearchOptions {
        pattern: "HashMap".to_string(),
        root: root.to_path_buf(),
        before_context: 0,
        after_context: 0,
        case_insensitive: false,
        file_type: None,
        glob: None,
    };

    let result = execute::execute_search(&reader, &search_opts, None).unwrap();
    assert!(result.used_index, "should use trigram index");
    assert!(
        result.candidate_count < result.total_files,
        "index should narrow candidates: {} < {}",
        result.candidate_count,
        result.total_files
    );
    // HashMap appears in hello.rs (2x), utils.py (2x), notes.txt (1x) = at least 5 matches
    assert!(
        result.matches.len() >= 3,
        "expected at least 3 matches, got {}",
        result.matches.len()
    );

    // Verify file names in results
    let files: std::collections::HashSet<&str> = result.matches.iter().map(|m| m.file.as_str()).collect();
    assert!(files.contains("hello.rs"), "should match hello.rs");
}

#[test]
fn test_search_regex() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    create_test_corpus(root);

    let opts = BuildOptions::new(root.to_path_buf());
    builder::build_index(&opts).unwrap();

    let reader = IndexReader::open(root).unwrap();

    // Search for regex pattern
    let search_opts = SearchOptions {
        pattern: r"impl\s+\w+".to_string(),
        root: root.to_path_buf(),
        before_context: 0,
        after_context: 0,
        case_insensitive: false,
        file_type: None,
        glob: None,
    };

    let result = execute::execute_search(&reader, &search_opts, None).unwrap();
    assert!(!result.matches.is_empty(), "should find impl matches");
}

#[test]
fn test_search_with_file_type_filter() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    create_test_corpus(root);

    let opts = BuildOptions::new(root.to_path_buf());
    builder::build_index(&opts).unwrap();

    let reader = IndexReader::open(root).unwrap();

    // Search for HashMap only in .rs files
    let search_opts = SearchOptions {
        pattern: "HashMap".to_string(),
        root: root.to_path_buf(),
        before_context: 0,
        after_context: 0,
        case_insensitive: false,
        file_type: Some("rs".to_string()),
        glob: None,
    };

    let result = execute::execute_search(&reader, &search_opts, None).unwrap();
    for m in &result.matches {
        assert!(
            m.file.ends_with(".rs"),
            "expected only .rs files, got: {}",
            m.file
        );
    }
}

#[test]
fn test_search_alternation() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    create_test_corpus(root);

    let opts = BuildOptions::new(root.to_path_buf());
    builder::build_index(&opts).unwrap();

    let reader = IndexReader::open(root).unwrap();

    let search_opts = SearchOptions {
        pattern: r"(TODO|FIXME)".to_string(),
        root: root.to_path_buf(),
        before_context: 0,
        after_context: 0,
        case_insensitive: false,
        file_type: None,
        glob: None,
    };

    let result = execute::execute_search(&reader, &search_opts, None).unwrap();
    assert!(
        result.matches.len() >= 2,
        "should find TODO and FIXME, got {} matches",
        result.matches.len()
    );
}

#[test]
fn test_search_case_insensitive() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    create_test_corpus(root);

    let opts = BuildOptions::new(root.to_path_buf());
    builder::build_index(&opts).unwrap();

    let reader = IndexReader::open(root).unwrap();

    let search_opts = SearchOptions {
        pattern: "hashmap".to_string(),
        root: root.to_path_buf(),
        before_context: 0,
        after_context: 0,
        case_insensitive: true,
        file_type: None,
        glob: None,
    };

    let result = execute::execute_search(&reader, &search_opts, None).unwrap();
    assert!(
        result.matches.len() >= 3,
        "case insensitive search should find HashMap variants, got {}",
        result.matches.len()
    );
}

#[test]
fn test_context_lines() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    create_test_corpus(root);

    let opts = BuildOptions::new(root.to_path_buf());
    builder::build_index(&opts).unwrap();

    let reader = IndexReader::open(root).unwrap();

    // Search with 1 line of context
    let search_opts = SearchOptions {
        pattern: "Hello, world".to_string(),
        root: root.to_path_buf(),
        before_context: 1,
        after_context: 1,
        case_insensitive: false,
        file_type: None,
        glob: None,
    };

    let result = execute::execute_search(&reader, &search_opts, None).unwrap();
    // Should have the match line plus context lines
    assert!(
        result.matches.len() >= 2,
        "should have match + context, got {}",
        result.matches.len()
    );
}

#[test]
fn test_unoptimizable_pattern() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    create_test_corpus(root);

    let opts = BuildOptions::new(root.to_path_buf());
    builder::build_index(&opts).unwrap();

    let reader = IndexReader::open(root).unwrap();

    // .* pattern cannot be optimized
    let search_opts = SearchOptions {
        pattern: ".*".to_string(),
        root: root.to_path_buf(),
        before_context: 0,
        after_context: 0,
        case_insensitive: false,
        file_type: None,
        glob: None,
    };

    let result = execute::execute_search(&reader, &search_opts, None).unwrap();
    assert!(!result.used_index, "should fallback to full scan");
    assert_eq!(
        result.candidate_count, result.total_files,
        "should scan all files"
    );
}

#[test]
fn test_delta_layer_finds_new_file() {
    use fastgrep_core::index::delta::DeltaLayer;

    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    create_test_corpus(root);

    // Build index
    let opts = BuildOptions::new(root.to_path_buf());
    builder::build_index(&opts).unwrap();

    // Now add a NEW file after index was built
    fs::write(
        root.join("new_feature.rs"),
        "pub fn unique_snowflake_function() {\n    println!(\"I am new!\");\n}\n",
    )
    .unwrap();

    let reader = IndexReader::open(root).unwrap();

    // Search WITHOUT delta — should NOT find the new file
    let search_opts = SearchOptions {
        pattern: "unique_snowflake_function".to_string(),
        root: root.to_path_buf(),
        before_context: 0,
        after_context: 0,
        case_insensitive: false,
        file_type: None,
        glob: None,
    };
    let result = execute::execute_search(&reader, &search_opts, None).unwrap();
    assert!(
        result.matches.is_empty(),
        "without delta, new file should not be found"
    );

    // Search WITH delta — should find it
    let delta = DeltaLayer::from_changed_files(
        root,
        &["new_feature.rs".to_string()],
        &[],
    )
    .unwrap();
    let result = execute::execute_search(&reader, &search_opts, Some(&delta)).unwrap();
    assert!(
        !result.matches.is_empty(),
        "with delta, new file should be found"
    );
    assert_eq!(result.matches[0].file, "new_feature.rs");
    assert!(result.delta_files > 0, "should report delta files searched");
}

#[test]
fn test_delta_layer_excludes_deleted_file() {
    use fastgrep_core::index::delta::DeltaLayer;

    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    create_test_corpus(root);

    // Build index (includes notes.txt with "HashMap")
    let opts = BuildOptions::new(root.to_path_buf());
    builder::build_index(&opts).unwrap();

    let reader = IndexReader::open(root).unwrap();

    // Search without delta — notes.txt should appear
    let search_opts = SearchOptions {
        pattern: "plain text file".to_string(),
        root: root.to_path_buf(),
        before_context: 0,
        after_context: 0,
        case_insensitive: false,
        file_type: None,
        glob: None,
    };
    let result = execute::execute_search(&reader, &search_opts, None).unwrap();
    let has_notes = result.matches.iter().any(|m| m.file == "notes.txt");
    assert!(has_notes, "notes.txt should be in results without delta");

    // Now "delete" notes.txt and search with delta
    fs::remove_file(root.join("notes.txt")).unwrap();
    let delta = DeltaLayer::from_changed_files(
        root,
        &[],
        &["notes.txt".to_string()],
    )
    .unwrap();
    let result = execute::execute_search(&reader, &search_opts, Some(&delta)).unwrap();
    let has_notes = result.matches.iter().any(|m| m.file == "notes.txt");
    assert!(!has_notes, "notes.txt should be excluded with delta layer");
}
