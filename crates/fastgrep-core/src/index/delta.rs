/// Delta layer: overlay for uncommitted changes on top of the main index.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use anyhow::Result;

use crate::ngram::extract::extract_trigrams;

/// Represents changes since the index was built.
pub struct DeltaLayer {
    /// New or modified files: file_path → set of trigram hashes
    pub modified_trigrams: BTreeMap<String, HashSet<u64>>,
    /// Deleted files (paths)
    pub deleted_files: HashSet<String>,
}

impl DeltaLayer {
    pub fn new() -> Self {
        Self {
            modified_trigrams: BTreeMap::new(),
            deleted_files: HashSet::new(),
        }
    }

    /// Build a delta layer from a list of changed file paths.
    pub fn from_changed_files(root: &Path, changed: &[String], deleted: &[String]) -> Result<Self> {
        let mut delta = Self::new();

        for path in changed {
            let full_path = root.join(path);
            if let Ok(data) = std::fs::read(&full_path) {
                // Skip binary files
                let check_len = data.len().min(8192);
                if !data[..check_len].contains(&0) {
                    let trigrams = extract_trigrams(&data);
                    delta.modified_trigrams.insert(path.clone(), trigrams);
                }
            }
        }

        for path in deleted {
            delta.deleted_files.insert(path.clone());
        }

        Ok(delta)
    }

    /// Check if a file path matches any trigram in the delta layer.
    pub fn lookup_trigram(&self, ngram_hash: u64) -> Vec<String> {
        let mut result = Vec::new();
        for (path, trigrams) in &self.modified_trigrams {
            if trigrams.contains(&ngram_hash) {
                result.push(path.clone());
            }
        }
        result
    }

    pub fn is_empty(&self) -> bool {
        self.modified_trigrams.is_empty() && self.deleted_files.is_empty()
    }
}

impl Default for DeltaLayer {
    fn default() -> Self {
        Self::new()
    }
}
