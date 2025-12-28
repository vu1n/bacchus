//! Git worktree management for isolated agent work
//!
//! This module manages git worktrees for beads, storing them in `.bacchus/worktrees/{bead_id}/`.
//! Each worktree operates on a separate branch `bacchus/{bead_id}`.
//!
//! Override worktrees directory with BACCHUS_WORKTREES environment variable.

use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;

/// Get the worktrees directory, checking BACCHUS_WORKTREES env var first
pub(crate) fn get_worktrees_dir(workspace_root: &Path) -> PathBuf {
    match std::env::var("BACCHUS_WORKTREES").ok().map(PathBuf::from) {
        Some(path) if path.is_absolute() => path,
        Some(path) => workspace_root.join(path),
        None => workspace_root.join(".bacchus/worktrees"),
    }
}

#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    pub bead_id: String,
    pub path: PathBuf,
    pub branch: String,
    pub head_commit: String,
}

#[derive(Debug, Error)]
pub enum WorktreeError {
    #[error("Git command failed: {0}")]
    GitError(String),
    #[error("Worktree already exists: {0}")]
    AlreadyExists(String),
    #[error("Worktree not found: {0}")]
    NotFound(String),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

/// Create a new worktree for a bead
/// Creates worktrees/{bead_id} on branch bacchus/{bead_id}
pub fn create_worktree(workspace_root: &Path, bead_id: &str) -> Result<WorktreeInfo, WorktreeError> {
    let worktrees_dir = get_worktrees_dir(workspace_root);
    let worktree_path = worktrees_dir.join(bead_id);
    let branch_name = format!("bacchus/{}", bead_id);

    // Ensure .bacchus/worktrees/ directory exists
    std::fs::create_dir_all(&worktrees_dir)?;

    // Check if worktree already exists
    if worktree_path.exists() {
        return Err(WorktreeError::AlreadyExists(bead_id.to_string()));
    }

    // Run: git worktree add .bacchus/worktrees/{bead_id} -b bacchus/{bead_id}
    let output = Command::new("git")
        .arg("worktree")
        .arg("add")
        .arg(&worktree_path)
        .arg("-b")
        .arg(&branch_name)
        .current_dir(workspace_root)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(WorktreeError::GitError(format!(
            "Failed to create worktree: {}",
            stderr
        )));
    }

    // Get HEAD commit
    let head_commit = get_head_commit_in_path(&worktree_path)?;

    Ok(WorktreeInfo {
        bead_id: bead_id.to_string(),
        path: worktree_path,
        branch: branch_name,
        head_commit,
    })
}

/// Remove a worktree (force=true discards uncommitted changes)
pub fn remove_worktree(workspace_root: &Path, bead_id: &str, force: bool) -> Result<(), WorktreeError> {
    let worktree_path = get_worktrees_dir(workspace_root).join(bead_id);
    let branch_name = format!("bacchus/{}", bead_id);

    // Check if worktree exists
    if !worktree_path.exists() {
        return Err(WorktreeError::NotFound(bead_id.to_string()));
    }

    // Run: git worktree remove .bacchus/worktrees/{bead_id} [--force]
    let mut cmd = Command::new("git");
    cmd.arg("worktree")
        .arg("remove")
        .arg(&worktree_path)
        .current_dir(workspace_root);

    if force {
        cmd.arg("--force");
    }

    let output = cmd.output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(WorktreeError::GitError(format!(
            "Failed to remove worktree: {}",
            stderr
        )));
    }

    // Delete branch: git branch -d/-D bacchus/{bead_id}
    delete_branch(workspace_root, &branch_name, force)?;

    Ok(())
}

/// Merge worktree branch to target (usually "main")
pub fn merge_worktree(workspace_root: &Path, bead_id: &str, target_branch: &str) -> Result<(), WorktreeError> {
    let branch_name = format!("bacchus/{}", bead_id);

    // Checkout target branch
    let output = Command::new("git")
        .arg("checkout")
        .arg(target_branch)
        .current_dir(workspace_root)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(WorktreeError::GitError(format!(
            "Failed to checkout {}: {}",
            target_branch, stderr
        )));
    }

    // Merge the worktree branch
    let output = Command::new("git")
        .arg("merge")
        .arg(&branch_name)
        .current_dir(workspace_root)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(WorktreeError::GitError(format!(
            "Failed to merge {}: {}",
            branch_name, stderr
        )));
    }

    Ok(())
}

/// Get current HEAD commit hash
pub fn get_head_commit(workspace_root: &Path) -> Result<String, WorktreeError> {
    get_head_commit_in_path(workspace_root)
}

fn get_head_commit_in_path(path: &Path) -> Result<String, WorktreeError> {
    let output = Command::new("git")
        .arg("rev-parse")
        .arg("HEAD")
        .current_dir(path)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(WorktreeError::GitError(format!(
            "Failed to get HEAD commit: {}",
            stderr
        )));
    }

    let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(commit)
}

// ============================================================================
// Merge Conflict Handling
// ============================================================================

/// Check if the repository is in a merge conflict state
pub fn is_in_merge_conflict(workspace_root: &Path) -> Result<bool, WorktreeError> {
    let merge_head = workspace_root.join(".git/MERGE_HEAD");
    Ok(merge_head.exists())
}

/// Get the branch name being merged (from MERGE_HEAD)
pub fn get_merge_branch(workspace_root: &Path) -> Result<Option<String>, WorktreeError> {
    let merge_head = workspace_root.join(".git/MERGE_HEAD");
    if !merge_head.exists() {
        return Ok(None);
    }

    // Get the branch name from the merge commit
    let output = Command::new("git")
        .args(["name-rev", "--name-only", "MERGE_HEAD"])
        .current_dir(workspace_root)
        .output()?;

    if !output.status.success() {
        return Ok(None);
    }

    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    // Remove remote prefix and ~N suffix if present (e.g., "remotes/origin/branch~1" -> "branch")
    let cleaned = branch
        .strip_prefix("remotes/origin/")
        .unwrap_or(&branch)
        .split('~')
        .next()
        .unwrap_or(&branch)
        .to_string();

    Ok(Some(cleaned))
}

/// Abort an in-progress merge
pub fn abort_merge(workspace_root: &Path) -> Result<(), WorktreeError> {
    let output = Command::new("git")
        .args(["merge", "--abort"])
        .current_dir(workspace_root)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(WorktreeError::GitError(format!(
            "Failed to abort merge: {}",
            stderr
        )));
    }

    Ok(())
}

/// Check for unresolved merge conflicts
pub fn has_unresolved_conflicts(workspace_root: &Path) -> Result<bool, WorktreeError> {
    // git ls-files -u lists unmerged files
    let output = Command::new("git")
        .args(["ls-files", "-u"])
        .current_dir(workspace_root)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(WorktreeError::GitError(format!(
            "Failed to check conflicts: {}",
            stderr
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(!stdout.trim().is_empty())
}

/// Complete a merge after conflicts have been resolved
pub fn complete_merge(workspace_root: &Path) -> Result<(), WorktreeError> {
    // First, check if there are still unresolved conflicts
    if has_unresolved_conflicts(workspace_root)? {
        return Err(WorktreeError::GitError(
            "Cannot complete merge: unresolved conflicts remain".to_string(),
        ));
    }

    // Stage all changes
    let output = Command::new("git")
        .args(["add", "-A"])
        .current_dir(workspace_root)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(WorktreeError::GitError(format!(
            "Failed to stage changes: {}",
            stderr
        )));
    }

    // Commit using the MERGE_MSG (git will use it automatically)
    let output = Command::new("git")
        .args(["commit", "--no-edit"])
        .current_dir(workspace_root)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(WorktreeError::GitError(format!(
            "Failed to complete merge: {}",
            stderr
        )));
    }

    Ok(())
}

// ============================================================================
// Branch Management
// ============================================================================

/// Delete a branch
pub fn delete_branch(workspace_root: &Path, branch: &str, force: bool) -> Result<(), WorktreeError> {
    let mut cmd = Command::new("git");
    cmd.arg("branch");

    if force {
        cmd.arg("-D");
    } else {
        cmd.arg("-d");
    }

    cmd.arg(branch).current_dir(workspace_root);

    let output = cmd.output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(WorktreeError::GitError(format!(
            "Failed to delete branch {}: {}",
            branch, stderr
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn init_test_repo() -> (TempDir, PathBuf) {
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().to_path_buf();

        Command::new("git")
            .arg("init")
            .current_dir(&repo_path)
            .output()
            .unwrap();

        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        fs::write(repo_path.join("test.txt"), "test").unwrap();

        Command::new("git")
            .args(["add", "."])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        (temp, repo_path)
    }

    #[test]
    fn test_create_worktree() {
        let (_temp, repo_path) = init_test_repo();
        let info = create_worktree(&repo_path, "test-bead").unwrap();

        assert_eq!(info.bead_id, "test-bead");
        assert_eq!(info.branch, "bacchus/test-bead");
        assert!(info.path.exists());
    }

    #[test]
    fn test_get_head_commit() {
        let (_temp, repo_path) = init_test_repo();
        let commit = get_head_commit(&repo_path).unwrap();
        assert_eq!(commit.len(), 40);
    }
}
