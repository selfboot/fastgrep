/// Git integration: HEAD commit detection and change tracking.

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use anyhow::{Context, Result};

/// Get the HEAD commit hash for a repository.
pub fn get_head_commit(root: &Path) -> Result<String> {
    let repo = gix::discover(root).context("discovering git repository")?;
    let head = repo.head_commit().context("getting HEAD commit")?;
    Ok(head.id().to_string())
}

/// Check if the index is fresh (matches current HEAD).
/// For non-git directories, always returns true (index is trusted as-is).
pub fn is_index_fresh(root: &Path, stored_commit: Option<&str>) -> bool {
    // If the directory is not a git repo, we can't track freshness via commits.
    // Trust the existing index — user can rebuild manually with `fastgrep index`.
    if !is_git_repo(root) {
        return true;
    }

    let stored = match stored_commit {
        Some(c) => c,
        // Index was built without a commit hash (e.g., built before git init,
        // or git was unavailable). In a git repo this is stale.
        None => return false,
    };
    match get_head_commit(root) {
        Ok(current) => current == stored,
        Err(_) => false,
    }
}

/// Check if a directory is inside a git repository.
pub fn is_git_repo(root: &Path) -> bool {
    gix::discover(root).is_ok()
}

/// Check if the working tree has uncommitted changes (staged, unstaged, or untracked).
/// Returns true if there are dirty files that the index might not cover.
pub fn has_working_tree_changes(root: &Path) -> bool {
    // Check for staged + unstaged changes
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=normal"])
        .current_dir(root)
        .output();
    match status {
        Ok(output) => !output.stdout.is_empty(),
        Err(_) => false, // Not a git repo or git not available — assume clean
    }
}

/// Get changed/untracked files relative to HEAD (working tree state).
/// Returns (modified_or_added, deleted) paths.
pub fn working_tree_changes(root: &Path) -> Result<(Vec<String>, Vec<String>)> {
    let output = std::process::Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=normal"])
        .current_dir(root)
        .output()
        .context("running git status")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut modified = Vec::new();
    let mut deleted = Vec::new();

    for line in stdout.lines() {
        if line.len() < 4 {
            continue;
        }
        let status = &line[..2];
        let path = line[3..].trim().to_string();

        // Handle renames: "R  old -> new"
        let path = if let Some(arrow_pos) = path.find(" -> ") {
            path[arrow_pos + 4..].to_string()
        } else {
            path
        };

        if status.contains('D') {
            deleted.push(path);
        } else {
            modified.push(path);
        }
    }

    Ok((modified, deleted))
}

/// Detect filesystem changes in a non-git directory by comparing file mtimes
/// against the index build timestamp.
///
/// Returns (modified_or_new, deleted) paths relative to root.
/// - Files with mtime > build_timestamp are considered modified/new.
/// - Files present in `indexed_files` but absent from disk are considered deleted.
pub fn detect_fs_changes(
    root: &Path,
    indexed_files: &[String],
    build_timestamp: u64,
) -> Result<(Vec<String>, Vec<String>)> {
    let build_time = UNIX_EPOCH + Duration::from_secs(build_timestamp);

    // Walk the directory to find new/modified files
    let mut modified = Vec::new();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(false)
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            name != ".fastgrep" && name != ".git"
        })
        .build();

    // Collect current files on disk for deletion detection
    let mut current_files: HashSet<String> = HashSet::new();

    for result in walker {
        let entry = match result {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.file_type().map_or(false, |ft| ft.is_file()) {
            continue;
        }
        let rel = match entry.path().strip_prefix(root) {
            Ok(r) => r.to_string_lossy().into_owned(),
            Err(_) => continue,
        };

        current_files.insert(rel.clone());

        // Check mtime
        if let Ok(metadata) = entry.metadata() {
            if let Ok(mtime) = metadata.modified() {
                if mtime > build_time {
                    modified.push(rel);
                }
            }
        }
    }

    // Find deleted files: in index but not on disk
    let deleted: Vec<String> = indexed_files
        .iter()
        .filter(|f| !current_files.contains(f.as_str()))
        .cloned()
        .collect();

    Ok((modified, deleted))
}
/// Returns (modified_or_added, deleted) paths relative to root.
pub fn changed_files_since(root: &Path, since_commit: &str) -> Result<(Vec<String>, Vec<String>)> {
    // Use git diff-index to find changes
    let output = std::process::Command::new("git")
        .args(["diff-index", "--name-status", since_commit])
        .current_dir(root)
        .output()
        .context("running git diff-index")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut modified = Vec::new();
    let mut deleted = Vec::new();

    for line in stdout.lines() {
        let parts: Vec<&str> = line.splitn(2, '\t').collect();
        if parts.len() != 2 {
            continue;
        }
        let status = parts[0].trim();
        let path = parts[1].trim().to_string();

        match status {
            "D" => deleted.push(path),
            _ => modified.push(path), // A, M, R, C, etc.
        }
    }

    // Also include untracked files
    let output = std::process::Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard"])
        .current_dir(root)
        .output()
        .context("running git ls-files")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let path = line.trim().to_string();
        if !path.is_empty() {
            modified.push(path);
        }
    }

    Ok((modified, deleted))
}
