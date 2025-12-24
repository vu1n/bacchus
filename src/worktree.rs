//! Git worktree management for isolated agent work
//!
//! This module manages git worktrees for beads, storing them in `.bacchus/worktrees/{bead_id}/`.
//! Each worktree operates on a separate branch `bacchus/{bead_id}`.

use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;

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
/// Creates .bacchus/worktrees/{bead_id} on branch bacchus/{bead_id}
pub fn create_worktree(workspace_root: &Path, bead_id: &str) -> Result<WorktreeInfo, WorktreeError> {
    let worktrees_dir = workspace_root.join(".bacchus/worktrees");
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
    let worktree_path = workspace_root.join(".bacchus/worktrees").join(bead_id);
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

/// List all active worktrees managed by bacchus
pub fn list_worktrees(workspace_root: &Path) -> Result<Vec<WorktreeInfo>, WorktreeError> {
    let output = Command::new("git")
        .arg("worktree")
        .arg("list")
        .arg("--porcelain")
        .current_dir(workspace_root)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(WorktreeError::GitError(format!(
            "Failed to list worktrees: {}",
            stderr
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut worktrees = Vec::new();
    let worktrees_prefix = workspace_root.join(".bacchus/worktrees");

    let mut current_path: Option<PathBuf> = None;
    let mut current_head: Option<String> = None;
    let mut current_branch: Option<String> = None;

    for line in stdout.lines() {
        if line.is_empty() {
            if let (Some(path), Some(head), Some(branch)) = (&current_path, &current_head, &current_branch) {
                if path.starts_with(&worktrees_prefix) {
                    if let Some(bead_id) = path.file_name().and_then(|n| n.to_str()) {
                        worktrees.push(WorktreeInfo {
                            bead_id: bead_id.to_string(),
                            path: path.clone(),
                            branch: branch.clone(),
                            head_commit: head.clone(),
                        });
                    }
                }
            }
            current_path = None;
            current_head = None;
            current_branch = None;
        } else if let Some(path) = line.strip_prefix("worktree ") {
            current_path = Some(PathBuf::from(path));
        } else if let Some(head) = line.strip_prefix("HEAD ") {
            current_head = Some(head.to_string());
        } else if let Some(branch) = line.strip_prefix("branch ") {
            current_branch = Some(branch.to_string());
        }
    }

    // Handle last entry
    if let (Some(path), Some(head), Some(branch)) = (current_path, current_head, current_branch) {
        if path.starts_with(&worktrees_prefix) {
            if let Some(bead_id) = path.file_name().and_then(|n| n.to_str()) {
                worktrees.push(WorktreeInfo {
                    bead_id: bead_id.to_string(),
                    path,
                    branch,
                    head_commit: head,
                });
            }
        }
    }

    Ok(worktrees)
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
