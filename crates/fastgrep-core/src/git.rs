/// Git integration: HEAD commit detection and change tracking.

use std::path::Path;

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

/// Get a list of files changed since a given commit hash.
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
